//! Shared dashboard state: everything the TUI / headless printer / snapshot
//! pusher read is snapshotted from here, and everything the service runners
//! and probes know lands here. Locks are std Mutexes held only for
//! microsecond-scale field updates — never across an await.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Log lines kept per service for the TUI tail (file on disk keeps the rest).
const TAIL_LINES: usize = 300;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Starting,
    Running,
    Restarting,
    CircuitOpen,
    /// Operator pressed Stop: suppressed and parked until an operator Start
    /// or a supervisor restart (⑩). Distinct from `Stopped`, which is the
    /// terminal phase during global shutdown.
    Halted,
    Stopped,
}

impl Phase {
    pub fn label(&self) -> &'static str {
        match self {
            Phase::Starting => "starting",
            Phase::Running => "running",
            Phase::Restarting => "restarting",
            Phase::CircuitOpen => "circuit open",
            Phase::Halted => "halted",
            Phase::Stopped => "stopped",
        }
    }

    /// Parse the wire label back (attach client reconstructs a local Dash
    /// from observation frames; unknown labels render as starting).
    pub fn from_label(label: &str) -> Phase {
        match label {
            "running" => Phase::Running,
            "restarting" => Phase::Restarting,
            "circuit open" => Phase::CircuitOpen,
            "halted" => Phase::Halted,
            "stopped" => Phase::Stopped,
            _ => Phase::Starting,
        }
    }
}

/// Operator intent for one service (⑩): Run = keep it alive (the normal
/// supervision state), Halt = suppressed by a manual Stop. In-memory only on
/// purpose — a reboot is a fresh intent, so an autostarted supervisor always
/// brings both services back and never lingers in a "registered but halted"
/// zombie state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Desired {
    Run,
    Halt,
}

impl Desired {
    pub fn label(&self) -> &'static str {
        match self {
            Desired::Run => "run",
            Desired::Halt => "halt",
        }
    }

    pub fn from_label(label: &str) -> Desired {
        match label {
            "halt" => Desired::Halt,
            _ => Desired::Run,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ProbeInfo {
    pub ok: bool,
    pub status: u16,
    pub rtt: Duration,
}

#[derive(Clone, Debug)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub uptime: Duration,
}

#[derive(Debug)]
pub struct ServiceStatus {
    pub phase: Phase,
    /// Operator intent (⑩): Run = supervised, Halt = manually suppressed.
    /// Display + snapshot only — the runner's actual driver is its command
    /// queue; this mirrors it for the TUI / admin page / attach clients.
    pub desired: Desired,
    pub pid: Option<u32>,
    pub started_at: Option<Instant>,
    pub restarts: u32,
    pub consecutive_failures: u32,
    /// Absolute deadline of the pending restart (display only; the runner
    /// sleeps its own sleep — this is what the TUI counts down from).
    pub restart_at: Option<Instant>,
    pub last_exit: Option<ExitInfo>,
    pub circuit_reason: Option<String>,
    pub probe: Option<ProbeInfo>,
    pub mem_bytes: u64,
    pub cpu_percent: f32,
    pub tail: VecDeque<String>,
}

impl ServiceStatus {
    pub fn new(phase: Phase) -> Self {
        Self {
            phase,
            desired: Desired::Run,
            pid: None,
            started_at: None,
            restarts: 0,
            consecutive_failures: 0,
            restart_at: None,
            last_exit: None,
            circuit_reason: None,
            probe: None,
            mem_bytes: 0,
            cpu_percent: 0.0,
            tail: VecDeque::new(),
        }
    }

    pub fn push_log(&mut self, line: String) {
        if self.tail.len() >= TAIL_LINES {
            self.pop_front_log();
        }
        self.tail.push_back(line);
    }

    fn pop_front_log(&mut self) {
        self.tail.pop_front();
    }

    pub fn uptime(&self) -> Option<Duration> {
        self.started_at.map(|t| t.elapsed())
    }
}

#[derive(Debug)]
pub struct PushInfo {
    /// False when ADMIN_TOKEN is unset: pushing is skipped entirely.
    pub enabled: bool,
    pub last_ok: Option<Instant>,
    pub last_error: Option<String>,
    pub last_error_at: Option<Instant>,
}

impl PushInfo {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            last_ok: None,
            last_error: None,
            last_error_at: None,
        }
    }
}

/// Which of the two supervised services a value belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    Sync,
    Web,
}

impl Slot {
    pub fn name(&self) -> &'static str {
        match self {
            Slot::Sync => "sync-server",
            Slot::Web => "web",
        }
    }
}

pub struct Dash {
    /// Supervisor start (attach clients re-anchor this from their first
    /// frame; behind a Mutex because the attach reader task writes it).
    pub started: Mutex<Instant>,
    pub sync: Mutex<ServiceStatus>,
    pub web: Mutex<ServiceStatus>,
    pub push: Mutex<PushInfo>,
}

impl Dash {
    pub fn new() -> Self {
        Self {
            started: Mutex::new(Instant::now()),
            sync: Mutex::new(ServiceStatus::new(Phase::Starting)),
            web: Mutex::new(ServiceStatus::new(Phase::Starting)),
            push: Mutex::new(PushInfo::disabled()),
        }
    }

    pub fn started_elapsed(&self) -> Duration {
        self.started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
    }

    pub fn set_started(&self, at: Instant) {
        *self.started.lock().unwrap_or_else(|e| e.into_inner()) = at;
    }

    pub fn status(&self, slot: Slot) -> MutexGuard<'_, ServiceStatus> {
        let m = match slot {
            Slot::Sync => &self.sync,
            Slot::Web => &self.web,
        };
        // A poisoned lock only means a panic while formatting a log line;
        // the data inside is still usable.
        m.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// `1h23m45s`-style compact uptime for table cells.
pub fn human_duration(d: Duration) -> String {
    let s = d.as_secs();
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

/// `45.2M`-style byte size for table cells (SI, like the readers' UI).
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = bytes as f64;
    let mut unit = 0usize;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}B")
    } else {
        format!("{v:.1}{}", UNITS[unit])
    }
}
