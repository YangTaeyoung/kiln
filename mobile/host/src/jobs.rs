//! Owned bounded native API helpers. This is preparation/transport isolation, never another inference worker.
use crate::{
    api::{Error, Result},
    metadata::PathIdentity,
};
use kiln::app::agent_gui::{LaunchOptions, prepare_with_options};
use kiln_accounts::{AccountManager, Env, Tool};
use kiln_daemon::client::Client as Daemon;
use kiln_proto::{ClientMsg, ServerMsg, SpawnSpec};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Serialize, Deserialize)]
pub enum Job {
    Prepare {
        cwd: std::path::PathBuf,
        provider: Tool,
        options: LaunchOptions,
    },
    Daemon {
        message: ClientMsg,
        guards: Vec<PathIdentity>,
    },
}
#[derive(Serialize, Deserialize)]
pub enum Output {
    Prepared(SpawnSpec),
    Native(ServerMsg),
    Queued,
}
#[derive(Clone, Default)]
pub struct Runner {
    pub stopping: Arc<AtomicBool>,
}
impl Runner {
    pub fn run(&self, job: Job) -> Result<Output> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(Error::unavailable());
        }
        let preparation = matches!(&job, Job::Prepare { .. });
        let create = matches!(
            &job,
            Job::Daemon {
                message: ClientMsg::Create { .. },
                ..
            }
        );
        let mutation = matches!(
            &job,
            Job::Daemon {
                message: ClientMsg::Create { .. } | ClientMsg::Input { .. },
                ..
            }
        );
        let deadline = Instant::now() + Duration::from_secs(if preparation { 30 } else { 8 });
        let bytes = serde_json::to_vec(&job).map_err(|_| Error::bad("invalid_payload"))?;
        if bytes.len() > 256 * 1024 {
            return Err(Error::bad("invalid_payload"));
        }
        let mut child = Command::new(std::env::current_exe().map_err(|_| Error::unavailable())?)
            .arg("__native-helper")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|_| Error::unavailable())?;
        let pid = child.id() as i32;
        let mut stdin = child.stdin.take();
        let mut stdout = child.stdout.take().unwrap();
        let nonblocking = |fd: i32| unsafe {
            let old = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, old | libc::O_NONBLOCK)
        };
        nonblocking(stdin.as_ref().unwrap().as_raw_fd());
        nonblocking(stdout.as_raw_fd());
        let mut sent = 0;
        let mut output = vec![];
        let mut eof = false;
        let mut status = None;
        let interrupted = || {
            Error::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                if mutation {
                    if create {
                        "launch_delivery_unknown"
                    } else {
                        "delivery_unknown"
                    }
                } else {
                    "native_unavailable"
                },
            )
        };
        let result = (|| -> Result<Output> {
            loop {
                if self.stopping.load(Ordering::Acquire) || Instant::now() >= deadline {
                    return Err(Error::new(
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        if mutation {
                            if create {
                                "launch_delivery_unknown"
                            } else {
                                "delivery_unknown"
                            }
                        } else if preparation {
                            "preparation_cancelled"
                        } else {
                            "native_unavailable"
                        },
                    ));
                }
                if let Some(pipe) = stdin.as_mut() {
                    match pipe.write(&bytes[sent..]) {
                        Ok(n) => sent += n,
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                            ) => {}
                        Err(_) => return Err(interrupted()),
                    }
                    if sent == bytes.len() {
                        stdin.take();
                    }
                }
                let mut chunk = [0u8; 8192];
                loop {
                    match stdout.read(&mut chunk) {
                        Ok(0) => {
                            eof = true;
                            break;
                        }
                        Ok(n) => {
                            output.extend_from_slice(&chunk[..n]);
                            if output.len() > 2 * 1024 * 1024 {
                                return Err(interrupted());
                            }
                        }
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                            ) =>
                        {
                            break;
                        }
                        Err(_) => return Err(interrupted()),
                    }
                }
                if status.is_none() {
                    status = child.try_wait().map_err(|_| interrupted())?;
                }
                if eof && status.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let response: std::result::Result<Output, String> = serde_json::from_slice(&output)
                .map_err(|_| {
                    Error::new(
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        if mutation {
                            if create {
                                "launch_delivery_unknown"
                            } else {
                                "delivery_unknown"
                            }
                        } else {
                            "native_unavailable"
                        },
                    )
                })?;
            response.map_err(|code| {
                Error::new(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    match code.as_str() {
                        "workspace_changed" => "workspace_changed",
                        "preparation_failed" => "preparation_failed",
                        "delivery_unknown" => "delivery_unknown",
                        "launch_delivery_unknown" => "launch_delivery_unknown",
                        _ => "native_unavailable",
                    },
                )
            })
        })();
        // Only this directly created helper process group is owned here. Native daemon/workers are separate.
        if status.is_none() || !eof {
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
            let until = Instant::now() + Duration::from_millis(200);
            while child.try_wait().ok().flatten().is_none() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            if child.try_wait().ok().flatten().is_none() {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
        let _ = child.wait();
        result
    }
}
/// Called only by this binary's hidden local helper entry; stdin/stdout are bounded private pipes.
pub fn helper_main() -> i32 {
    let mut bytes = vec![];
    let result = (|| -> std::result::Result<Output, String> {
        std::io::stdin()
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "native_unavailable".to_string())?;
        if bytes.len() > 256 * 1024 {
            return Err("native_unavailable".to_string());
        }
        let job: Job =
            serde_json::from_slice(&bytes).map_err(|_| "native_unavailable".to_string())?;
        match job {
            Job::Prepare {
                cwd,
                provider,
                options,
            } => {
                let accounts = AccountManager::with_env(Env::real());
                prepare_with_options(provider, cwd, &accounts, None, options)
                    .map(Output::Prepared)
                    .map_err(|_| "preparation_failed".into())
            }
            Job::Daemon { message, guards } => {
                let create = matches!(&message, ClientMsg::Create { .. });
                let mutation =
                    matches!(&message, ClientMsg::Create { .. } | ClientMsg::Input { .. });
                if !matches!(
                    &message,
                    ClientMsg::Create { .. }
                        | ClientMsg::ListSessions { .. }
                        | ClientMsg::ReadText { .. }
                        | ClientMsg::Input { .. }
                ) {
                    return Err("native_unavailable".to_string());
                }
                let daemon = Daemon::connect(&kiln_proto::socket_name(), None)
                    .map_err(|_| "native_unavailable".to_string())?;
                for guard in guards {
                    guard
                        .validate()
                        .map_err(|_| "workspace_changed".to_string())?;
                }
                match message {
                    ClientMsg::Input { .. } => {
                        // request Ping drains the same native writer after explicit Input.
                        daemon.send(message);
                        daemon
                            .request(|req| ClientMsg::Ping { req }, Duration::from_secs(30))
                            .map_err(|_| "delivery_unknown".to_string())?;
                        Ok(Output::Queued)
                    }
                    message => {
                        let response = daemon
                            .request(
                                |req| match message {
                                    ClientMsg::Create { spec, .. } => {
                                        ClientMsg::Create { req, spec }
                                    }
                                    ClientMsg::ListSessions { .. } => {
                                        ClientMsg::ListSessions { req }
                                    }
                                    ClientMsg::ReadText {
                                        session, history, ..
                                    } => ClientMsg::ReadText {
                                        req,
                                        session,
                                        history,
                                    },
                                    _ => unreachable!(),
                                },
                                Duration::from_secs(30),
                            )
                            .map_err(|_| {
                                if mutation && create {
                                    "launch_delivery_unknown".to_string()
                                } else {
                                    "native_unavailable".to_string()
                                }
                            })?;
                        Ok(Output::Native(response))
                    }
                }
            }
        }
    })();
    let encoded = serde_json::to_vec(&result)
        .unwrap_or_else(|_| b"{\"Err\":\"native_unavailable\"}".to_vec());
    if encoded.len() > 2 * 1024 * 1024 {
        return 1;
    }
    if std::io::stdout().write_all(&encoded).is_err() {
        return 1;
    }
    0
}
