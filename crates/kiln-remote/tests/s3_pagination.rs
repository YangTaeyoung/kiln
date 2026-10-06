use kiln_remote::*;
use std::time::{Duration, Instant};
fn profile() -> ConnectionProfile {
    ConnectionProfile {
        id: "fixture-pagination".into(),
        name: "S3 pagination fixture".into(),
        endpoint: RemoteEndpoint::S3 {
            bucket: "kiln-fixture".into(),
            region: "us-east-1".into(),
            endpoint: Some("http://127.0.0.1:29000".into()),
            path_style: true,
            prefix: "".into(),
            aws_profile: None,
            aws_auth: None,
        },
    }
}
fn secrets() -> Secrets {
    Secrets {
        access_key: Some("fixture-access".into()),
        secret_key: Some("fixture-secret-only".into()),
        ..Default::default()
    }
}
fn run(op: Operation) -> RemoteResult {
    let job = spawn(profile(), secrets(), op);
    let until = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(result) = job.try_recv() {
            return result.unwrap();
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
#[ignore = "Requires isolated S3 fixture seeded with 501 objects"]
fn listing_uses_next_page_cursor() {
    assert_eq!(std::env::var("KILN_REMOTE_FIXTURES").as_deref(), Ok("1"));
    let RemoteResult::Listed(first) = run(Operation::List {
        path: "pagination".into(),
        cursor: None,
    }) else {
        panic!()
    };
    assert_eq!(first.entries.len(), 500);
    assert!(first.next_cursor.is_some());
    let RemoteResult::Listed(next) = run(Operation::List {
        path: "pagination".into(),
        cursor: first.next_cursor,
    }) else {
        panic!()
    };
    assert_eq!(next.entries.len(), 1);
    assert!(next.next_cursor.is_none());
    assert!(!first.entries.iter().any(|e| e.path == next.entries[0].path));
}
#[test]
#[ignore = "Requires isolated S3 fixture"]
fn multipart_larger_than_one_chunk_is_complete() {
    assert_eq!(std::env::var("KILN_REMOTE_FIXTURES").as_deref(), Ok("1"));
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("large.bin");
    let output = dir.path().join("download.bin");
    let bytes = vec![0x5a; 9 * 1024 * 1024 + 17];
    std::fs::write(&input, &bytes).unwrap();
    let path = format!("multipart-{}.bin", std::process::id());
    run(Operation::Upload {
        local: input,
        path: path.clone(),
        overwrite: false,
    });
    run(Operation::Download {
        path: path.clone(),
        local: output.clone(),
    });
    assert_eq!(std::fs::read(output).unwrap(), bytes);
    run(Operation::Delete {
        path,
        is_dir: false,
    });
}

#[test]
#[ignore = "Requires isolated S3 fixture"]
fn cancelled_upload_never_publishes_the_destination() {
    assert_eq!(std::env::var("KILN_REMOTE_FIXTURES").as_deref(), Ok("1"));
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("cancel.bin");
    std::fs::File::create(&input)
        .unwrap()
        .set_len(64 * 1024 * 1024)
        .unwrap();
    let path = format!("cancel-{}.bin", std::process::id());
    let job = spawn(
        profile(),
        secrets(),
        Operation::Upload {
            local: input,
            path: path.clone(),
            overwrite: false,
        },
    );
    let until = Instant::now() + Duration::from_secs(90);
    loop {
        assert!(Instant::now() < until);
        if job.progress().bytes > 0 {
            job.cancel();
            break;
        }
        if let Some(result) = job.try_recv() {
            panic!("Upload completed before cancellation: {result:?}")
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    loop {
        assert!(Instant::now() < until);
        if let Some(result) = job.try_recv() {
            assert!(result.is_err());
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let RemoteResult::Listed(page) = run(Operation::List {
        path: String::new(),
        cursor: None,
    }) else {
        panic!()
    };
    assert!(
        !page
            .entries
            .iter()
            .any(|entry| entry.path.starts_with(&path)),
        "Cancelled upload left published or staging objects"
    );
}
