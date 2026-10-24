//! Observation socket + attach resolution (⑩ 内核/皮分离).
//!
//! The primary instance binds an ephemeral loopback TCP port and writes it to
//! `<exe_dir>/supervisor.port` — that file doubles as the single-instance
//! lock: a second invocation connects to it and becomes an attach client
//! (TUI or headless printer fed by pushed frames) instead of racing a second
//! set of children. A stale file (primary hard-killed) simply fails to
//! connect and is reclaimed by the next primary. Attach clients receive a
//! full status frame every second and may submit `ClientMsg` command lines;
//! an attach disconnect never touches the services (that is the whole point:
//! `q` in an attached TUI is not `q` in the in-process one).
//!
//! Loopback-only by design: anyone who can reach it can already kill the
//! processes locally. No auth on purpose (same trust domain as Ctrl+C).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, watch};

use crate::cmd::CommandHub;
use crate::snapshot::HostStatic;
use crate::state::Dash;
use crate::wire::{self, ClientMsg, StatusFrame};

pub const OBSERVE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Serialize, Deserialize)]
struct PortInfo {
    pid: u32,
    port: u16,
}

pub fn port_file(exe_dir: &Path) -> PathBuf {
    exe_dir.join("supervisor.port")
}

/// If a primary instance is alive here, connect as an attach client.
/// None = no port file or nothing listening (stale file) → caller becomes
/// the primary.
pub async fn try_attach(exe_dir: &Path) -> Option<TcpStream> {
    let content = std::fs::read_to_string(port_file(exe_dir)).ok()?;
    let info: PortInfo = serde_json::from_str(&content).ok()?;
    let connect = TcpStream::connect(("127.0.0.1", info.port));
    let stream = tokio::time::timeout(Duration::from_secs(2), connect)
        .await
        .ok()?
        .ok()?;
    Some(stream)
}

/// Serve the observation socket until the global kill: broadcast status
/// frames to every attached client, accept command lines from them, then
/// remove the port file on the way out.
#[allow(clippy::too_many_arguments)]
pub async fn run_observer(
    dash: Arc<Dash>,
    host: HostStatic,
    version: &'static str,
    headless: bool,
    hub: CommandHub,
    kill_tx: watch::Sender<bool>,
    exe_dir: PathBuf,
    mut kill: watch::Receiver<bool>,
) {
    let listener = match TcpListener::bind(("127.0.0.1", 0)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("supervisor: cannot bind the observation socket ({e}) — attach disabled");
            return;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let info = PortInfo {
        pid: std::process::id(),
        port,
    };
    let sock = port_file(&exe_dir);
    if let Err(e) = std::fs::write(&sock, serde_json::to_string(&info).unwrap_or_default()) {
        eprintln!(
            "supervisor: cannot write {} ({e}) — attach disabled",
            sock.display()
        );
    }

    let (frame_tx, _) = broadcast::channel::<String>(8);
    let mut tick = tokio::time::interval(OBSERVE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = kill.changed() => break,
            _ = tick.tick() => {
                let attached = frame_tx.receiver_count() as u32;
                let frame = wire::build_frame(&dash, &host, version, headless, attached);
                if let Ok(line) = serde_json::to_string(&frame) {
                    // No listeners: send fails, perfectly fine.
                    let _ = frame_tx.send(line);
                }
            }
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let frames = frame_tx.subscribe();
                let hub = hub.clone();
                let kill_tx = kill_tx.clone();
                let kill_c = kill.clone();
                tokio::spawn(handle_conn(stream, frames, hub, kill_tx, kill_c));
            }
        }
    }
    let _ = std::fs::remove_file(&sock);
}

async fn handle_conn(
    stream: TcpStream,
    mut frames: broadcast::Receiver<String>,
    hub: CommandHub,
    kill_tx: watch::Sender<bool>,
    mut kill: watch::Receiver<bool>,
) {
    stream.set_nodelay(true).ok();
    let (rd, mut wr) = stream.into_split();

    // Reader: command lines until the client goes away.
    let (gone_tx, mut gone_rx) = mpsc::channel::<()>(1);
    tokio::spawn(async move {
        let mut lines = BufReader::new(rd).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(msg) = serde_json::from_str::<ClientMsg>(&line) else {
                continue;
            };
            if msg.cmd == "shutdown" {
                // `supervisor uninstall` (default mode) uses this to stop the
                // running instance — same semantics as stop.bat.
                let _ = kill_tx.send(true);
            } else if let Some(slot) = msg.slot.as_deref() {
                if let Err(e) = hub.send_from_labels(slot, &msg.cmd) {
                    eprintln!("supervisor: attach command rejected: {e}");
                }
            }
        }
        let _ = gone_tx.send(()).await;
    });

    // Writer: forward broadcast frames; stop on client-gone or shutdown.
    loop {
        tokio::select! {
            frame = frames.recv() => match frame {
                Ok(line) => {
                    if wr.write_all(line.as_bytes()).await.is_err()
                        || wr.write_all(b"\n").await.is_err()
                    {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            },
            _ = gone_rx.recv() => return,
            _ = kill.changed() => return,
        }
    }
}

/// A connected attach client: a local Dash fed by pushed frames, plus a
/// queue that turns UI commands into wire lines. `closed` flips when the
/// primary goes away (then the UI should exit, not act).
pub struct AttachClient {
    pub dash: Arc<Dash>,
    pub host: HostStatic,
    pub cmd_tx: mpsc::UnboundedSender<String>,
    pub closed: Arc<AtomicBool>,
}

impl AttachClient {
    pub fn send(&self, slot: Option<&str>, cmd: &str) {
        let msg = ClientMsg {
            slot: slot.map(str::to_string),
            cmd: cmd.to_string(),
        };
        if let Ok(line) = serde_json::to_string(&msg) {
            let _ = self.cmd_tx.send(line);
        }
    }
}

/// Handshake an attach connection: wait for the first frame so the UI starts
/// with real data, then keep the local Dash fresh in the background.
/// Returns None on handshake failure (caller may fall back to primary).
pub async fn start_attach(stream: TcpStream) -> Option<AttachClient> {
    stream.set_nodelay(true).ok();
    let (rd, mut wr) = stream.into_split();

    let mut lines = BufReader::new(rd).lines();
    let first = match tokio::time::timeout(OBSERVE_INTERVAL * 3, lines.next_line()).await {
        Ok(Ok(Some(line))) => line,
        _ => return None,
    };
    let frame: StatusFrame = serde_json::from_str(&first).ok()?;
    let dash = Arc::new(Dash::new());
    let host = frame.apply(&dash, true)?;

    let closed = Arc::new(AtomicBool::new(false));
    {
        let dash = dash.clone();
        let closed = closed.clone();
        tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(frame) = serde_json::from_str::<StatusFrame>(&line) {
                    frame.apply(&dash, false);
                }
            }
            closed.store(true, Ordering::SeqCst);
        });
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if wr.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if wr.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });

    Some(AttachClient {
        dash,
        host,
        cmd_tx: tx,
        closed,
    })
}
