use crate::{
    Options,
    api::{self, Error},
    auth::Auth,
    metadata,
    runtime::{Mutations, Runtime},
};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
struct Shared {
    auth: Auth,
    authority: Vec<String>,
    origins: Vec<String>,
    runtime: Arc<Runtime>,
    mutations: Arc<Mutations>,
    requests: Arc<tokio::sync::Semaphore>,
    uploads: Arc<tokio::sync::Semaphore>,
}
pub async fn serve(o: Options) -> anyhow::Result<()> {
    serve_until(o, async {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal handler");
        tokio::select! {_ =tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
    })
    .await
}
pub async fn serve_until(
    o: Options,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        o.listen.ip().is_loopback(),
        "listen address must be loopback"
    );
    let cli = validate_cli(o.kiln_cli).await?;
    let workspaces = metadata::workspaces(&o.workspaces)?;
    let listener = tokio::net::TcpListener::bind(o.listen).await?;
    let addr = listener.local_addr()?;
    let url = format!("http://{addr}");
    let auth = Auth::new(o.token_ttl)?;
    let mut origins = vec![
        "capacitor://localhost".into(),
        "https://localhost".into(),
        "http://localhost".into(),
    ];
    if let Some(origin) = o.dev_origin {
        anyhow::ensure!(
            !origin.contains(['*', '\r', '\n']) && origin.len() < 256,
            "invalid development origin"
        );
        let uri: axum::http::Uri = origin.parse()?;
        anyhow::ensure!(
            uri.scheme_str() == Some("http")
                && matches!(uri.host(), Some("localhost" | "127.0.0.1" | "[::1]"))
                && uri.path() == "/",
            "development origin must be explicit loopback HTTP"
        );
        origins.push(origin.trim_end_matches('/').into());
    }
    let pairing = auth.pairing(&o.pairing_file, &url)?;
    let state = Arc::new(Shared {
        auth,
        authority: vec![
            addr.to_string(),
            format!("localhost:{}", addr.port()),
            format!("127.0.0.1:{}", addr.port()),
            format!("10.0.2.2:{}", addr.port()),
        ],
        origins,
        runtime: Arc::new(Runtime::new(workspaces, cli)),
        mutations: Arc::new(Mutations::default()),
        requests: Arc::new(tokio::sync::Semaphore::new(32)),
        uploads: Arc::new(tokio::sync::Semaphore::new(2)),
    });
    let app = Router::new()
        .fallback(any(dispatch))
        .layer(middleware::from_fn_with_state(state.clone(), boundary))
        .with_state(state.clone());
    let stopping = state.runtime.runner.stopping.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.await;
            stopping.store(true, std::sync::atomic::Ordering::Release);
        })
        .await?;
    // Tracked preparation owns its eventual outcome even when the initiating HTTP client disappeared.
    let _all = tokio::time::timeout(Duration::from_secs(2), state.requests.acquire_many(32))
        .await
        .map_err(|_| anyhow::anyhow!("host work drain deadline"))??;
    drop(pairing);
    Ok(())
}
async fn validate_cli(path: std::path::PathBuf) -> anyhow::Result<std::path::PathBuf> {
    tokio::task::spawn_blocking(move || {
        use std::{
            io::Read,
            os::unix::fs::PermissionsExt,
            process::{Command, Stdio},
            time::Instant,
        };
        let path = std::fs::canonicalize(path)?;
        let m = std::fs::metadata(&path)?;
        anyhow::ensure!(
            m.is_file() && m.permissions().mode() & 0o111 != 0,
            "Kiln CLI must be executable"
        );
        let mut child = Command::new(&path)
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(s) = child.try_wait()? {
                break s;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("Kiln CLI version deadline")
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut bytes = vec![];
        child
            .stdout
            .take()
            .unwrap()
            .take(129)
            .read_to_end(&mut bytes)?;
        anyhow::ensure!(
            status.success()
                && bytes.len() <= 128
                && String::from_utf8_lossy(&bytes).trim() == "kiln 0.3.0",
            "Kiln CLI 0.3.0 required"
        );
        Ok(path)
    })
    .await?
}
async fn boundary(State(s): State<Arc<Shared>>, mut req: Request, next: Next) -> Response {
    let bad = |status, code| Error::new(status, code).into_response();
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    if !host.is_some_and(|v| s.authority.iter().any(|h| h == v)) {
        return bad(StatusCode::FORBIDDEN, "invalid_host");
    }
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if req.headers().contains_key(header::ORIGIN)
        && !origin.as_ref().is_some_and(|v| s.origins.contains(v))
    {
        return bad(StatusCode::FORBIDDEN, "invalid_origin");
    }
    if req.method() == Method::OPTIONS {
        if origin.is_none() {
            return bad(StatusCode::FORBIDDEN, "invalid_origin");
        }
        return cors(StatusCode::NO_CONTENT.into_response(), origin);
    }
    if !s.auth.accepts(
        req.headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok()),
    ) {
        return bad(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let Ok(permit) = s.requests.clone().try_acquire_owned() else {
        return bad(StatusCode::TOO_MANY_REQUESTS, "request_limit");
    };
    let upload_permit =
        if req.uri().path().ends_with("/attachments") && req.method() == Method::POST {
            match s.uploads.clone().try_acquire_owned() {
                Ok(p) => Some(p),
                Err(_) => return bad(StatusCode::TOO_MANY_REQUESTS, "upload_limit"),
            }
        } else {
            None
        };
    req.extensions_mut().insert(WorkLease {
        _request: Arc::new(permit),
        _upload: upload_permit.map(Arc::new),
    });
    let origin2 = origin.clone();
    let response = tokio::time::timeout(Duration::from_secs(5), async move {
        if req.method() == Method::POST {
            if req
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_none_or(|v| {
                    !v.split(';')
                        .next()
                        .is_some_and(|v| v.trim() == "application/json")
                })
            {
                return bad(StatusCode::UNSUPPORTED_MEDIA_TYPE, "content_type");
            }
            let limit = if req.uri().path().ends_with("/attachments") {
                28 * 1024 * 1024 + 4096
            } else {
                128 * 1024
            };
            let (parts, body) = req.into_parts();
            let bytes = match to_bytes(body, limit).await {
                Ok(b) => b,
                Err(_) => return bad(StatusCode::PAYLOAD_TOO_LARGE, "body_limit"),
            };
            req = Request::from_parts(parts, Body::from(bytes));
        }
        next.run(req).await
    })
    .await
    .unwrap_or_else(|_| bad(StatusCode::REQUEST_TIMEOUT, "request_deadline"));
    cors(response, origin2)
}
fn cors(mut response: Response, origin: Option<String>) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    if let Some(origin) = origin {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.parse().unwrap());
        response
            .headers_mut()
            .insert(header::VARY, "Origin".parse().unwrap());
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            "GET, POST, OPTIONS".parse().unwrap(),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            "Authorization, Content-Type".parse().unwrap(),
        );
    }
    response
}
#[derive(Clone)]
struct WorkLease {
    _request: Arc<tokio::sync::OwnedSemaphorePermit>,
    _upload: Option<Arc<tokio::sync::OwnedSemaphorePermit>>,
}
async fn dispatch(State(s): State<Arc<Shared>>, req: Request) -> Response {
    match route(s, req).await {
        Ok((status, v)) => (status, Json(v)).into_response(),
        Err(e) => e.into_response(),
    }
}
async fn route(s: Arc<Shared>, req: Request) -> api::Result<(StatusCode, Value)> {
    let lease = req
        .extensions()
        .get::<WorkLease>()
        .cloned()
        .ok_or_else(Error::unavailable)?;
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let p: Vec<_> = path.trim_matches('/').split('/').collect();
    if p.first() != Some(&"v1") {
        return Err(Error::new(StatusCode::NOT_FOUND, "route_unavailable"));
    }
    if method == Method::GET {
        if p.as_slice() == ["v1", "bootstrap"] {
            // Match native agent_launch::executable's search roots without loading accounts.
            let mut extra = vec![];
            if let Some(home) = kiln_common::paths::home() {
                extra.push(home.join(".local/bin"));
            }
            extra.extend([
                std::path::PathBuf::from("/opt/homebrew/bin"),
                std::path::PathBuf::from("/usr/local/bin"),
            ]);
            let providers=kiln_accounts::Tool::ALL.iter().map(|t|{let program=match t{kiln_accounts::Tool::Claude=>"claude",kiln_accounts::Tool::Codex=>"codex",kiln_accounts::Tool::Antigravity=>"agy"};json!({"provider":t,"available":kiln_common::paths::find_executable(program,&extra).is_some()})}).collect::<Vec<_>>();
            return Ok((
                StatusCode::OK,
                json!({"api_version":1,"host_version":"0.1.0","host_name":"Kiln computer","workspaces":s.runtime.workspaces.iter().map(|w|json!({"id":w.id,"name":w.name})).collect::<Vec<_>>(),"providers":providers,"capabilities":{"conversations":true,"terminals":true,"image_upload":true,"native_actions":false,"structured_grants":false},"limits":{"text_bytes":120000,"snapshot_bytes":api::SNAPSHOT_LIMIT,"recent_entries":200,"image_bytes":20971520,"mutation_retention_seconds":600,"mutation_results":1024}}),
            ));
        }
        if let ["v1", "operations", id] = p.as_slice() {
            return Ok((StatusCode::OK, s.mutations.operation(id)?));
        }
        let runtime = s.runtime.clone();
        let p = p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let v = tokio::task::spawn_blocking(move || {
            let _lease = lease;
            match p.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                ["v1", "conversations"] => runtime.list(),
                ["v1", "conversations", id] => runtime.snapshot(id),
                ["v1", "terminals"] => runtime.terminals(),
                ["v1", "terminals", id] => runtime.terminal(id, None),
                _ => Err(Error::new(StatusCode::NOT_FOUND, "route_unavailable")),
            }
        })
        .await
        .map_err(|_| Error::unavailable())??;
        return Ok((StatusCode::OK, v));
    }
    if method != Method::POST {
        return Err(Error::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "method_unavailable",
        ));
    }
    let bytes = to_bytes(req.into_body(), 28 * 1024 * 1024 + 4096)
        .await
        .map_err(|_| Error::bad("invalid_json"))?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::bad("invalid_json"))?;
    let id = value["request_id"]
        .as_str()
        .ok_or_else(|| Error::bad("invalid_request_id"))?
        .to_string();
    let mut fingerprint = path.as_bytes().to_vec();
    fingerprint.extend(serde_json::to_vec(&value).unwrap());
    let fingerprint = ring::digest::digest(&ring::digest::SHA256, &fingerprint)
        .as_ref()
        .to_vec();
    if s.mutations.existing(&id, &fingerprint)? {
        return if p.as_slice() == ["v1", "conversations"] {
            Ok((StatusCode::ACCEPTED, s.mutations.receipt(&id)))
        } else {
            s.mutations.reply(&id).await
        };
    }
    // Validate route/payload before reserving; cached IDs still conflict across every mutation endpoint.
    enum Work {
        Launch(api::CreateConversation),
        Command(String, api::CommandRequest),
        Upload(String, api::Upload),
        Input(String, api::TerminalInput),
    }
    let parse = || Error::bad("invalid_payload");
    let work = match p.as_slice() {
        ["v1", "conversations"] => {
            let v: api::CreateConversation = serde_json::from_value(value).map_err(|_| parse())?;
            if !s.runtime.workspaces.iter().any(|w| w.id == v.workspace_id) {
                return Err(Error::bad("workspace_unavailable"));
            }
            Work::Launch(v)
        }
        ["v1", "conversations", cid, "commands"] => Work::Command(
            cid.to_string(),
            serde_json::from_value(value).map_err(|_| parse())?,
        ),
        ["v1", "conversations", cid, "attachments"] => Work::Upload(
            cid.to_string(),
            serde_json::from_value(value).map_err(|_| parse())?,
        ),
        ["v1", "terminals", tid, "input"] => Work::Input(
            tid.to_string(),
            serde_json::from_value(value).map_err(|_| parse())?,
        ),
        _ => return Err(Error::new(StatusCode::NOT_FOUND, "route_unavailable")),
    };
    let typed_id = match &work {
        Work::Launch(v) => &v.request_id,
        Work::Command(_, v) => &v.request_id,
        Work::Upload(_, v) => &v.request_id,
        Work::Input(_, v) => &v.request_id,
    };
    if typed_id != &id {
        return Err(Error::bad("invalid_request_id"));
    }
    let launch = matches!(work, Work::Launch(_));
    if !s.mutations.register(&id, fingerprint)? {
        return if launch {
            Ok((StatusCode::ACCEPTED, s.mutations.receipt(&id)))
        } else {
            s.mutations.reply(&id).await
        };
    }
    let permit = if launch {
        match s.mutations.preparations.clone().try_acquire_owned() {
            Ok(p) => Some(p),
            Err(_) => {
                s.mutations.finish(
                    &id,
                    Err(Error::new(
                        StatusCode::TOO_MANY_REQUESTS,
                        "preparation_limit",
                    )),
                    true,
                );
                return Ok((StatusCode::ACCEPTED, s.mutations.receipt(&id)));
            }
        }
    } else {
        None
    };
    let runtime = s.runtime.clone();
    let mutations = s.mutations.clone();
    let worker_id = id.clone();
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        let _permit = permit;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match work {
            Work::Launch(v) => runtime.launch(&v.workspace_id, v.provider, v.launch_options),
            Work::Command(cid, v) => runtime.command(&cid, &v.generation, v.command),
            Work::Upload(cid, v) => runtime.upload(&cid, v),
            Work::Input(tid, v) => runtime.terminal(&tid, Some(v)),
        }))
        .unwrap_or_else(|_| Err(Error::unavailable()));
        mutations.finish(&worker_id, result, launch);
    });
    if launch {
        Ok((StatusCode::ACCEPTED, s.mutations.receipt(&id)))
    } else {
        s.mutations.reply(&id).await
    }
}
