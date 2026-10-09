//! Reusable authentication metadata. Secrets live only in CredentialStore.
use crate::{ObjectAuthentication, ObjectProvider, RemoteEndpoint, S3Authentication, Secrets};
use anyhow::{Result, bail};
use kiln_accounts::CredentialStore;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum StorageIdentity {
    S3 {
        region: String,
        endpoint: Option<String>,
        path_style: bool,
        aws_profile: Option<String>,
        aws_auth: Option<S3Authentication>,
    },
    Object {
        provider: ObjectProvider,
        authentication: ObjectAuthentication,
        region: Option<String>,
        namespace: Option<String>,
        account_id: Option<String>,
    },
}
impl StorageIdentity {
    pub fn from_endpoint(endpoint: &RemoteEndpoint) -> Result<Self> {
        Ok(match endpoint {
            RemoteEndpoint::S3 {
                region,
                endpoint,
                path_style,
                aws_profile,
                aws_auth,
                ..
            } => Self::S3 {
                region: region.clone(),
                endpoint: endpoint.clone(),
                path_style: *path_style,
                aws_profile: aws_profile.clone(),
                aws_auth: *aws_auth,
            },
            RemoteEndpoint::ObjectStorage {
                provider,
                authentication,
                region,
                namespace,
                account_id,
                ..
            } => Self::Object {
                provider: *provider,
                authentication: authentication.clone(),
                region: region.clone(),
                namespace: namespace.clone(),
                account_id: account_id.clone(),
            },
            _ => bail!(kiln_common::i18n::tr("오브젝트 스토리지 인증만 프로필로 저장할 수 있습니다.")),
        })
    }
    pub fn endpoint(&self, bucket: String, prefix: String) -> RemoteEndpoint {
        match self {
            Self::S3 {
                region,
                endpoint,
                path_style,
                aws_profile,
                aws_auth,
            } => RemoteEndpoint::S3 {
                bucket,
                prefix,
                region: region.clone(),
                endpoint: endpoint.clone(),
                path_style: *path_style,
                aws_profile: aws_profile.clone(),
                aws_auth: *aws_auth,
            },
            Self::Object {
                provider,
                authentication,
                region,
                namespace,
                account_id,
            } => RemoteEndpoint::ObjectStorage {
                bucket,
                prefix,
                provider: *provider,
                authentication: authentication.clone(),
                region: region.clone(),
                namespace: namespace.clone(),
                account_id: account_id.clone(),
            },
        }
    }
    pub fn manual(&self) -> bool {
        matches!(
            self,
            Self::S3 {
                aws_profile: None,
                aws_auth: Some(S3Authentication::Manual),
                ..
            } | Self::Object {
                authentication: ObjectAuthentication::S3 {
                    aws_profile: None,
                    aws_auth: Some(S3Authentication::Manual),
                    ..
                },
                ..
            }
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthenticationProfile {
    pub id: String,
    pub name: String,
    pub identity: StorageIdentity,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub compartment_id: String,
}
#[derive(Clone)]
pub struct AuthenticationProfiles {
    inner: Arc<Inner>,
}
struct Inner {
    path: PathBuf,
    store: Arc<dyn CredentialStore>,
    profiles: Mutex<Vec<AuthenticationProfile>>,
    error: Option<String>,
}
impl AuthenticationProfiles {
    pub fn with_store(path: PathBuf, store: Arc<dyn CredentialStore>) -> Self {
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.to_string()),
        };
        let (profiles, error) = match data {
            Ok(p) => (p, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        Self {
            inner: Arc::new(Inner {
                path,
                store,
                profiles: Mutex::new(profiles),
                error,
            }),
        }
    }
    pub fn profiles(&self) -> Vec<AuthenticationProfile> {
        self.inner.profiles.lock().unwrap().clone()
    }
    pub fn error(&self) -> Option<&str> {
        self.inner.error.as_deref()
    }
    fn secret_id(id: &str) -> String {
        format!("authentication-{id}")
    }
    pub fn secrets(&self, id: &str) -> Result<Secrets> {
        crate::load_secrets(self.inner.store.as_ref(), &Self::secret_id(id))
    }
    pub fn remove(&self, id: &str) -> Result<()> {
        if let Some(error) = self.error() {
            bail!("{error}");
        }
        let mut rows = self.inner.profiles.lock().unwrap();
        let next = rows
            .iter()
            .filter(|p| p.id != id)
            .cloned()
            .collect::<Vec<_>>();
        let key = Self::secret_id(id);
        let old = self.secrets(id)?;
        crate::delete_secrets(self.inner.store.as_ref(), &key)?;
        if let Err(error) =
            kiln_common::safe_file::write(&self.inner.path, &serde_json::to_vec_pretty(&next)?)
        {
            crate::save_secrets(self.inner.store.as_ref(), &key, &old)?;
            return Err(error.into());
        }
        *rows = next;
        Ok(())
    }
    pub fn save(&self, profile: AuthenticationProfile, secrets: Secrets) -> Result<()> {
        if let Some(error) = self.error() {
            bail!("{error}");
        }
        crate::safe_text(&profile.id)?;
        crate::safe_text(&profile.name)?;
        if profile.id.is_empty() || profile.name.trim().is_empty() {
            bail!(kiln_common::i18n::tr("인증 프로필 이름을 입력하세요."));
        }
        crate::ConnectionProfile {
            id: profile.id.clone(),
            name: profile.name.clone(),
            endpoint: profile
                .identity
                .endpoint("validation-bucket".into(), String::new()),
        }
        .validate()?;
        if profile.identity.manual()
            && (secrets.access_key.as_deref().is_none_or(str::is_empty)
                || secrets.secret_key.as_deref().is_none_or(str::is_empty))
        {
            bail!(
                "{}",
                kiln_common::i18n::tr("Access Key ID 및 Secret Access Key를 함께 입력하세요")
            );
        }
        let mut rows = self.inner.profiles.lock().unwrap();
        if rows
            .iter()
            .any(|p| p.id != profile.id && p.name.trim() == profile.name.trim())
        {
            bail!(
                "{}",
                kiln_common::i18n::tr("같은 이름의 인증 프로필이 있습니다.")
            );
        }
        let mut next = rows.clone();
        if let Some(old) = next.iter_mut().find(|p| p.id == profile.id) {
            *old = profile.clone();
        } else {
            next.push(profile.clone());
        }
        let key = Self::secret_id(&profile.id);
        let old = self.secrets(&profile.id)?;
        let secrets = if profile.identity.manual() {
            secrets
        } else {
            Secrets::default()
        };
        crate::save_secrets(self.inner.store.as_ref(), &key, &secrets)?;
        let write = (|| -> Result<()> {
            std::fs::create_dir_all(
                self.inner
                    .path
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("Invalid profile path"))?,
            )?;
            kiln_common::safe_file::write(&self.inner.path, &serde_json::to_vec_pretty(&next)?)?;
            Ok(())
        })();
        if let Err(error) = write {
            crate::save_secrets(self.inner.store.as_ref(), &key, &old)?;
            return Err(error);
        }
        *rows = next;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reusable_identity_has_no_bucket_or_plaintext_secret_and_survives_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let store = Arc::new(kiln_accounts::MemoryStore::new());
        let profiles = AuthenticationProfiles::with_store(path.clone(), store.clone());
        let identity = StorageIdentity::S3 {
            region: "us-east-1".into(),
            endpoint: None,
            path_style: false,
            aws_profile: None,
            aws_auth: Some(S3Authentication::Manual),
        };
        profiles
            .save(
                AuthenticationProfile {
                    id: "one".into(),
                    name: "Production".into(),
                    identity: identity.clone(),
                    project: String::new(),
                    compartment_id: String::new(),
                },
                Secrets {
                    access_key: Some("fixture-access".into()),
                    secret_key: Some("fixture-secret".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let json = std::fs::read_to_string(&path).unwrap();
        assert!(!json.contains("fixture-access"));
        assert!(!json.contains("fixture-secret"));
        assert!(!json.contains("bucket"));
        let reloaded = AuthenticationProfiles::with_store(path, store);
        assert_eq!(reloaded.profiles().len(), 1);
        assert_eq!(
            reloaded.secrets("one").unwrap().secret_key.as_deref(),
            Some("fixture-secret")
        );
        for bucket in ["frontend", "backend"] {
            assert!(
                matches!(identity.endpoint(bucket.into(),String::new()),RemoteEndpoint::S3{bucket:b,..} if b==bucket)
            );
        }
    }
}
