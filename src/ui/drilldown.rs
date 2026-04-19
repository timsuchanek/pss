use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::App;
use crate::collector::ProcSample;

pub fn render(f: &mut Frame, screen: Rect, app: &App) {
    let Some(p) = app.drilldown_proc().cloned() else {
        return;
    };

    // Modal size: ~80% of screen, capped to reasonable bounds.
    let w = (screen.width as u32 * 80 / 100).clamp(60, 120) as u16;
    let h = (screen.height as u32 * 80 / 100).clamp(20, 36) as u16;
    let x = screen.x + (screen.width.saturating_sub(w)) / 2;
    let y = screen.y + (screen.height.saturating_sub(h)) / 2;
    let area = Rect::new(x, y, w, h);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            format!(" process · [{}] {} ", p.pid, short(&p.name, 24)),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            " K kill · y copy cmd · esc close ",
            Style::default().fg(Color::DarkGray),
        ));

    f.render_widget(Clear, area);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width < 20 || inner.height < 10 {
        return;
    }

    let lines = build_body(&p, app, inner.width as usize);
    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    f.render_widget(para, inner);
}

fn build_body(p: &ProcSample, app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();

    // Section: command (wrapped)
    out.push(Line::from(section("command")));
    for row in wrap_cmd(&p.cmd, width.saturating_sub(2)) {
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(row, Style::default().fg(Color::White)),
        ]));
    }
    out.push(Line::from(""));

    // Section: facts (two columns)
    out.push(Line::from(section("facts")));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let uptime = now.saturating_sub(p.started_at).max(p.run_time_secs);
    let started_utc = fmt_epoch(p.started_at);
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

    let facts: [(&str, String); 8] = [
        ("name", p.name.clone()),
        ("pid", p.pid.to_string()),
        ("ppid", ppid_str),
        ("user", user_str),
        ("status", p.status.clone()),
        ("uptime", format_duration(uptime)),
        ("started", started_utc),
        (
            "cwd",
            p.cwd
                .as_ref()
                .map(display_path)
                .unwrap_or_else(|| "-".into()),
        ),
    ];
    // Two-column render
    let col_w = width / 2;
    for pair in facts.chunks(2) {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (i, (k, v)) in pair.iter().enumerate() {
            let cell = format!("{:<9}{}", k, truncate(v, col_w.saturating_sub(12)));
            spans.push(Span::styled(
                format!("  {}", pad_right(&cell, col_w.saturating_sub(2))),
                if i == 0 {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::White)
                },
            ));
        }
        out.push(Line::from(spans));
    }
    // exe on its own line (often long)
    if let Some(exe) = p.exe.as_ref() {
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("exe      ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                truncate(&exe.display().to_string(), width.saturating_sub(12)),
                Style::default().fg(Color::White),
            ),
        ]));
    }
    out.push(Line::from(""));

    // Section: resources + sparklines
    out.push(Line::from(section("resources (last 60s)")));
    let spark_w = width.saturating_sub(30).max(10);
    let cpu_hist = app.pid_cpu_history(p.pid, spark_w);
    let mem_hist = app.pid_mem_history(p.pid, spark_w);

    let cpu_peak = cpu_hist.iter().copied().fold(0.0f32, f32::max);
    let mem_peak_mb = mem_hist.iter().copied().fold(0.0f32, f32::max);

    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("cpu   ", Style::default().fg(Color::DarkGray)),
        Span::styled(spark(&cpu_hist, spark_w), Style::default().fg(Color::Cyan)),
        Span::styled(
            format!("  now {:>5.1}%   peak {:>5.1}%", p.cpu, cpu_peak),
            Style::default().fg(Color::Gray),
        ),
    ]));
    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("mem   ", Style::default().fg(Color::DarkGray)),
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
        Span::styled("virt  ", Style::default().fg(Color::DarkGray)),
        Span::styled(fmt_bytes(p.virt), Style::default().fg(Color::White)),
    ]));
    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("io    ", Style::default().fg(Color::DarkGray)),
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

    // Section: children
    let children = app.children_of(p.pid);
    out.push(Line::from(section(&format!("children ({})", children.len()))));
    if children.is_empty() {
        out.push(Line::from(Span::styled(
            "  (none)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for c in children.iter().take(8) {
            out.push(Line::from(vec![
                Span::raw("  · "),
                Span::styled(
                    format!("[{}]", c.pid),
                    Style::default().fg(Color::LightYellow),
                ),
                Span::raw(" "),
                Span::styled(
                    pad_right(&truncate(&c.name, 20), 22),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("{:>6.1}%   {}", c.cpu, fmt_bytes(c.mem)),
                    Style::default().fg(Color::Gray),
                ),
            ]));
        }
        if children.len() > 8 {
            out.push(Line::from(Span::styled(
                format!("  (+{} more)", children.len() - 8),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    out
}

fn section(title: &str) -> Vec<Span<'static>> {
    vec![Span::styled(
        format!(" {}", title),
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
    )]
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

fn short(s: &str, max: usize) -> String {
    truncate(s, max)
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

fn fmt_epoch(secs: u64) -> String {
    // Very small stringifier: format as UTC YYYY-MM-DD HH:MM:SS without pulling chrono.
    // Uses SystemTime comparison, so we'll emit an ISO-ish "T+<since-boot>" fallback if
    // we can't compute. Keep dependency-free by using the difference from now.
    use std::time::{Duration, UNIX_EPOCH};
    let then = UNIX_EPOCH + Duration::from_secs(secs);
    let now = std::time::SystemTime::now();
    match now.duration_since(then) {
        Ok(ago) => {
            let a = ago.as_secs();
            format!("{} ago", format_duration(a))
        }
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
