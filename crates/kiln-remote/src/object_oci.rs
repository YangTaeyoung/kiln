//! Native OCI API signing is delegated to the official installed CLI.
use crate::*;
use serde_json::Value;

struct Oci<'a> {
    profile: &'a ConnectionProfile,
    control: &'a Control,
    #[cfg(test)]
    fixture: Option<(&'a Path, &'a [(String, String)])>,
}
impl Oci<'_> {
    fn args(&self, command: &str, extra: &[String]) -> Result<Vec<String>> {
        let RemoteEndpoint::ObjectStorage {
            bucket,
            authentication:
                ObjectAuthentication::Cli {
                    profile,
                    config_path,
                },
            region,
            namespace,
            ..
        } = &self.profile.endpoint
        else {
            bail!("OCI requires a native CLI profile");
        };
        let resolved = crate::object_profiles::oracle_config_path(config_path.as_deref());
        let mut args = vec![
            format!("--profile={profile}"),
            format!(
                "--auth={}",
                crate::object_profiles::oracle_auth_mode(profile, Some(&resolved))?
            ),
            "--output=json".into(),
            "--connection-timeout=15".into(),
            "--read-timeout=60".into(),
        ];
        args.push(format!("--config-file={}", resolved.to_string_lossy()));
        if let Some(r) = region.as_ref().filter(|r| !r.is_empty()) {
            args.push(format!("--region={r}"));
        }
        args.extend([
            "os".into(),
            "object".into(),
            command.into(),
            format!("--bucket-name={bucket}"),
        ]);
        if let Some(n) = namespace.as_ref().filter(|n| !n.is_empty()) {
            args.push(format!("--namespace-name={n}"));
        }
        args.extend_from_slice(extra);
        Ok(args)
    }
    fn call(&self, command: &str, extra: &[String]) -> Result<Vec<u8>> {
        #[cfg(test)]
        if let Some((executable, env)) = self.fixture {
            return crate::cloud_cli::execute_at(
                ObjectProvider::Oracle,
                executable,
                &self.args(command, extra)?,
                env,
                self.control,
                Duration::from_secs(3),
                1024 * 1024,
            );
        }
        crate::cloud_cli::execute(
            ObjectProvider::Oracle,
            &self.args(command, extra)?,
            &[],
            self.control,
        )
    }
    fn json(&self, command: &str, extra: &[String]) -> Result<Value> {
        serde_json::from_slice(&self.call(command, extra)?)
            .map_err(|_| anyhow::anyhow!("Invalid OCI response"))
    }
    fn list(
        &self,
        path: &str,
        cursor: Option<&str>,
        folders: bool,
        limit: u32,
    ) -> Result<ListPage> {
        let p = super::object_backend::key(path)?;
        let mut args = vec![
            format!("--prefix={p}"),
            format!("--limit={limit}"),
            "--fields=name,size,timeModified".into(),
        ];
        if folders {
            args.push("--delimiter=/".into());
        }
        if let Some(c) = cursor {
            safe_text(c)?;
            args.push(format!("--start={c}"));
        }
        let mut page = parse_list(&self.json("list", &args)?)?;
        if folders {
            page.entries.retain(|e| e.path != p);
        }
        Ok(page)
    }
    fn exists(&self, path: &str) -> Result<bool> {
        let p = self.list(path, None, false, 2)?;
        Ok(p.entries.iter().any(|e| e.path == path))
    }
    fn upload(&self, local: &Path, path: &str, overwrite: bool) -> Result<()> {
        let size = std::fs::metadata(local)?.len();
        self.control.total(size);
        if !overwrite && self.exists(path)? {
            bail!("A file with this name already exists");
        }
        // Single-request PUT: cancelling does not leave an unfinished multipart upload.
        let args = vec![
            format!("--file={}", local.to_string_lossy()),
            format!("--name={path}"),
            "--no-multipart".into(),
            if overwrite {
                "--force".into()
            } else {
                "--no-overwrite".into()
            },
        ];
        let bytes = self.call("put", &args)?;
        // OCI --no-overwrite exits 0 with no JSON when it skips an existing object.
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("OCI did not upload the file; it may already exist"))?;
        if value
            .get("etag")
            .or_else(|| value.get("data").and_then(|d| d.get("etag")))
            .is_none()
        {
            bail!("OCI did not confirm the uploaded object");
        }
        self.control.advance(size);
        Ok(())
    }
}
pub(super) fn run(
    profile: &ConnectionProfile,
    op: &Operation,
    c: &Control,
) -> Result<RemoteResult> {
    let o = Oci {
        profile,
        control: c,
        #[cfg(test)]
        fixture: None,
    };
    run_with(o, op)
}
fn run_with(o: Oci<'_>, op: &Operation) -> Result<RemoteResult> {
    let c = o.control;
    match op {
        Operation::List { path, cursor } => Ok(RemoteResult::Listed(o.list(
            path,
            cursor.as_deref(),
            true,
            500,
        )?)),
        Operation::Stat { path } => {
            let p = super::object_backend::require_file(path)?;
            let v = o.json("head", &[format!("--name={p}")])?;
            Ok(RemoteResult::Entry(RemoteEntry {
                name: leaf(p),
                path: p.into(),
                is_dir: false,
                size: v
                    .get("content-length")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                modified: v
                    .get("last-modified")
                    .and_then(Value::as_str)
                    .map(|s| modified_string(s.into())),
            }))
        }
        Operation::Download { path, local } => {
            let p = super::object_backend::require_file(path)?;
            let temp = destination_temp(local)?;
            o.call(
                "get",
                &[
                    format!("--name={p}"),
                    format!("--file={}", temp.path().to_string_lossy()),
                    "--no-multipart".into(),
                ],
            )?;
            c.check()?;
            let size = temp.as_file().metadata()?.len();
            c.total(size);
            temp.as_file().sync_all()?;
            temp.persist(local).map_err(|e| e.error)?;
            c.advance(size);
            Ok(RemoteResult::Done)
        }
        Operation::Upload {
            local,
            path,
            overwrite,
        } => {
            o.upload(
                local,
                super::object_backend::require_file(path)?,
                *overwrite,
            )?;
            Ok(RemoteResult::Done)
        }
        Operation::Rename {
            from,
            to,
            overwrite,
        } => {
            let a = super::object_backend::require_file(from)?;
            let b = super::object_backend::require_file(to)?;
            if a == b {
                return Ok(RemoteResult::Done);
            };
            let mut args = vec![format!("--name={a}"), format!("--new-name={b}")];
            if !overwrite {
                args.push("--new-if-none-match=*".into());
            }
            o.call("rename", &args)?;
            Ok(RemoteResult::Done)
        }
        Operation::Delete { path, is_dir } => {
            let p = super::object_backend::key(path)?;
            if p.is_empty() {
                bail!("Cannot delete the bucket root");
            }
            let p = if *is_dir {
                format!("{}/", p.trim_end_matches('/'))
            } else {
                super::object_backend::require_file(path)?.into()
            };
            if *is_dir {
                let page = o.list(&p, None, false, 2)?;
                if page.next_cursor.is_some() || page.entries.iter().any(|e| e.path != p) {
                    bail!("Folder is not empty");
                }
                if page.entries.is_empty() {
                    return Ok(RemoteResult::Done);
                }
            }
            o.call("delete", &[format!("--name={p}"), "--force".into()])?;
            Ok(RemoteResult::Done)
        }
        Operation::CreateDir { path } => {
            let p = super::object_backend::key(path)?;
            if p.is_empty() {
                bail!("A folder name is required");
            }
            let p = format!("{}/", p.trim_end_matches('/'));
            let page = o.list(&p, None, false, 2)?;
            if !page.entries.is_empty() {
                return Ok(RemoteResult::Done);
            }
            let temp = tempfile::NamedTempFile::new()?;
            o.upload(temp.path(), &p, false)?;
            Ok(RemoteResult::Done)
        }
    }
}
fn parse_list(v: &Value) -> Result<ListPage> {
    let data = v.get("data").context("Missing OCI list data")?;
    let objects = data
        .get("objects")
        .and_then(Value::as_array)
        .context("Missing OCI objects")?;
    let mut entries = Vec::new();
    for o in objects {
        let name = o
            .get("name")
            .and_then(Value::as_str)
            .context("Missing OCI object name")?;
        entries.push(RemoteEntry {
            name: leaf(name),
            path: name.into(),
            is_dir: name.ends_with('/'),
            size: o.get("size").and_then(Value::as_u64).unwrap_or(0),
            modified: o
                .get("time-modified")
                .and_then(Value::as_str)
                .map(|s| modified_string(s.into())),
        });
    }
    if let Some(prefixes) = data.get("prefixes").and_then(Value::as_array) {
        for p in prefixes {
            if let Some(p) = p.as_str() {
                entries.push(RemoteEntry {
                    name: leaf(p),
                    path: p.into(),
                    is_dir: true,
                    size: 0,
                    modified: None,
                });
            }
        }
    }
    // next-start-with is part of ListObjects, not the HTTP opc-next-page header.
    let next_cursor = data
        .get("next-start-with")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Ok(ListPage {
        entries: sorted(entries),
        next_cursor,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oci_pagination_and_explicit_profile_argv() {
        let v = serde_json::json!({"data":{"objects":[{"name":"folder/한글 [file]","size":9,"time-modified":"2026-01-01T00:00:00Z"}],"prefixes":["folder/child/"],"next-start-with":"folder/next"}});
        let page = parse_list(&v).unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("folder/next"));
        assert!(page.entries[0].is_dir);
        assert_eq!(page.entries[1].size, 9);
        let profile = ConnectionProfile {
            id: "test".into(),
            name: "test".into(),
            endpoint: RemoteEndpoint::ObjectStorage {
                provider: ObjectProvider::Oracle,
                bucket: "bucket".into(),
                prefix: "".into(),
                authentication: ObjectAuthentication::Cli {
                    profile: "TEAM".into(),
                    config_path: Some(PathBuf::from("/private/config")),
                },
                region: None,
                namespace: None,
                account_id: None,
            },
        };
        let c = Control::default();
        let args = Oci {
            profile: &profile,
            control: &c,
            fixture: None,
        }
        .args("get", &["--name=-x;rm -rf fake".into()])
        .unwrap();
        assert!(args.contains(&"--profile=TEAM".into()));
        assert!(args.contains(&"--config-file=/private/config".into()));
        assert_eq!(args.last().unwrap(), "--name=-x;rm -rf fake");
        assert!(!args.iter().any(|a| a.starts_with("--namespace")));
    }
    #[cfg(unix)]
    #[test]
    fn native_oci_worker_uses_real_owned_cli_fixture_for_all_operations() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let cli = d.path().join("oci-fixture");
        let capture = d.path().join("capture");
        std::fs::write(&cli,r#"#!/bin/sh
command=''
file=''
prefix=''
for arg do
  printf '%s\n' "$arg" >> "$KILN_FIXTURE_CAPTURE"
  case "$arg" in
    list|head|get|put|rename|delete) command="$arg";;
    --file=*) file="${arg#--file=}";;
    --prefix=*) prefix="${arg#--prefix=}";;
  esac
done
printf 'END\n' >> "$KILN_FIXTURE_CAPTURE"
case "$command" in
  list) if [ "$prefix" = 'nonempty/' ]; then printf '{"data":{"objects":[{"name":"nonempty/file","size":3}],"prefixes":[]}}'; else printf '{"data":{"objects":[],"prefixes":[]}}'; fi;;
  head) printf '{"content-length":"3","last-modified":"Tue, 06 Oct 2026 10:00:00 GMT"}';;
  get) printf abc > "$file";printf '{}';;
  put) if [ "$KILN_FIXTURE_SKIP" = 1 ];then exit 0;fi;printf '{"etag":"fixture-etag"}';;
  rename|delete) printf '{}';;
  *) exit 99;;
esac
"#).unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        let p = ConnectionProfile {
            id: "fixture".into(),
            name: "Fixture".into(),
            endpoint: RemoteEndpoint::ObjectStorage {
                provider: ObjectProvider::Oracle,
                bucket: "bucket".into(),
                prefix: "".into(),
                authentication: ObjectAuthentication::Cli {
                    profile: "TEAM".into(),
                    config_path: Some(d.path().join("config")),
                },
                region: Some("us-ashburn-1".into()),
                namespace: Some("fixture-namespace".into()),
                account_id: None,
            },
        };
        let c = Control::default();
        let env = vec![(
            "KILN_FIXTURE_CAPTURE".into(),
            capture.to_string_lossy().into_owned(),
        )];
        let local = d.path().join("download");
        std::fs::write(&local, "original").unwrap();
        for operation in [
            Operation::List {
                path: "folder/".into(),
                cursor: Some("next-key".into()),
            },
            Operation::Stat {
                path: "-literal; file".into(),
            },
            Operation::Download {
                path: "-literal; file".into(),
                local: local.clone(),
            },
            Operation::Upload {
                local: local.clone(),
                path: "upload".into(),
                overwrite: false,
            },
            Operation::Rename {
                from: "source".into(),
                to: "target".into(),
                overwrite: false,
            },
            Operation::Delete {
                path: "target".into(),
                is_dir: false,
            },
            Operation::CreateDir {
                path: "new-folder".into(),
            },
        ] {
            run_with(
                Oci {
                    profile: &p,
                    control: &c,
                    fixture: Some((&cli, &env)),
                },
                &operation,
            )
            .unwrap();
        }
        assert_eq!(std::fs::read(&local).unwrap(), b"abc");
        let text = std::fs::read_to_string(&capture).unwrap();
        assert!(text.contains("--profile=TEAM\n"));
        assert!(text.contains("--namespace-name=fixture-namespace\n"));
        assert!(text.contains("--name=-literal; file\n"));
        assert!(text.contains("--start=next-key\n"));
        assert!(text.contains("--new-if-none-match=*\n"));
        assert!(text.contains("--no-multipart\n"));
        assert!(!text.contains("--force\nput"));
        let before = std::fs::read_to_string(&capture)
            .unwrap()
            .matches("\ndelete\n")
            .count();
        let result = run_with(
            Oci {
                profile: &p,
                control: &c,
                fixture: Some((&cli, &env)),
            },
            &Operation::Delete {
                path: "nonempty".into(),
                is_dir: true,
            },
        );
        assert!(result.unwrap_err().to_string().contains("not empty"));
        assert_eq!(
            std::fs::read_to_string(&capture)
                .unwrap()
                .matches("\ndelete\n")
                .count(),
            before
        );
        let skip_env = vec![
            (
                "KILN_FIXTURE_CAPTURE".into(),
                capture.to_string_lossy().into_owned(),
            ),
            ("KILN_FIXTURE_SKIP".into(), "1".into()),
        ];
        let result = run_with(
            Oci {
                profile: &p,
                control: &c,
                fixture: Some((&cli, &skip_env)),
            },
            &Operation::Upload {
                local,
                path: "raced-file".into(),
                overwrite: false,
            },
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("may already exist")
        );
    }
}
