use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::app::{App, Pane, Selection, SidebarSortKey, TreeRow};

use super::titled_block;

// Fixed column widths (inside the sidebar block).
pub const CPU_COL_W: usize = 7;
pub const MEM_COL_W: usize = 7;
pub const NET_COL_W: usize = 7;
pub const COL_GUTTER: usize = 1;

pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    let active = app.pane == Pane::Sidebar;
    let title = if !app.search_query.is_empty() {
        format!("projects · filter: {}", app.search_query)
    } else {
        "projects".to_string()
    };
    let block = titled_block(&title, active);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 2 || inner.width < 10 {
        return;
    }

    // Header row sits on the first row of the inner area.
    let header_rect = Rect::new(inner.x, inner.y, inner.width, 1);
    render_header(f, header_rect, app);

    // Tree list occupies the rest.
    let list_rect = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);

    let rows = app.tree_rows();
    let selected_idx = rows
        .iter()
        .position(|r| r.selection == app.selection)
        .unwrap_or(0);

    let inner_w = list_rect.width as usize;
    let label_w =
        inner_w.saturating_sub(CPU_COL_W + MEM_COL_W + NET_COL_W + COL_GUTTER * 2);

    let items: Vec<ListItem> = rows
        .iter()
        .map(|r| ListItem::new(render_row(r, label_w)))
        .collect();

    let mut state = ListState::default();
    state.select(Some(selected_idx));

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("");

    f.render_stateful_widget(list, list_rect, &mut state);
}

fn render_header(f: &mut Frame, area: Rect, app: &App) {
    let cpu_active = app.sidebar_sort_key == SidebarSortKey::Cpu;
    let mem_active = app.sidebar_sort_key == SidebarSortKey::Mem;
    let net_active = app.sidebar_sort_key == SidebarSortKey::Net;

    let decorate = |label: &str, active: bool| {
        if active {
            format!("{} {}", label, app.sidebar_sort_dir.glyph())
        } else {
            label.to_string()
        }
    };
    let cpu_label = decorate("cpu", cpu_active);
    let mem_label = decorate("mem", mem_active);
    let net_label = decorate("net", net_active);

    let label_w = (area.width as usize)
        .saturating_sub(CPU_COL_W + MEM_COL_W + NET_COL_W + COL_GUTTER * 2);

    let style_of = |active: bool| {
        if active {
            Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        }
    };

    let line = Line::from(vec![
        Span::raw(" ".repeat(label_w)),
        Span::styled(
            format!("{:>width$}", cpu_label, width = CPU_COL_W),
            style_of(cpu_active),
        ),
        Span::raw(" ".repeat(COL_GUTTER)),
        Span::styled(
            format!("{:>width$}", mem_label, width = MEM_COL_W),
            style_of(mem_active),
        ),
        Span::raw(" ".repeat(COL_GUTTER)),
        Span::styled(
            format!("{:>width$}", net_label, width = NET_COL_W),
            style_of(net_active),
        ),
    ]);

    f.render_widget(Paragraph::new(line), area);
}

fn render_row(r: &TreeRow, label_w: usize) -> Line<'static> {
    let indent = match r.depth {
        0 => String::new(),
        _ => "  ".repeat(r.depth as usize),
    };
    let twisty = if matches!(r.selection, Selection::All) {
        ""
    } else if r.has_children {
        if r.is_expanded { "▼ " } else { "▸ " }
    } else if r.depth > 0 {
        "· "
    } else {
        "  "
    };

    let avail = label_w
        .saturating_sub(indent.chars().count())
        .saturating_sub(twisty.chars().count())
        .max(4);
    let label = truncate(&r.label, avail);
    let label_padded = format!("{:<width$}", label, width = avail);

    let cpu = format!("{:>width$.1}", r.cpu, width = CPU_COL_W - 1);
    let cpu_cell = format!("{}%", cpu);
    let mem_cell = format!("{:>width$}", fmt_mem(r.mem), width = MEM_COL_W);
    let net_cell = format!("{:>width$}", fmt_rate(r.net), width = NET_COL_W);

    Line::from(vec![
        Span::raw(indent),
        Span::styled(twisty.to_string(), Style::default().fg(Color::DarkGray)),
        Span::styled(label_padded, Style::default().fg(label_color(r))),
        Span::styled(cpu_cell, Style::default().fg(cpu_color(r.cpu))),
        Span::raw(" ".repeat(COL_GUTTER)),
        Span::styled(mem_cell, Style::default().fg(mem_color(r.mem))),
        Span::raw(" ".repeat(COL_GUTTER)),
        Span::styled(net_cell, Style::default().fg(net_color(r.net))),
    ])
}

fn fmt_rate(bps: u64) -> String {
    if bps == 0 {
        return "—".into();
    }
    let b = bps as f64;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1}G", b / 1024.0 / 1024.0 / 1024.0)
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.1}M", b / 1024.0 / 1024.0)
    } else if b >= 1024.0 {
        format!("{:.0}K", b / 1024.0)
    } else {
        format!("{}B", bps)
    }
}

fn net_color(bps: u64) -> Color {
    let mb = bps as f64 / 1024.0 / 1024.0;
    if mb >= 10.0 {
        Color::LightRed
    } else if mb >= 1.0 {
        Color::Yellow
    } else if bps >= 1024 {
        Color::Green
    } else if bps > 0 {
        Color::Gray
    } else {
        Color::DarkGray
    }
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

fn mem_color(mem: u64) -> Color {
    let gb = mem as f32 / 1024.0 / 1024.0 / 1024.0;
    if gb >= 4.0 {
        Color::LightRed
    } else if gb >= 1.0 {
        Color::Yellow
    } else if mem >= 100 * 1024 * 1024 {
        Color::Green
    } else {
        Color::DarkGray
    }
}

fn label_color(r: &TreeRow) -> Color {
    match r.selection {
        Selection::All => Color::LightCyan,
        Selection::Process(_, _) => Color::Gray,
        Selection::Bucket(_) => Color::White,
    }
}

fn fmt_mem(bytes: u64) -> String {
    let b = bytes as f32;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1}G", b / 1024.0 / 1024.0 / 1024.0)
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.0}M", b / 1024.0 / 1024.0)
    } else if b >= 1024.0 {
        format!("{:.0}K", b / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}
