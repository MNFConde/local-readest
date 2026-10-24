//! Metrics sampling (sysinfo per-PID), the host-snapshot JSON the admin page
//! consumes, and the pusher task that delivers it to the sync server.
//!
//! Anti-loop rule #3 lives here: a failed push is logged and displayed and
//! NOTHING else. It never feeds a restart decision, never kills a child,
//! and the last successful snapshot simply goes stale. Push and probe are
//! separate channels with separate failure semantics on purpose — "sync
//! server down" must not lead the supervisor into any control action beyond
//! what the sync service's own runner already does.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::watch;

use crate::probe;
use crate::state::{human_bytes, Dash, Phase, Slot};

pub const SNAPSHOT_SCHEMA: u32 = 1;
pub const PUSH_INTERVAL: Duration = Duration::from_secs(5);
pub const PROBE_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Serialize, Clone)]
pub struct HostStatic {
    pub os: String,
    pub cores: u32,
    pub total_memory_bytes: u64,
}

#[derive(Serialize)]
struct ProbeSnap {
    ok: bool,
    status: u16,
    rtt_ms: u64,
}

#[derive(Serialize)]
struct ServiceSnap {
    name: &'static str,
    state: &'static str,
    /// Operator intent (⑩): "run" = supervised, "halt" = manually suppressed.
    desired: &'static str,
    pid: Option<u32>,
    uptime_secs: Option<u64>,
    restarts: u32,
    consecutive_failures: u32,
    last_exit: Option<LastExitSnap>,
    probe: Option<ProbeSnap>,
    memory_bytes: u64,
    cpu_percent: f32,
    circuit_reason: Option<String>,
}

#[derive(Serialize)]
struct LastExitSnap {
    code: Option<i32>,
    uptime_secs: u64,
}

#[derive(Serialize)]
struct SupervisorSnap {
    version: &'static str,
    pid: u32,
    uptime_secs: u64,
    headless: bool,
}

#[derive(Serialize)]
struct Snapshot {
    schema: u32,
    ts: u64,
    supervisor: SupervisorSnap,
    host: HostStatic,
    services: Vec<ServiceSnap>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Serialize the current dashboard as a host-snapshot payload (the contract
/// documented in docs/selfhosted/sync-protocol.md §6.6).
pub fn build_snapshot_json(
    dash: &Dash,
    host: &HostStatic,
    version: &'static str,
    headless: bool,
) -> String {
    let mut services = Vec::with_capacity(2);
    for slot in [Slot::Sync, Slot::Web] {
        let st = dash.status(slot);
        services.push(ServiceSnap {
            name: slot.name(),
            state: st.phase.label(),
            desired: st.desired.label(),
            pid: st.pid,
            uptime_secs: st.uptime().map(|d| d.as_secs()),
            restarts: st.restarts,
            consecutive_failures: st.consecutive_failures,
            last_exit: st.last_exit.as_ref().map(|e| LastExitSnap {
                code: e.code,
                uptime_secs: e.uptime.as_secs(),
            }),
            probe: st.probe.as_ref().map(|p| ProbeSnap {
                ok: p.ok,
                status: p.status,
                rtt_ms: p.rtt.as_millis() as u64,
            }),
            memory_bytes: st.mem_bytes,
            cpu_percent: (st.cpu_percent * 10.0).round() / 10.0,
            circuit_reason: st.circuit_reason.clone(),
        });
    }
    let snap = Snapshot {
        schema: SNAPSHOT_SCHEMA,
        ts: now_unix(),
        supervisor: SupervisorSnap {
            version,
            pid: std::process::id(),
            uptime_secs: dash.started_elapsed().as_secs(),
            headless,
        },
        host: host.clone(),
        services,
    };
    serde_json::to_string(&snap).unwrap_or_else(|_| "{}".into())
}

/// Periodically measure per-service RTT against each service's own port
/// (direct connections only — anti-loop rule #2).
pub async fn run_probes(
    dash: Arc<Dash>,
    sync_addr: String,
    web_port: u16,
    mut kill: watch::Receiver<bool>,
) {
    let web_addr = format!("127.0.0.1:{web_port}");
    loop {
        for (slot, addr, path) in [
            (Slot::Sync, sync_addr.clone(), "/api/health"),
            (Slot::Web, web_addr.clone(), "/"),
        ] {
            let running = dash.status(slot).phase == Phase::Running;
            if !running {
                let mut st = dash.status(slot);
                st.probe = None;
                continue;
            }
            let outcome = probe::get_status(&addr, path, Duration::from_secs(2)).await;
            let mut st = dash.status(slot);
            st.probe = Some(match outcome {
                Ok((status, rtt)) => crate::state::ProbeInfo {
                    ok: (200..400).contains(&status),
                    status,
                    rtt,
                },
                Err(_) => crate::state::ProbeInfo {
                    ok: false,
                    status: 0,
                    rtt: Duration::ZERO,
                },
            });
        }
        tokio::select! {
            _ = tokio::time::sleep(PROBE_INTERVAL) => {}
            _ = kill.changed() => return,
        }
    }
}

/// Sample per-PID memory/CPU into the dashboard every couple of seconds.
pub async fn run_metrics(dash: Arc<Dash>, mut kill: watch::Receiver<bool>) {
    let mut sys = sysinfo::System::new();
    loop {
        for slot in [Slot::Sync, Slot::Web] {
            let pid = dash.status(slot).pid;
            let mut st = dash.status(slot);
            match pid.map(sysinfo::Pid::from_u32) {
                Some(pid) => {
                    sys.refresh_processes_specifics(
                        sysinfo::ProcessesToUpdate::Some(&[pid]),
                        true,
                        sysinfo::ProcessRefreshKind::everything(),
                    );
                    match sys.process(pid) {
                        Some(p) => {
                            st.mem_bytes = p.memory();
                            st.cpu_percent = p.cpu_usage();
                        }
                        None => {
                            st.mem_bytes = 0;
                            st.cpu_percent = 0.0;
                        }
                    }
                }
                None => {
                    st.mem_bytes = 0;
                    st.cpu_percent = 0.0;
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(PROBE_INTERVAL) => {}
            _ = kill.changed() => return,
        }
    }
}

/// Push the snapshot to the sync server's /api/admin/host-snapshot every
/// PUSH_INTERVAL. Failures are display-only (see the module comment).
///
/// ⑩: the response body carries pending admin-page service commands back —
/// the operator's intent, stored server-side, relayed on the ONE transport
/// this task already owns. The server only stores and forwards; it never
/// invents commands, and a failed/absent response changes nothing here (the
/// mailbox simply stays queued until the next push or its 60s TTL).
#[allow(clippy::too_many_arguments)]
pub async fn run_pusher(
    dash: Arc<Dash>,
    host: HostStatic,
    sync_addr: String,
    admin_token: Option<String>,
    version: &'static str,
    headless: bool,
    hub: crate::cmd::CommandHub,
    mut kill: watch::Receiver<bool>,
) {
    let Some(token) = admin_token else {
        // Not an error: pushing is an optional observability feed.
        let mut p = dash.push.lock().unwrap_or_else(|e| e.into_inner());
        p.enabled = false;
        return;
    };
    {
        let mut p = dash.push.lock().unwrap_or_else(|e| e.into_inner());
        p.enabled = true;
    }
    let url_path = "/api/admin/host-snapshot";
    loop {
        let body = build_snapshot_json(&dash, &host, version, headless);
        let outcome =
            probe::post_json(&sync_addr, url_path, &token, &body, Duration::from_secs(3)).await;
        // Scope the lock so it never sits across the select await below
        // (std guards are not async-aware; holding one would make this task
        // !Send and also block the TUI's reads).
        {
            let mut p = dash.push.lock().unwrap_or_else(|e| e.into_inner());
            match outcome {
                Ok((status, response)) if (200..300).contains(&status) => {
                    p.last_ok = Some(std::time::Instant::now());
                    p.last_error = None;
                    p.last_error_at = None;
                    relay_admin_commands(&dash, &hub, &response);
                }
                Ok((status, _)) => {
                    p.last_error = Some(format!("HTTP {status}"));
                    p.last_error_at = Some(std::time::Instant::now());
                }
                Err(e) => {
                    p.last_error = Some(e.to_string());
                    p.last_error_at = Some(std::time::Instant::now());
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(PUSH_INTERVAL) => {}
            _ = kill.changed() => return,
        }
    }
}

/// `{"commands":[{"service":"web","command":"stop"}]}` → the command queue.
/// A malformed or legacy (204/empty) body simply means no commands.
fn relay_admin_commands(dash: &Dash, hub: &crate::cmd::CommandHub, response: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(response) else {
        return;
    };
    let Some(list) = value.get("commands").and_then(|c| c.as_array()) else {
        return;
    };
    for entry in list {
        let (Some(service), Some(command)) = (
            entry.get("service").and_then(|v| v.as_str()),
            entry.get("command").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        match hub.send_from_labels(service, command) {
            Ok(()) => {
                let slot = if service == "web" {
                    Slot::Web
                } else {
                    Slot::Sync
                };
                dash.status(slot).push_log(format!(
                    "[supervisor] command from admin page: {command} {service}"
                ));
            }
            Err(e) => eprintln!("supervisor: admin-page command rejected: {e}"),
        }
    }
}

/// Host facts sampled once at startup (os/cores/RAM don't change).
pub fn collect_host_static() -> HostStatic {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    HostStatic {
        os: std::env::consts::OS.to_string(),
        cores: std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1),
        total_memory_bytes: sys.total_memory(),
    }
}

/// One-line status for the headless printer and TUI footer.
pub fn summarize(dash: &Dash) -> String {
    let mut parts = Vec::with_capacity(3);
    for slot in [Slot::Sync, Slot::Web] {
        let st = dash.status(slot);
        let probe = match &st.probe {
            Some(p) if p.ok => format!("{}ms", p.rtt.as_millis()),
            Some(_) => "down".into(),
            None => "—".into(),
        };
        let icon = match st.phase {
            Phase::Running => "●",
            Phase::Starting => "○",
            Phase::Restarting => "↻",
            Phase::CircuitOpen => "✖",
            Phase::Halted => "▪",
            Phase::Stopped => "■",
        };
        let cpu = format!("{:.1}%", st.cpu_percent);
        let restarts = format!("restarts {}", st.restarts);
        parts.push(format!(
            "{} {} pid {} {} {} {} probe {}",
            slot.name(),
            icon,
            st.pid.unwrap_or(0),
            human_bytes(st.mem_bytes),
            cpu,
            restarts,
            probe,
        ));
    }
    parts.join(" │ ")
}
