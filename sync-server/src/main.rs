use std::sync::Arc;

use sync_server::config::Config;
use sync_server::{db, routes};
fn main() -> anyhow::Result<()> {
    // CLI subcommands (add-user / list-users / disable-user / passwd) arrive
    // with the account system; until then only `serve` (the default) exists.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("serve") => {
            dotenvy::dotenv().ok();
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .init();
            let cfg = Config::from_env()?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            runtime.block_on(serve(cfg))
        }
        Some(other) => {
            eprintln!("unknown subcommand: {other}\nusage: sync-server [serve]");
            std::process::exit(2);
        }
    }
}

async fn serve(cfg: Config) -> anyhow::Result<()> {
    std::fs::create_dir_all(cfg.files_root())?;
    let conn = db::open_db(&cfg)?;
    let state: db::SharedState = Arc::new(db::AppState {
        db: conn.into(),
        cfg,
    });

    let addr = state.cfg.listen_addr.clone();
    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("sync-server listening on {addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
