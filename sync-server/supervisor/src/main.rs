//! readest-supervisor — the single entry point of the self-host server
//! bundle (ROADMAP 待办⑦;⑩ 内核/皮分离). It spawns and supervises the two
//! services:
//!
//!   sync-server(.exe)   sync/auth/OPDS/admin API   (sibling of this exe)
//!   node apps/readest-app/server.js                (web reader, ./web/)
//!
//! and owns their lifetime: exponential-backoff restarts with a circuit
//! breaker that parks (an operator Start recovers it), a command queue the
//! TUI / tray / admin-page buttons submit Start/Stop/Restart to, an
//! observation socket that makes a second invocation an attach client
//! (single instance, `supervisor.port` as the lock file), optional boot
//! autostart (`install` / `uninstall`), and — the invariant everything else
//! hangs off — both children die when this process dies, hard kill included
//! (Windows Job Object / Unix PDEATHSIG).
//!
//! It also feeds the admin console's server monitor by POSTing a metrics
//! snapshot to the sync server every few seconds (disabled without
//! ADMIN_TOKEN; failures never affect supervision; the response relays the
//! admin page's service commands back — ⑩ P2).
//!
//! Layout convention (the bundle this ships in):
//!   <dir>/ readest-supervisor(.exe), sync-server(.exe), web/, .env, logs/

mod autostart;
mod cmd;
mod headless;
mod observe;
mod platform;
mod policy;
mod probe;
mod service;
mod snapshot;
mod state;
// Tray is Windows-only on purpose (⑩ P4 finding): every tray-icon release's
// Linux backend is the GTK stack (muda/glib/gdk/pango system deps), which a
// headless server build must not drag in. Linux control surfaces are
// systemctl, the attach TUI and the admin page.
#[cfg(windows)]
mod tray;
mod tui;
mod wire;

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

Usage: readest-supervisor [--headless] [--no-tray]
       readest-supervisor install
       readest-supervisor uninstall [--keep-running]

  (no flags)   dashboard TUI; ↑↓ select, s/x/r start/stop/restart, q quits
               (stops both services). If another instance is already
               running, this ATTACHES to it instead (q then only detaches).
  --headless   plain status lines on stdout (services, CI, NSSM/systemd;
               autostarted instances run this way — tray remains available)
  --no-tray    run without the system tray icon (Windows only; other
                platforms never build one)

  install      register boot autostart for THIS user (Windows: Startup
               shortcut, minimized, --headless; Linux: systemd user unit +
               enable --now) and print what was created.
  uninstall    remove the autostart registration and stop the running
               instance (full termination). --keep-running only unregisters.

Single instance: the running primary owns supervisor.port beside the exe;
later invocations attach to it rather than starting a second set of
services. Temporary stop without unregistering: {}.

Reads the same .env as the sync server (beside this exe). Children:
  sync-server(.exe)  in this directory
  node web/apps/readest-app/server.js  (Node >= 20 on PATH, beside the exe,
                                        or wherever READEST_NODE points)",
        autostart::temporary_stop_hint()
    );
}

/// `uninstall` (default = full termination): stop the running instance via
/// its observation socket, if any. Prints what it did.
async fn stop_running_instance(exe_dir: &Path) {
    match observe::try_attach(exe_dir).await {
        Some(stream) => {
            if let Some(client) = observe::start_attach(stream).await {
                client.send(None, "shutdown");
                // Give the primary a beat to tear its children down.
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                println!("  running supervisor told to stop (children go with it)");
            }
        }
        None => println!("  no running supervisor found"),
    }
}

async fn run_install(exe: &Path) -> ExitCode {
    match autostart::install(exe) {
        Ok(created) => {
            println!("autostart registered: {created}");
            if cfg!(windows) {
                println!("  takes effect at the next logon (minimized, --headless; tray = its control surface)");
            } else {
                println!("  systemd --user unit enabled and started (linger: loginctl enable-linger $USER)");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("install failed: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run_uninstall(exe: &Path, keep_running: bool) -> ExitCode {
    let exe_dir = exe.parent().unwrap_or_else(|| Path::new("."));
    let stopped = autostart::unregister(exe_dir);
    match stopped {
        Ok(true) => println!("autostart registration removed"),
        Ok(false) => println!("no autostart registration found (nothing to remove)"),
        Err(e) => {
            eprintln!("uninstall failed while removing the registration: {e}");
            return ExitCode::FAILURE;
        }
    }
    if keep_running {
        println!(
            "  --keep-running: the running instance is untouched; {}",
            autostart::temporary_stop_hint()
        );
    } else {
        println!("  stopping the running instance…");
        stop_running_instance(exe_dir).await;
        #[cfg(unix)]
        {
            // systemd user instance: stopping the unit is the authoritative
            // stop even if the socket path was skipped.
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "stop", autostart::AUTOSTART_NAME])
                .status();
        }
        println!(
            "done — nothing autostarts or runs anymore (services: verify ports 8080/3000 are free)"
        );
    }
    ExitCode::SUCCESS
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let headless = args.iter().any(|a| a == "--headless");
    #[cfg(windows)]
    let no_tray = args.iter().any(|a| a == "--no-tray");
    let wants_install = args.iter().any(|a| a == "install");
    let wants_uninstall = args.iter().any(|a| a == "uninstall");
    let keep_running = args.iter().any(|a| a == "--keep-running");
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return ExitCode::SUCCESS;
    }

    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("readest-supervisor"));
    let exe_dir = exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    if wants_install {
        return run_install(&exe).await;
    }
    if wants_uninstall {
        return run_uninstall(&exe, keep_running).await;
    }

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

    // Second invocation: attach to the running primary instead of racing a
    // second set of children (the port file is the single-instance lock).
    if let Some(stream) = observe::try_attach(&exe_dir).await {
        if let Some(client) = observe::start_attach(stream).await {
            let ui: anyhow::Result<()> = if headless {
                headless::run_headless_attach(client, sync_probe_addr.clone(), web_port).await;
                Ok(())
            } else {
                let ctx = tui::UiContext {
                    dash: client.dash.clone(),
                    host: client.host.clone(),
                    version: VERSION,
                    sync_addr: sync_probe_addr.clone(),
                    web_port,
                };
                let backend = tui::TuiBackend::Attach {
                    cmd_tx: client.cmd_tx.clone(),
                    closed: client.closed.clone(),
                };
                let (kill_tx, _kill_rx) = watch::channel(false); // unused in attach
                tui::run_tui(ctx, backend, kill_tx).await
            };
            if let Err(e) = ui {
                eprintln!("supervisor: ui error: {e}");
            }
            return ExitCode::SUCCESS;
        }
        eprintln!("supervisor: port file exists but handshake failed — becoming the primary");
    }

    platform::init();

    let dash = Arc::new(Dash::new());
    let host = snapshot::collect_host_static();
    let (kill_tx, kill_rx) = watch::channel(false);
    let (hub, taps) = cmd::CommandHub::pair();

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

    let tasks = vec![
        tokio::spawn(service::run_service(
            sync_spec,
            dash.clone(),
            taps.sync,
            kill_rx.clone(),
        )),
        tokio::spawn(service::run_service(
            web_spec,
            dash.clone(),
            taps.web,
            kill_rx.clone(),
        )),
        tokio::spawn(snapshot::run_probes(
            dash.clone(),
            sync_probe_addr.clone(),
            web_port,
            kill_rx.clone(),
        )),
        tokio::spawn(snapshot::run_metrics(dash.clone(), kill_rx.clone())),
        tokio::spawn(snapshot::run_pusher(
            dash.clone(),
            host.clone(),
            sync_probe_addr.clone(),
            admin_token.clone(),
            VERSION,
            headless,
            hub.clone(),
            kill_rx.clone(),
        )),
        tokio::spawn(observe::run_observer(
            dash.clone(),
            host.clone(),
            VERSION,
            headless,
            hub.clone(),
            kill_tx.clone(),
            exe_dir.clone(),
            kill_rx.clone(),
        )),
    ];

    // Ctrl+C / SIGTERM: graceful stop even in TUI mode (raw mode normally
    // swallows Ctrl+C into a key event, which the TUI also handles).
    {
        let kill_tx_c = kill_tx.clone();
        tokio::spawn(async move {
            let ctrl_c = tokio::signal::ctrl_c();
            #[cfg(unix)]
            {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
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

    #[cfg(windows)]
    if !no_tray {
        tray::spawn(
            dash.clone(),
            hub.clone(),
            kill_tx.clone(),
            exe.clone(),
            web_port,
        );
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
        let backend = tui::TuiBackend::Primary { hub };
        tui::run_tui(ctx, backend, kill_tx.clone()).await
    };
    if let Err(e) = ui {
        eprintln!("supervisor: ui error: {e}");
    }

    // Shutdown: tell every task, make sure children actually die (a runner
    // could be parked in Halted/circuit with no child to kill), then reap —
    // with ONE global budget, not five seconds per task (observed on Linux:
    // probes into dead services can each eat their full timeout).
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
