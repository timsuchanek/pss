mod chart;
mod processes;
mod recommendations;
mod sidebar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;

pub fn render(f: &mut Frame, app: &mut App) {
    let size = f.area();

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(size);

    render_header(f, outer[0], app);
    render_body(f, outer[1], app);
    render_footer(f, outer[2], app);

    if app.show_help {
        render_help(f, size);
    }
}

fn render_help(f: &mut Frame, area: Rect) {
    let width = 56.min(area.width.saturating_sub(4));
    let height = 16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    let lines = vec![
        Line::from(Span::styled(
            "  keybindings",
            Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  j / k / ↑ ↓     navigate within pane"),
        Line::from("  h / l / ← →     collapse / expand to right pane"),
        Line::from("  tab             cycle panes"),
        Line::from("  K                SIGTERM selected process or rec"),
        Line::from("  c / m / n        sort procs by cpu / mem / name"),
        Line::from("  ?                toggle this overlay"),
        Line::from("  q / esc / ^C     quit"),
        Line::from(""),
        Line::from(Span::styled(
            "  esc closes this overlay",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " help ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));

    f.render_widget(Clear, popup);
    f.render_widget(Paragraph::new(lines).block(block), popup);
}

fn bar(pct: f32, width: usize) -> String {
    let pct = pct.clamp(0.0, 100.0) / 100.0;
    let filled = (pct * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);
    format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(empty),
    )
}

fn render_header(f: &mut Frame, area: Rect, app: &App) {
    let (total_cpu, mem_used, mem_total, proc_count) = app
        .history
        .latest()
        .map(|s| (s.total_cpu, s.total_mem, s.avail_mem, s.procs.len()))
        .unwrap_or((0.0, 0, 1, 0));

    // Total CPU is summed across all cores; normalize against logical core count.
    let cores = num_cpus();
    let cpu_pct = (total_cpu / cores.max(1) as f32).min(100.0);
    let mem_pct = (mem_used as f32 / mem_total.max(1) as f32) * 100.0;
    let mem_used_gb = mem_used as f32 / 1024.0 / 1024.0 / 1024.0;
    let mem_total_gb = mem_total as f32 / 1024.0 / 1024.0 / 1024.0;

    let src_tag = match app.recs_source {
        crate::app::RecsSource::Llm => "ai",
        crate::app::RecsSource::Local => {
            if app.has_llm {
                "ai·pending"
            } else {
                "local"
            }
        }
    };

    let line = Line::from(vec![
        Span::styled(
            " pss ",
            Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("CPU ", Style::default().fg(Color::DarkGray)),
        Span::styled(bar(cpu_pct, 10), Style::default().fg(cpu_color(cpu_pct))),
        Span::raw(format!(" {:>5.1}%   ", cpu_pct)),
        Span::styled("MEM ", Style::default().fg(Color::DarkGray)),
        Span::styled(bar(mem_pct, 10), Style::default().fg(mem_color(mem_pct))),
        Span::raw(format!(
            " {:>3.0}% ({:.1}/{:.1} GB)   ",
            mem_pct, mem_used_gb, mem_total_gb
        )),
        Span::styled("procs ", Style::default().fg(Color::DarkGray)),
        Span::raw(format!("{}   ", proc_count)),
        Span::styled("recs ", Style::default().fg(Color::DarkGray)),
        Span::styled(src_tag, Style::default().fg(Color::Magenta)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

fn cpu_color(pct: f32) -> Color {
    if pct >= 85.0 {
        Color::Red
    } else if pct >= 60.0 {
        Color::LightRed
    } else if pct >= 30.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn mem_color(pct: f32) -> Color {
    if pct >= 90.0 {
        Color::Red
    } else if pct >= 75.0 {
        Color::LightRed
    } else if pct >= 50.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn render_body(f: &mut Frame, area: Rect, app: &mut App) {
    let sidebar_w = app.sidebar_width.min(area.width.saturating_sub(30));
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(sidebar_w), Constraint::Min(20)])
        .split(area);

    sidebar::render(f, cols[0], app);

    let chart_h = app.chart_height.min(area.height.saturating_sub(14));
    let recs_h = app.recs_height.min(area.height.saturating_sub(14));
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(chart_h),
            Constraint::Min(6),
            Constraint::Length(2),
            Constraint::Length(recs_h),
        ])
        .split(cols[1]);

    // Split chart area: CPU on top, MEM below, each getting roughly half.
    let cpu_h = chart_h / 2 + chart_h % 2; // cpu gets the extra row if odd
    let mem_h = chart_h - cpu_h;
    let charts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(cpu_h), Constraint::Length(mem_h)])
        .split(right[0]);

    chart::render(f, charts[0], app, chart::Metric::Cpu);
    chart::render(f, charts[1], app, chart::Metric::Mem);
    processes::render(f, right[1], app);
    render_detail(f, right[2], app);
    recommendations::render(f, right[3], app);
}

fn render_detail(f: &mut Frame, area: Rect, app: &App) {
    let sel = app.selected_pid().and_then(|pid| {
        app.history
            .latest()
            .and_then(|s| s.procs.iter().find(|p| p.pid == pid).cloned())
    });
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let lines: Vec<Line> = match sel {
        None => vec![
            Line::from(Span::styled(
                "  pick a process with tab+j/k for details",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
        ],
        Some(p) => {
            let uptime = now.saturating_sub(p.started_at);
            let cwd = p
                .cwd
                .as_ref()
                .map(|c| display_path(c))
                .unwrap_or_else(|| "(none)".to_string());
            let cmd = if p.cmd.trim().is_empty() {
                p.name.clone()
            } else {
                p.cmd.clone()
            };
            vec![
                Line::from(vec![
                    Span::styled("▸ ", Style::default().fg(Color::Cyan)),
                    Span::styled(
                        format!("[{}] ", p.pid),
                        Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(truncate(&cmd, 90), Style::default().fg(Color::White)),
                ]),
                Line::from(vec![
                    Span::styled("  cwd ", Style::default().fg(Color::DarkGray)),
                    Span::styled(truncate(&cwd, 50), Style::default().fg(Color::Cyan)),
                    Span::styled(" · ppid ", Style::default().fg(Color::DarkGray)),
                    Span::raw(
                        p.ppid
                            .map(|p| p.to_string())
                            .unwrap_or_else(|| "-".into()),
                    ),
                    Span::styled(" · up ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format_duration(uptime)),
                    Span::styled(" · mem ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}M", p.mem / 1024 / 1024)),
                ]),
            ]
        }
    };

    f.render_widget(Paragraph::new(lines), area);
}

fn display_path(p: &std::path::Path) -> String {
    if let Ok(home) = std::env::var("HOME") {
        if let Ok(rel) = p.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    p.display().to_string()
}

fn format_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept)
}

fn render_footer(f: &mut Frame, area: Rect, _app: &App) {
    let hint = "j/k nav · h/l pane · tab cycle · K kill · c/m/n sort · ? help · q quit";
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
}

pub(crate) fn titled_block(title: &str, active: bool) -> Block<'_> {
    let style = if active {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(format!(" {} ", title), style))
        .border_style(style)
}
