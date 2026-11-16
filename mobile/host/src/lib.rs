//! Optional explicitly started loopback companion. Native inference/session ownership remains Kiln's.
mod api;
mod auth;
mod jobs;
mod metadata;
#[doc(hidden)]
pub use jobs::helper_main;
mod runtime;
mod server;
use std::{net::SocketAddr, path::PathBuf, time::Duration};
#[derive(Clone)]
pub struct Options {
    pub workspaces: Vec<PathBuf>,
    pub kiln_cli: PathBuf,
    pub listen: SocketAddr,
    pub pairing_file: PathBuf,
    pub dev_origin: Option<String>,
    pub token_ttl: Duration,
}
pub use server::{serve, serve_until};
