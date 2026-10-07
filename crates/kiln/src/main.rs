use clap::{Parser, Subcommand, ValueEnum};
use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg, SpawnSpec};
use std::time::Duration;

use kiln::app;

const CLI_VERSION: &str = if cfg!(feature = "updater-test") {
    concat!(env!("CARGO_PKG_VERSION"), "-updater-test")
} else { env!("CARGO_PKG_VERSION") };

#[derive(Parser)]
#[command(name = "kiln", version = CLI_VERSION, about = "Kiln — terminal-first IDE with persistent sessions")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// 워크스페이스로 열 폴더.
    path: Option<std::path::PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
enum ActivityState { Running, Waiting, Done, Failed, Unknown }
impl ActivityState {
    fn as_str(self) -> &'static str { match self { Self::Running=>"running",Self::Waiting=>"waiting",Self::Done=>"done",Self::Failed=>"failed",Self::Unknown=>"unknown" } }
}

#[derive(Subcommand)]
enum Cmd {
    #[cfg(feature = "updater-test")]
    #[command(hide = true)]
    UpdaterTestRoot,
    /// Internal menu-bar companion; never owns terminal sessions.
    #[cfg(target_os = "macos")]
    #[command(hide = true)]
    StatusBar,
    /// 세션 데몬을 실행한다.
    Daemon {
        #[arg(long)]
        foreground: bool,
        #[arg(long)]
        restore: Option<String>,
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        wait_pid: Option<u32>,
    },
    /// (내부용) 세션 하나의 PTY 를 소유하는 호스트 프로세스.
    #[command(hide = true)]
    PtyHost {
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        session: u64,
        #[arg(long)]
        spec: String,
    },
    /// 데몬의 세션 목록.
    Ls {
        #[arg(long)]
        json: bool,
    },
    /// 새 세션을 만들고 id 를 출력한다.
    New {
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// 세션에 텍스트를 입력한다.
    Send {
        session: u64,
        text: String,
        /// 끝에 Enter 를 붙인다.
        #[arg(long, short = 'e')]
        enter: bool,
    },
    /// 세션 화면(+스크롤백)을 출력한다.
    Read {
        session: u64,
        #[arg(long, default_value_t = 0)]
        history: u32,
    },
    /// 세션을 종료한다.
    Kill { session: u64 },
    /// 현재 터미널에 알림 이스케이프를 쓴다(Kiln 사이드바 알림).
    Notify { title: String, body: Option<String> },
    /// 현재 터미널에 명시적인 에이전트 상태를 전송한다.
    Activity { #[arg(value_enum)] state: ActivityState },
    /// 실행 중인 데몬을 이 실행 파일로 교체한다(세션 유지).
    UpgradeDaemon,
    /// 데몬과 모든 세션을 종료한다.
    ShutdownDaemon,
    /// 데몬 상태.
    Status,
}

fn exe() -> std::path::PathBuf {
    std::env::current_exe().expect("current_exe")
}

fn connect() -> anyhow::Result<Client> {
    Client::connect_or_spawn(&kiln_proto::socket_name(), &exe(), None)
}

fn main() -> anyhow::Result<()> {
    #[cfg(feature = "updater-test")]
    kiln::isolate_updater_fixture()?;
    let cli = Cli::parse();
    #[cfg(feature = "updater-test")]
    match &cli.cmd {
        Some(Cmd::Daemon { socket: Some(socket), .. }) => anyhow::ensure!(
            socket == &kiln_proto::socket_name(), "fixture daemon socket must stay isolated"),
        Some(Cmd::PtyHost { endpoint, session, .. }) => anyhow::ensure!(
            endpoint == &kiln_daemon::ptyhost::endpoint_for(&kiln_proto::socket_name(), *session),
            "fixture PTY endpoint must stay isolated"),
        _ => {}
    }
    match cli.cmd {
        #[cfg(feature = "updater-test")]
        Some(Cmd::UpdaterTestRoot) => {
            println!("{}", std::path::Path::new(option_env!("KILN_UPDATER_TEST_ROOT").unwrap()).canonicalize()?.display());
            Ok(())
        }
        #[cfg(target_os = "macos")]
        Some(Cmd::StatusBar) => kiln::status_bar::run(),
        None => {
            #[cfg(target_os = "macos")]
            kiln::status_bar::ensure_running();
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
            app::run(cli.path)
        }
        Some(Cmd::Daemon { foreground: _, restore, socket, wait_pid }) => {
            let socket = socket.unwrap_or_else(kiln_proto::socket_name);
            #[cfg(target_os = "macos")]
            if socket == kiln_proto::socket_name() { kiln::status_bar::ensure_running(); }
            // 분리 실행(WMI 등)에서는 표준 에러가 없으므로 로그 파일에 직접 쓴다.
            let log_path = kiln_daemon::client::daemon_log_path(&socket);
            let mut logger = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
            if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
                logger.target(env_logger::Target::Pipe(Box::new(f)));
            }
            logger.init();
            log::info!("daemon start pid {} args {:?}", std::process::id(), std::env::args().collect::<Vec<_>>());
            kiln_daemon::server::run(kiln_daemon::server::RunOptions { socket, restore, wait_pid })
        }
        Some(Cmd::PtyHost { endpoint, session, spec }) => {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD.decode(spec)?;
            let spec: SpawnSpec = postcard::from_bytes(&bytes)?;
            kiln_daemon::ptyhost::run_host(&endpoint, spec, session)
        }
        Some(Cmd::Ls { json }) => {
            let c = connect()?;
            let ServerMsg::Sessions { sessions, .. } = c.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(5))? else {
                anyhow::bail!("unexpected reply")
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&sessions.iter().map(|s| serde_json::json!({
                    "id": s.id, "pid": s.pid, "name": s.name, "title": s.title, "cwd": s.cwd,
                    "process": s.fg_process, "workspace": s.workspace, "exited": s.exited, "attention": s.attention,
                })).collect::<Vec<_>>())?);
            } else {
                println!("{:<5} {:<8} {:<12} {:<10} CWD", "ID", "PID", "PROCESS", "STATE");
                for s in sessions {
                    let state = match s.exited { Some(c) => format!("exit {c}"), None if s.attention => "attention".into(), None => "running".into() };
                    println!("{:<5} {:<8} {:<12} {:<10} {}", s.id, s.pid, s.fg_process.unwrap_or_default(), state, s.cwd.unwrap_or_default());
                }
            }
            Ok(())
        }
        Some(Cmd::New { cwd, name, command }) => {
            let c = connect()?;
            let (program, args) = match command.split_first() {
                Some((p, a)) => (Some(p.clone()), a.to_vec()),
                None => (None, vec![]),
            };
            let cwd = cwd.or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()));
            let spec = SpawnSpec { cwd, program, args, cols: 120, rows: 32, name, ..Default::default() };
            let ServerMsg::Created { session, .. } = c.request(|req| ClientMsg::Create { req, spec }, Duration::from_secs(5))? else {
                anyhow::bail!("unexpected reply")
            };
            println!("{session}");
            Ok(())
        }
        Some(Cmd::Send { session, text, enter }) => {
            let c = connect()?;
            let mut data = text.into_bytes();
            if enter {
                data.push(b'\r');
            }
            c.send(ClientMsg::Input { session, data });
            c.request(|req| ClientMsg::Ping { req }, Duration::from_secs(5))?;
            Ok(())
        }
        Some(Cmd::Read { session, history }) => {
            let c = connect()?;
            let ServerMsg::Text { text, .. } = c.request(|req| ClientMsg::ReadText { req, session, history }, Duration::from_secs(5))? else {
                anyhow::bail!("unexpected reply")
            };
            print!("{text}");
            Ok(())
        }
        Some(Cmd::Kill { session }) => {
            let c = connect()?;
            c.send(ClientMsg::Kill { session });
            c.request(|req| ClientMsg::Ping { req }, Duration::from_secs(5))?;
            Ok(())
        }
        Some(Cmd::Notify { title, body }) => {
            use std::io::Write;
            let seq = format!("\x1b]777;notify;{};{}\x07", title.replace(';', ","), body.unwrap_or_default().replace(';', ","));
            #[cfg(unix)]
            let mut out: Box<dyn Write> = match std::fs::OpenOptions::new().write(true).open("/dev/tty") {
                Ok(f) => Box::new(f),
                Err(_) => Box::new(std::io::stdout()),
            };
            #[cfg(windows)]
            let mut out: Box<dyn Write> = Box::new(std::io::stdout());
            out.write_all(seq.as_bytes())?;
            out.flush()?;
            Ok(())
        }
        Some(Cmd::Activity { state }) => {
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            write!(out, "\x1b]777;kiln-agent;{}\x07", state.as_str())?;
            out.flush()?;
            Ok(())
        }
        Some(Cmd::UpgradeDaemon) => {
            let socket = kiln_proto::socket_name();
            let old = Client::connect(&socket, None).map(|c| c.server_pid).ok();
            kiln_daemon::client::request_upgrade(&socket, &exe())?;
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(n) = Client::connect(&socket, None) {
                    if n.server_build == kiln_daemon::build_id() {
                        println!("daemon upgraded (pid {} → {}, build {})", old.map(|p| p.to_string()).unwrap_or_else(|| "?".into()), n.server_pid, n.server_build);
                        return Ok(());
                    }
                }
                if std::time::Instant::now() > deadline {
                    anyhow::bail!("upgrade did not complete; see {}", kiln_daemon::client::daemon_log_path(&socket).display());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        Some(Cmd::ShutdownDaemon) => {
            if let Ok(c) = Client::connect(&kiln_proto::socket_name(), None) {
                c.send(ClientMsg::Shutdown);
                std::thread::sleep(Duration::from_millis(200));
            }
            Ok(())
        }
        Some(Cmd::Status) => {
            match Client::connect(&kiln_proto::socket_name(), None) {
                Ok(c) => {
                    let same = c.server_build == kiln_daemon::build_id();
                    println!("daemon pid {} build {} ({})", c.server_pid, c.server_build, if same { "current" } else { "outdated" });
                }
                Err(e) => println!("daemon not running ({e})"),
            }
            Ok(())
        }
    }
}
