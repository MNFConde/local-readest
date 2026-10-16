//! Per-service child lifecycle: spawn → watch → restart with backoff →
//! circuit-break. One `run_service` task owns one child at a time; a shared
//! `Dash` publishes its status for the TUI / headless printer / snapshot.
//!
//! Ownership rules (ROADMAP 待办⑦):
//!   * control flows one way only — this runner decides when its child
//!     starts and dies; probes, the snapshot pusher and the TUI only read;
//!   * the child's `wait()` runs in its own waiter task (owning the Child),
//!     and the runner talks to it through a oneshot — so the kill path never
//!     fights the wait future for the Child borrow.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Instant;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, watch};

use crate::policy;
use crate::state::{Dash, ExitInfo, Phase, Slot};

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
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(spec.log_path()) {
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
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if let Some(f) = file.as_mut() {
                    let _ = writeln!(f, "{line}");
                }
                dash.status(slot).push_log(line);
            }
            Ok(None) | Err(_) => break, // pipe closed: the child exited
        }
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

/// Supervise one service until the kill signal or the circuit breaker ends it.
pub async fn run_service(
    spec: ServiceSpec,
    dash: std::sync::Arc<Dash>,
    mut kill: watch::Receiver<bool>,
) {
    let slot = spec.slot;
    let mut failures: u32 = 0;

    loop {
        let mut child = match build_command(&spec).spawn() {
            Ok(c) => c,
            Err(e) => {
                // A missing binary or bad cwd will not fix itself by
                // retrying: open the circuit immediately with the reason.
                let reason = spawn_failure_reason(&spec, &e);
                let mut st = dash.status(slot);
                st.phase = Phase::CircuitOpen;
                st.circuit_reason = Some(reason.clone());
                drop(st);
                log_event(&spec, &dash, format!("giving up: {reason}"));
                return;
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
        // it can still react to the kill signal without borrow gymnastics.
        let (exit_tx, mut exit_rx) = oneshot::channel::<ExitStatus>();
        tokio::spawn(async move {
            if let Ok(status) = child.wait().await {
                let _ = exit_tx.send(status);
            }
        });

        let exited: Option<ExitStatus> = tokio::select! {
            st = &mut exit_rx => st.ok(),
            _ = kill.changed() => {
                crate::platform::terminate_children();
                exit_rx.await.ok()
            }
        };
        if let Some(p) = pid {
            crate::platform::unregister_pid(p);
        }

        let uptime = started.elapsed();
        let code = exited.and_then(|s| s.code());

        // Shutdown path: kill fired → this exit is our own doing.
        if *kill.borrow() {
            let mut st = dash.status(slot);
            st.phase = Phase::Stopped;
            st.pid = None;
            return;
        }

        if policy::ran_long_enough_to_reset(uptime) {
            failures = 0;
        } else {
            failures += 1;
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
                code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
                uptime.as_secs()
            ),
        );

        if policy::should_circuit_break(failures) {
            let reason = format!(
                "{} rapid failures in a row (last: exit {} after {}s) — giving up; check {}, fix the cause, restart the supervisor",
                failures,
                code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
                uptime.as_secs(),
                spec.log_path().display()
            );
            let mut st = dash.status(slot);
            st.phase = Phase::CircuitOpen;
            st.circuit_reason = Some(reason.clone());
            drop(st);
            log_event(&spec, &dash, format!("circuit open: {reason}"));
            return;
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
        tokio::select! {
            _ = tokio::time::sleep(delay) => continue,
            _ = kill.changed() => {
                let mut st = dash.status(slot);
                st.phase = Phase::Stopped;
                st.restart_at = None;
                return;
            }
        }
    }
}
