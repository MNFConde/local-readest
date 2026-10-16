//! The dashboard: services table (state/uptime/restarts/PID/memory/CPU/RTT),
//! per-service log tails, and a footer. Read-only by design — the TUI has no
//! way to start, stop or restart anything (ROADMAP 待办⑦ decision: manual
//! per-service control is not a feature; quit is the only action).

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{poll, read, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Row, Table};
use ratatui::Frame;
use tokio::sync::watch;

use crate::snapshot::HostStatic;
use crate::state::{human_bytes, human_duration, Dash, Phase, Slot};

pub struct UiContext {
    pub dash: Arc<Dash>,
    pub host: HostStatic,
    pub version: &'static str,
    pub sync_addr: String,
    pub web_port: u16,
}

pub async fn run_tui(ctx: UiContext, mut kill_tx: watch::Sender<bool>) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let result = ui_loop(&mut terminal, &ctx, &mut kill_tx).await;
    ratatui::restore();
    result
}

async fn ui_loop(
    terminal: &mut ratatui::DefaultTerminal,
    ctx: &UiContext,
    kill_tx: &mut watch::Sender<bool>,
) -> anyhow::Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, ctx))?;
        // Redraw at ~4fps — plenty for 2s-period metrics, cheap to render.
        if poll(Duration::from_millis(250))? {
            if let Event::Key(key) = read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let quit = matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL));
                if quit {
                    let _ = kill_tx.send(true);
                    return Ok(());
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

fn draw(frame: &mut Frame, ctx: &UiContext) {
    let outer = Layout::vertical([
        Constraint::Length(5),
        Constraint::Length(7),
        Constraint::Min(6),
        Constraint::Length(1),
    ])
    .split(frame.area());

    draw_header(frame, ctx, outer[0]);
    draw_table(frame, ctx, outer[1]);
    draw_logs(frame, ctx, outer[2]);
    draw_footer(frame, ctx, outer[3]);
}

fn draw_header(frame: &mut Frame, ctx: &UiContext, area: ratatui::layout::Rect) {
    let dash = &ctx.dash;
    let push = dash.push.lock().unwrap_or_else(|e| e.into_inner());
    let push_line = if !push.enabled {
        Line::from(vec![
            "metrics push  ".into(),
            Span::styled("disabled (ADMIN_TOKEN not set)", Style::new().fg(Color::DarkGray)),
        ])
    } else {
        match (&push.last_ok, &push.last_error) {
            (Some(ok), None) => Line::from(vec![
                "metrics push  ".into(),
                Span::styled(format!("ok {}s ago", ok.elapsed().as_secs()), Style::new().fg(Color::Green)),
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

    let lines = vec![
        Line::from(vec![
            format!("Readest Self-Host Supervisor v{} ", ctx.version).bold(),
            Span::raw(format!(
                "· up {} · {} · {} cores · {} RAM",
                human_duration(dash.started.elapsed()),
                ctx.host.os,
                ctx.host.cores,
                human_bytes(ctx.host.total_memory_bytes)
            )),
        ]),
        Line::from(vec![
            "sync  ".into(),
            Span::styled(format!("http://{}", ctx.sync_addr), Style::new().fg(Color::Cyan)),
            "   web  ".into(),
            Span::styled(format!("http://127.0.0.1:{}", ctx.web_port), Style::new().fg(Color::Cyan)),
        ]),
        push_line,
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" supervisor ")),
        area,
    );
}

fn draw_table(frame: &mut Frame, ctx: &UiContext, area: ratatui::layout::Rect) {
    let header = Row::new([
        "service",
        "state",
        "uptime",
        "restarts",
        "pid",
        "memory",
        "cpu",
        "probe",
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
        rows.push(Row::new(vec![
            Span::raw(slot.name().to_string()),
            state_cell(&st),
            Span::raw(st.uptime().map(human_duration).unwrap_or_else(|| "—".into())),
            Span::raw(st.restarts.to_string()),
            Span::raw(st.pid.map(|p| p.to_string()).unwrap_or_else(|| "—".into())),
            Span::raw(human_bytes(st.mem_bytes)),
            Span::raw(format!("{:.1}%", st.cpu_percent)),
            probe,
        ]));
    }
    let table = Table::new(
        rows,
        [
            Constraint::Length(13),
            Constraint::Length(18),
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
    let cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
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

fn draw_footer(frame: &mut Frame, _ctx: &UiContext, area: ratatui::layout::Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" q ", Style::new().fg(Color::Black).bg(Color::White)),
            " quit (stops both services)   ".into(),
            Span::styled(" ⏎ ", Style::new().fg(Color::Black).bg(Color::White)),
            " closing this window stops everything   full logs: ./logs/".into(),
        ])),
        area,
    );
}
