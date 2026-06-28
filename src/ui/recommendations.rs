use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};

use crate::app::{App, Pane, RecsSource};

use super::titled_block;

pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    let active = app.pane == Pane::Recommendations;
    let source = match app.recs_source {
        RecsSource::Llm => "openrouter",
        RecsSource::Local => {
            if app.has_llm {
                "local · llm pending"
            } else {
                "local"
            }
        }
    };
    let title = format!("recommendations — {}", source);
    let block = titled_block(&title, active);

    // Shared visible projection (raw indices in display order).
    let visible = app.visible_recs();

    // Window the visible list to what fits, keeping the selection on-screen,
    // and store the offset so mouse/keyboard hit-tests map rows the same way.
    let vis_rows = area.height.saturating_sub(2) as usize; // minus block borders
    let offset = if visible.len() <= vis_rows || vis_rows == 0 {
        0
    } else {
        let sel = app.selected_rec.min(visible.len() - 1);
        sel.saturating_sub(vis_rows - 1).min(visible.len() - vis_rows)
    };
    app.recs_offset = offset;
    let end = (offset + vis_rows).min(visible.len());
    let window = if offset < end { &visible[offset..end] } else { &[][..] };
    // Store the count of data rows actually drawn so hit-tests reject clicks on
    // rows that weren't rendered (short-terminal geometry gap → never a
    // scrolled-off, non-visible rec under a destructive action).
    app.recs_vis_rows = window.len();

    let items: Vec<ListItem> = if visible.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "  no matching recommendations.",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        window
            .iter()
            .map(|&raw| {
                let r = &app.recommendations[raw];
                let icon = match r.action.as_str() {
                    "kill" => "☠",
                    "reclaim" => "◆",
                    "throttle" => "⛓",
                    _ => "·",
                };
                let saved = if r.estimated_saved_mb > 0 {
                    format!("  ~{}M", r.estimated_saved_mb)
                } else {
                    String::new()
                };
                let conf_color = if r.confidence >= 90 {
                    Color::Green
                } else if r.confidence >= 70 {
                    Color::Yellow
                } else {
                    Color::DarkGray
                };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!(" {} ", icon),
                        Style::default().fg(Color::LightMagenta).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:<30}", truncate(&r.target, 30)),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(
                        format!("conf {:>3}%", r.confidence),
                        Style::default().fg(conf_color),
                    ),
                    Span::raw(saved),
                    Span::raw("  "),
                    Span::styled(truncate(&r.reason, 60), Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect()
    };

    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    if active && !visible.is_empty() {
        let sel = app.selected_rec.min(visible.len() - 1);
        state.select(Some(sel - offset)); // position within the drawn window
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept)
}
