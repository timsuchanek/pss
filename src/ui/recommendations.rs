use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};

use crate::app::{App, Pane, RecsSource};

use super::titled_block;

pub fn render(f: &mut Frame, area: Rect, app: &App) {
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

    let filtered: Vec<&crate::llm::Recommendation> = app
        .recommendations
        .iter()
        .filter(|r| {
            if app.search_query.is_empty() {
                return true;
            }
            if let Some(pid) = r.pid {
                if app.pid_visible(pid) {
                    return true;
                }
            }
            let q = app.search_query.to_ascii_lowercase();
            r.target.to_ascii_lowercase().contains(&q)
                || r.reason.to_ascii_lowercase().contains(&q)
        })
        .collect();

    let items: Vec<ListItem> = if filtered.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "  no matching recommendations.",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        filtered
            .iter()
            .map(|r| {
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
                    Span::styled(
                        truncate(&r.reason, 60),
                        Style::default().fg(Color::DarkGray),
                    ),
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
    if active && !filtered.is_empty() {
        state.select(Some(app.selected_rec.min(filtered.len() - 1)));
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
