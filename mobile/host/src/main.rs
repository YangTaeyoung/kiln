use clap::Parser;
use std::{net::SocketAddr, path::PathBuf, time::Duration};
#[derive(Parser)]
#[command(
    name = "kiln-mobile-host",
    version,
    about = "Explicit optional Kiln loopback mobile companion"
)]
struct Args {
    #[arg(long = "workspace", required = true)]
    workspaces: Vec<PathBuf>,
    #[arg(long)]
    kiln_cli: PathBuf,
    #[arg(long, default_value = "127.0.0.1:0")]
    listen: SocketAddr,
    #[arg(long)]
    pairing_file: PathBuf,
    #[arg(long)]
    dev_origin: Option<String>,
}
fn main() {
    if std::env::args().nth(1).as_deref() == Some("__native-helper") {
        std::process::exit(kiln_mobile_host::helper_main());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("host runtime");
    let result = runtime.block_on(run());
    runtime.shutdown_timeout(Duration::from_secs(2));
    if result.is_err() {
        std::process::exit(1)
    }
}
async fn run() -> anyhow::Result<()> {
    let a = Args::parse();
    let o = kiln_mobile_host::Options {
        workspaces: a.workspaces,
        kiln_cli: a.kiln_cli,
        listen: a.listen,
        pairing_file: a.pairing_file,
        dev_origin: a.dev_origin,
        token_ttl: Duration::from_secs(8 * 3600),
    };
    if kiln_mobile_host::serve(o).await.is_err() {
        eprintln!(
            "Kiln mobile host could not start or continue. Check explicit CLI, workspace, loopback address and private pairing directory."
        );
        anyhow::bail!("host stopped without a complete drain")
    }
    Ok(())
}
