//! Connection values are read from the credential store only at launch.
use crate::Tool;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MARKER: &str = "KILN_AGENT_CONNECTION";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    #[default]
    ApiKey,
    Gateway,
    Bedrock,
    Custom,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AwsAuth {
    #[default]
    Profile,
    Bearer,
    AccessKey,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Connection {
    pub preset: Preset,
    pub base_url: String,
    pub key: String,
    pub x_api_key: bool,
    pub model: String,
    pub region: String,
    pub aws_auth: AwsAuth,
    pub aws_profile: String,
    pub aws_access_id: String,
    pub aws_session_token: String,
    pub headers: BTreeMap<String, String>,
    pub extra_env: BTreeMap<String, String>,
}

#[derive(Default)]
pub struct Launch {
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub remove_env: Vec<&'static str>,
}

impl Launch {
    fn set(&mut self, name: &str, value: &str) {
        self.env.insert(name.into(), value.into());
    }
    fn config(&mut self, key: &str, value: &str) {
        self.args.extend([
            "-c".into(),
            format!("{key}={}", toml::Value::String(value.into())),
        ]);
    }
}

impl Connection {
    pub fn launch(&self, tool: Tool) -> Result<Launch> {
        ensure!(
            matches!(tool, Tool::Claude | Tool::Codex),
            "Unsupported agent connection"
        );
        let mut out = Launch::default();
        let claude = tool == Tool::Claude;
        if self.preset != Preset::Custom {
            out.remove_env.extend([
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_API_HOST",
                "ANTHROPIC_UNIX_SOCKET",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_VERTEX",
                "CLAUDE_CODE_USE_FOUNDRY",
                "CLAUDE_CODE_USE_MANTLE",
                "CLAUDE_CODE_USE_GATEWAY",
                "CLAUDE_CODE_USE_ANTHROPIC_AWS",
                "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
                "ANTHROPIC_CUSTOM_HEADERS",
                "ANTHROPIC_BEDROCK_BASE_URL",
                "OPENAI_API_KEY",
                "OPENAI_BASE_URL",
                "CODEX_API_KEY",
                "AWS_BEARER_TOKEN_BEDROCK",
                "AWS_ACCESS_KEY_ID",
                "AWS_SECRET_ACCESS_KEY",
                "AWS_SESSION_TOKEN",
                "AWS_PROFILE",
                "AWS_DEFAULT_PROFILE",
            ]);
        }
        let key = self.key.trim();
        let valid_key = || !key.is_empty() && !key.chars().any(char::is_control);
        match self.preset {
            Preset::ApiKey | Preset::Gateway => {
                ensure!(valid_key(), "Enter a valid API key");
                let gateway = self.preset == Preset::Gateway;
                let url = if gateway {
                    Some(endpoint(&self.base_url)?)
                } else {
                    None
                };
                if claude {
                    if let Some(url) = url {
                        out.set("ANTHROPIC_BASE_URL", url.as_str());
                    }
                    out.set(
                        if gateway && !self.x_api_key {
                            "ANTHROPIC_AUTH_TOKEN"
                        } else {
                            "ANTHROPIC_API_KEY"
                        },
                        key,
                    );
                } else {
                    out.config("model_provider", "kiln");
                    out.config("model_providers.kiln.name", "Kiln");
                    out.config(
                        "model_providers.kiln.base_url",
                        url.as_ref()
                            .map_or("https://api.openai.com/v1", url::Url::as_str),
                    );
                    out.config("model_providers.kiln.wire_api", "responses");
                    out.config("model_providers.kiln.env_key", "KILN_CODEX_API_KEY");
                    out.set("KILN_CODEX_API_KEY", key);
                }
            }
            Preset::Bedrock => {
                let region = self.region.trim();
                ensure!(
                    !region.is_empty()
                        && region
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                    "Enter an AWS region"
                );
                out.set("AWS_REGION", region);
                out.set("AWS_DEFAULT_REGION", region);
                if claude {
                    out.set("CLAUDE_CODE_USE_BEDROCK", "1");
                    if !self.base_url.trim().is_empty() {
                        out.set(
                            "ANTHROPIC_BEDROCK_BASE_URL",
                            endpoint(&self.base_url)?.as_str(),
                        );
                    }
                } else {
                    out.config("model_provider", "amazon-bedrock");
                    out.config("model_providers.amazon-bedrock.aws.region", region);
                }
                match self.aws_auth {
                    AwsAuth::Profile => {
                        let profile = self.aws_profile.trim();
                        ensure!(
                            !profile.is_empty() && !profile.chars().any(char::is_control),
                            "Enter an AWS profile"
                        );
                        out.set("AWS_PROFILE", profile);
                        out.set("AWS_DEFAULT_PROFILE", profile);
                        if !claude {
                            out.config("model_providers.amazon-bedrock.aws.profile", profile);
                        }
                    }
                    AwsAuth::Bearer => {
                        ensure!(valid_key(), "Enter a valid Bedrock API key");
                        out.set("AWS_BEARER_TOKEN_BEDROCK", key);
                    }
                    AwsAuth::AccessKey => {
                        ensure!(
                            valid_key() && !self.aws_access_id.trim().is_empty(),
                            "Enter an AWS access key and secret"
                        );
                        out.set("AWS_ACCESS_KEY_ID", self.aws_access_id.trim());
                        out.set("AWS_SECRET_ACCESS_KEY", key);
                        if !self.aws_session_token.trim().is_empty() {
                            out.set("AWS_SESSION_TOKEN", self.aws_session_token.trim());
                        }
                    }
                }
            }
            Preset::Custom => {}
        }

        let model = self.model.trim();
        ensure!(!model.chars().any(char::is_control), "Invalid model");
        if !model.is_empty() {
            if claude {
                out.set("ANTHROPIC_MODEL", model);
            } else {
                out.config("model", model);
            }
        }

        let mut headers = Vec::new();
        let mut codex_headers = toml::Table::new();
        for (name, value) in &self.headers {
            ensure!(
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)),
                "Invalid header name"
            );
            ensure!(!value.chars().any(char::is_control), "Invalid header value");
            ensure!(
                !["authorization", "x-api-key", "host"]
                    .iter()
                    .any(|reserved| name.eq_ignore_ascii_case(reserved)),
                "Use the authentication fields for credentials"
            );
            if claude {
                headers.push(format!("{name}: {value}"));
            } else {
                ensure!(
                    matches!(self.preset, Preset::ApiKey | Preset::Gateway),
                    "Extra Codex headers require an API key or gateway connection"
                );
                let variable = format!("KILN_CONNECTION_HEADER_{}", codex_headers.len());
                out.set(&variable, value);
                codex_headers.insert(name.clone(), toml::Value::String(variable));
            }
        }
        if !headers.is_empty() {
            out.set("ANTHROPIC_CUSTOM_HEADERS", &headers.join("\n"));
        }
        if !codex_headers.is_empty() {
            out.args.extend([
                "-c".into(),
                format!(
                    "model_providers.kiln.env_http_headers={}",
                    toml::Value::Table(codex_headers)
                ),
            ]);
        }

        for (name, value) in &self.extra_env {
            let upper = name.to_ascii_uppercase();
            ensure!(
                name.bytes().enumerate().all(|(i, b)| b == b'_'
                    || b.is_ascii_alphabetic()
                    || i > 0 && b.is_ascii_digit())
                    && !name.is_empty()
                    && !upper.starts_with("KILN_")
                    && ![
                        "HOME",
                        "USERPROFILE",
                        "PATH",
                        "CODEX_HOME",
                        "CLAUDE_CONFIG_DIR"
                    ]
                    .contains(&upper.as_str()),
                "Invalid or reserved environment variable name"
            );
            ensure!(
                !out.env.keys().any(|key| key.eq_ignore_ascii_case(name))
                    && !out
                        .remove_env
                        .iter()
                        .any(|key| key.eq_ignore_ascii_case(name)),
                "An extra variable conflicts with the connection preset"
            );
            out.set(name, value);
        }
        ensure!(
            out.env.values().all(|value| !value.contains('\0')),
            "Invalid environment value"
        );
        Ok(out)
    }

    pub fn destination(&self) -> Option<String> {
        match self.preset {
            Preset::Gateway => endpoint(&self.base_url).ok()?.host_str().map(str::to_owned),
            Preset::Bedrock => Some(self.region.trim().to_owned()),
            _ => None,
        }
    }
}

fn endpoint(value: &str) -> Result<url::Url> {
    let url = url::Url::parse(value.trim())
        .ok()
        .context("Enter an absolute Base URL")?;
    let loopback = matches!(url.host(), Some(url::Host::Domain("localhost")))
        || matches!(url.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        || matches!(url.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback());
    ensure!(
        url.host().is_some() && (url.scheme() == "https" || (url.scheme() == "http" && loopback)),
        "Use HTTPS, or HTTP on localhost"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Base URL cannot contain credentials, a query, or a fragment"
    );
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gateway_keys_and_headers_never_enter_arguments() {
        let connection = Connection {
            preset: Preset::Gateway,
            base_url: "https://gateway.example/v1".into(),
            key: "fake-key".into(),
            headers: BTreeMap::from([("X-Team".into(), "fake-secret-header".into())]),
            ..Default::default()
        };
        let codex = connection.launch(Tool::Codex).unwrap();
        assert_eq!(codex.env["KILN_CODEX_API_KEY"], "fake-key");
        assert_eq!(codex.env["KILN_CONNECTION_HEADER_0"], "fake-secret-header");
        assert!(codex.args.iter().all(|arg| !arg.contains("fake-")));
        let claude = connection.launch(Tool::Claude).unwrap();
        assert_eq!(claude.env["ANTHROPIC_AUTH_TOKEN"], "fake-key");
        assert!(claude.remove_env.contains(&"ANTHROPIC_API_KEY"));
    }

    #[test]
    fn gateway_rejects_credentials_in_url_and_reserved_environment_overrides() {
        let mut connection = Connection {
            preset: Preset::Gateway,
            base_url: "https://gateway.example/v1?key=secret".into(),
            key: "fixture".into(),
            ..Default::default()
        };
        assert!(connection.launch(Tool::Codex).is_err());
        connection.base_url = "http://127.0.0.1:12345/v1".into();
        assert!(connection.launch(Tool::Codex).is_ok());
        connection
            .extra_env
            .insert("codex_home".into(), "elsewhere".into());
        assert!(connection.launch(Tool::Codex).is_err());
    }
}
