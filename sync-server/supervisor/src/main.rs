//! readest-supervisor — the single entry point of the self-host server
//! bundle (ROADMAP 待办⑦). It spawns and supervises the two services:
//!
//!   sync-server(.exe)   sync/auth/OPDS/admin API   (sibling of this exe)
//!   node apps/readest-app/server.js                (web reader, ./web/)
//!
//! and owns their lifetime: exponential-backoff restarts with a circuit
//! breaker, a TUI dashboard (`--headless` for plain-log mode), and — the
//! invariant everything else hangs off — both children die when this process
//! dies, hard kill included (Windows Job Object / Unix PDEATHSIG).
//!
//! It also feeds the admin console's server monitor by POSTing a metrics
//! snapshot to the sync server every few seconds (disabled without
//! ADMIN_TOKEN; failures never affect supervision).
//!
//! Layout convention (the bundle this ships in):
//!   <dir>/ readest-supervisor(.exe), sync-server(.exe), web/, .env, logs/

mod headless;
mod platform;
mod policy;
mod probe;
mod service;
mod snapshot;
mod state;
mod tui;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use state::Dash;
use tokio::sync::watch;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Locate the Node binary: explicit READEST_NODE, a copy beside the exe
/// (bundle-embedded Node), then whatever PATH provides.
fn resolve_node(exe_dir: &Path) -> PathBuf {
    if let Ok(explicit) = std::env::var("READEST_NODE") {
        return PathBuf::from(explicit);
    }
    let beside = exe_dir.join(exe_name("node"));
    if beside.exists() {
        return beside;
    }
    PathBuf::from("node")
}

fn print_help() {
    println!(
        "readest-supervisor v{VERSION} — Readest self-host server process manager

Usage: readest-supervisor [--headless]

  (no flags)   dashboard TUI; press q to quit (stops both services)
  --headless   plain status lines on stdout (services, CI, NSSM/systemd)

Reads the same .env as the sync server (beside this exe). Children:
  sync-server(.exe)  in this directory
  node web/apps/readest-app/server.js  (Node >= 20 on PATH, beside the exe,
                                        or wherever READEST_NODE points)"
    );
}

#[tokio::main]
async fn main() -> ExitCode {
    let headless = std::env::args().any(|a| a == "--headless");
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        print_help();
        return ExitCode::SUCCESS;
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));

    // Same .env as the sync server; existing environment wins (dotenvy does
    // not override), which matches the bundle start.bat's `if not defined`.
    let _ = dotenvy::from_path(exe_dir.join(".env"));

    let listen_addr = env_or("LISTEN_ADDR", "0.0.0.0:8080");
    let sync_probe_addr = probe::probe_addr(&listen_addr, 8080);
    let web_port: u16 = env_or("PORT", "3000").parse().unwrap_or(3000);
    let admin_token = std::env::var("ADMIN_TOKEN").ok().filter(|t| !t.is_empty());
    let node = resolve_node(&exe_dir);
    let log_dir = exe_dir.join("logs");
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("supervisor: cannot create {}: {e}", log_dir.display());
    }

    platform::init();

    let dash = Arc::new(Dash::new());
    let host = snapshot::collect_host_static();
    let (kill_tx, kill_rx) = watch::channel(false);

    let sync_spec = service::ServiceSpec {
        slot: state::Slot::Sync,
        program: exe_dir.join(exe_name("sync-server")),
        args: vec![],
        working_dir: exe_dir.clone(),
        env_defaults: vec![],
        log_dir: log_dir.clone(),
        display: "sync-server".into(),
    };
    let web_spec = service::ServiceSpec {
        slot: state::Slot::Web,
        program: node,
        args: vec!["apps/readest-app/server.js".into()],
        working_dir: exe_dir.join("web"),
        // Mirror the bundle's start.bat defaults; operator env wins.
        env_defaults: vec![
            ("PORT", "3000"),
            ("HOSTNAME", "0.0.0.0"),
            ("NODE_ENV", "production"),
            ("SELF_HOSTED", "true"),
        ],
        log_dir: log_dir.clone(),
        display: "node apps/readest-app/server.js".into(),
    };

    let mut tasks = Vec::new();
    tasks.push(tokio::spawn(service::run_service(
        sync_spec,
        dash.clone(),
        kill_rx.clone(),
    )));
    tasks.push(tokio::spawn(service::run_service(
        web_spec,
        dash.clone(),
        kill_rx.clone(),
    )));
    tasks.push(tokio::spawn(snapshot::run_probes(
        dash.clone(),
        sync_probe_addr.clone(),
        web_port,
        kill_rx.clone(),
    )));
    tasks.push(tokio::spawn(snapshot::run_metrics(
        dash.clone(),
        kill_rx.clone(),
    )));
    tasks.push(tokio::spawn(snapshot::run_pusher(
        dash.clone(),
        host.clone(),
        sync_probe_addr.clone(),
        admin_token.clone(),
        VERSION,
        headless,
        kill_rx.clone(),
    )));

    // Ctrl+C / SIGTERM: graceful stop even in TUI mode (raw mode normally
    // swallows Ctrl+C into a key event, which the TUI also handles).
    {
        let kill_tx_c = kill_tx.clone();
        tokio::spawn(async move {
            let ctrl_c = tokio::signal::ctrl_c();
            #[cfg(unix)]
            {
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
                tokio::select! {
                    _ = ctrl_c => {}
                    _ = term.recv() => {}
                }
            }
            #[cfg(not(unix))]
            {
                let _ = ctrl_c.await;
            }
            let _ = kill_tx_c.send(true);
        });
    }

    let ui: anyhow::Result<()> = if headless {
        headless::run_headless(
            dash.clone(),
            sync_probe_addr.clone(),
            web_port,
            admin_token.is_some(),
            kill_rx.clone(),
        )
        .await;
        Ok(())
    } else {
        let ctx = tui::UiContext {
            dash: dash.clone(),
            host,
            version: VERSION,
            sync_addr: sync_probe_addr.clone(),
            web_port,
        };
        tui::run_tui(ctx, kill_tx.clone()).await
    };
    if let Err(e) = ui {
        eprintln!("supervisor: ui error: {e}");
    }

    // Shutdown: tell every task, make sure children actually die (a runner
    // could be mid-backoff with no child to kill), then reap — with ONE
    // global budget, not five seconds per task (observed on Linux: probes
    // into dead services can each eat their full timeout).
    let _ = kill_tx.send(true);
    platform::terminate_children();
    let reaper = tokio::spawn(async move {
        for t in tasks {
            let _ = t.await;
        }
    });
    let _ = tokio::time::timeout(std::time::Duration::from_secs(6), reaper).await;
    ExitCode::SUCCESS
}
