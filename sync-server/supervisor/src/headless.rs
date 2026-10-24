//! --headless: no TUI, just a one-line status every few seconds on stdout.
//! This is the mode for automation, service managers (NSSM/systemd capture
//! stdout into journal logs) and smoke tests — and, since ⑩, also what an
//! autostarted instance runs (the tray and an attached TUI are its local
//! control surfaces). A second --headless invocation attaches to the running
//! primary through the observation socket and prints the same lines.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::observe::AttachClient;
use crate::snapshot;
use crate::state::{human_duration, Dash};

pub async fn run_headless(
    dash: Arc<Dash>,
    sync_addr: String,
    web_port: u16,
    admin_token_set: bool,
    mut kill: watch::Receiver<bool>,
) {
    println!(
        "readest-supervisor v{} (headless) — sync http://{sync_addr} · web http://127.0.0.1:{web_port}",
        env!("CARGO_PKG_VERSION")
    );
    println!(
        "  metrics push: {}",
        if admin_token_set {
            "enabled (ADMIN_TOKEN found)"
        } else {
            "disabled — set ADMIN_TOKEN in .env to feed the admin console's server monitor"
        }
    );
    println!("  stopping this process stops both services; logs: ./logs/");
    printer(dash, &mut kill).await;
}

/// Second invocation that found a live primary: observe-only. Ctrl+C leaves
/// the primary untouched.
pub async fn run_headless_attach(client: AttachClient, sync_addr: String, web_port: u16) {
    println!(
        "readest-supervisor v{} (attached) — observing the running supervisor; sync http://{sync_addr} · web http://127.0.0.1:{web_port}",
        env!("CARGO_PKG_VERSION")
    );
    println!("  Ctrl+C detaches only (services keep running)");
    let (kill_tx, mut kill) = watch::channel(false);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = kill_tx.send(true);
    });
    printer(client.dash.clone(), &mut kill).await;
}

async fn printer(dash: Arc<Dash>, kill: &mut watch::Receiver<bool>) {
    loop {
        println!(
            "[up {}] {}",
            human_duration(dash.started_elapsed()),
            snapshot::summarize(&dash)
        );
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            _ = kill.changed() => return,
        }
    }
}
