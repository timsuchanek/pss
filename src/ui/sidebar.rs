use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};

use crate::app::{App, Pane, Selection, TreeRow};

use super::titled_block;

pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    let active = app.pane == Pane::Sidebar;
    let rows = app.tree_rows();
    let selected_idx = rows
        .iter()
        .position(|r| r.selection == app.selection)
        .unwrap_or(0);

    // Subtract block border (2), twisty (2), cpu% (6), and a 1-cell gutter.
    let inner_w = area.width.saturating_sub(2) as usize;
    let label_w = inner_w.saturating_sub(2 + 6 + 1);

    let items: Vec<ListItem> = rows
        .iter()
        .map(|r| ListItem::new(render_row(r, label_w)))
        .collect();

    let mut state = ListState::default();
    state.select(Some(selected_idx));

    let list = List::new(items)
        .block(titled_block("projects", active))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("");

    f.render_stateful_widget(list, area, &mut state);
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

    let available = label_w
        .saturating_sub(indent.chars().count())
        .saturating_sub(twisty.chars().count());
    let label = truncate(&r.label, available.max(4));
    let cpu_span = if matches!(r.selection, Selection::All) {
        format!("{:>5.0}%", r.cpu)
    } else {
        format!("{:>5.1}%", r.cpu)
    };

    let label_padded = format!("{:<width$}", label, width = available.max(4));

    Line::from(vec![
        Span::raw(indent),
        Span::styled(twisty.to_string(), Style::default().fg(Color::DarkGray)),
        Span::styled(label_padded, Style::default().fg(label_color(r))),
        Span::styled(cpu_span, Style::default().fg(cpu_color(r.cpu))),
    ])
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

fn label_color(r: &TreeRow) -> Color {
    match r.selection {
        Selection::All => Color::LightCyan,
        Selection::Process(_, _) => Color::Gray,
        Selection::Bucket(_) => Color::White,
    }
}
