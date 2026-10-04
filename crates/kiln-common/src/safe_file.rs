//! Replace document contents only after writing and syncing a sibling temporary file.
//! Symbolic links are followed, never replaced. Existing Unix mode bits are retained.
use std::{fs::{self, File, OpenOptions}, io::{self, Write}, path::Path};

pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace_with(path, |file| file.write_all(bytes))
}

fn replace_with(path: &Path, write: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
    let target = match fs::symlink_metadata(path) {
        Ok(_) => fs::canonicalize(path)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            fs::canonicalize(parent)?.join(path.file_name().ok_or_else(|| io::Error::other(kiln_common::i18n::tr("파일 이름이 없습니다")))?)
        }
        Err(e) => return Err(e),
    };
    let metadata = match fs::metadata(&target) { Ok(m) => Some(m), Err(e) if e.kind()==io::ErrorKind::NotFound => None, Err(e)=>return Err(e) };
    if let Some(m) = &metadata {
        if !m.is_file() || m.permissions().readonly() { return Err(io::Error::new(io::ErrorKind::PermissionDenied,kiln_common::i18n::tr("일반 쓰기 가능 파일만 안전하게 저장할 수 있습니다"))); }
        #[cfg(unix)] { use std::os::unix::fs::MetadataExt; if m.nlink()>1 { return Err(io::Error::other(kiln_common::i18n::tr("하드 링크 파일은 별도 파일로 저장하세요. 기존 연결은 변경하지 않았습니다."))); } }
    }
    let original = if metadata.is_some() { Some(fs::read(&target)?) } else { None };
    let parent=target.parent().ok_or_else(|| io::Error::other(kiln_common::i18n::tr("상위 폴더가 없습니다")))?;
    static SEQ: std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
    let mut temp=None;
    for _ in 0..100 {
        let candidate=parent.join(format!(".kiln-save-{}-{}",std::process::id(),SEQ.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));
        let mut opts=OpenOptions::new();opts.write(true).create_new(true);
        #[cfg(unix)] {use std::os::unix::fs::OpenOptionsExt;opts.mode(0o600);}
        match opts.open(&candidate) { Ok(file)=>{temp=Some((candidate,file));break},Err(e) if e.kind()==io::ErrorKind::AlreadyExists=>continue,Err(e)=>return Err(e) }
    }
    let (temp_path,mut file)=temp.ok_or_else(|| io::Error::other(kiln_common::i18n::tr("임시 파일 생성 충돌")))?;
    let result=(|| {
        write(&mut file)?;
        if let Some(m)=&metadata {
            #[cfg(target_os="macos")]
            copy_metadata(&File::open(&target)?,&file)?;
            #[cfg(not(target_os="macos"))]
            file.set_permissions(m.permissions())?;
            let _=m;
            file.set_modified(std::time::SystemTime::now())?;
        }
        file.sync_all()?;
        let current=match fs::read(&target){Ok(b)=>Some(b),Err(e) if e.kind()==io::ErrorKind::NotFound=>None,Err(e)=>return Err(e)};
        if current!=original {return Err(io::Error::other(kiln_common::i18n::tr("저장 중 파일이 변경되어 덮어쓰기를 중단했습니다")));}
        // A retargeted symlink must not silently save a different file.
        if metadata.is_some() && fs::canonicalize(path)?!=target {return Err(io::Error::other(kiln_common::i18n::tr("저장 중 파일 경로가 변경되었습니다")));}
        fs::rename(&temp_path,&target)?;
        #[cfg(unix)] File::open(parent)?.sync_all().map_err(|e|io::Error::other(kiln_common::trf!("파일은 저장됐지만 디스크 동기화를 확인하지 못했습니다: {e}")))?;
        Ok(())
    })();
    if result.is_err(){let _=fs::remove_file(&temp_path);}
    result
}

#[cfg(target_os="macos")]
fn copy_metadata(source:&File,destination:&File)->io::Result<()> {
    use std::os::fd::AsRawFd;
    // macOS SDK copyfile.h: COPYFILE_METADATA = ACL | STAT | XATTR (bits 0..2).
    // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/copyfile.3.html
    unsafe extern "C" {fn fcopyfile(from:std::ffi::c_int,to:std::ffi::c_int,state:*mut std::ffi::c_void,flags:u32)->std::ffi::c_int;}
    // Both descriptors are owned, live regular files. Null state is supported by the public API.
    if unsafe {fcopyfile(source.as_raw_fd(),destination.as_raw_fd(),std::ptr::null_mut(),7)}<0{return Err(io::Error::last_os_error());}
    Ok(())
}

#[cfg(test)] mod tests {
    use super::*;
    fn fixture() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        loop {
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("kiln-safe-file-{}-{id}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return path,
                // A previous test process can leave directories behind after a crash.
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create test fixture {}: {error}", path.display()),
            }
        }
    }
    #[test]
    fn parallel_fixtures_have_distinct_directories() {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));
        let threads: Vec<_> = (0..16).map(|_| {
            let barrier = barrier.clone();
            std::thread::spawn(move || { barrier.wait(); fixture() })
        }).collect();
        let paths: Vec<_> = threads.into_iter().map(|thread| thread.join().unwrap()).collect();
        assert_eq!(paths.iter().collect::<std::collections::HashSet<_>>().len(), paths.len());
        for path in paths { fs::remove_dir(path).unwrap(); }
    }
    #[test] fn partial_write_failure_preserves_original_and_cleans_temporary_file(){
        let dir=fixture();let path=dir.join("a");fs::write(&path,b"original").unwrap();
        let error=replace_with(&path,|f|{f.write_all(b"partial")?;Err(io::Error::other("injected disk-full failure"))});
        assert!(error.is_err());assert_eq!(fs::read(&path).unwrap(),b"original");assert_eq!(fs::read_dir(&dir).unwrap().count(),1);fs::remove_dir_all(dir).unwrap();
    }
    #[test] fn external_change_before_commit_is_not_overwritten(){
        let dir=fixture();let path=dir.join("a");fs::write(&path,b"original").unwrap();
        assert!(replace_with(&path,|f|{f.write_all(b"mine")?;fs::write(&path,b"external")}).is_err());
        assert_eq!(fs::read(&path).unwrap(),b"external");fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(target_os="macos")] #[test] fn mac_metadata_acl_owner_and_extended_attributes_survive(){
        use std::os::unix::fs::MetadataExt;
        let dir=fixture();let path=dir.join("document");fs::write(&path,b"old").unwrap();
        assert!(std::process::Command::new("/usr/bin/xattr").args(["-w","com.kiln.save-test","preserved"]).arg(&path).status().unwrap().success());
        assert!(std::process::Command::new("/bin/chmod").args(["+a","everyone allow read"]).arg(&path).status().unwrap().success());
        let before=fs::metadata(&path).unwrap();
        let acl_before=std::process::Command::new("/bin/ls").arg("-le").arg(&path).output().unwrap();
        write(&path,b"new content").unwrap();
        let after=fs::metadata(&path).unwrap();assert_eq!((before.uid(),before.gid(),before.mode()),(after.uid(),after.gid(),after.mode()));
        let attr=std::process::Command::new("/usr/bin/xattr").args(["-p","com.kiln.save-test"]).arg(&path).output().unwrap();assert_eq!(String::from_utf8_lossy(&attr.stdout).trim(),"preserved");
        let acl_after=std::process::Command::new("/bin/ls").arg("-le").arg(&path).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&acl_before.stdout).lines().skip(1).collect::<Vec<_>>(),String::from_utf8_lossy(&acl_after.stdout).lines().skip(1).collect::<Vec<_>>());
        assert_eq!(fs::read(&path).unwrap(),b"new content");fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(unix)] #[test] fn symlink_and_executable_permissions_are_preserved(){
        use std::os::unix::fs::{symlink,PermissionsExt};
        let dir=fixture();let target=dir.join("target");let link=dir.join("link");fs::write(&target,b"old").unwrap();fs::set_permissions(&target,fs::Permissions::from_mode(0o750)).unwrap();symlink("target",&link).unwrap();
        write(&link,b"new").unwrap();assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());assert_eq!(fs::read(&target).unwrap(),b"new");assert_eq!(fs::metadata(&target).unwrap().permissions().mode()&0o777,0o750);fs::remove_dir_all(dir).unwrap();
    }
}
