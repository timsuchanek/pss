use std::collections::{HashMap, HashSet};

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Cell, Row, Table, TableState};

use crate::app::{App, Pane};

use super::titled_block;

const SPARK_WIDTH: usize = 14;

pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let active = app.pane == Pane::Processes;
    let title = format!("processes — sort {}", app.sort.label());
    let block = titled_block(&title, active);

    let procs = app.procs_in_selected();
    let pids: HashSet<u32> = procs.iter().map(|p| p.pid).collect();
    let histories = cpu_histories(app, &pids, SPARK_WIDTH);

    let rows: Vec<Row> = procs
        .iter()
        .map(|p| {
            let mem_mb = p.mem / 1024 / 1024;
            let sparkline = histories
                .get(&p.pid)
                .map(|v| spark(v, SPARK_WIDTH))
                .unwrap_or_else(|| " ".repeat(SPARK_WIDTH));
            Row::new(vec![
                Cell::from(format!("{:>6}", p.pid)),
                Cell::from(Span::styled(
                    format!("{:>5.1}", p.cpu),
                    Style::default().fg(cpu_color(p.cpu)),
                )),
                Cell::from(Span::styled(
                    sparkline,
                    Style::default().fg(Color::Cyan),
                )),
                Cell::from(format!("{:>5}M", mem_mb)),
                Cell::from(truncate(&p.name, 18)),
                Cell::from(Span::styled(
                    p.cmd.clone(),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
        })
        .collect();

    let header = Row::new(vec!["pid", "cpu%", "last 60s", "mem", "name", "cmd"])
        .style(Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD));

    let widths = [
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(SPARK_WIDTH as u16),
        Constraint::Length(6),
        Constraint::Length(19),
        Constraint::Min(10),
    ];

    let mut state = TableState::default();
    if let Some(pid) = app.selected_pid() {
        let idx = procs.iter().position(|p| p.pid == pid);
        state.select(idx);
    }

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    f.render_stateful_widget(table, area, &mut state);
}

fn cpu_color(cpu: f32) -> Color {
    if cpu >= 90.0 {
        Color::Red
    } else if cpu >= 60.0 {
        Color::LightRed
    } else if cpu >= 20.0 {
        Color::Yellow
    } else if cpu >= 1.0 {
        Color::Green
    } else {
        Color::DarkGray
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept)
}

fn cpu_histories(app: &App, pids: &HashSet<u32>, width: usize) -> HashMap<u32, Vec<f32>> {
    let buf = &app.history.buf;
    let take_n = width.min(buf.len());
    let skip = buf.len() - take_n;
    let mut out: HashMap<u32, Vec<f32>> = pids
        .iter()
        .map(|pid| (*pid, Vec::with_capacity(take_n)))
        .collect();
    for snap in buf.iter().skip(skip) {
        let mut seen: HashSet<u32> = HashSet::with_capacity(pids.len());
        for p in &snap.procs {
            if let Some(v) = out.get_mut(&p.pid) {
                v.push(p.cpu);
                seen.insert(p.pid);
            }
        }
        for pid in pids {
            if !seen.contains(pid) {
                if let Some(v) = out.get_mut(pid) {
                    v.push(0.0);
                }
            }
        }
    }
    out
}

fn spark(values: &[f32], width: usize) -> String {
    const BARS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if width == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(width);
    let pad = width.saturating_sub(values.len());
    for _ in 0..pad {
        out.push(' ');
    }
    let local_max = values.iter().copied().fold(0.0f32, f32::max).max(1.0);
    let offset = values.len().saturating_sub(width);
    for v in &values[offset..] {
        let n = (v.max(0.0) / local_max * (BARS.len() - 1) as f32).round() as usize;
        out.push(BARS[n.min(BARS.len() - 1)]);
    }
    out
}
