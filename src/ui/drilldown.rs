use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, DrilldownTab};
use crate::collector::ProcSample;

pub fn render(f: &mut Frame, screen: Rect, app: &App) {
    let Some(p) = app.drilldown_proc().cloned() else {
        return;
    };

    let w = (screen.width as u32 * 80 / 100).clamp(70, 140) as u16;
    let h = (screen.height as u32 * 80 / 100).clamp(24, 46) as u16;
    let x = screen.x + (screen.width.saturating_sub(w)) / 2;
    let y = screen.y + (screen.height.saturating_sub(h)) / 2;
    let area = Rect::new(x, y, w, h);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            format!(" process · [{}] {} ", p.pid, truncate(&p.name, 32)),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            " K kill · r refresh · h/l tabs · j/k scroll · esc close ",
            Style::default().fg(Color::DarkGray),
        ));

    f.render_widget(Clear, area);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 8 || inner.width < 30 {
        return;
    }

    // Layout: tab bar (1) + content (rest).
    let slots = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(2)])
        .split(inner);

    render_tab_bar(f, slots[0], app);

    let content = slots[1];
    let lines: Vec<Line<'static>> = match app.drilldown_tab {
        DrilldownTab::Facts => body_facts(&p, app, content.width as usize),
        DrilldownTab::Env => body_env(app, content.width as usize),
        DrilldownTab::Files => body_files(app, content.width as usize),
        DrilldownTab::Sockets => body_sockets(app, content.width as usize),
        DrilldownTab::Tree => body_tree(&p, app, content.width as usize),
    };

    let scroll = app.drilldown_scroll;
    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    f.render_widget(para, content);
}

fn render_tab_bar(f: &mut Frame, area: Rect, app: &App) {
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::raw(" "));
    for (i, tab) in DrilldownTab::ALL.iter().enumerate() {
        let is_active = *tab == app.drilldown_tab;
        let style = if is_active {
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(
            format!(" {} {} ", i + 1, tab.label()),
            style,
        ));
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// --- tab: facts --------------------------------------------------------

fn body_facts(p: &ProcSample, app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();

    out.push(section("command"));
    for row in wrap_cmd(&p.cmd, width.saturating_sub(2)) {
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(row, Style::default().fg(Color::White)),
        ]));
    }
    out.push(Line::from(""));

    // Resource sparklines
    out.push(section("resources (last 60s)"));
    let spark_w = width.saturating_sub(30).max(10);
    let cpu_hist = app.pid_cpu_history(p.pid, spark_w);
    let mem_hist = app.pid_mem_history(p.pid, spark_w);
    let cpu_peak = cpu_hist.iter().copied().fold(0.0f32, f32::max);
    let mem_peak_mb = mem_hist.iter().copied().fold(0.0f32, f32::max);

    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("cpu  ", Style::default().fg(Color::DarkGray)),
        Span::styled(spark(&cpu_hist, spark_w), Style::default().fg(Color::Cyan)),
        Span::styled(
            format!("  now {:>5.1}%   peak {:>5.1}%", p.cpu, cpu_peak),
            Style::default().fg(Color::Gray),
        ),
    ]));
    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("mem  ", Style::default().fg(Color::DarkGray)),
        Span::styled(spark(&mem_hist, spark_w), Style::default().fg(Color::LightMagenta)),
        Span::styled(
            format!(
                "  rss {}   peak {}",
                fmt_bytes(p.mem),
                fmt_mb(mem_peak_mb)
            ),
            Style::default().fg(Color::Gray),
        ),
    ]));
    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("virt ", Style::default().fg(Color::DarkGray)),
        Span::styled(fmt_bytes(p.virt), Style::default().fg(Color::White)),
        Span::raw("    "),
        Span::styled("io   ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!(
                "read {}   write {}",
                fmt_bytes(p.io_read),
                fmt_bytes(p.io_write)
            ),
            Style::default().fg(Color::White),
        ),
    ]));
    out.push(Line::from(""));

    // Facts
    out.push(section("facts"));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let uptime = now.saturating_sub(p.started_at).max(p.run_time_secs);
    let started_rel = fmt_started(p.started_at);
    let parent_name = p
        .ppid
        .and_then(|ppid| app.process_by_pid(ppid))
        .map(|pp| pp.name.clone())
        .unwrap_or_else(|| "-".to_string());
    let ppid_str = p
        .ppid
        .map(|x| format!("{} ({})", x, parent_name))
        .unwrap_or_else(|| "-".into());
    let user_str = p
        .uid
        .map(|u| format!("uid {}", u))
        .unwrap_or_else(|| "-".into());

    let details = app.drilldown_details.as_ref();
    let fds_str = details
        .and_then(|d| d.fds_count)
        .map(|n| n.to_string())
        .unwrap_or_else(|| "…".into());
    let threads_str = details
        .and_then(|d| d.threads_count)
        .map(|n| n.to_string())
        .unwrap_or_else(|| "…".into());
    let nice_str = details
        .and_then(|d| d.nice)
        .map(|n| n.to_string())
        .unwrap_or_else(|| "…".into());

    let facts: Vec<(&str, String)> = vec![
        ("name", p.name.clone()),
        ("pid", p.pid.to_string()),
        ("ppid", ppid_str),
        ("user", user_str),
        ("status", p.status.clone()),
        ("threads", threads_str),
        ("fds", fds_str),
        ("nice", nice_str),
        ("uptime", format_duration(uptime)),
        ("started", started_rel),
    ];
    let col_w = width / 2;
    for pair in facts.chunks(2) {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (k, v) in pair {
            let cell = format!("{:<9}{}", k, truncate(v, col_w.saturating_sub(12)));
            spans.push(Span::styled(
                format!("  {}", pad_right(&cell, col_w.saturating_sub(2))),
                Style::default().fg(Color::White),
            ));
        }
        out.push(Line::from(spans));
    }
    out.push(kv(
        "cwd",
        &p.cwd
            .as_ref()
            .map(display_path)
            .unwrap_or_else(|| "-".into()),
        width,
    ));
    if let Some(exe) = p.exe.as_ref() {
        out.push(kv("exe", &exe.display().to_string(), width));
    }

    out
}

// --- tab: env ----------------------------------------------------------

fn body_env(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let Some(d) = app.drilldown_details.as_ref() else {
        out.push(note("loading…"));
        return out;
    };
    let Some(env) = d.env.as_ref() else {
        out.push(note("loading…"));
        return out;
    };
    if env.is_empty() {
        out.push(note("no environment visible (different uid or not yet started)"));
        return out;
    }
    out.push(section(&format!("environment ({} vars)", env.len())));
    for e in env {
        let key_w = 26.min(width.saturating_sub(2));
        let val_w = width.saturating_sub(key_w + 4);
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                pad_right(&truncate(&e.key, key_w), key_w),
                Style::default().fg(Color::LightYellow),
            ),
            Span::raw("  "),
            Span::styled(
                truncate(&e.value, val_w),
                Style::default().fg(Color::White),
            ),
        ]));
    }
    out
}

// --- tab: files --------------------------------------------------------

fn body_files(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let Some(d) = app.drilldown_details.as_ref() else {
        out.push(note("loading…"));
        return out;
    };
    let Some(files) = d.files.as_ref() else {
        out.push(note("loading… (spawning lsof)"));
        return out;
    };
    if files.is_empty() {
        out.push(note("no open files (or lsof missing)"));
        return out;
    }
    out.push(section(&format!("open files ({})", files.len())));
    for f in files {
        let left = format!(" {:>5}  {:<5}  ", f.fd, f.kind);
        let path_w = width.saturating_sub(left.chars().count() + 2);
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(left, Style::default().fg(Color::DarkGray)),
            Span::styled(
                truncate(&f.path, path_w),
                Style::default().fg(Color::White),
            ),
        ]));
    }
    out
}

// --- tab: sockets ------------------------------------------------------

fn body_sockets(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let Some(d) = app.drilldown_details.as_ref() else {
        out.push(note("loading…"));
        return out;
    };
    let Some(socks) = d.sockets.as_ref() else {
        out.push(note("loading… (spawning lsof)"));
        return out;
    };
    if socks.is_empty() {
        out.push(note("no open sockets"));
        return out;
    }
    out.push(section(&format!("sockets ({})", socks.len())));
    for s in socks {
        let state_badge = if s.state.is_empty() {
            String::new()
        } else {
            format!(" [{}]", s.state)
        };
        let local = truncate(&s.local, 28);
        let remote_part = if s.remote.is_empty() {
            String::new()
        } else {
            format!(" → {}", truncate(&s.remote, 28))
        };
        let _ = width;
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                pad_right(&s.proto, 5),
                Style::default().fg(Color::LightCyan),
            ),
            Span::raw(" "),
            Span::styled(pad_right(&local, 30), Style::default().fg(Color::White)),
            Span::styled(remote_part, Style::default().fg(Color::Gray)),
            Span::styled(state_badge, Style::default().fg(state_color(&s.state))),
        ]));
    }
    out
}

fn state_color(state: &str) -> Color {
    match state {
        "LISTEN" => Color::LightGreen,
        "ESTABLISHED" => Color::Cyan,
        "CLOSE_WAIT" | "FIN_WAIT_1" | "FIN_WAIT_2" | "TIME_WAIT" => Color::Yellow,
        _ => Color::DarkGray,
    }
}

// --- tab: tree ---------------------------------------------------------

fn body_tree(p: &ProcSample, app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();

    // Ancestors: from init down to this process.
    let mut ancestors = app.ancestors_of(p.pid);
    ancestors.reverse(); // now root-first

    out.push(section("ancestors"));
    for (i, a) in ancestors.iter().enumerate() {
        let indent = "  ".repeat(i);
        out.push(Line::from(vec![
            Span::raw(format!("  {}", indent)),
            Span::styled(
                format!("[{}] ", a.pid),
                Style::default().fg(Color::LightYellow),
            ),
            Span::styled(
                truncate(&a.name, width.saturating_sub(indent.len() + 10)),
                Style::default().fg(Color::White),
            ),
        ]));
    }
    // The selected process at bottom of ancestors
    let depth = ancestors.len();
    let indent = "  ".repeat(depth);
    out.push(Line::from(vec![
        Span::raw(format!("  {}", indent)),
        Span::styled(
            format!("[{}] ", p.pid),
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            truncate(&p.name, width.saturating_sub(indent.len() + 10)),
            Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  (here)", Style::default().fg(Color::DarkGray)),
    ]));

    // Children + grandchildren
    let children = app.children_of(p.pid);
    out.push(Line::from(""));
    out.push(section(&format!("descendants (direct: {})", children.len())));
    for c in &children {
        let indent = "  ".repeat(depth + 1);
        out.push(Line::from(vec![
            Span::raw(format!("  {}", indent)),
            Span::styled(
                format!("[{}] ", c.pid),
                Style::default().fg(Color::LightYellow),
            ),
            Span::styled(
                pad_right(
                    &truncate(&c.name, 20),
                    21,
                ),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                format!("{:>6.1}%   {}", c.cpu, fmt_bytes(c.mem)),
                Style::default().fg(Color::Gray),
            ),
        ]));
        let grand = app.children_of(c.pid);
        for gc in grand.iter().take(4) {
            let gindent = "  ".repeat(depth + 2);
            out.push(Line::from(vec![
                Span::raw(format!("  {}", gindent)),
                Span::styled(
                    format!("[{}] ", gc.pid),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    truncate(&gc.name, 20),
                    Style::default().fg(Color::Gray),
                ),
            ]));
        }
        if grand.len() > 4 {
            let gindent = "  ".repeat(depth + 2);
            out.push(Line::from(vec![
                Span::raw(format!("  {}", gindent)),
                Span::styled(
                    format!("… +{} more", grand.len() - 4),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }
    out
}

// --- helpers -----------------------------------------------------------

fn kv(key: &str, value: &str, width: usize) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            pad_right(key, 9),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            truncate(value, width.saturating_sub(12)),
            Style::default().fg(Color::White),
        ),
    ])
}

fn section(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {}", title),
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
    ))
}

fn note(msg: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {}", msg),
        Style::default().fg(Color::DarkGray),
    ))
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

fn wrap_cmd(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let end = (i + width).min(chars.len());
        out.push(chars[i..end].iter().collect());
        i = end;
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept)
}

fn pad_right(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count >= width {
        return s.to_string();
    }
    let mut out = s.to_string();
    for _ in 0..(width - count) {
        out.push(' ');
    }
    out
}

fn display_path(p: &std::path::PathBuf) -> String {
    if let Ok(home) = std::env::var("HOME") {
        if let Ok(rel) = p.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    p.display().to_string()
}

fn format_duration(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if d > 0 {
        format!("{}d {:02}:{:02}:{:02}", d, h, m, s)
    } else if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}

fn fmt_started(secs: u64) -> String {
    use std::time::{Duration, UNIX_EPOCH};
    let then = UNIX_EPOCH + Duration::from_secs(secs);
    let now = std::time::SystemTime::now();
    match now.duration_since(then) {
        Ok(ago) => format!("{} ago", format_duration(ago.as_secs())),
        Err(_) => format!("@{}s", secs),
    }
}

fn fmt_bytes(b: u64) -> String {
    let bf = b as f32;
    if bf >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1}G", bf / 1024.0 / 1024.0 / 1024.0)
    } else if bf >= 1024.0 * 1024.0 {
        format!("{:.0}M", bf / 1024.0 / 1024.0)
    } else if bf >= 1024.0 {
        format!("{:.0}K", bf / 1024.0)
    } else {
        format!("{}B", b)
    }
}

fn fmt_mb(mb: f32) -> String {
    if mb >= 1024.0 {
        format!("{:.1}G", mb / 1024.0)
    } else {
        format!("{:.0}M", mb)
    }
}
