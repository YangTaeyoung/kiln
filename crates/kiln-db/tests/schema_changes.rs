//! Schema plans run only against isolated temporary fixtures, never user connections.
use kiln_db::{ConnConfig,ConnId,DbManager,Driver,TableRef};
use kiln_db::schema::{ColumnSpec,IndexColumn,IndexSpec,SchemaAction};

fn fixture()->(tempfile::TempDir,DbManager,ConnId){
    let dir=tempfile::tempdir().unwrap();let file=dir.path().join("schema.db");std::fs::write(&file,[]).unwrap();
    let m=DbManager::in_memory();let id=m.add(ConnConfig{driver:Driver::Sqlite,file:file.to_string_lossy().into_owned(),..Default::default()},None);
    m.block_on(m.connect(id)).unwrap();(dir,m,id)
}
fn exec(m:&DbManager,id:ConnId,sql:&str){m.block_on(m.query(id,sql,None)).unwrap();}
fn scalar(m:&DbManager,id:ConnId,sql:&str)->String{m.block_on(m.query(id,sql,None)).unwrap().result.scalar_string().unwrap()}
fn table(name:&str)->TableRef{TableRef::new(Some("main".into()),name)}
fn alter(m:&DbManager,id:ConnId,t:&TableRef,column:&str,edit:impl FnOnce(&mut ColumnSpec))->SchemaAction{
    let details=m.block_on(m.table_details(id,t)).unwrap();let mut spec=ColumnSpec::from(details.columns.iter().find(|c|c.name==column).unwrap());edit(&mut spec);SchemaAction::AlterColumn{column:column.into(),spec}
}
fn apply(m:&DbManager,id:ConnId,t:&TableRef,action:SchemaAction){let plan=m.block_on(m.prepare_schema_change(id,t,action)).unwrap();m.block_on(m.apply_schema_change(id,plan)).unwrap();}

#[test]
fn sqlite_rebuild_preserves_rows_fk_view_trigger_expression_partial_index_and_sequence(){
    let (_d,m,id)=fixture();
    for sql in [
        "CREATE TABLE parent (id INTEGER PRIMARY KEY AUTOINCREMENT, label TEXT NOT NULL DEFAULT 'a', score INTEGER CHECK(score>=0))",
        "CREATE TABLE child (parent_id INTEGER REFERENCES parent(id) ON DELETE SET NULL)",
        "CREATE TABLE audit (message TEXT)",
        "CREATE TRIGGER parent_audit AFTER UPDATE ON parent BEGIN INSERT INTO audit VALUES(new.label); END",
        "CREATE VIEW parent_view AS SELECT id,label,score FROM parent",
        "CREATE INDEX parent_expression ON parent(lower(label) DESC) WHERE score>0",
        "INSERT INTO parent(id,label,score) VALUES(7,'first',4),(99,'deleted',1)",
        "DELETE FROM parent WHERE id=99", "INSERT INTO child VALUES(7)",
    ] {exec(&m,id,sql);}
    let t=table("parent");let action=alter(&m,id,&t,"label",|s|s.default=Some("'new'".into()));apply(&m,id,&t,action);
    assert_eq!(scalar(&m,id,"SELECT label FROM parent_view WHERE id=7"),"first");
    assert_eq!(scalar(&m,id,"SELECT count(*) FROM pragma_foreign_key_check"),"0");
    let definition=scalar(&m,id,"SELECT sql FROM sqlite_schema WHERE name='parent_expression'");assert!(definition.contains("lower(label) DESC"));assert!(definition.contains("WHERE score>0"));
    assert_eq!(scalar(&m,id,"SELECT count(*) FROM audit"),"0");exec(&m,id,"UPDATE parent SET label='updated' WHERE id=7");assert_eq!(scalar(&m,id,"SELECT message FROM audit"),"updated");
    exec(&m,id,"INSERT INTO parent(score) VALUES(2)");assert_eq!(scalar(&m,id,"SELECT id FROM parent WHERE label='new'"),"100");
    exec(&m,id,"DELETE FROM parent WHERE id=7");assert_eq!(scalar(&m,id,"SELECT count(*) FROM child WHERE parent_id IS NULL"),"1");
}

#[test]
fn sqlite_rebuild_retains_hidden_rowids_and_generated_columns(){
    let (_d,m,id)=fixture();exec(&m,id,"CREATE TABLE notes (body TEXT, upper_body TEXT GENERATED ALWAYS AS (upper(body)) STORED)");
    exec(&m,id,"INSERT INTO notes(rowid,body) VALUES(7,'a'),(99,'b')");
    let t=table("notes");let action=alter(&m,id,&t,"body",|s|s.default=Some("'c'".into()));apply(&m,id,&t,action);
    assert_eq!(scalar(&m,id,"SELECT group_concat(rowid, ',') FROM notes ORDER BY rowid"),"7,99");
    assert_eq!(scalar(&m,id,"SELECT upper_body FROM notes WHERE rowid=99"),"B");
}

#[test]
fn sqlite_strict_and_without_rowid_options_survive_and_constraint_failure_rolls_back(){
    let (_d,m,id)=fixture();
    for (name,suffix) in [("strict_items","STRICT"),("key_items","WITHOUT ROWID")]{
        exec(&m,id,&format!("CREATE TABLE {name} (id TEXT PRIMARY KEY, body TEXT) {suffix}"));
        exec(&m,id,&format!("INSERT INTO {name} VALUES('a',NULL)"));
        let t=table(name);let original=scalar(&m,id,&format!("SELECT sql FROM sqlite_schema WHERE name='{name}'"));
        let action=alter(&m,id,&t,"body",|s|s.nullable=false);let plan=m.block_on(m.prepare_schema_change(id,&t,action)).unwrap();
        assert!(m.block_on(m.apply_schema_change(id,plan)).is_err());
        assert_eq!(scalar(&m,id,&format!("SELECT sql FROM sqlite_schema WHERE name='{name}'")),original);
        assert_eq!(scalar(&m,id,&format!("SELECT count(*) FROM {name} WHERE body IS NULL")),"1");
        let action=alter(&m,id,&t,"body",|s|s.default=Some("'ok'".into()));apply(&m,id,&t,action);
        assert!(scalar(&m,id,&format!("SELECT sql FROM sqlite_schema WHERE name='{name}'")).contains(suffix));
    }
    assert_eq!(scalar(&m,id,"SELECT count(*) FROM sqlite_schema WHERE name LIKE '__kiln_schema_%'"),"0");
    assert_eq!(scalar(&m,id,"PRAGMA foreign_keys"),"1");
}

#[test]
fn sqlite_column_fk_set_null_and_literal_autoincrement_are_not_rewritten_as_constraints(){
    let (_d,m,id)=fixture();exec(&m,id,"CREATE TABLE owners (id INTEGER PRIMARY KEY)");
    exec(&m,id,"CREATE TABLE references_owner (owner INTEGER REFERENCES owners(id) ON DELETE SET NULL, note TEXT DEFAULT 'AUTOINCREMENT')");
    exec(&m,id,"INSERT INTO owners VALUES(1)");exec(&m,id,"INSERT INTO references_owner(owner) VALUES(1)");
    let t=table("references_owner");let action=alter(&m,id,&t,"owner",|s|s.nullable=false);apply(&m,id,&t,action);
    let definition=scalar(&m,id,"SELECT sql FROM sqlite_schema WHERE name='references_owner'");assert!(definition.contains("ON DELETE SET NULL"));assert!(definition.contains("NOT NULL"));
    // FK action is preserved: deleting the parent now fails NOT NULL, and cannot silently cascade.
    assert!(m.block_on(m.query(id,"DELETE FROM owners WHERE id=1",None)).is_err());assert_eq!(scalar(&m,id,"SELECT count(*) FROM owners"),"1");
}

#[test]
fn sqlite_plans_reject_stale_catalog_or_connection_and_unsafe_default_fragments(){
    let (_d,m,id)=fixture();exec(&m,id,"CREATE TABLE entries(id INTEGER PRIMARY KEY, body TEXT)");let t=table("entries");
    let action=||SchemaAction::AddColumn(ColumnSpec{name:"extra".into(),data_type:"TEXT".into(),nullable:true,default:None});
    let plan=m.block_on(m.prepare_schema_change(id,&t,action())).unwrap();exec(&m,id,"CREATE TABLE concurrent_change(id INTEGER)");
    assert!(m.block_on(m.apply_schema_change(id,plan)).is_err());
    let plan=m.block_on(m.prepare_schema_change(id,&t,action())).unwrap();m.disconnect(id);m.block_on(m.connect(id)).unwrap();
    assert!(m.block_on(m.apply_schema_change(id,plan)).is_err());
    for default in ["0 NOT NULL","0 UNIQUE","0 PRIMARY KEY","0; DROP TABLE entries","0 REFERENCES other(id)"]{
        assert!(m.block_on(m.prepare_schema_change(id,&t,SchemaAction::AddColumn(ColumnSpec{name:"bad".into(),data_type:"INTEGER".into(),nullable:true,default:Some(default.into())}))).is_err(),"accepted {default}");
    }
    assert_eq!(m.block_on(m.table_details(id,&t)).unwrap().columns.len(),2);
}

#[test]
fn sqlite_rename_add_column_indexes_and_drop_are_real_actions(){
    let (_d,m,id)=fixture();exec(&m,id,"CREATE TABLE entries(id INTEGER PRIMARY KEY, body TEXT)");let t=table("entries");
    apply(&m,id,&t,SchemaAction::AddColumn(ColumnSpec{name:"한글".into(),data_type:"TEXT".into(),nullable:true,default:Some("'x'".into())}));
    apply(&m,id,&t,SchemaAction::AddIndex(IndexSpec{name:"entries_body".into(),columns:vec![IndexColumn{name:"body".into(),descending:true}],unique:true}));
    let ix=m.block_on(m.table_details(id,&t)).unwrap().indexes;assert!(ix.iter().any(|i|i.name=="entries_body"&&i.unique));
    apply(&m,id,&t,SchemaAction::DropIndex{name:"entries_body".into()});
    apply(&m,id,&t,SchemaAction::RenameTable{name:"renamed".into()});let t=table("renamed");
    exec(&m,id,"INSERT INTO renamed(body) VALUES('test')");assert_eq!(scalar(&m,id,"SELECT \"한글\" FROM renamed"),"x");
    apply(&m,id,&t,SchemaAction::DropTable);assert_eq!(scalar(&m,id,"SELECT count(*) FROM sqlite_schema WHERE name='renamed'"),"0");
}

#[test]
fn sqlite_integer_primary_key_desc_does_not_lose_independent_rowid(){
    let (_d,m,id)=fixture();exec(&m,id,"CREATE TABLE unusual (id INTEGER PRIMARY KEY DESC, body TEXT)");exec(&m,id,"INSERT INTO unusual(rowid,id,body) VALUES(99,7,'old')");
    let t=table("unusual");let action=alter(&m,id,&t,"body",|s|s.default=Some("'new'".into()));apply(&m,id,&t,action);
    assert_eq!(scalar(&m,id,"SELECT rowid FROM unusual WHERE id=7"),"99");
}

#[test]
fn sqlite_reviewed_plan_cannot_be_mutated_before_execution(){
    let (_d,m,id)=fixture();exec(&m,id,"CREATE TABLE entries(id INTEGER PRIMARY KEY, body TEXT)");exec(&m,id,"INSERT INTO entries VALUES(7,'keep')");let t=table("entries");
    let original=m.block_on(m.prepare_schema_change(id,&t,SchemaAction::AddColumn(ColumnSpec{name:"extra".into(),data_type:"TEXT".into(),nullable:true,default:None}))).unwrap();
    for mutation in 0..5 {
        let mut plan=original.clone();
        match mutation {
            0=>plan.sql=vec!["DROP TABLE entries".into()],
            1=>plan.action=SchemaAction::DropTable,
            2=>plan.table=table("another_table"),
            3=>plan.epoch=plan.epoch.wrapping_add(1),
            _=>plan.driver=Driver::Postgres,
        }
        assert!(m.block_on(m.apply_schema_change(id,plan)).is_err(),"accepted mutation {mutation}");
        assert_eq!(scalar(&m,id,"SELECT body FROM entries WHERE id=7"),"keep");
        assert_eq!(m.block_on(m.table_details(id,&t)).unwrap().columns.len(),2);
    }
    // Rejections do not invalidate the original reviewed plan or leave a transaction open.
    m.block_on(m.apply_schema_change(id,original)).unwrap();
    assert_eq!(m.block_on(m.table_details(id,&t)).unwrap().columns.len(),3);
}

#[test]
fn sqlite_named_constraints_and_fk_set_default_keep_their_meaning(){
    let (_d,m,id)=fixture();
    exec(&m,id,"CREATE TABLE owners(id INTEGER PRIMARY KEY)");exec(&m,id,"INSERT INTO owners VALUES(0),(1)");
    exec(&m,id,"CREATE TABLE records(owner INTEGER CONSTRAINT fallback DEFAULT 0 CONSTRAINT owner_fk REFERENCES owners(id) ON DELETE SET DEFAULT, body TEXT CONSTRAINT body_required NOT NULL CONSTRAINT body_default DEFAULT 'old' CONSTRAINT body_nonempty CHECK(length(body)>0))");
    exec(&m,id,"INSERT INTO records(rowid,owner,body) VALUES(42,1,'kept')");
    let t=table("records");let action=alter(&m,id,&t,"body",|s|s.default=Some("'new'".into()));apply(&m,id,&t,action);
    let ddl=scalar(&m,id,"SELECT sql FROM sqlite_schema WHERE name='records'");
    for fragment in ["CONSTRAINT owner_fk", "ON DELETE SET DEFAULT", "CONSTRAINT body_nonempty"] {assert!(ddl.contains(fragment),"missing {fragment}: {ddl}");}
    exec(&m,id,"DELETE FROM owners WHERE id=1");assert_eq!(scalar(&m,id,"SELECT owner FROM records WHERE rowid=42"),"0");
    exec(&m,id,"INSERT INTO records(owner) VALUES(0)");assert_eq!(scalar(&m,id,"SELECT count(*) FROM records WHERE body='new'"),"1");
    assert!(m.block_on(m.query(id,"INSERT INTO records(owner,body) VALUES(0,'')",None)).is_err());
    assert!(m.block_on(m.query(id,"INSERT INTO records(owner,body) VALUES(0,NULL)",None)).is_err());
    // Editing the FK-bearing column must not interpret SET DEFAULT as its own DEFAULT clause.
    let action=alter(&m,id,&t,"owner",|s|s.nullable=false);apply(&m,id,&t,action);
    exec(&m,id,"INSERT INTO owners VALUES(2)");exec(&m,id,"INSERT INTO records(owner) VALUES(2)");exec(&m,id,"DELETE FROM owners WHERE id=2");
    assert_eq!(scalar(&m,id,"SELECT count(*) FROM records WHERE owner=0"),"3");
    assert_eq!(scalar(&m,id,"SELECT count(*) FROM pragma_foreign_key_check"),"0");
}

struct ServerFixture { m:DbManager,id:ConnId,namespace:String,driver:Driver }
impl Drop for ServerFixture {fn drop(&mut self){let keyword=if self.driver==Driver::Postgres{"SCHEMA"}else{"DATABASE"};let cascade=if self.driver==Driver::Postgres{" CASCADE"}else{""};let _=self.m.block_on(self.m.query(self.id,&format!("DROP {keyword} {}{cascade}",self.namespace),None));self.m.disconnect(self.id);}}
fn server_fixture(driver:Driver)->Option<ServerFixture>{
    let var=match driver {Driver::Postgres=>"KILN_TEST_PG_URL",Driver::MariaDb=>"KILN_TEST_MARIA_URL",_=>"KILN_TEST_MYSQL_URL"};
    let Ok(url)=std::env::var(var) else{eprintln!("{var} absent: server schema test not exercised");return None;};
    let m=DbManager::in_memory();let id=m.import_url(&url).unwrap();
    if driver==Driver::MariaDb {let mut cfg=m.get(id).unwrap();cfg.driver=driver;m.update(cfg,None);}
    let namespace=format!("kiln_schema_review_{}",std::process::id());let keyword=if driver==Driver::Postgres{"SCHEMA"}else{"DATABASE"};
    // No IF NOT EXISTS: never adopt or drop a pre-existing namespace.
    exec(&m,id,&format!("CREATE {keyword} {namespace}"));Some(ServerFixture{m,id,namespace,driver})
}
#[test]
fn postgres_schema_changes_preserve_constraints_and_rollback_failed_ddl(){
    let Some(f)=server_fixture(Driver::Postgres) else{return;};let m=&f.m;let id=f.id;let ns=&f.namespace;let t=TableRef::new(Some(ns.clone()),"items");
    exec(m,id,&format!("CREATE TABLE {ns}.items (id INTEGER PRIMARY KEY, name VARCHAR(40) NOT NULL DEFAULT 'seed' CHECK(length(name)>0), note TEXT)"));
    exec(m,id,&format!("INSERT INTO {ns}.items VALUES(7,'persisted',NULL)"));
    let action=alter(m,id,&t,"name",|s|s.default=Some("'next'".into()));apply(m,id,&t,action);
    assert_eq!(scalar(m,id,&format!("SELECT name FROM {ns}.items WHERE id=7")),"persisted");
    let action=alter(m,id,&t,"note",|s|s.nullable=false);let plan=m.block_on(m.prepare_schema_change(id,&t,action)).unwrap();assert!(m.block_on(m.apply_schema_change(id,plan)).is_err());
    assert!(m.block_on(m.table_details(id,&t)).unwrap().columns.iter().find(|c|c.name=="note").unwrap().nullable);
    apply(m,id,&t,SchemaAction::AddIndex(IndexSpec{name:"items_name".into(),columns:vec![IndexColumn{name:"name".into(),descending:true}],unique:true}));
    assert!(m.block_on(m.table_details(id,&t)).unwrap().indexes.iter().any(|i|i.name=="items_name"&&i.unique));
    assert!(m.block_on(m.query(id,&format!("INSERT INTO {ns}.items(id,name) VALUES(8,'')"),None)).is_err());
}
#[test]
fn mysql_schema_changes_preserve_collation_comments_and_existing_data(){mysql_family_changes(Driver::MySql);}
#[test]
fn mariadb_schema_changes_preserve_collation_comments_and_existing_data(){mysql_family_changes(Driver::MariaDb);}
fn mysql_family_changes(driver:Driver){
    let Some(f)=server_fixture(driver) else{return;};let m=&f.m;let id=f.id;let ns=&f.namespace;let t=TableRef::new(Some(ns.clone()),"items");
    exec(m,id,&format!("CREATE TABLE {ns}.items (id INT PRIMARY KEY, name VARCHAR(40) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL DEFAULT 'seed' COMMENT 'keep me', note TEXT)"));
    exec(m,id,&format!("INSERT INTO {ns}.items VALUES(7,'persisted',NULL)"));
    let action=alter(m,id,&t,"name",|s|s.data_type="varchar(80)".into());apply(m,id,&t,action);
    assert_eq!(scalar(m,id,&format!("SELECT name FROM {ns}.items WHERE id=7")),"persisted");
    let ddl=m.block_on(m.table_ddl(id,&t)).unwrap();assert!(ddl.contains("keep me"));assert!(ddl.contains("utf8mb4_bin"));
    apply(m,id,&t,SchemaAction::AddIndex(IndexSpec{name:"items_name".into(),columns:vec![IndexColumn{name:"name".into(),descending:true}],unique:true}));
    assert!(m.block_on(m.table_details(id,&t)).unwrap().indexes.iter().any(|i|i.name=="items_name"&&i.unique));
}

#[test]
fn sqlite_drop_column_refuses_silent_index_or_fk_removal() {
    let (_d,m,id)=fixture();exec(&m,id,"CREATE TABLE parent(id INTEGER PRIMARY KEY)");
    exec(&m,id,"CREATE TABLE entry(id INTEGER PRIMARY KEY, body TEXT, owner INTEGER REFERENCES parent(id), extra TEXT)");
    exec(&m,id,"CREATE INDEX entry_body ON entry(body)");let t=table("entry");
    for name in ["body","owner"] {assert!(m.block_on(m.prepare_schema_change(id,&t,SchemaAction::DropColumn{column:name.into()})).is_err());}
    apply(&m,id,&t,SchemaAction::DropColumn{column:"extra".into()});
    assert_eq!(m.block_on(m.table_details(id,&t)).unwrap().columns.len(),3);
}
