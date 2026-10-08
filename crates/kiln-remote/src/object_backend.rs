//! Native GCS/R2 object APIs; provider CLI tokens never leave this worker.
use crate::*;
use futures_util::StreamExt;
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde_json::Value;
use tokio::io::AsyncWriteExt;

const R2_MAX_UPLOAD: u64 = R2_API_MAX_UPLOAD_BYTES;
const JSON_LIMIT: usize = 16 * 1024 * 1024;
enum Auth {
    Bearer(String),
    CloudflareKey { key: String, email: String },
}
struct Api {
    provider: ObjectProvider,
    bucket: String,
    base: String,
    upload_base: String,
    auth: Auth,
    client: reqwest::Client,
}

pub(super) fn key(path: &str) -> Result<&str> {
    safe_text(path)?;
    Ok(path.trim_start_matches('/'))
}
pub(super) fn require_file(path: &str) -> Result<&str> {
    let p = key(path)?;
    if p.is_empty() || p.ends_with('/') {
        bail!("Select a file, not a folder");
    }
    Ok(p)
}
fn parse_json(bytes: &[u8]) -> Result<Value> {
    serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!("Invalid object storage response"))
}
fn checked_token(value: &str) -> Result<String> {
    let s = value.trim();
    if s.is_empty() || s.len() > 65536 || s.chars().any(char::is_whitespace) {
        bail!("The provider CLI did not return a valid access token");
    }
    reqwest::header::HeaderValue::from_str(s)
        .map_err(|_| anyhow::anyhow!("Invalid provider access token"))?;
    Ok(s.into())
}
fn token(provider: ObjectProvider, auth: &ObjectAuthentication, c: &Control) -> Result<Auth> {
    match (provider, auth) {
        (
            ObjectProvider::Google,
            ObjectAuthentication::Cli {
                profile,
                config_path,
            },
        ) => {
            let mut env = Vec::new();
            if let Some(p) = config_path {
                env.push(("CLOUDSDK_CONFIG".into(), p.to_string_lossy().into_owned()));
            }
            let bytes = crate::cloud_cli::execute(
                provider,
                &[
                    "auth".into(),
                    "print-access-token".into(),
                    format!("--configuration={profile}"),
                    "--quiet".into(),
                ],
                &env,
                c,
            )?;
            let s = std::str::from_utf8(&bytes)
                .map_err(|_| anyhow::anyhow!("Invalid provider access token"))?;
            Ok(Auth::Bearer(checked_token(s)?))
        }
        (ObjectProvider::Google, ObjectAuthentication::GoogleAdc { credentials_path }) => {
            let mut env = Vec::new();
            if let Some(p) = credentials_path {
                env.push((
                    "GOOGLE_APPLICATION_CREDENTIALS".into(),
                    p.to_string_lossy().into_owned(),
                ));
            }
            let bytes = crate::cloud_cli::execute(
                provider,
                &[
                    "auth".into(),
                    "application-default".into(),
                    "print-access-token".into(),
                    "--quiet".into(),
                ],
                &env,
                c,
            )?;
            let s = std::str::from_utf8(&bytes)
                .map_err(|_| anyhow::anyhow!("Invalid provider access token"))?;
            Ok(Auth::Bearer(checked_token(s)?))
        }
        (
            ObjectProvider::Cloudflare,
            ObjectAuthentication::Cli {
                profile,
                config_path,
            },
        ) => {
            // Wrangler has no supported global credential directory override. Never
            // silently load another source after HOME/XDG configuration changes.
            let current = crate::object_profiles::ProfileFiles::current(provider).config;
            if config_path.as_ref().is_some_and(|p| p != &current) {
                bail!("Wrangler profile source has changed; select the profile again");
            }
            let bytes = crate::cloud_cli::execute(
                provider,
                &[
                    "auth".into(),
                    "token".into(),
                    "--json".into(),
                    format!("--profile={profile}"),
                ],
                &[],
                c,
            )?;
            let v = parse_json(&bytes)?;
            match v.get("type").and_then(Value::as_str) {
                Some("oauth" | "api_token") => Ok(Auth::Bearer(checked_token(
                    v.get("token")
                        .and_then(Value::as_str)
                        .context("Missing Wrangler access token")?,
                )?)),
                Some("api_key") => Ok(Auth::CloudflareKey {
                    key: checked_token(
                        v.get("key")
                            .and_then(Value::as_str)
                            .context("Missing Wrangler API key")?,
                    )?,
                    email: checked_token(
                        v.get("email")
                            .and_then(Value::as_str)
                            .context("Missing Wrangler email")?,
                    )?,
                }),
                _ => bail!(
                    "Wrangler did not return a supported authentication source; update Wrangler and sign in to the selected profile"
                ),
            }
        }
        _ => bail!("This provider requires its own native authentication source"),
    }
}
pub(super) async fn run(
    profile: &ConnectionProfile,
    secrets: &Secrets,
    op: &Operation,
    c: &Arc<Control>,
) -> Result<RemoteResult> {
    let RemoteEndpoint::ObjectStorage {
        provider,
        bucket,
        prefix,
        authentication,
        account_id,
        ..
    } = &profile.endpoint
    else {
        unreachable!()
    };
    if let ObjectAuthentication::S3 {
        region,
        endpoint,
        path_style,
        aws_profile,
        aws_auth,
    } = authentication
    {
        let legacy = ConnectionProfile {
            id: profile.id.clone(),
            name: profile.name.clone(),
            endpoint: RemoteEndpoint::S3 {
                bucket: bucket.clone(),
                prefix: prefix.clone(),
                region: region.clone(),
                endpoint: Some(endpoint.clone()),
                path_style: *path_style,
                aws_profile: aws_profile.clone(),
                aws_auth: *aws_auth,
            },
        };
        // Keep existing multipart abort/cleanup behavior for compatible uploads.
        if matches!(op, Operation::Upload { .. }) {
            return crate::s3_backend::run(&legacy, secrets, op, c).await;
        }
        return tokio::select! {r=crate::s3_backend::run(&legacy,secrets,op,c)=>r,_=cancelled(c)=>Err(anyhow::anyhow!("Operation cancelled"))};
    }
    if *provider == ObjectProvider::Oracle {
        return crate::object_oci::run(profile, op, c);
    }
    let auth = token(*provider, authentication, c)?;
    c.check()?;
    let base = match provider {
        ObjectProvider::Google => format!("https://storage.googleapis.com/storage/v1/b/{bucket}/o"),
        ObjectProvider::Cloudflare => format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/r2/buckets/{bucket}/objects",
            account_id
                .as_deref()
                .context("A Cloudflare account ID is required")?
        ),
        _ => unreachable!(),
    };
    let api = Api {
        provider: *provider,
        bucket: bucket.clone(),
        base,
        upload_base: format!("https://storage.googleapis.com/upload/storage/v1/b/{bucket}/o"),
        auth,
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
    };
    api.run(op, c).await
}
impl Api {
    fn object_url(&self, path: &str) -> Result<url::Url> {
        if path == "."
            || path == ".."
            || (self.provider == ObjectProvider::Cloudflare
                && path.split('/').any(|p| matches!(p, "." | "..")))
        {
            bail!("Use S3-compatible credentials for object names containing dot path segments");
        }
        let mut url = url::Url::parse(&self.base)?;
        {
            let mut parts = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("Invalid object API URL"))?;
            // Cloudflare documents literal '/' separators; GCS takes a single
            // percent-encoded object-name path parameter. Both escape ?/#/%.
            if self.provider == ObjectProvider::Cloudflare {
                for segment in path.split('/') {
                    parts.push(segment);
                }
            } else {
                parts.push(path);
            }
        }
        Ok(url)
    }
    fn request(&self, method: Method, url: impl reqwest::IntoUrl) -> RequestBuilder {
        let request = self.client.request(method, url);
        match &self.auth {
            Auth::Bearer(token) => request.bearer_auth(token),
            Auth::CloudflareKey { key, email } => request
                .header("X-Auth-Key", key)
                .header("X-Auth-Email", email),
        }
    }
    async fn send(&self, request: RequestBuilder, c: &Control) -> Result<Response> {
        c.check()?;
        tokio::select! {r=request.send()=>r.map_err(|_|anyhow::anyhow!("Object storage request failed; check network connectivity")),_=cancelled(c)=>bail!("Operation cancelled")}
    }
    fn success(response: Response) -> Result<Response> {
        if !response.status().is_success() {
            bail!(
                "Object storage request failed (HTTP {})",
                response.status().as_u16()
            );
        }
        Ok(response)
    }
    async fn json(&self, request: RequestBuilder, c: &Control) -> Result<Value> {
        let response = Self::success(self.send(request, c).await?)?;
        let v = self.read_value(response, c).await?;
        if self.provider == ObjectProvider::Cloudflare
            && v.get("success").and_then(Value::as_bool) != Some(true)
        {
            bail!("Cloudflare rejected the object storage request");
        }
        Ok(v)
    }
    async fn read_value(&self, response: Response, c: &Control) -> Result<Value> {
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        loop {
            let chunk =
                tokio::select! {v=stream.next()=>v,_=cancelled(c)=>bail!("Operation cancelled")};
            let Some(chunk) = chunk else {
                break;
            };
            let chunk =
                chunk.map_err(|_| anyhow::anyhow!("Cannot read object storage response"))?;
            if bytes.len() + chunk.len() > JSON_LIMIT {
                bail!("Object storage response exceeded the safe limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        parse_json(&bytes)
    }
    async fn list(
        &self,
        path: &str,
        cursor: Option<&str>,
        delimiter: bool,
        limit: u32,
        c: &Control,
    ) -> Result<ListPage> {
        let mut url = url::Url::parse(&self.base)?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("prefix", path);
            q.append_pair(
                if self.provider == ObjectProvider::Google {
                    "maxResults"
                } else {
                    "per_page"
                },
                &limit.to_string(),
            );
            if delimiter {
                q.append_pair("delimiter", "/");
            }
            if let Some(cursor) = cursor {
                safe_text(cursor)?;
                q.append_pair(
                    if self.provider == ObjectProvider::Google {
                        "pageToken"
                    } else {
                        "cursor"
                    },
                    cursor,
                );
            }
        }
        parse_list(
            self.provider,
            &self.json(self.request(Method::GET, url), c).await?,
            path,
            delimiter,
        )
    }
    async fn stat(&self, path: &str, c: &Control) -> Result<Option<(RemoteEntry, Option<String>)>> {
        if self.provider == ObjectProvider::Google {
            let response = self
                .send(self.request(Method::GET, self.object_url(path)?), c)
                .await?;
            if response.status() == StatusCode::NOT_FOUND {
                return Ok(None);
            }
            let v = self.read_value(Self::success(response)?, c).await?;
            return Ok(Some((
                entry(ObjectProvider::Google, &v)?,
                v.get("generation")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            )));
        }
        let response = self
            .send(self.request(Method::GET, self.object_url(path)?), c)
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = Self::success(response)?;
        let headers = response.headers();
        let e = RemoteEntry {
            name: leaf(path),
            path: path.into(),
            is_dir: path.ends_with('/'),
            size: headers
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            modified: headers
                .get("last-modified")
                .and_then(|v| v.to_str().ok())
                .map(|s| modified_string(s.into())),
        };
        let version = headers
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        Ok(Some((e, version)))
    }
    async fn download(
        &self,
        path: &str,
        local: &Path,
        generation: Option<&str>,
        c: &Control,
    ) -> Result<reqwest::header::HeaderMap> {
        self.download_limited(path, local, generation, None, c)
            .await
    }
    async fn download_limited(
        &self,
        path: &str,
        local: &Path,
        generation: Option<&str>,
        limit: Option<u64>,
        c: &Control,
    ) -> Result<reqwest::header::HeaderMap> {
        let mut url = self.object_url(path)?;
        if self.provider == ObjectProvider::Google {
            url.query_pairs_mut().append_pair("alt", "media");
            if let Some(g) = generation {
                url.query_pairs_mut().append_pair("generation", g);
            }
        }
        let response = Self::success(self.send(self.request(Method::GET, url), c).await?)?;
        let headers = response.headers().clone();
        if let Some(n) = response.content_length() {
            c.total(n);
        }
        let temp = destination_temp(local)?;
        let mut file = tokio::fs::File::from_std(temp.reopen()?);
        let mut stream = response.bytes_stream();
        let mut received = 0u64;
        loop {
            let chunk =
                tokio::select! {r=stream.next()=>r,_=cancelled(c)=>bail!("Operation cancelled")};
            let Some(chunk) = chunk else {
                break;
            };
            let chunk = chunk.map_err(|_| anyhow::anyhow!("Download interrupted"))?;
            c.check()?;
            received = received
                .checked_add(chunk.len() as u64)
                .context("Download size overflow")?;
            if limit.is_some_and(|n| received > n) {
                bail!(
                    "Moving this R2 object requires S3-compatible credentials (300 MB API upload limit)"
                );
            }
            file.write_all(&chunk).await?;
            c.advance(chunk.len() as u64);
        }
        file.sync_all().await?;
        drop(file);
        c.check()?;
        temp.persist(local).map_err(|e| e.error)?;
        Ok(headers)
    }
    async fn upload(&self, path: &str, local: &Path, overwrite: bool, c: &Control) -> Result<()> {
        self.upload_with_class(path, local, overwrite, None, c)
            .await
    }
    async fn upload_with_class(
        &self,
        path: &str,
        local: &Path,
        overwrite: bool,
        storage_class: Option<&str>,
        c: &Control,
    ) -> Result<()> {
        self.object_url(path)?;
        let size = tokio::fs::metadata(local).await?.len();
        c.total(size);
        if self.provider == ObjectProvider::Cloudflare && size > R2_MAX_UPLOAD {
            bail!(
                "Cloudflare API uploads are limited to 300 MB; use S3-compatible credentials for larger files"
            );
        }
        if !overwrite
            && self.provider == ObjectProvider::Cloudflare
            && self.stat(path, c).await?.is_some()
        {
            bail!("A file with this name already exists");
        }
        let (method, url) = if self.provider == ObjectProvider::Google {
            let mut url = url::Url::parse(&self.upload_base)?;
            url.query_pairs_mut()
                .append_pair("uploadType", "media")
                .append_pair("name", path);
            if !overwrite {
                url.query_pairs_mut().append_pair("ifGenerationMatch", "0");
            }
            (Method::POST, url)
        } else {
            (Method::PUT, self.object_url(path)?)
        };
        let file = tokio::fs::File::open(local).await?;
        let stream = tokio_util::io::ReaderStream::new(file);
        let mut request = self
            .request(method, url)
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", size)
            .body(reqwest::Body::wrap_stream(stream));
        if let Some(class) = storage_class {
            request = request.header("cf-r2-storage-class", class);
        }
        let confirmed = self.json(request, c).await?;
        let name = if self.provider == ObjectProvider::Google {
            confirmed.get("name")
        } else {
            confirmed.get("result").and_then(|v| v.get("key"))
        };
        if name.and_then(Value::as_str) != Some(path) {
            bail!(
                "The provider did not confirm the uploaded object; refresh the listing before retrying"
            );
        }
        if let Some(class) = storage_class {
            if confirmed
                .get("result")
                .and_then(|v| v.get("storage_class"))
                .and_then(Value::as_str)
                != Some(class)
            {
                bail!(
                    "Cloudflare did not confirm the copied object's storage class; the source was retained"
                );
            }
        }
        c.advance(size);
        Ok(())
    }
    async fn delete(&self, path: &str, generation: Option<&str>, c: &Control) -> Result<()> {
        let mut url = self.object_url(path)?;
        if self.provider == ObjectProvider::Google {
            if let Some(g) = generation {
                url.query_pairs_mut().append_pair("ifGenerationMatch", g);
            }
        }
        let response = self.send(self.request(Method::DELETE, url), c).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        let response = Self::success(response)?;
        if self.provider == ObjectProvider::Cloudflare {
            let v = self.read_value(response, c).await?;
            if v.get("success").and_then(Value::as_bool) != Some(true) {
                bail!("Cloudflare rejected object deletion");
            }
        }
        Ok(())
    }
    async fn rename(&self, from: &str, to: &str, overwrite: bool, c: &Control) -> Result<()> {
        if from == to {
            return Ok(());
        }
        self.object_url(to)?;
        let (source, version) = self
            .stat(from, c)
            .await?
            .context("The source object does not exist")?;
        if self.provider == ObjectProvider::Google {
            let generation = version
                .as_deref()
                .context("GCS did not return the source generation")?;
            let mut base = self.object_url(from)?;
            {
                let mut parts = base
                    .path_segments_mut()
                    .map_err(|_| anyhow::anyhow!("Invalid object URL"))?;
                parts.extend(["rewriteTo", "b", &self.bucket, "o", to]);
            }
            let mut token = None::<String>;
            let deadline = std::time::Instant::now() + Duration::from_secs(300);
            for _ in 0..10000 {
                c.check()?;
                if std::time::Instant::now() > deadline {
                    bail!("GCS rewrite timed out; the source was retained");
                }
                let mut url = base.clone();
                {
                    let mut q = url.query_pairs_mut();
                    q.append_pair("ifSourceGenerationMatch", generation);
                    if !overwrite {
                        q.append_pair("ifGenerationMatch", "0");
                    }
                    if let Some(t) = &token {
                        q.append_pair("rewriteToken", t);
                    }
                }
                let v = self
                    .json(
                        self.request(Method::POST, url).json(&serde_json::json!({})),
                        c,
                    )
                    .await?;
                if v.get("done").and_then(Value::as_bool) == Some(true) {
                    let resource = v.get("resource").context(
                        "GCS did not confirm the copied object; the source was retained",
                    )?;
                    if resource.get("name").and_then(Value::as_str) != Some(to)
                        || resource
                            .get("generation")
                            .and_then(Value::as_str)
                            .is_none_or(str::is_empty)
                    {
                        bail!("GCS did not confirm the copied object; the source was retained");
                    }
                    c.check()?;
                    self.delete(from, Some(generation), c).await?;
                    return Ok(());
                }
                let next = v
                    .get("rewriteToken")
                    .and_then(Value::as_str)
                    .context("Missing GCS rewrite cursor")?;
                if token.as_deref() == Some(next) {
                    bail!("GCS rewrite cursor did not advance");
                }
                token = Some(next.into());
            }
            bail!("GCS rewrite exceeded the safe page limit");
        }
        if source.size > R2_MAX_UPLOAD {
            bail!(
                "Moving this R2 object requires S3-compatible credentials (300 MB API upload limit)"
            );
        }
        if !overwrite && self.stat(to, c).await?.is_some() {
            bail!("A file with this name already exists");
        }
        // List metadata is authoritative; response headers alone can omit custom
        // metadata or contain cache headers added by the API edge.
        let mut metadata_url = url::Url::parse(&self.base)?;
        metadata_url
            .query_pairs_mut()
            .append_pair("prefix", from)
            .append_pair("per_page", "2");
        let metadata = self
            .json(self.request(Method::GET, metadata_url), c)
            .await?;
        let object=metadata.get("result").and_then(Value::as_array).and_then(|a|a.iter().find(|o|o.get("key").and_then(Value::as_str)==Some(from))).context("Source metadata is unavailable; use S3-compatible credentials to move this R2 object")?;
        if object.get("storage_class").and_then(Value::as_str) != Some("Standard")
            || object
                .get("ssec")
                .is_some_and(|v| !v.is_null() && v.as_bool() != Some(false))
        {
            bail!(
                "Use S3-compatible credentials to move R2 objects with non-Standard or unknown storage class, or customer-supplied encryption"
            );
        }
        let custom = object
            .get("custom_metadata")
            .and_then(Value::as_object)
            .is_some_and(|m| !m.is_empty());
        let http = object
            .get("http_metadata")
            .and_then(Value::as_object)
            .is_some_and(|m| {
                m.iter().any(|(k, v)| {
                    !v.is_null()
                        && v.as_str() != Some("")
                        && !(k == "contentType" && v.as_str() == Some("application/octet-stream"))
                })
            });
        if custom || http {
            bail!("Use S3-compatible credentials to move R2 objects with HTTP or custom metadata");
        }
        let dir = tempfile::tempdir()?;
        let local = dir.path().join("object");
        let headers = self
            .download_limited(from, &local, None, Some(R2_MAX_UPLOAD), c)
            .await?;
        // Native R2 does not expose an atomic rename. Refuse metadata-bearing
        // objects rather than silently strip their HTTP/custom metadata.
        if headers.keys().any(|h| {
            h.as_str().starts_with("x-amz-meta-")
                || matches!(
                    h.as_str(),
                    "cache-control"
                        | "content-disposition"
                        | "content-encoding"
                        | "content-language"
                        | "expires"
                )
        }) || headers
            .get("content-type")
            .is_some_and(|v| v != "application/octet-stream")
        {
            bail!("Use S3-compatible credentials to move R2 objects with HTTP or custom metadata");
        }
        c.check()?;
        c.bytes.store(0, Ordering::Relaxed);
        self.upload_with_class(to, &local, overwrite, Some("Standard"), c)
            .await?;
        c.check()?;
        let now = self
            .stat(from, c)
            .await?
            .context("Source changed during the move; the destination copy was retained")?;
        if version.is_none() || version != now.1 {
            bail!("Source changed during the move; both objects were retained");
        }
        self.delete(from, None, c).await
    }
    async fn run(&self, op: &Operation, c: &Control) -> Result<RemoteResult> {
        match op {
            Operation::List { path, cursor } => Ok(RemoteResult::Listed(
                self.list(key(path)?, cursor.as_deref(), true, 500, c)
                    .await?,
            )),
            Operation::Stat { path } => Ok(RemoteResult::Entry(
                self.stat(require_file(path)?, c)
                    .await?
                    .context("The object does not exist")?
                    .0,
            )),
            Operation::Download { path, local } => {
                self.download(require_file(path)?, local, None, c).await?;
                Ok(RemoteResult::Done)
            }
            Operation::Upload {
                local,
                path,
                overwrite,
            } => {
                self.upload(require_file(path)?, local, *overwrite, c)
                    .await?;
                Ok(RemoteResult::Done)
            }
            Operation::Rename {
                from,
                to,
                overwrite,
            } => {
                self.rename(require_file(from)?, require_file(to)?, *overwrite, c)
                    .await?;
                Ok(RemoteResult::Done)
            }
            Operation::Delete { path, is_dir } => {
                let p = key(path)?;
                if p.is_empty() {
                    bail!("Cannot delete the bucket root");
                }
                let p = if *is_dir {
                    format!("{}/", p.trim_end_matches('/'))
                } else {
                    require_file(path)?.into()
                };
                if *is_dir {
                    let page = self.list(&p, None, false, 2, c).await?;
                    if page.next_cursor.is_some() || page.entries.iter().any(|e| e.path != p) {
                        bail!("Folder is not empty");
                    }
                    if page.entries.is_empty() {
                        return Ok(RemoteResult::Done);
                    }
                }
                self.delete(&p, None, c).await?;
                Ok(RemoteResult::Done)
            }
            Operation::CreateDir { path } => {
                let p = key(path)?;
                if p.is_empty() {
                    bail!("A folder name is required");
                }
                let p = format!("{}/", p.trim_end_matches('/'));
                if !self.list(&p, None, false, 1, c).await?.entries.is_empty() {
                    return Ok(RemoteResult::Done);
                }
                let temp = tempfile::NamedTempFile::new()?;
                self.upload(&p, temp.path(), false, c).await?;
                Ok(RemoteResult::Done)
            }
        }
    }
}
fn entry(provider: ObjectProvider, v: &Value) -> Result<RemoteEntry> {
    let path = v
        .get(if provider == ObjectProvider::Google {
            "name"
        } else {
            "key"
        })
        .and_then(Value::as_str)
        .context("Object storage response omitted a name")?;
    let size = v
        .get("size")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0);
    Ok(RemoteEntry {
        name: leaf(path),
        path: path.into(),
        is_dir: path.ends_with('/'),
        size,
        modified: v
            .get(if provider == ObjectProvider::Google {
                "updated"
            } else {
                "last_modified"
            })
            .and_then(Value::as_str)
            .map(|s| modified_string(s.into())),
    })
}
fn parse_list(
    provider: ObjectProvider,
    v: &Value,
    prefix: &str,
    delimiter: bool,
) -> Result<ListPage> {
    let mut entries = Vec::new();
    let objects = v.get(if provider == ObjectProvider::Google {
        "items"
    } else {
        "result"
    });
    if provider == ObjectProvider::Cloudflare && objects.is_none() {
        bail!("Cloudflare response omitted the object list");
    }
    if let Some(objects) = objects {
        let objects = objects.as_array().context("Invalid object list")?;
        for o in objects {
            let e = entry(provider, o)?;
            if !delimiter || e.path != prefix {
                entries.push(e);
            }
        }
    }
    let (prefixes, cursor) = if provider == ObjectProvider::Google {
        (
            v.get("prefixes"),
            v.get("nextPageToken").and_then(Value::as_str),
        )
    } else {
        let info = v.get("result_info");
        let cursor = info
            .and_then(|v| v.get("cursor"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        if info
            .and_then(|v| v.get("is_truncated"))
            .and_then(Value::as_bool)
            == Some(true)
            && cursor.is_none()
        {
            bail!("Cloudflare returned a truncated list without a cursor");
        }
        (
            info.and_then(|v| v.get("delimited")),
            if info
                .and_then(|v| v.get("is_truncated"))
                .and_then(Value::as_bool)
                == Some(false)
            {
                None
            } else {
                cursor
            },
        )
    };
    if let Some(prefixes) = prefixes.and_then(Value::as_array) {
        for p in prefixes {
            if let Some(p) = p.as_str() {
                if !entries.iter().any(|e| e.path == p) {
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
    }
    Ok(ListPage {
        entries: sorted(entries),
        next_cursor: cursor.filter(|s| !s.is_empty()).map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
    };
    struct Request {
        method: String,
        target: String,
        headers: String,
        body: Vec<u8>,
    }
    struct Fixture {
        base: String,
        rx: mpsc::Receiver<Request>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl Fixture {
        fn new(responses: Vec<Vec<u8>>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/objects", listener.local_addr().unwrap());
            let (tx, rx) = mpsc::channel();
            listener.set_nonblocking(true).unwrap();
            let thread = std::thread::spawn(move || {
                // A speculative/idle socket must not monopolize the fixture's
                // accept loop or consume a scripted response. Read connections
                // concurrently; assign responses only to complete requests.
                let (ready_tx, ready_rx) = mpsc::channel::<(std::net::TcpStream, Request)>();
                let mut workers = Vec::new();
                for (response_index, response) in responses.into_iter().enumerate() {
                    let started = std::time::Instant::now();
                    let (mut stream, request) = loop {
                        if let Ok(request) = ready_rx.try_recv() {
                            break request;
                        }
                        assert!(
                            started.elapsed() < Duration::from_secs(5),
                            "Fixture response {response_index}: complete request did not arrive"
                        );
                        match listener.accept() {
                            Ok((mut stream, _)) => {
                                assert!(workers.len() < 64, "Too many fixture connections");
                                let ready = ready_tx.clone();
                                workers.push(std::thread::spawn(move || {
                                    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                                    let mut bytes = Vec::new();
                                    loop {
                                        let mut b = [0];
                                        if let Err(e) = stream.read_exact(&mut b) {
                                            if bytes.is_empty() && matches!(e.kind(),
                                                std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) {
                                                return; // Idle socket, no request or response.
                                            }
                                            panic!("Fixture request header stopped after {} bytes: {e}", bytes.len());
                                        }
                                        bytes.push(b[0]);
                                        if bytes.ends_with(b"\r\n\r\n") { break; }
                                        assert!(bytes.len() < 16384);
                                    }
                                    let headers = String::from_utf8(bytes).unwrap();
                                    let mut words = headers.lines().next().unwrap().split_whitespace();
                                    let method = words.next().unwrap().into();
                                    let target = words.next().unwrap().into();
                                    let length = headers.lines().find_map(|l| {
                                        l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                                            .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                                    }).unwrap_or(0);
                                    assert!(length < 1024 * 1024);
                                    let mut body = vec![0; length];
                                    stream.read_exact(&mut body).unwrap();
                                    ready.send((stream, Request { method, target, headers, body })).unwrap();
                                }));
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                std::thread::sleep(Duration::from_millis(5))
                            }
                            Err(e) => panic!("fixture accept failed: {e}"),
                        }
                    };
                    tx.send(request).unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let _ = stream.write_all(&response);
                }
                for worker in workers {
                    worker.join().unwrap();
                }
                assert!(
                    ready_rx.try_recv().is_err(),
                    "Unexpected additional fixture request"
                );
            });
            Self {
                base,
                rx,
                thread: Some(thread),
            }
        }
        fn requests(mut self, n: usize) -> Vec<Request> {
            let mut requests = Vec::new();
            for _ in 0..n {
                requests.push(self.rx.recv_timeout(Duration::from_secs(4)).unwrap());
            }
            self.thread.take().unwrap().join().unwrap();
            assert!(self.rx.try_recv().is_err());
            requests
        }
    }
    fn response(status: u16, body: &str) -> Vec<u8> {
        format!("HTTP/1.1 {status} fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).into_bytes()
    }
    fn api(provider: ObjectProvider, base: String) -> Api {
        Api {
            provider,
            bucket: "fixture-bucket".into(),
            upload_base: base.clone(),
            base,
            auth: Auth::Bearer("fixture-not-a-real-token".into()),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
        }
    }
    #[tokio::test]
    async fn idle_connection_cannot_block_a_real_request_or_consume_its_response() {
        let f = Fixture::new(vec![response(
            200,
            r#"{"items":[{"name":"actual","size":"1"}]}"#,
        )]);
        let address = f
            .base
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        let idle = std::net::TcpStream::connect(address).unwrap();
        // The idle socket is queued before reqwest opens the active socket.
        let a = api(ObjectProvider::Google, f.base.clone());
        let page = tokio::time::timeout(
            Duration::from_secs(2),
            a.list("", None, true, 500, &Control::default()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].name, "actual");
        drop(idle);
        let requests = f.requests(1);
        assert_eq!(requests[0].method, "GET");
        assert!(!requests[0].target.contains("alt=media"));
    }
    #[test]
    fn provider_key_encoding_and_pagination_are_exact() {
        let g = api(
            ObjectProvider::Google,
            "https://storage.googleapis.com/storage/v1/b/bucket/o".into(),
        );
        let r = api(
            ObjectProvider::Cloudflare,
            "https://api.cloudflare.com/client/v4/accounts/fixture/r2/buckets/bucket/objects"
                .into(),
        );
        let path = "folder/한글 [file]?#%.txt";
        assert!(g.object_url(path).unwrap().path().contains("folder%2F"));
        let url = r.object_url(path).unwrap();
        assert!(url.path().contains("/folder/"));
        assert!(url.query().is_none());
        assert!(url.fragment().is_none());
        let v = serde_json::json!({"items":[{"name":"folder/","size":"0"},{"name":"folder/a","size":"9"}],"prefixes":["folder/child/"],"nextPageToken":"next"});
        let page = parse_list(ObjectProvider::Google, &v, "folder/", true).unwrap();
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.next_cursor.as_deref(), Some("next"));
        assert!(page.entries[0].is_dir);
        assert_eq!(
            parse_list(ObjectProvider::Google, &v, "folder/", false)
                .unwrap()
                .entries
                .len(),
            3
        );
        let v = serde_json::json!({"success":true,"result":[{"key":"a","size":7}],"result_info":{"is_truncated":true}});
        assert!(parse_list(ObjectProvider::Cloudflare, &v, "", true).is_err());
    }
    #[test]
    fn invalid_tokens_and_api_payloads_do_not_echo_secrets() {
        for s in ["", "private\nsecret", "private secret"] {
            assert!(
                !checked_token(s)
                    .unwrap_err()
                    .to_string()
                    .contains("private")
            );
        }
        assert!(
            !parse_json(b"private-token-is-not-json")
                .unwrap_err()
                .to_string()
                .contains("private-token")
        );
    }
    #[test]
    fn saved_legacy_s3_remains_deserializable() {
        let json = r#"{"id":"legacy","name":"S3","endpoint":{"S3":{"bucket":"fixture-bucket","region":"us-east-1","endpoint":null,"path_style":false,"prefix":"/"}}}"#;
        let p: ConnectionProfile = serde_json::from_str(json).unwrap();
        p.validate().unwrap();
        assert_eq!(p.endpoint.protocol(), "S3");
    }
    #[tokio::test]
    async fn gcs_crud_uses_literal_keys_generation_guards_and_atomic_downloads() {
        let f = Fixture::new(vec![
            response(
                200,
                r#"{"items":[{"name":"folder/file","size":"3"}],"nextPageToken":"next"}"#,
            ),
            response(200, "abc"),
            response(200, r#"{"name":"folder/한글 [file]","size":"3"}"#),
            response(200, r#"{"name":"source","size":"3","generation":"42"}"#),
            response(200, r#"{"done":false,"rewriteToken":"continue"}"#),
            response(
                200,
                r#"{"done":true,"resource":{"name":"target","generation":"43"}}"#,
            ),
            response(204, ""),
            response(200, "{}"),
            response(200, r#"{"name":"new-folder/","size":"0"}"#),
            response(200, r#"{"items":[{"name":"new-folder/","size":"0"}]}"#),
            response(204, ""),
        ]);
        let a = api(ObjectProvider::Google, f.base.clone());
        let c = Control::default();
        let d = tempfile::tempdir().unwrap();
        let local = d.path().join("file");
        std::fs::write(&local, "old").unwrap();
        let page = a
            .list("folder/", Some("previous"), true, 500, &c)
            .await
            .unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("next"));
        a.download("folder/한글 [file]", &local, None, &c)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&local).unwrap(), b"abc");
        a.upload("folder/한글 [file]", &local, false, &c)
            .await
            .unwrap();
        a.rename("source", "target", false, &c).await.unwrap();
        a.run(
            &Operation::CreateDir {
                path: "new-folder".into(),
            },
            &c,
        )
        .await
        .unwrap();
        a.run(
            &Operation::Delete {
                path: "new-folder/".into(),
                is_dir: true,
            },
            &c,
        )
        .await
        .unwrap();
        let r = f.requests(11);
        assert_eq!(r[0].method, "GET");
        assert!(r[0].target.contains("pageToken=previous"));
        assert!(r[0].target.contains("delimiter=%2F"));
        assert!(r[1].target.contains("folder%2F"));
        assert!(r[1].target.ends_with("?alt=media"));
        assert_eq!(r[2].body, b"abc");
        assert!(r[2].target.contains("ifGenerationMatch=0"));
        assert!(
            r[2].headers
                .to_ascii_lowercase()
                .contains("authorization: bearer fixture-not-a-real-token")
        );
        assert!(r[4].target.contains("ifSourceGenerationMatch=42"));
        assert!(r[4].target.contains("ifGenerationMatch=0"));
        assert!(r[5].target.contains("rewriteToken=continue"));
        assert_eq!(r[6].method, "DELETE");
        assert!(r[6].target.contains("ifGenerationMatch=42"));
        assert!(r[8].body.is_empty());
        assert_eq!(r[10].method, "DELETE");
    }
    #[tokio::test]
    async fn failed_copy_and_nonempty_folder_never_send_source_delete() {
        let f = Fixture::new(vec![
            response(200, r#"{"name":"source","size":"3","generation":"42"}"#),
            response(412, r#"{"private-token":"must not be echoed"}"#),
            response(
                200,
                r#"{"items":[{"name":"folder/","size":"0"},{"name":"folder/file","size":"2"}]}"#,
            ),
        ]);
        let a = api(ObjectProvider::Google, f.base.clone());
        let c = Control::default();
        let err = a.rename("source", "target", false, &c).await.unwrap_err();
        assert!(err.to_string().contains("412"));
        assert!(!err.to_string().contains("private-token"));
        assert!(
            a.run(
                &Operation::Delete {
                    path: "folder/".into(),
                    is_dir: true
                },
                &c
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("not empty")
        );
        let r = f.requests(3);
        assert!(r.iter().all(|r| r.method != "DELETE"));
    }
    #[tokio::test]
    async fn r2_crud_preserves_literal_slashes_and_checks_collisions() {
        let f = Fixture::new(vec![
            response(
                200,
                r#"{"success":true,"result":[{"key":"folder/a","size":3}],"result_info":{"delimited":["folder/child/"],"is_truncated":true,"cursor":"next"}}"#,
            ),
            response(404, "{}"),
            response(
                200,
                r#"{"success":true,"result":{"key":"folder/한글 [file]"}}"#,
            ),
            response(200, "abc"),
            response(
                200,
                r#"{"success":true,"result":{"key":"folder/한글 [file]"}}"#,
            ),
        ]);
        let a = api(ObjectProvider::Cloudflare, f.base.clone());
        let c = Control::default();
        let d = tempfile::tempdir().unwrap();
        let local = d.path().join("file");
        std::fs::write(&local, "abc").unwrap();
        let page = a.list("folder/", None, true, 500, &c).await.unwrap();
        assert!(page.entries[0].is_dir);
        assert_eq!(page.next_cursor.as_deref(), Some("next"));
        a.upload("folder/한글 [file]", &local, false, &c)
            .await
            .unwrap();
        a.download("folder/한글 [file]", &local, None, &c)
            .await
            .unwrap();
        a.delete("folder/한글 [file]", None, &c).await.unwrap();
        let r = f.requests(5);
        assert_eq!(r[2].method, "PUT");
        assert_eq!(r[2].body, b"abc");
        assert!(r[2].target.contains("/objects/folder/"));
        assert!(!r[2].target.contains("folder%2F"));
        assert_eq!(r[4].method, "DELETE");
    }
    #[tokio::test]
    async fn failed_or_cancelled_download_does_not_replace_existing_file() {
        let broken =
            b"HTTP/1.1 200 fixture\r\nContent-Length: 9\r\nConnection: close\r\n\r\nabc".to_vec();
        let f = Fixture::new(vec![broken]);
        let a = api(ObjectProvider::Google, f.base.clone());
        let d = tempfile::tempdir().unwrap();
        let local = d.path().join("file");
        std::fs::write(&local, "original").unwrap();
        assert!(
            a.download("file", &local, None, &Control::default())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&local).unwrap(), b"original");
        f.requests(1);
        let c = Control::default();
        c.cancel.store(true, Ordering::Relaxed);
        assert!(
            a.download("file", &local, None, &c)
                .await
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        assert_eq!(std::fs::read(&local).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
    }
    #[tokio::test]
    async fn r2_limits_and_dot_segments_fail_before_network_mutation() {
        let a = api(
            ObjectProvider::Cloudflare,
            "http://127.0.0.1:1/objects".into(),
        );
        let d = tempfile::tempdir().unwrap();
        let local = d.path().join("large");
        let file = std::fs::File::create(&local).unwrap();
        file.set_len(R2_MAX_UPLOAD + 1).unwrap();
        assert!(
            a.upload("file", &local, true, &Control::default())
                .await
                .unwrap_err()
                .to_string()
                .contains("300 MB")
        );
        assert!(a.object_url("folder/../wrong").is_err());
        assert!(a.object_url("folder/./wrong").is_err());
    }
    fn r2_object(body: &str, etag: &str) -> Vec<u8> {
        format!("HTTP/1.1 200 fixture\r\nContent-Type: application/octet-stream\r\nETag: {etag}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).into_bytes()
    }
    #[tokio::test]
    async fn r2_move_only_deletes_after_completed_copy_and_unchanged_source() {
        let f = Fixture::new(vec![
            r2_object("abc", "source-version"),
            response(404, "{}"),
            response(
                200,
                r#"{"success":true,"result":[{"key":"source","size":3,"storage_class":"Standard","custom_metadata":{},"http_metadata":{"contentType":"application/octet-stream"}}]}"#,
            ),
            r2_object("abc", "source-version"),
            response(404, "{}"),
            response(
                200,
                r#"{"success":true,"result":{"key":"target","storage_class":"Standard"}}"#,
            ),
            r2_object("abc", "source-version"),
            response(200, r#"{"success":true,"result":{"key":"source"}}"#),
        ]);
        let a = api(ObjectProvider::Cloudflare, f.base.clone());
        a.rename("source", "target", false, &Control::default())
            .await
            .unwrap();
        let r = f.requests(8);
        assert_eq!(r[5].method, "PUT");
        assert_eq!(r[5].body, b"abc");
        // Explicit Standard wins even if the destination bucket defaults to IA.
        assert!(
            r[5].headers
                .to_ascii_lowercase()
                .contains("cf-r2-storage-class: standard")
        );
        assert_eq!(r[6].method, "GET");
        assert_eq!(r[7].method, "DELETE");
        assert!(r[7].target.ends_with("/source"));
        let f = Fixture::new(vec![
            r2_object("abc", "source-version"),
            response(404, "{}"),
            response(
                200,
                r#"{"success":true,"result":[{"key":"source","size":3,"storage_class":"Standard"}]}"#,
            ),
            r2_object("abc", "source-version"),
            response(404, "{}"),
            response(
                503,
                r#"{"success":false,"errors":[{"message":"private-secret"}]}"#,
            ),
        ]);
        let a = api(ObjectProvider::Cloudflare, f.base.clone());
        let err = a
            .rename("source", "target", false, &Control::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("503"));
        assert!(!err.to_string().contains("private-secret"));
        assert!(f.requests(6).iter().all(|r| r.method != "DELETE"));
        let f = Fixture::new(vec![
            r2_object("abc", "source-version"),
            response(404, "{}"),
            response(
                200,
                r#"{"success":true,"result":[{"key":"source","size":3,"storage_class":"Standard","custom_metadata":{"keep":"yes"}}]}"#,
            ),
        ]);
        let a = api(ObjectProvider::Cloudflare, f.base.clone());
        assert!(
            a.rename("source", "target", false, &Control::default())
                .await
                .unwrap_err()
                .to_string()
                .contains("metadata")
        );
        assert!(f.requests(3).iter().all(|r| r.method == "GET"));
    }
    #[tokio::test]
    async fn r2_move_retains_source_for_unknown_storage_class_encryption_or_unconfirmed_destination()
     {
        for extra in [
            r#""storage_class":"InfrequentAccess""#,
            r#""storage_class":"future-class""#,
            r#""ssec":false"#,
            r#""storage_class":"Standard","ssec":true"#,
        ] {
            let metadata =
                format!(r#"{{"success":true,"result":[{{"key":"source","size":3,{extra}}}]}}"#);
            let f = Fixture::new(vec![
                r2_object("abc", "source-version"),
                response(404, "{}"),
                response(200, &metadata),
            ]);
            let a = api(ObjectProvider::Cloudflare, f.base.clone());
            assert!(
                a.rename("source", "target", false, &Control::default())
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("S3-compatible")
            );
            assert!(f.requests(3).iter().all(|r| r.method == "GET"));
        }
        for destination in [
            r#"{"key":"target","storage_class":"InfrequentAccess"}"#,
            r#"{"key":"target"}"#,
        ] {
            let confirmation = format!(r#"{{"success":true,"result":{destination}}}"#);
            let f = Fixture::new(vec![
                r2_object("abc", "source-version"),
                response(404, "{}"),
                response(
                    200,
                    r#"{"success":true,"result":[{"key":"source","size":3,"storage_class":"Standard","ssec":false}]}"#,
                ),
                r2_object("abc", "source-version"),
                response(404, "{}"),
                response(200, &confirmation),
            ]);
            let a = api(ObjectProvider::Cloudflare, f.base.clone());
            assert!(
                a.rename("source", "target", false, &Control::default())
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("source was retained")
            );
            let requests = f.requests(6);
            assert!(requests.iter().all(|r| r.method != "DELETE"));
            assert!(
                requests[5]
                    .headers
                    .to_ascii_lowercase()
                    .contains("cf-r2-storage-class: standard")
            );
        }
    }
    #[tokio::test]
    async fn cancellation_interrupts_a_stalled_response_and_preserves_local_file() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/objects", listener.local_addr().unwrap());
        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut headers = Vec::new();
            loop {
                let mut b = [0];
                socket.read_exact(&mut b).unwrap();
                headers.push(b[0]);
                if headers.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            socket
                .write_all(
                    b"HTTP/1.1 200 fixture\r\nContent-Length: 9\r\nConnection: close\r\n\r\nabc",
                )
                .unwrap();
            ready_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(3));
        });
        let c = Arc::new(Control::default());
        let cc = c.clone();
        let cancel = std::thread::spawn(move || {
            ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            cc.cancel.store(true, Ordering::Relaxed);
        });
        let a = api(ObjectProvider::Google, base);
        let d = tempfile::tempdir().unwrap();
        let local = d.path().join("file");
        std::fs::write(&local, "original").unwrap();
        let started = std::time::Instant::now();
        let err = a.download("file", &local, None, &c).await.unwrap_err();
        assert!(err.to_string().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(std::fs::read(&local).unwrap(), b"original");
        release_tx.send(()).unwrap();
        thread.join().unwrap();
        cancel.join().unwrap();
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
    }
    #[tokio::test]
    async fn unconfirmed_success_payloads_never_trigger_source_delete() {
        let f = Fixture::new(vec![
            response(200, r#"{"name":"source","size":"3","generation":"42"}"#),
            response(200, r#"{"done":true}"#),
        ]);
        let a = api(ObjectProvider::Google, f.base.clone());
        assert!(
            a.rename("source", "target", false, &Control::default())
                .await
                .unwrap_err()
                .to_string()
                .contains("source was retained")
        );
        assert!(f.requests(2).iter().all(|r| r.method != "DELETE"));
        let f = Fixture::new(vec![
            r2_object("abc", "source-version"),
            response(404, "{}"),
            response(
                200,
                r#"{"success":true,"result":[{"key":"source","size":3,"storage_class":"Standard"}]}"#,
            ),
            r2_object("abc", "source-version"),
            response(404, "{}"),
            response(200, r#"{"success":true,"result":{}}"#),
        ]);
        let a = api(ObjectProvider::Cloudflare, f.base.clone());
        let err = a
            .rename("source", "target", false, &Control::default())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("did not confirm"),
            "Expected upload confirmation guard, got: {err:#}"
        );
        assert!(f.requests(6).iter().all(|r| r.method != "DELETE"));
    }
}
