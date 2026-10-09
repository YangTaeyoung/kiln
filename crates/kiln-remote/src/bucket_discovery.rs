//! Account-scoped bucket discovery. Read-only; never probes objects or changes CLI accounts.
use crate::*;
use futures_util::StreamExt;
use s3::request::{Request, tokio_backend::ReqwestRequest};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketScope {
    pub project: String,
    pub compartment_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BucketChoice {
    pub name: String,
    pub region: Option<String>,
    pub namespace: Option<String>,
}
pub struct BucketDiscoveryJob {
    rx: crossbeam_channel::Receiver<Result<Vec<BucketChoice>>>,
    control: Arc<Control>,
}
impl BucketDiscoveryJob {
    pub fn try_recv(&self) -> Option<Result<Vec<BucketChoice>>> {
        self.rx.try_recv().ok()
    }
    pub fn cancel(&self) {
        self.control.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for BucketDiscoveryJob {
    fn drop(&mut self) {
        self.cancel();
    }
}
/// The caller owns the request provenance: discard this job whenever its source or scope changes.
pub fn spawn_bucket_discovery(
    endpoint: RemoteEndpoint,
    secrets: Secrets,
    scope: BucketScope,
) -> BucketDiscoveryJob {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let control = Arc::new(Control::default());
    let c = control.clone();
    std::thread::spawn(move || {
        let result = (|| {
            validate(&endpoint, &scope)?;
            c.check()?;
            if matches!(
                &endpoint,
                RemoteEndpoint::ObjectStorage {
                    provider: ObjectProvider::Oracle,
                    authentication: ObjectAuthentication::Cli { .. },
                    ..
                }
            ) {
                return oracle(&endpoint, &scope, &c);
            }
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(async{
                tokio::select!{r=discover(&endpoint,&secrets,&scope,&c)=>r,_=cancelled(&c)=>bail!("Operation cancelled"),_=tokio::time::sleep(Duration::from_secs(120))=>bail!("Bucket discovery timed out")}
            })
        })();
        let _ = tx.send(result);
    });
    BucketDiscoveryJob { rx, control }
}
fn required(value: &str, label: &str) -> Result<()> {
    safe_text(value)?;
    if value.trim().is_empty() {
        bail!("{label} is required to list buckets");
    }
    Ok(())
}
fn validate(endpoint: &RemoteEndpoint, scope: &BucketScope) -> Result<()> {
    match endpoint {
        RemoteEndpoint::S3 { .. } => {}
        RemoteEndpoint::ObjectStorage {
            provider,
            authentication,
            account_id,
            ..
        } => {
            if let ObjectAuthentication::Cli { profile, .. } = authentication {
                object_profiles::validate_provider_profile(*provider, profile)?;
            }
            if !matches!(authentication, ObjectAuthentication::S3 { .. }) {
                match provider {
                    ObjectProvider::Google => required(&scope.project, "Google project")?,
                    ObjectProvider::Oracle => {
                        required(&scope.compartment_id, "Oracle compartment")?
                    }
                    ObjectProvider::Cloudflare => {
                        let id = account_id.as_deref().unwrap_or("");
                        if id.len() != 32 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
                            bail!("A valid Cloudflare account ID is required to list buckets");
                        }
                    }
                }
            }
        }
        _ => bail!("Bucket discovery requires an object-storage connection"),
    }
    Ok(())
}
fn next_cursor(seen: &mut HashSet<String>, value: Option<&str>) -> Result<Option<String>> {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.len() > 65536 || !seen.insert(value.into()) {
        bail!("Bucket pagination did not advance");
    }
    Ok(Some(value.into()))
}
fn add(
    rows: &mut BTreeMap<String, BucketChoice>,
    name: &str,
    region: Option<&str>,
    namespace: Option<&str>,
) -> Result<()> {
    safe_text(name)?;
    if name.is_empty() {
        bail!("Invalid bucket listing response");
    }
    rows.insert(
        name.into(),
        BucketChoice {
            name: name.into(),
            region: region.map(str::to_owned),
            namespace: namespace.map(str::to_owned),
        },
    );
    if rows.len() > 100_000 {
        bail!("Bucket listing exceeded the safe limit");
    }
    Ok(())
}
async fn discover(
    endpoint: &RemoteEndpoint,
    secrets: &Secrets,
    scope: &BucketScope,
    c: &Control,
) -> Result<Vec<BucketChoice>> {
    match endpoint {
        RemoteEndpoint::S3 { .. } => s3(endpoint, secrets, c).await,
        RemoteEndpoint::ObjectStorage {
            authentication:
                ObjectAuthentication::S3 {
                    region,
                    endpoint,
                    path_style,
                    aws_profile,
                    aws_auth,
                },
            ..
        } => {
            s3(
                &RemoteEndpoint::S3 {
                    bucket: String::new(),
                    region: region.clone(),
                    endpoint: Some(endpoint.clone()),
                    path_style: *path_style,
                    prefix: String::new(),
                    aws_profile: aws_profile.clone(),
                    aws_auth: *aws_auth,
                },
                secrets,
                c,
            )
            .await
        }
        RemoteEndpoint::ObjectStorage {
            provider,
            authentication,
            account_id,
            ..
        } => {
            let auth=object_backend::token(*provider,authentication,c).map_err(|_|anyhow::anyhow!("Cannot authenticate the selected identity; check its CLI login and configuration"))?;
            let client = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(60))
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            let base = match provider {
                ObjectProvider::Google => "https://storage.googleapis.com/storage/v1/b".into(),
                ObjectProvider::Cloudflare => format!(
                    "https://api.cloudflare.com/client/v4/accounts/{}/r2/buckets",
                    account_id.as_deref().unwrap_or("")
                ),
                _ => bail!("Unsupported bucket authentication"),
            };
            http_pages(*provider, &client, &auth, &base, scope, c).await
        }
        _ => bail!("Unsupported bucket discovery"),
    }
}
async fn http_pages(
    provider: ObjectProvider,
    client: &reqwest::Client,
    auth: &object_backend::Auth,
    base: &str,
    scope: &BucketScope,
    c: &Control,
) -> Result<Vec<BucketChoice>> {
    let mut cursor = None::<String>;
    let mut seen = HashSet::new();
    let mut rows = BTreeMap::new();
    for _ in 0..1000 {
        c.check()?;
        let mut url = url::Url::parse(base)?;
        {
            let mut q = url.query_pairs_mut();
            match provider {
                ObjectProvider::Google => {
                    q.append_pair("project", &scope.project)
                        .append_pair("maxResults", "1000");
                    if let Some(v) = &cursor {
                        q.append_pair("pageToken", v);
                    }
                }
                _ => {
                    q.append_pair("per_page", "1000");
                    if let Some(v) = &cursor {
                        q.append_pair("cursor", v);
                    }
                }
            }
        }
        let request = client.get(url);
        let request = if provider == ObjectProvider::Cloudflare {
            request.header("cf-r2-jurisdiction", "default")
        } else {
            request
        };
        let request = match auth {
            object_backend::Auth::Bearer(token) => request.bearer_auth(token),
            object_backend::Auth::CloudflareKey { key, email } => request
                .header("X-Auth-Key", key)
                .header("X-Auth-Email", email),
        };
        let response = tokio::select! {r=request.send()=>r.map_err(|_|anyhow::anyhow!("Bucket request failed; check network connectivity"))?,_=cancelled(c)=>bail!("Operation cancelled")};
        if !response.status().is_success() {
            bail!(
                "Bucket listing failed (HTTP {}); check the selected identity and scope",
                response.status().as_u16()
            );
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        loop {
            let chunk =
                tokio::select! {r=stream.next()=>r,_=cancelled(c)=>bail!("Operation cancelled")};
            let Some(chunk) = chunk else { break };
            let chunk = chunk.map_err(|_| anyhow::anyhow!("Bucket response interrupted"))?;
            if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                bail!("Bucket response exceeded the safe limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid bucket response"))?;
        let next = parse_http_page(provider, &v, &mut rows)?;
        cursor = next_cursor(&mut seen, next)?;
        if cursor.is_none() {
            return Ok(rows.into_values().collect());
        }
    }
    bail!("Bucket listing exceeded the safe page limit")
}
fn parse_http_page<'a>(
    provider: ObjectProvider,
    v: &'a Value,
    rows: &mut BTreeMap<String, BucketChoice>,
) -> Result<Option<&'a str>> {
    if provider == ObjectProvider::Google {
        if v.get("unreachable")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
        {
            bail!("Some buckets are unreachable; check project permissions and retry");
        }
        if let Some(items) = v.get("items") {
            for b in items.as_array().context("Invalid bucket list")? {
                add(
                    rows,
                    b.get("name")
                        .and_then(Value::as_str)
                        .context("Missing bucket name")?,
                    None,
                    None,
                )?;
            }
        }
        Ok(v.get("nextPageToken").and_then(Value::as_str))
    } else {
        if v.get("success").and_then(Value::as_bool) != Some(true) {
            bail!("Cloudflare rejected bucket discovery");
        }
        for b in v
            .pointer("/result/buckets")
            .and_then(Value::as_array)
            .context("Missing Cloudflare buckets")?
        {
            add(
                rows,
                b.get("name")
                    .and_then(Value::as_str)
                    .context("Missing bucket name")?,
                None,
                None,
            )?;
        }
        Ok(v.pointer("/result_info/cursor").and_then(Value::as_str))
    }
}
#[derive(Deserialize)]
struct S3Page {
    #[serde(rename = "Buckets", default)]
    buckets: S3Buckets,
    #[serde(rename = "ContinuationToken")]
    cursor: Option<String>,
}
#[derive(Default, Deserialize)]
struct S3Buckets {
    #[serde(rename = "Bucket", default)]
    items: Vec<S3Bucket>,
}
#[derive(Deserialize)]
struct S3Bucket {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "BucketRegion")]
    region: Option<String>,
}
async fn s3(
    endpoint: &RemoteEndpoint,
    secrets: &Secrets,
    c: &Control,
) -> Result<Vec<BucketChoice>> {
    let mut endpoint = endpoint.clone();
    if let RemoteEndpoint::S3 {
        bucket,
        region,
        path_style,
        prefix,
        ..
    } = &mut endpoint
    {
        bucket.clear();
        prefix.clear();
        *path_style = true;
        if region.is_empty() {
            *region = "us-east-1".into();
        }
    }
    let profile = ConnectionProfile {
        id: "bucket-discovery".into(),
        name: String::new(),
        endpoint,
    };
    let mut bucket = s3_backend::bucket(&profile, secrets, c).map_err(|_| {
        anyhow::anyhow!(
            "Cannot authenticate the selected S3 identity; check credentials and configuration"
        )
    })?;
    let mut rows = BTreeMap::new();
    let mut seen = HashSet::new();
    let mut continuation = None::<String>;
    for _ in 0..1000 {
        c.check()?;
        // rust-s3 ListBuckets uses Bucket::url verbatim and ignores extra_query.
        // The empty path-style bucket represents the service root; append only
        // encoded ListBuckets query parameters, never a user bucket name.
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("max-buckets", "1000");
        if let Some(token) = &continuation {
            query.append_pair("continuation-token", token);
        }
        bucket.name = format!("?{}", query.finish());
        let request = ReqwestRequest::new(&bucket, "", s3::command::Command::ListBuckets).await?;
        let mut response = tokio::select! {r=request.response_data_to_stream()=>r.map_err(|_|anyhow::anyhow!("S3 bucket listing failed; check endpoint, credentials and ListAllMyBuckets permission"))?,_=cancelled(c)=>bail!("Operation cancelled")};
        if !(200..300).contains(&response.status_code) {
            bail!("S3 bucket listing failed (HTTP {})", response.status_code);
        }
        let mut bytes = Vec::new();
        loop {
            let chunk = tokio::select! {r=response.bytes.next()=>r,_=cancelled(c)=>bail!("Operation cancelled")};
            let Some(chunk) = chunk else { break };
            let chunk = chunk.map_err(|_| anyhow::anyhow!("S3 bucket response interrupted"))?;
            if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                bail!("Bucket response exceeded the safe limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        let page: S3Page = quick_xml::de::from_reader(bytes.as_slice())
            .map_err(|_| anyhow::anyhow!("Invalid S3 bucket listing response"))?;
        for b in page.buckets.items {
            add(&mut rows, &b.name, b.region.as_deref(), None)?;
        }
        let cursor = next_cursor(&mut seen, page.cursor.as_deref())?;
        let Some(cursor) = cursor else {
            return Ok(rows.into_values().collect());
        };
        continuation = Some(cursor);
    }
    bail!("Bucket listing exceeded the safe page limit")
}
fn oracle(
    endpoint: &RemoteEndpoint,
    scope: &BucketScope,
    c: &Control,
) -> Result<Vec<BucketChoice>> {
    let executable = cloud_cli::find("oci").context("Install oci to list buckets")?;
    oracle_at(endpoint, scope, c, &executable)
}
fn oracle_at(
    endpoint: &RemoteEndpoint,
    scope: &BucketScope,
    c: &Control,
    executable: &std::path::Path,
) -> Result<Vec<BucketChoice>> {
    let RemoteEndpoint::ObjectStorage {
        authentication:
            ObjectAuthentication::Cli {
                profile,
                config_path,
            },
        region,
        namespace,
        ..
    } = endpoint
    else {
        bail!("Oracle CLI profile required")
    };
    let config = object_profiles::oracle_config_path(config_path.as_deref());
    let mut base = vec![
        format!("--profile={profile}"),
        format!("--config-file={}", config.to_string_lossy()),
        format!(
            "--auth={}",
            object_profiles::oracle_auth_mode(profile, Some(&config))?
        ),
        "--output=json".into(),
        "--connection-timeout=15".into(),
        "--read-timeout=30".into(),
    ];
    if let Some(region) = region.as_ref().filter(|s| !s.is_empty()) {
        base.push(format!("--region={region}"));
    }
    base.extend([
        "os".into(),
        "bucket".into(),
        "list".into(),
        format!("--compartment-id={}", scope.compartment_id),
        "--limit=1000".into(),
    ]);
    if let Some(ns) = namespace.as_ref().filter(|s| !s.is_empty()) {
        base.push(format!("--namespace-name={ns}"));
    }
    let start = std::time::Instant::now();
    let mut rows = BTreeMap::new();
    let mut cursor = None;
    let mut seen = HashSet::new();
    for _ in 0..1000 {
        c.check()?;
        if start.elapsed() > Duration::from_secs(120) {
            bail!("Bucket discovery timed out");
        }
        let mut args = base.clone();
        if let Some(v) = &cursor {
            args.push(format!("--page={v}"));
        }
        let bytes = cloud_cli::execute_at(
            ObjectProvider::Oracle,
            &executable,
            &args,
            &[],
            c,
            Duration::from_secs(120).saturating_sub(start.elapsed()),
            16 * 1024 * 1024,
        )?;
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid Oracle bucket listing"))?;
        for b in v
            .get("data")
            .and_then(Value::as_array)
            .context("Missing Oracle bucket list")?
        {
            add(
                &mut rows,
                b.get("name")
                    .and_then(Value::as_str)
                    .context("Missing bucket name")?,
                region.as_deref(),
                b.get("namespace")
                    .and_then(Value::as_str)
                    .or(namespace.as_deref()),
            )?;
        }
        cursor = next_cursor(&mut seen, v.get("opc-next-page").and_then(Value::as_str))?;
        if cursor.is_none() {
            return Ok(rows.into_values().collect());
        }
    }
    bail!("Bucket listing exceeded the safe page limit")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    fn fixture(responses: Vec<(u16, String)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (code, body) in responses {
                let end = std::time::Instant::now() + Duration::from_secs(10);
                let mut socket = loop {
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(std::time::Instant::now() < end, "request timed out");
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => panic!("{e}"),
                    }
                };
                // BSD/macOS accept inherits O_NONBLOCK from the listener.
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut b = [0];
                    socket.read_exact(&mut b).unwrap();
                    request.push(b[0]);
                    assert!(request.len() < 65536);
                }
                requests.push(String::from_utf8(request).unwrap());
                write!(socket,"HTTP/1.1 {code} fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
            requests
        });
        (url, thread)
    }
    #[test]
    #[cfg(unix)]
    fn oracle_pages_use_only_selected_profile_scope_and_preserve_namespace() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("oci-config");
        std::fs::write(&config, "[SELECTED]\nuser=fixture\n").unwrap();
        let executable = dir.path().join("fixture-oci");
        let log = dir.path().join("args");
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$@" >> '{}'
case " $* " in
  *" --page=page-two "*) printf '%s' '{{"data":[{{"name":"beta","namespace":"selected-ns"}}]}}' ;;
  *) printf '%s' '{{"data":[{{"name":"alpha","namespace":"selected-ns"}}],"opc-next-page":"page-two"}}' ;;
esac
"#,
            log.display()
        );
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let endpoint = RemoteEndpoint::ObjectStorage {
            provider: ObjectProvider::Oracle,
            bucket: "ignored-bucket".into(),
            prefix: "ignored-prefix".into(),
            authentication: ObjectAuthentication::Cli {
                profile: "SELECTED".into(),
                config_path: Some(config),
            },
            region: Some("us-ashburn-1".into()),
            namespace: None,
            account_id: None,
        };
        let result = oracle_at(
            &endpoint,
            &BucketScope {
                compartment_id: "ocid1.compartment.fixture".into(),
                ..Default::default()
            },
            &Control::default(),
            &executable,
        )
        .unwrap();
        assert_eq!(
            result.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
        assert!(
            result
                .iter()
                .all(|r| r.namespace.as_deref() == Some("selected-ns")
                    && r.region.as_deref() == Some("us-ashburn-1"))
        );
        let args = std::fs::read_to_string(log).unwrap();
        assert_eq!(args.matches("--profile=SELECTED").count(), 2);
        assert_eq!(
            args.matches("--compartment-id=ocid1.compartment.fixture")
                .count(),
            2
        );
        assert!(args.contains("--page=page-two"));
        assert!(!args.contains("ignored-"));
    }
    #[tokio::test]
    async fn native_bucket_pages_preserve_scope_and_never_echo_server_errors() {
        for provider in [ObjectProvider::Google, ObjectProvider::Cloudflare] {
            let pages = if provider == ObjectProvider::Google {
                vec![
                    r#"{"items":[{"name":"alpha"}],"nextPageToken":"next /+"}"#,
                    r#"{"items":[{"name":"beta"}]}"#,
                ]
            } else {
                vec![
                    r#"{"success":true,"result":{"buckets":[{"name":"alpha"}]},"result_info":{"cursor":"next /+"}}"#,
                    r#"{"success":true,"result":{"buckets":[{"name":"beta"}]}}"#,
                ]
            };
            let (url, thread) = fixture(pages.into_iter().map(|p| (200, p.into())).collect());
            let scope = BucketScope {
                project: "chosen-project".into(),
                ..Default::default()
            };
            let auth = object_backend::Auth::Bearer("fixture-token".into());
            let rows = http_pages(
                provider,
                &reqwest::Client::new(),
                &auth,
                &url,
                &scope,
                &Control::default(),
            )
            .await
            .unwrap();
            assert_eq!(
                rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
                ["alpha", "beta"]
            );
            let requests = thread.join().unwrap();
            assert!(requests.iter().all(|r| r.starts_with("GET ")));
            assert!(requests[1].contains("next+%2F%2B"));
            if provider == ObjectProvider::Google {
                assert!(
                    requests
                        .iter()
                        .all(|r| r.contains("project=chosen-project"))
                );
            }
        }
        let (url, thread) = fixture(vec![(403, "private-service-token".into())]);
        let error = http_pages(
            ObjectProvider::Google,
            &reqwest::Client::new(),
            &object_backend::Auth::Bearer("fixture-token".into()),
            &url,
            &BucketScope::default(),
            &Control::default(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("403"));
        assert!(!format!("{error:#}").contains("private-service-token"));
        thread.join().unwrap();
    }
    #[tokio::test]
    async fn real_signed_s3_list_buckets_pages_ignore_connection_bucket_and_prefix() {
        let page = |name: &str, cursor: &str| {
            format!(
                "<ListAllMyBucketsResult><Buckets><Bucket><Name>{name}</Name><BucketRegion>us-west-2</BucketRegion></Bucket></Buckets>{cursor}</ListAllMyBucketsResult>"
            )
        };
        let (url, thread) = fixture(vec![
            (
                200,
                page("alpha", "<ContinuationToken>next-token</ContinuationToken>"),
            ),
            (200, page("beta", "")),
        ]);
        let endpoint = RemoteEndpoint::S3 {
            bucket: "must-not-be-sent".into(),
            prefix: "private-prefix".into(),
            region: "us-east-1".into(),
            endpoint: Some(url),
            path_style: false,
            aws_profile: None,
            aws_auth: Some(S3Authentication::Manual),
        };
        let secrets = Secrets {
            access_key: Some("fixture-key".into()),
            secret_key: Some("fixture-secret".into()),
            ..Default::default()
        };
        let rows = s3(&endpoint, &secrets, &Control::default()).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].region.as_deref(), Some("us-west-2"));
        let requests = thread.join().unwrap();
        assert!(requests[0].starts_with("GET /?"));
        assert!(requests[0].contains("max-buckets=1000"));
        assert!(requests[1].contains("continuation-token=next-token"));
        assert!(requests.iter().all(|r| {
            !r.contains("must-not-be-sent")
                && !r.contains("private-prefix")
                && r.to_ascii_lowercase()
                    .contains("authorization: aws4-hmac-sha256")
        }));
    }
    #[tokio::test]
    async fn oversized_s3_response_is_rejected_before_waiting_for_its_declared_body() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = RemoteEndpoint::S3 {
            bucket: String::new(),
            prefix: String::new(),
            region: "us-east-1".into(),
            endpoint: Some(format!("http://{}", listener.local_addr().unwrap())),
            path_style: true,
            aws_profile: None,
            aws_auth: Some(S3Authentication::Manual),
        };
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "oversized response was not requested"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            // Synchronous fixture I/O requires blocking mode even when accept
            // itself is polled through a nonblocking listener.
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 65536);
            }
            assert!(
                String::from_utf8(request)
                    .unwrap()
                    .starts_with("GET /?max-buckets=1000")
            );
            // Never finish this advertised 1GiB body. The bounded reader must
            // fail on the first excess chunk instead of waiting for EOF.
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                1024 * 1024 * 1024
            )
            .unwrap();
            let chunk = vec![b' '; 64 * 1024];
            for _ in 0..(17 * 1024 / 64) {
                if let Err(error) = socket.write_all(&chunk) {
                    return match error.kind() {
                        std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut => Ok(error.kind()),
                        _ => Err(format!("oversized response sender failed: {error:?}")),
                    };
                }
            }
            let mut byte = [0];
            match socket.read(&mut byte) {
                Ok(0) => Ok(std::io::ErrorKind::BrokenPipe),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    Ok(error.kind())
                }
                other => Err(format!(
                    "unexpected oversized response sender result: {other:?}"
                )),
            }
        });
        let secrets = Secrets {
            access_key: Some("fixture-access".into()),
            secret_key: Some("fixture-secret".into()),
            ..Default::default()
        };
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            s3(&endpoint, &secrets, &Control::default()),
        )
        .await;
        // Joining on the current-thread Tokio runtime would prevent reqwest's
        // connection driver from processing the dropped response/socket.
        let sender = tokio::task::spawn_blocking(move || server.join())
            .await
            .unwrap()
            .unwrap();
        // The contract is bounded response consumption, not instantaneous TCP
        // shutdown by the HTTP client's asynchronous connection driver. The
        // owned sender may stop on socket closure or bounded backpressure.
        let result = result.expect("reader waited for the unbounded body");
        assert!(
            result.is_err(),
            "oversized body was accepted; sender={sender:?}"
        );
        let error = result.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Bucket response exceeded the safe limit",
            "sender={sender:?}"
        );
        assert!(sender.is_ok(), "{sender:?}");
    }

    #[test]
    fn missing_scope_and_repeated_cursor_fail_closed() {
        let endpoint = RemoteEndpoint::ObjectStorage {
            provider: ObjectProvider::Google,
            bucket: String::new(),
            prefix: String::new(),
            authentication: ObjectAuthentication::GoogleAdc {
                credentials_path: Some("/fixture/adc".into()),
            },
            region: None,
            namespace: None,
            account_id: None,
        };
        assert!(validate(&endpoint, &BucketScope::default()).is_err());
        assert!(
            validate(
                &endpoint,
                &BucketScope {
                    project: "explicit".into(),
                    ..Default::default()
                }
            )
            .is_ok()
        );
        let mut seen = HashSet::new();
        assert!(next_cursor(&mut seen, Some("same")).unwrap().is_some());
        assert!(next_cursor(&mut seen, Some("same")).is_err());
        let mut rows = BTreeMap::new();
        assert!(
            parse_http_page(
                ObjectProvider::Google,
                &serde_json::json!({"unreachable":["hidden"]}),
                &mut rows
            )
            .is_err()
        );
    }
    #[tokio::test]
    async fn cancelled_discovery_does_not_start_an_http_request() {
        let c = Control::default();
        c.cancel.store(true, Ordering::Relaxed);
        assert!(
            http_pages(
                ObjectProvider::Google,
                &reqwest::Client::new(),
                &object_backend::Auth::Bearer("fixture".into()),
                "http://127.0.0.1:9",
                &BucketScope::default(),
                &c
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("cancelled")
        );
    }
}
