//! The one command queue every frontend submits to (⑩ 内核/皮分离).
//!
//! The TUI (in-process or attached over the observation socket), the tray and
//! the admin page's buttons (relayed through the sync server's response to
//! the metrics push) are three peer UIs — none of them touches a child or a
//! runner directly. They all turn user intent into a `SvcCmd` on this queue,
//! and the per-service runner in service.rs is the only consumer of its
//! slot's FIFO. Control flows one way; the global kill switch (Ctrl+C /
//! stop.bat / q in the in-process TUI) stays a separate, higher-priority
//! channel.

use tokio::sync::mpsc;

use crate::state::Slot;

/// Operator intent for one service, in FIFO order per service.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SvcCmd {
    /// Resume supervision: clears the rapid-failure counter and any circuit
    /// reason, starts immediately (never walks the backoff ladder).
    Start,
    /// Suppress the service until an operator Start or a supervisor restart.
    /// Never counted as a failure.
    Stop,
    /// Fresh operator intent: a running child is killed blamelessly and
    /// comes back immediately; a halted/circuit-open one is equivalent to
    /// Start.
    Restart,
}

impl SvcCmd {
    pub fn from_label(label: &str) -> Option<SvcCmd> {
        match label {
            "start" => Some(SvcCmd::Start),
            "stop" => Some(SvcCmd::Stop),
            "restart" => Some(SvcCmd::Restart),
            _ => None,
        }
    }
}

/// Cloneable handle the frontends share. Each slot is its own FIFO whose
/// receiving end belongs to that slot's runner (mpsc would otherwise hand a
/// message to only one of the two consumers).
#[derive(Clone)]
pub struct CommandHub {
    sync: mpsc::UnboundedSender<SvcCmd>,
    web: mpsc::UnboundedSender<SvcCmd>,
}

/// The runner-owned ends, handed to run_service in main.
pub struct CommandTaps {
    pub sync: mpsc::UnboundedReceiver<SvcCmd>,
    pub web: mpsc::UnboundedReceiver<SvcCmd>,
}

impl CommandHub {
    pub fn pair() -> (CommandHub, CommandTaps) {
        let (sync_tx, sync_rx) = mpsc::unbounded_channel();
        let (web_tx, web_rx) = mpsc::unbounded_channel();
        (
            CommandHub {
                sync: sync_tx,
                web: web_tx,
            },
            CommandTaps {
                sync: sync_rx,
                web: web_rx,
            },
        )
    }

    /// Submit one command. A dead receiver only happens during shutdown —
    /// dropping it then is exactly right.
    pub fn send(&self, slot: Slot, cmd: SvcCmd) {
        let tx = match slot {
            Slot::Sync => &self.sync,
            Slot::Web => &self.web,
        };
        let _ = tx.send(cmd);
    }

    /// `{"slot": "web", "cmd": "stop"}`-shaped wire strings → queue.
    pub fn send_from_labels(&self, slot_label: &str, cmd_label: &str) -> Result<(), String> {
        let slot = match slot_label {
            "sync-server" => Slot::Sync,
            "web" => Slot::Web,
            other => return Err(format!("unknown service {other:?}")),
        };
        let cmd = SvcCmd::from_label(cmd_label)
            .ok_or_else(|| format!("unknown command {cmd_label:?}"))?;
        self.send(slot, cmd);
        Ok(())
    }
}
