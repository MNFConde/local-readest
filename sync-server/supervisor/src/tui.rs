//! The dashboard: services table (state/uptime/restarts/PID/memory/CPU/RTT),
//! per-service log tails, and a footer. Since ⑩ it is also a control surface:
//! ↑/↓ selects a service, s/x/r submit Start/Stop/Restart to the command
//! queue. Two equally-valid deployments share this rendering —
//!
//!   * Primary (in-process): commands go straight to the hub; q = stop
//!     everything, same as Ctrl+C / stop.bat.
//!   * Attached (second invocation connected to the observation socket):
//!     commands become wire lines the primary executes; q = disconnect
//!     observation only — the services never notice.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{poll, read, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Row, Table};
use ratatui::Frame;
use tokio::sync::{mpsc, watch};

use crate::cmd::{CommandHub, SvcCmd};
use crate::snapshot::HostStatic;
use crate::state::{human_bytes, human_duration, Dash, Phase, Slot};

/// Where commands go and what quitting means (see the module comment).
pub enum TuiBackend {
    Primary {
        hub: CommandHub,
    },
    Attach {
        cmd_tx: mpsc::UnboundedSender<String>,
        closed: Arc<AtomicBool>,
    },
}

impl TuiBackend {
    fn apply(&self, slot: Slot, cmd: SvcCmd) {
        match self {
            TuiBackend::Primary { hub } => hub.send(slot, cmd),
            TuiBackend::Attach { cmd_tx, .. } => {
                let slot_label = match slot {
                    Slot::Sync => "sync-server",
                    Slot::Web => "web",
                };
                let cmd_label = match cmd {
                    SvcCmd::Start => "start",
                    SvcCmd::Stop => "stop",
                    SvcCmd::Restart => "restart",
                };
                let _ = cmd_tx.send(format!(
                    "{{\"slot\":\"{slot_label}\",\"cmd\":\"{cmd_label}\"}}"
                ));
            }
        }
    }

    fn is_attach(&self) -> bool {
        matches!(self, TuiBackend::Attach { .. })
    }
}

pub struct UiContext {
    pub dash: Arc<Dash>,
    pub host: HostStatic,
    pub version: &'static str,
    pub sync_addr: String,
    pub web_port: u16,
}

pub async fn run_tui(
    ctx: UiContext,
    backend: TuiBackend,
    kill_tx: watch::Sender<bool>,
) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let result = ui_loop(&mut terminal, &ctx, &backend, &kill_tx).await;
    ratatui::restore();
    result
}

async fn ui_loop(
    terminal: &mut ratatui::DefaultTerminal,
    ctx: &UiContext,
    backend: &TuiBackend,
    kill_tx: &watch::Sender<bool>,
) -> anyhow::Result<()> {
    let slots = [Slot::Sync, Slot::Web];
    let mut selection = 0usize;
    loop {
        // The primary going away ends an attached TUI (its Dash would freeze
        // mid-action); the in-process TUI ends via the kill watch as before.
        if let TuiBackend::Attach { closed, .. } = backend {
            if closed.load(Ordering::SeqCst) {
                return Ok(());
            }
        }
        let sel = slots[selection];
        terminal.draw(|frame| draw(frame, ctx, backend, sel))?;
        // Redraw at ~4fps — plenty for 2s-period metrics, cheap to render.
        if poll(Duration::from_millis(250))? {
            if let Event::Key(key) = read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        if backend.is_attach() {
                            // Disconnect observation only.
                            return Ok(());
                        }
                        let _ = kill_tx.send(true);
                        return Ok(());
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if backend.is_attach() {
                            return Ok(());
                        }
                        let _ = kill_tx.send(true);
                        return Ok(());
                    }
                    KeyCode::Up | KeyCode::Down | KeyCode::Tab => {
                        selection = 1 - selection;
                    }
                    KeyCode::Char('s') => backend.apply(sel, SvcCmd::Start),
                    KeyCode::Char('x') => backend.apply(sel, SvcCmd::Stop),
                    KeyCode::Char('r') => backend.apply(sel, SvcCmd::Restart),
                    _ => {}
                }
            }
        }
    }
}

fn phase_style(phase: Phase) -> Style {
    match phase {
        Phase::Running => Style::new().fg(Color::Green),
        Phase::Starting => Style::new().fg(Color::Blue),
        Phase::Restarting => Style::new().fg(Color::Yellow),
        Phase::CircuitOpen => Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        Phase::Halted => Style::new().fg(Color::Magenta),
        Phase::Stopped => Style::new().fg(Color::DarkGray),
    }
}

fn state_cell(st: &crate::state::ServiceStatus) -> Span<'static> {
    let mut label = st.phase.label().to_string();
    if st.phase == Phase::Restarting {
        if let Some(at) = st.restart_at {
            let remain = at.saturating_duration_since(Instant::now());
            label = format!("{} in {}", label, human_duration(remain));
        }
    }
    Span::styled(label, phase_style(st.phase))
}

fn draw(frame: &mut Frame, ctx: &UiContext, backend: &TuiBackend, selected: Slot) {
    let outer = Layout::vertical([
        Constraint::Length(5),
        Constraint::Length(7),
        Constraint::Min(6),
        Constraint::Length(1),
    ])
    .split(frame.area());

    draw_header(frame, ctx, backend, outer[0]);
    draw_table(frame, ctx, selected, outer[1]);
    draw_logs(frame, ctx, outer[2]);
    draw_footer(frame, backend, outer[3]);
}

fn draw_header(
    frame: &mut Frame,
    ctx: &UiContext,
    backend: &TuiBackend,
    area: ratatui::layout::Rect,
) {
    let dash = &ctx.dash;
    let push = dash.push.lock().unwrap_or_else(|e| e.into_inner());
    let push_line = if !push.enabled {
        Line::from(vec![
            "metrics push  ".into(),
            Span::styled(
                "disabled (ADMIN_TOKEN not set)",
                Style::new().fg(Color::DarkGray),
            ),
        ])
    } else {
        match (&push.last_ok, &push.last_error) {
            (Some(ok), None) => Line::from(vec![
                "metrics push  ".into(),
                Span::styled(
                    format!("ok {}s ago", ok.elapsed().as_secs()),
                    Style::new().fg(Color::Green),
                ),
            ]),
            (_, err) => {
                let err = err.clone().unwrap_or_else(|| "never sent".into());
                Line::from(vec![
                    "metrics push  ".into(),
                    Span::styled(format!("error: {err}"), Style::new().fg(Color::Yellow)),
                ])
            }
        }
    };
    drop(push);

    let title = if backend.is_attach() {
        " supervisor (attached) "
    } else {
        " supervisor "
    };
    let lines = vec![
        Line::from(vec![
            format!("Readest Self-Host Supervisor v{} ", ctx.version).bold(),
            Span::raw(format!(
                "· up {} · {} · {} cores · {} RAM",
                human_duration(dash.started_elapsed()),
                ctx.host.os,
                ctx.host.cores,
                human_bytes(ctx.host.total_memory_bytes)
            )),
        ]),
        Line::from(vec![
            "sync  ".into(),
            Span::styled(
                format!("http://{}", ctx.sync_addr),
                Style::new().fg(Color::Cyan),
            ),
            "   web  ".into(),
            Span::styled(
                format!("http://127.0.0.1:{}", ctx.web_port),
                Style::new().fg(Color::Cyan),
            ),
        ]),
        push_line,
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title)),
        area,
    );
}

fn draw_table(frame: &mut Frame, ctx: &UiContext, selected: Slot, area: ratatui::layout::Rect) {
    let header = Row::new([
        "service", "state", "intent", "uptime", "restarts", "pid", "memory", "cpu", "probe",
    ])
    .style(Style::new().fg(Color::Yellow).bold());
    let mut rows = Vec::with_capacity(2);
    for slot in [Slot::Sync, Slot::Web] {
        let st = ctx.dash.status(slot);
        let probe = match &st.probe {
            Some(p) if p.ok => Span::styled(
                format!("{}ms (HTTP {})", p.rtt.as_millis(), p.status),
                Style::new().fg(Color::Green),
            ),
            Some(p) => Span::styled(
                if p.status == 0 {
                    "no answer".to_string()
                } else {
                    format!("HTTP {} (unexpected)", p.status)
                },
                Style::new().fg(Color::Red),
            ),
            None => Span::raw("—"),
        };
        let row_style = if slot == selected {
            Style::new().bg(Color::Rgb(48, 48, 64))
        } else {
            Style::new()
        };
        rows.push(
            Row::new(vec![
                Span::raw(slot.name().to_string()),
                state_cell(&st),
                Span::styled(
                    st.desired.label().to_string(),
                    if st.desired == crate::state::Desired::Halt {
                        Style::new().fg(Color::Magenta)
                    } else {
                        Style::new()
                    },
                ),
                Span::raw(
                    st.uptime()
                        .map(human_duration)
                        .unwrap_or_else(|| "—".into()),
                ),
                Span::raw(st.restarts.to_string()),
                Span::raw(st.pid.map(|p| p.to_string()).unwrap_or_else(|| "—".into())),
                Span::raw(human_bytes(st.mem_bytes)),
                Span::raw(format!("{:.1}%", st.cpu_percent)),
                probe,
            ])
            .style(row_style),
        );
    }
    let table = Table::new(
        rows,
        [
            Constraint::Length(13),
            Constraint::Length(18),
            Constraint::Length(7),
            Constraint::Length(10),
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Length(9),
            Constraint::Length(7),
            Constraint::Length(20),
        ],
    )
    .header(header)
    .block(Block::bordered().title(" services "));
    frame.render_widget(table, area);
}

fn draw_logs(frame: &mut Frame, ctx: &UiContext, area: ratatui::layout::Rect) {
    if area.width < 4 || area.height < 3 {
        return;
    }
    let cols =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    for (slot, rect) in [(Slot::Sync, cols[0]), (Slot::Web, cols[1])] {
        let st = ctx.dash.status(slot);
        let height = rect.height.saturating_sub(2) as usize; // borders
        let start = st.tail.len().saturating_sub(height);
        let lines: Vec<Line> = st
            .tail
            .iter()
            .skip(start)
            .map(|l| Line::from(l.clone()))
            .collect();
        frame.render_widget(
            Paragraph::new(lines).block(Block::bordered().title(format!(" {} log ", slot.name()))),
            rect,
        );
    }
}

fn draw_footer(frame: &mut Frame, backend: &TuiBackend, area: ratatui::layout::Rect) {
    let key = |k: &str| {
        Span::styled(
            format!(" {k} "),
            Style::new().fg(Color::Black).bg(Color::White),
        )
    };
    let spans = if backend.is_attach() {
        vec![
            key("↑↓"),
            " select   ".into(),
            key("s"),
            " start   ".into(),
            key("x"),
            " stop   ".into(),
            key("r"),
            " restart   ".into(),
            key("q"),
            " detach (services keep running)   full logs: ./logs/".into(),
        ]
    } else {
        vec![
            key("↑↓"),
            " select   ".into(),
            key("s"),
            " start   ".into(),
            key("x"),
            " stop   ".into(),
            key("r"),
            " restart   ".into(),
            key("q"),
            " quit (stops both services)   closing this window stops everything   logs: ./logs/"
                .into(),
        ]
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
