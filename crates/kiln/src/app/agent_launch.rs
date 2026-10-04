//! Keep long requests out of the terminal line discipline and shell history.
use std::{fs,io::Write,path::{Path,PathBuf},sync::atomic::{AtomicU64,Ordering}};

pub const MAX_REQUEST_CHARS:usize=32_000;
const MAX_PAYLOAD_BYTES:usize=120_000;
static SEQUENCE:AtomicU64=AtomicU64::new(0);

pub struct Prepared { pub command:String, pub title:String, pub request_path:PathBuf, pub request_offset:usize }
fn quote(value:&str)->String {format!("'{}'",value.replace('\'',"'\\''"))}
fn executable(name:&str)->Option<PathBuf>{
    let mut dirs:Vec<_>=std::env::var_os("PATH").map(|p|std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home)=std::env::var_os("HOME").or_else(||std::env::var_os("USERPROFILE")) {dirs.push(PathBuf::from(home).join(".local/bin"));}
    dirs.extend([PathBuf::from("/opt/homebrew/bin"),PathBuf::from("/usr/local/bin")]);
    dirs.into_iter().flat_map(|dir| {
        let mut paths=vec![dir.join(name)];
        if cfg!(windows){paths.extend([dir.join(format!("{name}.exe")),dir.join(format!("{name}.cmd"))]);}
        paths
    }).find(|p|{
        let Ok(metadata)=p.metadata() else{return false;};
        if !metadata.is_file(){return false;}
        #[cfg(unix)]{use std::os::unix::fs::PermissionsExt;metadata.permissions().mode()&0o111!=0}
        #[cfg(not(unix))]{true}
    })
}

pub fn prepare(program:&str,context:&str,request:&str)->Result<Prepared,String>{
    if !matches!(program,"codex"|"claude"){return Err(kiln_common::i18n::tr("지원하지 않는 에이전트입니다.").into());}
    let binary=executable(program).ok_or_else(||kiln_common::trf!("{program}을 찾을 수 없습니다. 설치 후 다시 시작해 주세요."))?;
    prepare_in(&kiln_common::paths::config_dir().join("agent-requests"),&binary,context,request)
}
fn prepare_in(dir:&Path,binary:&Path,context:&str,request:&str)->Result<Prepared,String>{
    if request.trim().is_empty(){return Err(kiln_common::i18n::tr("요청을 입력해 주세요.").into());}
    if request.chars().count()>MAX_REQUEST_CHARS{return Err(kiln_common::i18n::tr("요청은 32,000자 이내로 입력해 주세요.").into());}
    let payload=format!("{context}\n## 사용자 요청\n{request}");
    if payload.contains('\0') || payload.len()>MAX_PAYLOAD_BYTES {return Err(kiln_common::i18n::tr("요청과 저장소 정보가 너무 큽니다. 요청을 나누어 시작해 주세요.").into());}
    // The shell starts in the workspace, which need not be the GUI process cwd.
    let dir=std::path::absolute(dir).map_err(|e|kiln_common::trf!("요청 폴더를 확인할 수 없습니다: {e}"))?;
    let binary=std::path::absolute(binary).map_err(|e|kiln_common::trf!("에이전트 경로를 확인할 수 없습니다: {e}"))?;
    let mut builder=fs::DirBuilder::new();builder.recursive(true);
    #[cfg(unix)]{use std::os::unix::fs::DirBuilderExt;builder.mode(0o700);}
    builder.create(&dir).map_err(|e|kiln_common::trf!("요청을 보관할 수 없습니다: {e}"))?;
    let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let path=dir.join(format!("request-{now}-{}-{}.md",std::process::id(),SEQUENCE.fetch_add(1,Ordering::Relaxed)));
    let mut options=fs::OpenOptions::new();options.write(true).create_new(true);
    #[cfg(unix)]{use std::os::unix::fs::OpenOptionsExt;options.mode(0o600);}
    let result=(||{let mut file=options.open(&path)?;file.write_all(payload.as_bytes())?;file.sync_all()})();
    if let Err(error)=result{let _=fs::remove_file(&path);return Err(kiln_common::trf!("요청을 저장할 수 없습니다: {error}"));}
    // The durable request remains available to an explicitly retried launch after restart.
    #[cfg(not(windows))]
    let command=format!("/bin/sh -c {}",quote(&format!("request=$(/bin/cat {} && printf .) && exec {} \"${{request%.}}\"",quote(&path.to_string_lossy()),quote(&binary.to_string_lossy()))));
    #[cfg(windows)]
    let command={
        use base64::Engine;
        let ps_quote=|s:&str|format!("'{}'",s.replace('\'',"''"));
        let script=format!("& {} (Get-Content -LiteralPath {} -Raw -Encoding UTF8)",ps_quote(&binary.to_string_lossy()),ps_quote(&path.to_string_lossy()));
        let bytes:Vec<u8>=script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        format!("powershell.exe -NoProfile -EncodedCommand {}",base64::engine::general_purpose::STANDARD.encode(bytes))
    };
    let line=request.lines().find(|line|!line.trim().is_empty()).unwrap_or(request).trim();
    let mut title:String=line.chars().take(40).collect();if line.chars().count()>40{title.push('…');}
    Ok(Prepared{command,title,request_path:path,request_offset:context.len()+"\n## 사용자 요청\n".len()})
}

#[cfg(all(test,unix))]
mod tests{
    use super::*;
    #[test]
    fn long_unicode_request_roundtrips_without_shell_expansion(){
        use std::os::unix::fs::PermissionsExt;
        let dir=tempfile::tempdir().unwrap();let binary=dir.path().join("capture agent's input");
        fs::write(&binary,"#!/bin/sh\nprintf '%s' \"$1\" > \"$CAPTURE_FILE\"\n").unwrap();fs::set_permissions(&binary,fs::Permissions::from_mode(0o700)).unwrap();
        let request=format!("프론트와 백엔드를 함께 개선해주세요.\n{}\n' \" $(touch PWNED) `touch PWNED` ; 끝\n\n","한글 요청 ".repeat(1800));
        let context="루트: personal\n- frontend\n- backend";
        let prepared=prepare_in(&dir.path().join("private request's"),&binary,context,&request).unwrap();
        assert!(prepared.command.len()<1024);assert!(!prepared.command.contains("한글"));
        let output=dir.path().join("received");let status=std::process::Command::new("/bin/sh").args(["-c",&prepared.command]).env("CAPTURE_FILE",&output).current_dir(dir.path()).status().unwrap();
        assert!(status.success());assert_eq!(fs::read_to_string(output).unwrap(),format!("{context}\n## 사용자 요청\n{request}"));assert!(!dir.path().join("PWNED").exists());
        let payload=fs::read_dir(dir.path().join("private request's")).unwrap().next().unwrap().unwrap().path();assert_eq!(payload.metadata().unwrap().permissions().mode()&0o777,0o600);
    }
    #[test]
    fn relative_executable_and_config_paths_survive_workspace_cwd() {
        use std::os::unix::fs::PermissionsExt;
        let dir=tempfile::tempdir_in(".").unwrap();
        let relative=PathBuf::from(dir.path().file_name().unwrap());
        let binary=relative.join("agent");
        fs::write(&binary,"#!/bin/sh\nprintf '%s' \"$1\" > \"$CAPTURE_FILE\"\n").unwrap();
        fs::set_permissions(&binary,fs::Permissions::from_mode(0o700)).unwrap();
        let requests=relative.join("requests");
        assert!(requests.is_relative());assert!(binary.is_relative());
        let prepared=prepare_in(&requests,&binary,"parent workspace","front and back").unwrap();
        let workspace=tempfile::tempdir().unwrap();let output=workspace.path().join("captured");
        let status=std::process::Command::new("/bin/sh").args(["-c",&prepared.command])
            .current_dir(workspace.path()).env("CAPTURE_FILE",&output).status().unwrap();
        assert!(status.success());assert_eq!(fs::read_to_string(output).unwrap(),"parent workspace\n## 사용자 요청\nfront and back");
    }

    #[test]
    fn invalid_request_never_creates_a_launch_artifact(){
        let dir=tempfile::tempdir().unwrap();let requests=dir.path().join("requests");
        for request in [" ".to_owned(),"가".repeat(MAX_REQUEST_CHARS+1),"bad\0input".into()]{assert!(prepare_in(&requests,Path::new("/bin/echo"),"context",&request).is_err());}
        assert!(!requests.exists());
    }
}
