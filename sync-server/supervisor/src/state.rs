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
    Stopped,
}

impl Phase {
    pub fn label(&self) -> &'static str {
        match self {
            Phase::Starting => "starting",
            Phase::Running => "running",
            Phase::Restarting => "restarting",
            Phase::CircuitOpen => "circuit open",
            Phase::Stopped => "stopped",
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
    pub started: Instant,
    pub sync: Mutex<ServiceStatus>,
    pub web: Mutex<ServiceStatus>,
    pub push: Mutex<PushInfo>,
}

impl Dash {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            sync: Mutex::new(ServiceStatus::new(Phase::Starting)),
            web: Mutex::new(ServiceStatus::new(Phase::Starting)),
            push: Mutex::new(PushInfo::disabled()),
        }
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
