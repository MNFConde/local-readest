//! --headless: no TUI, just a one-line status every few seconds on stdout.
//! This is the mode for automation, service managers (NSSM/systemd capture
//! stdout into journal logs) and smoke tests.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

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
    loop {
        println!(
            "[up {}] {}",
            human_duration(dash.started.elapsed()),
            snapshot::summarize(&dash)
        );
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            _ = kill.changed() => return,
        }
    }
}
