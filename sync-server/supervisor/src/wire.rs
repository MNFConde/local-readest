//! Wire types for the observation socket (primary ⇄ attach client, ⑩).
//!
//! Newline-delimited JSON both ways on a loopback TCP connection: the
//! primary pushes a full `StatusFrame` every second (same numbers the TUI
//! reads off the live Dash, plus the log tails the §6.6 admin snapshot does
//! not carry), and attach clients send `ClientMsg` command lines. The
//! §6.6 host-snapshot shape is intentionally NOT reused: this is a different
//! consumer (a full dashboard) with different fields (tails, desired,
//! countdowns).

use serde::{Deserialize, Serialize};

use crate::state::{Dash, Desired, Phase, ProbeInfo, Slot};

pub const WIRE_SCHEMA: u32 = 1;
/// Log lines carried per service in a status frame (loopback, once a second).
pub const WIRE_TAIL_LINES: usize = 50;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StatusFrame {
    pub schema: u32,
    pub supervisor: SupervisorFrame,
    pub host: HostFrame,
    pub push: PushFrame,
    pub services: Vec<ServiceFrame>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SupervisorFrame {
    pub version: String,
    pub pid: u32,
    pub uptime_secs: u64,
    pub headless: bool,
    /// Attached observation clients right now.
    pub attached: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HostFrame {
    pub os: String,
    pub cores: u32,
    pub total_memory_bytes: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PushFrame {
    pub enabled: bool,
    pub last_ok_age_secs: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServiceFrame {
    pub name: String,
    pub state: String,
    pub desired: String,
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
    pub restarts: u32,
    pub consecutive_failures: u32,
    /// Seconds until the pending restart fires (Restarting countdown).
    pub restart_in_secs: Option<u64>,
    pub last_exit: Option<LastExitFrame>,
    pub probe: Option<ProbeFrame>,
    pub memory_bytes: u64,
    pub cpu_percent: f32,
    pub circuit_reason: Option<String>,
    pub tail: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LastExitFrame {
    pub code: Option<i32>,
    pub uptime_secs: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProbeFrame {
    pub ok: bool,
    pub status: u16,
    pub rtt_ms: u64,
}

/// Attach → primary. `cmd` is `start|stop|restart` (needs `slot`) or
/// `shutdown` (global stop — used by `supervisor uninstall`'s default).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ClientMsg {
    pub slot: Option<String>,
    pub cmd: String,
}

fn slot_by_name(name: &str) -> Option<Slot> {
    match name {
        "sync-server" => Some(Slot::Sync),
        "web" => Some(Slot::Web),
        _ => None,
    }
}

/// Snapshot the live Dash into a wire frame (primary side).
pub fn build_frame(
    dash: &Dash,
    host: &crate::snapshot::HostStatic,
    version: &str,
    headless: bool,
    attached: u32,
) -> StatusFrame {
    let mut services = Vec::with_capacity(2);
    for slot in [Slot::Sync, Slot::Web] {
        let st = dash.status(slot);
        services.push(ServiceFrame {
            name: slot.name().to_string(),
            state: st.phase.label().to_string(),
            desired: st.desired.label().to_string(),
            pid: st.pid,
            uptime_secs: st.uptime().map(|d| d.as_secs()),
            restarts: st.restarts,
            consecutive_failures: st.consecutive_failures,
            restart_in_secs: st.restart_at.map(|at| {
                at.saturating_duration_since(std::time::Instant::now())
                    .as_secs()
            }),
            last_exit: st.last_exit.as_ref().map(|e| LastExitFrame {
                code: e.code,
                uptime_secs: e.uptime.as_secs(),
            }),
            probe: st.probe.as_ref().map(|p| ProbeFrame {
                ok: p.ok,
                status: p.status,
                rtt_ms: p.rtt.as_millis() as u64,
            }),
            memory_bytes: st.mem_bytes,
            cpu_percent: (st.cpu_percent * 10.0).round() / 10.0,
            circuit_reason: st.circuit_reason.clone(),
            tail: st
                .tail
                .iter()
                .rev()
                .take(WIRE_TAIL_LINES)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
        });
    }
    let push = dash.push.lock().unwrap_or_else(|e| e.into_inner());
    StatusFrame {
        schema: WIRE_SCHEMA,
        supervisor: SupervisorFrame {
            version: version.to_string(),
            pid: std::process::id(),
            uptime_secs: dash.started_elapsed().as_secs(),
            headless,
            attached,
        },
        host: HostFrame {
            os: host.os.clone(),
            cores: host.cores,
            total_memory_bytes: host.total_memory_bytes,
        },
        push: PushFrame {
            enabled: push.enabled,
            last_ok_age_secs: push.last_ok.map(|t| t.elapsed().as_secs()),
            last_error: push.last_error.clone(),
        },
        services,
    }
}

impl StatusFrame {
    /// Apply the frame onto a local Dash (attach side). `started` is applied
    /// only when `adopt_started` is true: the attach client fixes it once,
    /// before concurrent readers exist (Instant is not atomically writable).
    /// Returns the host facts for the UI header.
    pub fn apply(&self, dash: &Dash, adopt_started: bool) -> Option<crate::snapshot::HostStatic> {
        if adopt_started {
            let anchor = std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(self.supervisor.uptime_secs))
                .unwrap_or_else(std::time::Instant::now);
            dash.set_started(anchor);
        }
        {
            let mut p = dash.push.lock().unwrap_or_else(|e| e.into_inner());
            p.enabled = self.push.enabled;
            p.last_ok = self.push.last_ok_age_secs.map(|age| {
                std::time::Instant::now()
                    .checked_sub(std::time::Duration::from_secs(age))
                    .unwrap_or_else(std::time::Instant::now)
            });
            p.last_error = self.push.last_error.clone();
        }
        for svc in &self.services {
            let Some(slot) = slot_by_name(&svc.name) else {
                continue;
            };
            let mut st = dash.status(slot);
            st.phase = Phase::from_label(&svc.state);
            st.desired = Desired::from_label(&svc.desired);
            st.pid = svc.pid;
            st.started_at = svc.uptime_secs.map(|u| {
                std::time::Instant::now()
                    .checked_sub(std::time::Duration::from_secs(u))
                    .unwrap_or_else(std::time::Instant::now)
            });
            st.restarts = svc.restarts;
            st.consecutive_failures = svc.consecutive_failures;
            st.restart_at = svc.restart_in_secs.map(|s| {
                std::time::Instant::now()
                    .checked_add(std::time::Duration::from_secs(s))
                    .unwrap_or_else(std::time::Instant::now)
            });
            st.last_exit = svc.last_exit.as_ref().map(|e| crate::state::ExitInfo {
                code: e.code,
                uptime: std::time::Duration::from_secs(e.uptime_secs),
            });
            st.probe = svc.probe.as_ref().map(|p| ProbeInfo {
                ok: p.ok,
                status: p.status,
                rtt: std::time::Duration::from_millis(p.rtt_ms),
            });
            st.mem_bytes = svc.memory_bytes;
            st.cpu_percent = svc.cpu_percent;
            st.circuit_reason = svc.circuit_reason.clone();
            st.tail = svc.tail.iter().cloned().collect();
        }
        Some(crate::snapshot::HostStatic {
            os: self.host.os.clone(),
            cores: self.host.cores,
            total_memory_bytes: self.host.total_memory_bytes,
        })
    }
}
