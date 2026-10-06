//! Opt-in isolated protocol fixtures. Never reads user SSH config or AWS credentials.
use kiln_remote::*;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
fn execute(p: &ConnectionProfile, s: &Secrets, op: Operation) -> anyhow::Result<RemoteResult> {
    let job = spawn(p.clone(), s.clone(), op);
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(result) = job.try_recv() {
            return result;
        }
        assert!(Instant::now() < deadline, "Remote fixture timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn exercise(p: ConnectionProfile, s: Secrets, root: &str) {
    let local = tempfile::tempdir().unwrap();
    let source = local.path().join("source.txt");
    let bytes = "Hello 한글 remote files\n".as_bytes();
    std::fs::write(&source, bytes).unwrap();
    let dir = format!(
        "{}/fixture-{}",
        root.trim_end_matches('/'),
        std::process::id()
    );
    execute(&p, &s, Operation::CreateDir { path: dir.clone() }).unwrap();
    let path = format!("{dir}/한글 [literal] with spaces.txt");
    execute(
        &p,
        &s,
        Operation::Upload {
            local: source.clone(),
            path: path.clone(),
            overwrite: false,
        },
    )
    .unwrap();
    assert!(
        execute(
            &p,
            &s,
            Operation::Upload {
                local: source.clone(),
                path: path.clone(),
                overwrite: false
            }
        )
        .is_err()
    );
    let RemoteResult::Listed(page) = execute(
        &p,
        &s,
        Operation::List {
            path: dir.clone(),
            cursor: None,
        },
    )
    .unwrap() else {
        panic!("Expected listing")
    };
    assert!(
        page.entries
            .iter()
            .any(|e| e.name == "한글 [literal] with spaces.txt" && e.size == bytes.len() as u64),
        "Unexpected listing: {:?}",
        page.entries
    );
    let dest = local.path().join("download.txt");
    std::fs::write(&dest, b"previous").unwrap();
    execute(
        &p,
        &s,
        Operation::Download {
            path: path.clone(),
            local: dest.clone(),
        },
    )
    .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), bytes);
    let replacement = b"Changed remote file\n";
    std::fs::write(&source, replacement).unwrap();
    execute(
        &p,
        &s,
        Operation::Upload {
            local: source.clone(),
            path: path.clone(),
            overwrite: true,
        },
    )
    .unwrap();
    execute(
        &p,
        &s,
        Operation::Download {
            path: path.clone(),
            local: dest.clone(),
        },
    )
    .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), replacement);
    let to = format!("{dir}/renamed.txt");
    execute(
        &p,
        &s,
        Operation::Rename {
            from: path,
            to: to.clone(),
            overwrite: false,
        },
    )
    .unwrap();
    assert!(
        execute(
            &p,
            &s,
            Operation::Delete {
                path: dir.clone(),
                is_dir: true
            }
        )
        .is_err()
    );
    let RemoteResult::Entry(entry) = execute(&p, &s, Operation::Stat { path: to.clone() }).unwrap()
    else {
        panic!("Expected stat")
    };
    assert_eq!(entry.size, replacement.len() as u64);
    execute(
        &p,
        &s,
        Operation::Delete {
            path: to,
            is_dir: false,
        },
    )
    .unwrap();
    execute(
        &p,
        &s,
        Operation::Delete {
            path: dir,
            is_dir: true,
        },
    )
    .unwrap();
}
#[test]
#[ignore = "Requires isolated loopback FTP fixture"]
fn ftp_round_trip() {
    assert_eq!(std::env::var("KILN_REMOTE_FIXTURES").as_deref(), Ok("1"));
    exercise(
        ConnectionProfile {
            id: "fixture-ftp".into(),
            name: "FTP fixture".into(),
            endpoint: RemoteEndpoint::Ftp {
                host: "127.0.0.1".into(),
                port: 22121,
                username: "fixture".into(),
                tls: false,
                root: "/".into(),
            },
        },
        Secrets {
            password: Some("fixture-only".into()),
            ..Default::default()
        },
        "/",
    );
}
#[test]
#[ignore = "Requires isolated loopback OpenSSH fixture"]
fn sftp_round_trip() {
    assert_eq!(std::env::var("KILN_REMOTE_FIXTURES").as_deref(), Ok("1"));
    exercise(
        ConnectionProfile {
            id: "fixture-sftp".into(),
            name: "SFTP fixture".into(),
            endpoint: RemoteEndpoint::Sftp {
                alias: "kiln-remote-fixture".into(),
                config_path: Some(PathBuf::from("/tmp/kiln-remote-fixture/ssh/config")),
                root: "/files".into(),
            },
        },
        Secrets::default(),
        "/files",
    );
}
#[test]
#[ignore = "Requires isolated loopback S3 fixture"]
fn s3_round_trip() {
    assert_eq!(std::env::var("KILN_REMOTE_FIXTURES").as_deref(), Ok("1"));
    exercise(
        ConnectionProfile {
            id: "fixture-s3".into(),
            name: "S3 fixture".into(),
            endpoint: RemoteEndpoint::S3 {
                bucket: "kiln-fixture".into(),
                region: "us-east-1".into(),
                endpoint: Some("http://127.0.0.1:29000".into()),
                path_style: true,
                prefix: "".into(),
                aws_profile: None,
                aws_auth: None,
            },
        },
        Secrets {
            access_key: Some("fixture-access".into()),
            secret_key: Some("fixture-secret-only".into()),
            ..Default::default()
        },
        "",
    );
}
#[test]
fn cancel_before_connect_does_not_mutate() {
    let p = ConnectionProfile {
        id: "invalid".into(),
        name: "Invalid".into(),
        endpoint: RemoteEndpoint::Ftp {
            host: "".into(),
            port: 0,
            username: "".into(),
            tls: false,
            root: "/".into(),
        },
    };
    let job = spawn(
        p,
        Secrets::default(),
        Operation::Delete {
            path: "/should-not-exist".into(),
            is_dir: false,
        },
    );
    job.cancel();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(result) = job.try_recv() {
            assert!(result.is_err());
            return;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
}
