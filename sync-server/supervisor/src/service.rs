//! Per-service child lifecycle: spawn → watch → restart with backoff →
//! circuit-break, plus ⑩'s operator control. One `run_service` task owns one
//! child at a time; a shared `Dash` publishes its status for the TUI /
//! headless printer / snapshot; a per-slot `SvcCmd` FIFO is the ONLY way any
//! frontend (TUI, tray, admin-page buttons) influences this runner.
//!
//! State machine (⑩; decisions at ─ waits in ▒▒▒):
//!
//! ```text
//!                global kill (Ctrl+C / stop.bat / q) — from ANY state,
//!                highest priority ──────────────────────────► Stopped, exit
//!                                                                  ▲
//!   ┌──spawn Ok──► Running ──operator Stop/Restart──► kill own pid──┘ (kill)
//!   │                │                                  │ blameless
//!   │                │ natural exit                      │ (policy::Operator)
//!   │                ▼                                  ├─ Stop ──► Halted ▒▒ parks
//!   │        policy::classify_exit                       └─ Restart ─► spawn now
//!   │                │ Reset/Count → failures ±
//!   │                │ ├─ failures ≥ 5 ──► CircuitOpen ▒▒ parks (waits for
//!   │                │ │                   an operator Start — the runner no
//!   │                │ │                   longer gives up and exits; only a
//!   │                │ │                   human recovers it, unchanged)
//!   │                │ └─ else ──► Restarting ▒▒ backoff sleep
//!   │                │               (Start/Restart cancel it → spawn now;
//!   │                │                Stop cancels it → Halted)
//!   └──spawn Err──► CircuitOpen ▒▒ (reason = spawn error)
//!
//!   Halted / CircuitOpen residency: every command is inspected. Start or
//!   Restart clears failures + circuit reason and spawns immediately (never
//!   the backoff ladder); Stop transitions CircuitOpen → Halted; in Halted it
//!   is a no-op. Desired (Run/Halt) lives in memory only: a supervisor
//!   restart is a fresh intent, so an autostarted instance always brings both
//!   services back — no "registered autostart but forever halted" zombies.
//! ```
//!
//! Ownership rules (unchanged since ⑦):
//!   * control flows one way only — this runner decides when its child
//!     starts and dies; probes, the snapshot pusher and the TUI only read
//!     (frontends submit intent via the command FIFO, nothing else);
//!   * the child's `wait()` runs in its own waiter task (owning the Child),
//!     and the runner talks to it through a oneshot — so the kill path never
//!     fights the wait future for the Child borrow. Per-service kills go
//!     through `platform::kill_pid` (the child is in the global Job Object /
//!     has PDEATHSIG only for supervisor-death cleanup, so pid-kill here
//!     touches just this child, never the sibling).

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Instant;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, watch};

use crate::cmd::SvcCmd;
use crate::policy;
use crate::state::{Dash, Desired, ExitInfo, Phase, Slot};

pub struct ServiceSpec {
    pub slot: Slot,
    /// Executable to run (absolute path, or a bare name resolved from PATH).
    pub program: PathBuf,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    /// Env vars set only when absent (operator env / .env wins).
    pub env_defaults: Vec<(&'static str, &'static str)>,
    pub log_dir: PathBuf,
    /// Human-readable command line for error messages.
    pub display: String,
}

impl ServiceSpec {
    fn log_path(&self) -> PathBuf {
        self.log_dir.join(format!("{}.log", self.slot.name()))
    }

    fn open_log(&self) -> Option<std::fs::File> {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_path())
            .map_err(|e| {
                eprintln!("supervisor: cannot open {}: {e}", self.log_path().display());
            })
            .ok()
    }
}

/// Log a supervisor-side event into the service's tail AND its log file, so
/// "why did it restart" is readable next to the child's own output.
fn log_event(spec: &ServiceSpec, dash: &Dash, message: String) {
    let line = format!("[supervisor] {message}");
    dash.status(spec.slot).push_log(line.clone());
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(spec.log_path())
    {
        let _ = writeln!(f, "{line}");
    }
}

fn build_command(spec: &ServiceSpec) -> Command {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .current_dir(&spec.working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in &spec.env_defaults {
        if std::env::var_os(k).is_none() {
            cmd.env(k, v);
        }
    }
    #[cfg(unix)]
    crate::platform::prepare(&mut cmd);
    cmd
}

async fn pump_logs<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    reader: R,
    mut file: Option<std::fs::File>,
    dash: std::sync::Arc<Dash>,
    slot: Slot,
) {
    let mut lines = BufReader::new(reader).lines();
    // A read error also just ends the pump: the child exited either way.
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(f) = file.as_mut() {
            let _ = writeln!(f, "{line}");
        }
        dash.status(slot).push_log(line);
    }
}

fn attach_pumps(spec: &ServiceSpec, dash: &std::sync::Arc<Dash>, child: &mut Child) {
    if let Some(out) = child.stdout.take() {
        tokio::spawn(pump_logs(out, spec.open_log(), dash.clone(), spec.slot));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(pump_logs(err, spec.open_log(), dash.clone(), spec.slot));
    }
}

fn spawn_failure_reason(spec: &ServiceSpec, e: &std::io::Error) -> String {
    let bare = spec.program.components().count() == 1;
    match e.kind() {
        std::io::ErrorKind::NotFound if bare => format!(
            "cannot start: {} not found on PATH — install Node.js >= 20, put node(.exe) beside the supervisor, or set READEST_NODE",
            spec.display
        ),
        std::io::ErrorKind::NotFound => format!("cannot start: {} not found", spec.display),
        _ => format!("cannot start {}: {e}", spec.display),
    }
}

/// What woke a parked/waiting runner.
enum Wake {
    Kill,
    Cmd(SvcCmd),
}

/// Await the next command or the global kill. `kill.changed()` alone would
/// miss an already-fired signal, so callers check `*kill.borrow()` on their
/// exit paths; a closed command channel means shutdown is tearing down.
async fn next_signal(
    cmd_rx: &mut mpsc::UnboundedReceiver<SvcCmd>,
    kill: &mut watch::Receiver<bool>,
) -> Wake {
    tokio::select! {
        _ = kill.changed() => Wake::Kill,
        Some(cmd) = cmd_rx.recv() => Wake::Cmd(cmd),
        else => Wake::Kill,
    }
}

fn mark_stopped(dash: &Dash, slot: Slot) {
    let mut st = dash.status(slot);
    st.phase = Phase::Stopped;
    st.pid = None;
    st.restart_at = None;
}

/// Park in `phase` (CircuitOpen after a break, Halted after a manual Stop)
/// until an operator Start/Restart or the global kill. Returns Resume with
/// failures/circuit-reason left for the caller to clear. A Stop arriving in
/// CircuitOpen transitions to Halted (the operator explicitly wants it down,
/// not just "given up").
async fn park(
    spec: &ServiceSpec,
    dash: &std::sync::Arc<Dash>,
    cmd_rx: &mut mpsc::UnboundedReceiver<SvcCmd>,
    kill: &mut watch::Receiver<bool>,
    mut phase: Phase,
) -> Wake {
    loop {
        {
            let mut st = dash.status(spec.slot);
            st.phase = phase;
            st.pid = None;
            st.restart_at = None;
            st.desired = if phase == Phase::Halted {
                Desired::Halt
            } else {
                Desired::Run
            };
        }
        match next_signal(cmd_rx, kill).await {
            Wake::Kill => return Wake::Kill,
            Wake::Cmd(SvcCmd::Start | SvcCmd::Restart) => {
                log_event(spec, dash, "resumed by operator — starting now".into());
                let mut st = dash.status(spec.slot);
                st.desired = Desired::Run;
                return Wake::Cmd(SvcCmd::Start);
            }
            Wake::Cmd(SvcCmd::Stop) => {
                if phase != Phase::Halted {
                    log_event(
                        spec,
                        dash,
                        "suppressed by operator (no auto-restart)".into(),
                    );
                    phase = Phase::Halted;
                }
            }
        }
    }
}

/// Supervise one service until the global kill signal; manual commands may
/// park it (Halted) or break its circuit, but only the kill switch or a
/// supervisor exit ends this task.
pub async fn run_service(
    spec: ServiceSpec,
    dash: std::sync::Arc<Dash>,
    mut cmd_rx: mpsc::UnboundedReceiver<SvcCmd>,
    mut kill: watch::Receiver<bool>,
) {
    let slot = spec.slot;
    let mut failures: u32 = 0;

    loop {
        // Never spawn a fresh child after shutdown was requested.
        if *kill.borrow() {
            mark_stopped(&dash, slot);
            return;
        }

        // Local helper: clear operator-intent bookkeeping for a fresh start.
        macro_rules! clear_for_start {
            () => {{
                failures = 0;
                let mut st = dash.status(slot);
                st.consecutive_failures = 0;
                st.circuit_reason = None;
            }};
        }

        let mut child = match build_command(&spec).spawn() {
            Ok(c) => c,
            Err(e) => {
                // A missing binary or bad cwd will not fix itself by
                // retrying: open the circuit immediately with the reason and
                // park — an operator Start retries after the fix (⑩: no more
                // "restart the whole supervisor" recovery).
                let reason = spawn_failure_reason(&spec, &e);
                {
                    let mut st = dash.status(slot);
                    st.phase = Phase::CircuitOpen;
                    st.circuit_reason = Some(reason.clone());
                }
                log_event(&spec, &dash, format!("circuit open: {reason}"));
                if let Wake::Kill =
                    park(&spec, &dash, &mut cmd_rx, &mut kill, Phase::CircuitOpen).await
                {
                    mark_stopped(&dash, slot);
                    return;
                }
                clear_for_start!();
                continue;
            }
        };

        crate::platform::assign_child(&child);
        let started = Instant::now();
        let pid = child.id();
        if let Some(p) = pid {
            crate::platform::register_pid(p);
        }
        {
            let mut st = dash.status(slot);
            st.phase = Phase::Running;
            st.pid = pid;
            st.started_at = Some(started);
            st.restart_at = None;
        }
        log_event(
            &spec,
            &dash,
            format!("started {} (pid {})", spec.display, pid.unwrap_or(0)),
        );
        attach_pumps(&spec, &dash, &mut child);

        // Waiter task owns the Child; runner observes through the oneshot so
        // it can still react to commands and the kill signal.
        let (exit_tx, mut exit_rx) = oneshot::channel::<ExitStatus>();
        tokio::spawn(async move {
            if let Ok(status) = child.wait().await {
                let _ = exit_tx.send(status);
            }
        });

        // The operator command (if any) that killed this child — makes the
        // exit blameless and decides Halted vs immediate respawn.
        let mut operator_kill: Option<SvcCmd> = None;
        let exited: Option<ExitStatus> = loop {
            tokio::select! {
                st = &mut exit_rx => break st.ok(),
                _ = kill.changed() => {
                    crate::platform::terminate_children();
                    break exit_rx.await.ok();
                }
                Some(cmd) = cmd_rx.recv() => match cmd {
                    SvcCmd::Start => {
                        log_event(&spec, &dash, "start ignored: already running".into());
                    }
                    stop_or_restart @ (SvcCmd::Stop | SvcCmd::Restart) => {
                        let previous = operator_kill.replace(stop_or_restart);
                        if previous.is_none() {
                            log_event(
                                &spec,
                                &dash,
                                if stop_or_restart == SvcCmd::Stop {
                                    "stop requested by operator — killing child (not a failure)".into()
                                } else {
                                    "restart requested by operator — killing child (not a failure)".into()
                                },
                            );
                        }
                        dash.status(slot).desired =
                            if stop_or_restart == SvcCmd::Stop { Desired::Halt } else { Desired::Run };
                        if let Some(p) = pid {
                            crate::platform::kill_pid(p);
                        }
                    }
                },
                else => break None, // command channel gone: shutdown teardown
            }
        };
        if let Some(p) = pid {
            crate::platform::unregister_pid(p);
        }

        let uptime = started.elapsed();
        let code = exited.and_then(|s| s.code());

        // Shutdown path: kill fired → this exit is our own doing.
        if *kill.borrow() {
            mark_stopped(&dash, slot);
            return;
        }

        // Blameless operator exit: Stop parks in Halted, Restart respawns
        // immediately — neither touches the failure counter.
        if let Some(cmd) = operator_kill {
            {
                let mut st = dash.status(slot);
                st.pid = None;
                st.last_exit = Some(ExitInfo { code, uptime });
            }
            if cmd == SvcCmd::Stop {
                if let Wake::Kill = park(&spec, &dash, &mut cmd_rx, &mut kill, Phase::Halted).await
                {
                    mark_stopped(&dash, slot);
                    return;
                }
                clear_for_start!();
                continue;
            }
            clear_for_start!();
            continue;
        }

        // Natural exit: classify, then backoff or circuit-break.
        match policy::classify_exit(uptime, false) {
            policy::ExitAccounting::Reset => failures = 0,
            policy::ExitAccounting::Count | policy::ExitAccounting::Operator => failures += 1,
        }
        {
            let mut st = dash.status(slot);
            st.phase = Phase::Restarting;
            st.pid = None;
            st.last_exit = Some(ExitInfo { code, uptime });
            st.consecutive_failures = failures;
        }
        log_event(
            &spec,
            &dash,
            format!(
                "exited (code {}, uptime {}s)",
                code.map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into()),
                uptime.as_secs()
            ),
        );

        if policy::should_circuit_break(failures) {
            let reason = format!(
                "{} rapid failures in a row (last: exit {} after {}s) — circuit open, waiting for an operator start; check {}",
                failures,
                code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
                uptime.as_secs(),
                spec.log_path().display()
            );
            {
                let mut st = dash.status(slot);
                st.phase = Phase::CircuitOpen;
                st.circuit_reason = Some(reason.clone());
            }
            log_event(&spec, &dash, format!("circuit open: {reason}"));
            if let Wake::Kill = park(&spec, &dash, &mut cmd_rx, &mut kill, Phase::CircuitOpen).await
            {
                mark_stopped(&dash, slot);
                return;
            }
            clear_for_start!();
            continue;
        }

        let delay = policy::backoff_delay(failures);
        let restart_no = {
            let mut st = dash.status(slot);
            st.restarts += 1;
            st.restart_at = Some(Instant::now() + delay);
            st.restarts
        };
        log_event(
            &spec,
            &dash,
            format!(
                "restart #{restart_no} in {}s (rapid failure {}/{})",
                delay.as_secs(),
                failures,
                policy::MAX_CONSECUTIVE_FAILURES
            ),
        );
        // Interruptible backoff, single pass — every branch resolves it: the
        // sleep elapses into the restart, an operator Start/Restart skips the
        // ladder (failures cleared below), a Stop parks in Halted until an
        // operator resumes, and shutdown/kill ends the runner.
        let operator_interrupt = tokio::select! {
            _ = tokio::time::sleep(delay) => false,
            _ = kill.changed() => {
                mark_stopped(&dash, slot);
                return;
            }
            Some(cmd) = cmd_rx.recv() => match cmd {
                SvcCmd::Start | SvcCmd::Restart => {
                    log_event(&spec, &dash, "operator start — restarting now".into());
                    dash.status(slot).restart_at = None;
                    true
                }
                SvcCmd::Stop => {
                    if let Wake::Kill =
                        park(&spec, &dash, &mut cmd_rx, &mut kill, Phase::Halted).await
                    {
                        mark_stopped(&dash, slot);
                        return;
                    }
                    true
                }
            },
            else => {
                mark_stopped(&dash, slot);
                return;
            }
        };
        if operator_interrupt {
            clear_for_start!();
        }
    }
}
