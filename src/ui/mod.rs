mod chart;
pub mod context_menu;
mod drilldown;
mod processes;
mod recommendations;
pub mod sidebar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;

pub fn render(f: &mut Frame, app: &mut App) {
    let size = f.area();
    app.term_size = (size.width, size.height);

    let search_h = if app.search_active || !app.search_query.is_empty() {
        1
    } else {
        0
    };
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(search_h),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(size);

    render_header(f, outer[0], app);
    if search_h == 1 {
        render_search_bar(f, outer[1], app);
    }
    render_body(f, outer[2], app);
    render_footer(f, outer[3], app);

    if app.show_help {
        render_help(f, size);
    }
    if app.drilldown_pid.is_some() {
        drilldown::render(f, size, app);
    }
    if app.kill_menu.is_some() {
        render_kill_menu(f, size, app);
    }
    if app.context_menu.is_some() {
        context_menu::render(f, size, app);
    }
    if app.show_thermal {
        render_thermal_overlay(f, size, app);
    }
}

fn render_thermal_overlay(f: &mut Frame, area: Rect, app: &App) {
    use crate::thermal::SensorKind;
    let sensors: Vec<_> = app
        .thermal
        .as_ref()
        .map(|t| t.sensors.clone())
        .unwrap_or_default();

    let w = 60u16.min(area.width.saturating_sub(4));
    let h = (sensors.len() as u16 + 8).clamp(12, area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);

    let mut lines: Vec<Line<'static>> = Vec::new();
    if sensors.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no thermal sensors available",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        // Group by kind for readability.
        let groups: [(SensorKind, &str); 6] = [
            (SensorKind::Cpu, "cpu"),
            (SensorKind::Gpu, "gpu"),
            (SensorKind::Ane, "neural"),
            (SensorKind::Memory, "memory"),
            (SensorKind::Battery, "battery"),
            (SensorKind::Other, "other"),
        ];
        for (kind, label) in groups.iter() {
            let members: Vec<_> = sensors.iter().filter(|s| s.kind == *kind).collect();
            if members.is_empty() {
                continue;
            }
            lines.push(Line::from(Span::styled(
                format!(" {}", label),
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            )));
            for s in members {
                let name_w = (w as usize).saturating_sub(14);
                let name = truncate_label(&s.label, name_w);
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        pad_right_s(&name, name_w.saturating_sub(2)),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(
                        format!("  {:>5.1}°C", s.celsius),
                        Style::default().fg(temp_color(s.celsius)),
                    ),
                ]));
            }
            lines.push(Line::from(""));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightCyan))
        .title(Span::styled(
            " thermal sensors ",
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            " j/k scroll · esc close ",
            Style::default().fg(Color::DarkGray),
        ));

    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(block).scroll((app.thermal_scroll, 0)),
        rect,
    );
}

fn pad_right_s(s: &str, width: usize) -> String {
    let c = s.chars().count();
    if c >= width {
        return s.to_string();
    }
    let mut o = s.to_string();
    for _ in 0..(width - c) {
        o.push(' ');
    }
    o
}

fn render_kill_menu(f: &mut Frame, area: Rect, app: &App) {
    let Some(menu) = app.kill_menu.as_ref() else {
        return;
    };
    let w = 48u16.min(area.width.saturating_sub(4));
    let h = 16u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);

    let lines = vec![
        Line::from(Span::styled(
            crate::app::kill_menu_title(&menu.targets, &truncate_label(&menu.name, 28)),
            Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        signal_row("t", "TERM", "graceful, default"),
        signal_row("k", "KILL", "forceful, unblockable"),
        signal_row("h", "HUP", "hangup"),
        signal_row("i", "INT", "interrupt (^C)"),
        signal_row("s", "STOP", "pause process"),
        signal_row("c", "CONT", "resume after STOP"),
        signal_row("q", "QUIT", "core-dump quit"),
        signal_row("1", "USR1", "user-defined 1"),
        signal_row("2", "USR2", "user-defined 2"),
        Line::from(""),
        Line::from(Span::styled(
            " enter sends TERM · esc cancels",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightYellow))
        .title(Span::styled(
            " send signal ",
            Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn signal_row(key: &str, sig: &str, note: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("[{}]", key),
            Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{:<5}", sig),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            format!(" {}", note),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

fn truncate_label(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept)
}

fn render_search_bar(f: &mut Frame, area: Rect, app: &App) {
    let label = if app.search_active { " / " } else { " (filter) " };
    let label_style = if app.search_active {
        Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    };
    let cursor = if app.search_active { "▎" } else { "" };
    let count_text = if !app.search_query.is_empty() {
        let procs = app
            .fuzzy_pids
            .as_ref()
            .map(|s| s.len())
            .unwrap_or(0);
        let buckets = app
            .fuzzy_bucket_labels
            .as_ref()
            .map(|s| s.len())
            .unwrap_or(0);
        format!("   {} procs · {} projects", procs, buckets)
    } else {
        String::new()
    };
    let hint = if !app.search_active {
        "   enter=commit · esc=clear"
    } else {
        ""
    };
    let line = Line::from(vec![
        Span::styled(label, label_style),
        Span::raw(" "),
        Span::styled(app.search_query.clone(), Style::default().fg(Color::White)),
        Span::styled(cursor, Style::default().fg(Color::Cyan).add_modifier(Modifier::RAPID_BLINK)),
        Span::styled(count_text, Style::default().fg(Color::LightCyan)),
        Span::styled(hint, Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(line), area);
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
        Line::from("  enter            drill-down modal on selected process"),
        Line::from("  K                kill: signal menu (TERM/KILL/HUP/INT/…)"),
        Line::from("  c / m / n / w    sort procs by cpu / mem / name / network"),
        Line::from("  /                fuzzy filter (nucleo)"),
        Line::from("  space            pause sampling"),
        Line::from("  [  /  ]          faster / slower sampling (250ms step)"),
        Line::from("  H / U / S        toggle kernel / only-mine / hide-self"),
        Line::from("  T                thermal sensor overlay"),
        Line::from("  esc              clear filter / close overlay / quit"),
        Line::from("  ?                toggle this overlay"),
        Line::from("  q / ^C           quit"),
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

    let mut spans: Vec<Span> = vec![
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
    ];
    if let Some(t) = app.thermal.as_ref() {
        use crate::thermal::SensorKind;
        let cpu = t.max_of(SensorKind::Cpu);
        let gpu = t.max_of(SensorKind::Gpu);
        if cpu.is_some() || gpu.is_some() {
            spans.push(Span::styled("therm ", Style::default().fg(Color::DarkGray)));
            if let Some(c) = cpu {
                spans.push(Span::styled(
                    format!("cpu {:>4.1}° ", c),
                    Style::default().fg(temp_color(c)),
                ));
            }
            if let Some(g) = gpu {
                spans.push(Span::styled(
                    format!("gpu {:>4.1}°", g),
                    Style::default().fg(temp_color(g)),
                ));
            }
            spans.push(Span::raw("   "));
        }
    }
    if let Some(r) = app.net.as_ref() {
        spans.push(Span::styled("net ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            format!("↑{:>8}  ", fmt_rate(r.tx_bytes_per_sec)),
            Style::default().fg(rate_color(r.tx_bytes_per_sec)),
        ));
        spans.push(Span::styled(
            format!("↓{:>8}", fmt_rate(r.rx_bytes_per_sec)),
            Style::default().fg(rate_color(r.rx_bytes_per_sec)),
        ));
        spans.push(Span::raw("   "));
    }
    spans.extend(vec![
        Span::styled("procs ", Style::default().fg(Color::DarkGray)),
        Span::raw(format!("{}   ", proc_count)),
        Span::styled("recs ", Style::default().fg(Color::DarkGray)),
        Span::styled(src_tag, Style::default().fg(Color::Magenta)),
        Span::raw("   "),
        Span::styled(
            if app.is_paused() { "⏸ paused " } else { "" },
            Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{}ms", app.sample_interval_ms()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  "),
        Span::styled(
            filter_badges(app),
            Style::default().fg(Color::LightMagenta),
        ),
    ]);
    let line = Line::from(spans);
    f.render_widget(Paragraph::new(line), area);
}

fn filter_badges(app: &App) -> String {
    let mut s = String::new();
    if app.hide_kernel {
        s.push_str("[kern-hid]");
    }
    if app.only_my_uid {
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str("[mine]");
    }
    if app.hide_self {
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str("[self-hid]");
    }
    s
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

fn fmt_rate(bps: f64) -> String {
    let b = bps.max(0.0);
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1}G/s", b / 1024.0 / 1024.0 / 1024.0)
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.1}M/s", b / 1024.0 / 1024.0)
    } else if b >= 1024.0 {
        format!("{:.0}K/s", b / 1024.0)
    } else {
        format!("{:.0}B/s", b)
    }
}

fn rate_color(bps: f64) -> Color {
    let mb = bps / 1024.0 / 1024.0;
    if mb >= 50.0 {
        Color::Red
    } else if mb >= 10.0 {
        Color::LightRed
    } else if mb >= 1.0 {
        Color::Yellow
    } else if mb >= 0.01 {
        Color::Green
    } else {
        Color::DarkGray
    }
}

fn temp_color(c: f32) -> Color {
    if c >= 95.0 {
        Color::Red
    } else if c >= 80.0 {
        Color::LightRed
    } else if c >= 65.0 {
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

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
    if let Some(msg) = app.status_text() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {} ", msg),
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
            ))),
            area,
        );
        return;
    }
    let hint = if app.search_active {
        "type to filter · enter commit · esc cancel · ↑↓ nav"
    } else {
        "j/k nav · enter drill · x menu · K kill · c/m/n/w sort · T therm · / search · space pause · ? help · q quit"
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray)))),
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
