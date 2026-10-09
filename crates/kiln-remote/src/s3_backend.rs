use crate::*;
use s3::{Bucket, Region, creds::Credentials};
use std::{
    pin::Pin,
    task::{Context as TaskContext, Poll},
};
use tokio::io::AsyncWrite;
fn status(code: u16) -> Result<()> {
    if !(200..300).contains(&code) {
        bail!("S3 request failed (HTTP {code})")
    }
    Ok(())
}
fn key(path: &str) -> Result<&str> {
    safe_text(path)?;
    Ok(path.trim_start_matches('/'))
}
fn require_object(path: &str) -> Result<&str> {
    let p = key(path)?;
    if p.is_empty() || p.ends_with('/') {
        bail!("Select a file, not a folder")
    };
    Ok(p)
}
pub(super) fn bucket(
    profile: &ConnectionProfile,
    secrets: &Secrets,
    control: &Control,
) -> Result<Box<Bucket>> {
    let RemoteEndpoint::S3 {
        bucket,
        region,
        endpoint,
        path_style,
        aws_profile,
        aws_auth,
        ..
    } = &profile.endpoint
    else {
        unreachable!()
    };
    let region = if let Some(endpoint) = endpoint {
        Region::Custom {
            region: region.clone(),
            endpoint: endpoint.clone(),
        }
    } else {
        region.parse()?
    };
    let resolved;
    let secrets = if let Some(name) = aws_profile {
        resolved = crate::aws_profiles::resolve(name, control)?;
        &resolved
    } else if *aws_auth == Some(S3Authentication::Default) {
        resolved = Secrets::default();
        &resolved
    } else {
        if *aws_auth == Some(S3Authentication::Manual)
            && (secrets.access_key.is_none() || secrets.secret_key.is_none())
        {
            bail!(
                "{}",
                kiln_common::i18n::tr("Access Key ID 및 Secret Access Key를 함께 입력하세요")
            );
        }
        secrets
    };
    let credentials = match (&secrets.access_key, &secrets.secret_key) {
        (Some(a), Some(s)) if !a.is_empty() && !s.is_empty() => Credentials::new(
            Some(a),
            Some(s),
            None,
            secrets.session_token.as_deref(),
            None,
        )?,
        (None, None) => {
            if let Some(s) = crate::aws_profiles::resolve_default(control)? {
                Credentials::new(
                    s.access_key.as_deref(),
                    s.secret_key.as_deref(),
                    None,
                    s.session_token.as_deref(),
                    None,
                )?
            } else {
                Credentials::default()?
            }
        }
        _ => bail!("Both access key and secret key are required"),
    };
    let b = Bucket::new(bucket, region, credentials)?;
    let mut b = if *path_style { b.with_path_style() } else { b };
    b.set_request_timeout(Some(Duration::from_secs(60)));
    Ok(b)
}
async fn exists(b: &Bucket, path: &str) -> Result<bool> {
    let (_, code) = b.head_object(path).await?;
    if code == 404 {
        return Ok(false);
    }
    status(code)?;
    Ok(true)
}
async fn no_replace(b: &Bucket, path: &str, overwrite: bool) -> Result<()> {
    if !overwrite && exists(b, path).await? {
        bail!("Destination already exists; confirm replacement")
    };
    Ok(())
}
pub async fn run(
    profile: &ConnectionProfile,
    secrets: &Secrets,
    op: &Operation,
    c: &Arc<Control>,
) -> Result<RemoteResult> {
    let b = bucket(profile, secrets, c)?;
    c.check()?;
    match op {
        Operation::List { path, cursor } => {
            let mut prefix = key(path)?.to_string();
            if !prefix.is_empty() && !prefix.ends_with('/') {
                prefix.push('/');
            }
            let (page, code) = b
                .list_page(
                    prefix.clone(),
                    Some("/".into()),
                    cursor.clone(),
                    None,
                    Some(500),
                )
                .await?;
            status(code)?;
            let mut entries = Vec::new();
            for p in page.common_prefixes.unwrap_or_default() {
                entries.push(RemoteEntry {
                    name: leaf(&p.prefix),
                    path: p.prefix,
                    is_dir: true,
                    size: 0,
                    modified: None,
                });
            }
            for p in page.contents {
                if p.key == prefix {
                    continue;
                }
                entries.push(RemoteEntry {
                    name: leaf(&p.key),
                    path: p.key.clone(),
                    is_dir: p.key.ends_with('/'),
                    size: p.size,
                    modified: Some(modified_string(p.last_modified)),
                });
            }
            if page.is_truncated && page.next_continuation_token.is_none() {
                bail!("S3 server omitted the next page cursor")
            }
            Ok(RemoteResult::Listed(ListPage {
                entries: sorted(entries),
                next_cursor: page.next_continuation_token,
            }))
        }
        Operation::Stat { path } => {
            let (meta, code) = b.head_object(key(path)?).await?;
            status(code)?;
            Ok(RemoteResult::Entry(RemoteEntry {
                name: leaf(path),
                path: path.clone(),
                is_dir: path.ends_with('/'),
                size: meta.content_length.unwrap_or_default().max(0) as u64,
                modified: meta.last_modified.map(modified_string),
            }))
        }
        Operation::Download { path, local } => {
            let path = require_object(path)?;
            let (meta, code) = b.head_object(path).await?;
            status(code)?;
            c.total(meta.content_length.unwrap_or_default().max(0) as u64);
            let temp = destination_temp(local)?;
            let f = tokio::fs::File::from_std(temp.reopen()?);
            let mut writer = ProgressWrite {
                inner: f,
                c: c.clone(),
            };
            status(b.get_object_to_writer(path, &mut writer).await?)?;
            tokio::io::AsyncWriteExt::flush(&mut writer).await?;
            writer.inner.sync_all().await?;
            c.check()?;
            temp.persist(local).map_err(|e| e.error)?;
            Ok(RemoteResult::Done)
        }
        Operation::Upload {
            local,
            path,
            overwrite,
        } => {
            let path = require_object(path)?;
            no_replace(&b, path, *overwrite).await?;
            let f = tokio::fs::File::open(local).await?;
            let size = f.metadata().await?.len();
            if size > 5 * 1024 * 1024 * 1024 {
                bail!("Staged S3 uploads currently support files up to 5 GiB")
            };
            c.total(size);
            // Multipart streaming keeps memory bounded. The final copy replaces the
            // destination only after the staging object has completely uploaded.
            let temp = temporary_remote(path);
            upload(&b, f, &temp, c).await?;
            let publish = async {
                c.check()?;
                no_replace(&b, path, *overwrite).await?;
                copy_object(&b, &temp, path, *overwrite, c).await?;
                Ok::<_, anyhow::Error>(())
            }
            .await;
            let cleanup = b.delete_object(&temp).await;
            publish?;
            let _ = cleanup;
            Ok(RemoteResult::Done)
        }
        Operation::Rename {
            from,
            to,
            overwrite,
        } => {
            let from = require_object(from)?;
            let to = require_object(to)?;
            let (meta, code) = b.head_object(from).await?;
            status(code)?;
            if meta.content_length.unwrap_or_default() > 5 * 1024 * 1024 * 1024 {
                bail!("S3 rename currently supports files up to 5 GiB")
            };
            if from == to {
                return Ok(RemoteResult::Done);
            };
            no_replace(&b, to, *overwrite).await?;
            c.check()?;
            copy_object(&b, from, to, *overwrite, c).await?;
            c.check()?;
            status(b.delete_object(from).await?.status_code())?;
            Ok(RemoteResult::Done)
        }
        Operation::Delete { path, is_dir } => {
            let path = key(path)?;
            if path.is_empty() {
                bail!("Cannot delete the bucket root")
            };
            if *is_dir {
                let prefix = format!("{}/", path.trim_end_matches('/'));
                let (page, code) = b
                    .list_page(prefix.clone(), None, None, None, Some(2))
                    .await?;
                status(code)?;
                if page.is_truncated || page.contents.iter().any(|v| v.key != prefix) {
                    bail!("Folder is not empty; delete its files first")
                };
                status(b.delete_object(&prefix).await?.status_code())?;
            } else {
                status(b.delete_object(path).await?.status_code())?;
            }
            Ok(RemoteResult::Done)
        }
        Operation::CreateDir { path } => {
            let p = key(path)?.trim_end_matches('/');
            if p.is_empty() {
                bail!("Enter a folder name")
            };
            status(b.put_object(format!("{p}/"), &[]).await?.status_code())?;
            Ok(RemoteResult::Done)
        }
    }
}
async fn request<T>(
    c: &Control,
    f: impl std::future::Future<Output = Result<T, s3::error::S3Error>>,
) -> Result<T> {
    tokio::select! {r=tokio::time::timeout(Duration::from_secs(60),f)=>Ok(r.context("S3 request timed out")??),_=crate::cancelled(c)=>bail!("Operation cancelled")}
}
async fn upload(b: &Bucket, mut file: tokio::fs::File, path: &str, c: &Control) -> Result<()> {
    use tokio::io::AsyncReadExt;
    if file.metadata().await?.len() == 0 {
        status(request(c, b.put_object(path, &[])).await?.status_code())?;
        return Ok(());
    }
    let start = request(
        c,
        b.initiate_multipart_upload(path, "application/octet-stream"),
    )
    .await?;
    let transfer = async {
        let mut parts = Vec::new();
        loop {
            c.check()?;
            let mut chunk = vec![0; 8 * 1024 * 1024];
            let mut size = 0;
            while size < chunk.len() {
                let n = file.read(&mut chunk[size..]).await?;
                if n == 0 {
                    break;
                }
                size += n;
            }
            if size == 0 {
                break;
            }
            chunk.truncate(size);
            let number = parts.len() as u32 + 1;
            if number > 10_000 {
                bail!("File exceeds the multipart upload limit")
            };
            parts.push(
                request(
                    c,
                    b.put_multipart_chunk(
                        chunk,
                        path,
                        number,
                        &start.upload_id,
                        "application/octet-stream",
                    ),
                )
                .await?,
            );
            c.advance(size as u64);
        }
        let response = request(
            c,
            b.complete_multipart_upload(path, &start.upload_id, parts),
        )
        .await?;
        status(response.status_code())?;
        confirm_copy(response.as_str()?)
            .context("S3 multipart completion did not confirm success")?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if transfer.is_err() {
        let _ = tokio::time::timeout(
            Duration::from_secs(30),
            b.abort_upload(path, &start.upload_id),
        )
        .await;
        let _ = tokio::time::timeout(Duration::from_secs(30), b.delete_object(path)).await;
    }
    transfer
}
struct ProgressWrite {
    inner: tokio::fs::File,
    c: Arc<Control>,
}
impl AsyncWrite for ProgressWrite {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.c.check().is_err() {
            return Poll::Ready(Err(std::io::Error::other("Operation cancelled")));
        }
        let p = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &p {
            self.c.advance(*n as u64);
        }
        p
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[derive(Deserialize)]
struct CopyResult {
    #[serde(rename = "ETag")]
    _etag: String,
}
async fn copy_object(b: &Bucket, from: &str, to: &str, overwrite: bool, c: &Control) -> Result<()> {
    use s3::request::{Request, tokio_backend::ReqwestRequest};
    let mut destination = b.clone();
    if !overwrite {
        destination.add_header("If-None-Match", "*");
    }
    let from = format!(
        "{}/{}",
        b.name(),
        percent_encoding::utf8_percent_encode(from, percent_encoding::NON_ALPHANUMERIC)
    );
    let req = ReqwestRequest::new(
        &destination,
        to,
        s3::command::Command::CopyObject { from: &from },
    )
    .await?;
    let response = request(c, req.response_data(false)).await?;
    status(response.status_code())?;
    confirm_copy(response.as_str()?)
        .context("S3 copy did not confirm success; the source was preserved")?;
    Ok(())
}

fn confirm_copy(body: &str) -> Result<()> {
    let result: CopyResult = quick_xml::de::from_str(body)?;
    if result._etag.trim().is_empty() {
        bail!("S3 returned an empty copy confirmation")
    };
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_s3_errors_never_confirm_copy() {
        assert!(confirm_copy("<CopyObjectResult><ETag>fixture</ETag></CopyObjectResult>").is_ok());
        assert!(
            confirm_copy("<Error><Code>InternalError</Code><Message>Not copied</Message></Error>")
                .is_err()
        );
        assert!(confirm_copy("<CopyObjectResult><ETag></ETag></CopyObjectResult>").is_err());
    }
}
