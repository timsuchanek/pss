//! Geometry + rendering for the anchored context-menu cascade. The geometry
//! helpers (`place`, `submenu_origin`, `menu_hit`) are pure and unit tested;
//! `render` is added once `App` carries the menu state.

use ratatui::layout::Rect;

/// Fixed popup width (columns) for all menu levels.
pub const MENU_W: u16 = 26;

/// Result of hit-testing a click against a popup rect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Outside,
    Border,
    Item(usize),
}

/// Clamp a `w`x`h` popup anchored at `(col, row)` to stay fully within a
/// `screen_w`x`screen_h` terminal: pull left if it would overflow the right
/// edge, up if it would overflow the bottom.
pub fn place(col: u16, row: u16, w: u16, h: u16, screen_w: u16, screen_h: u16) -> Rect {
    let w = w.min(screen_w);
    let h = h.min(screen_h);
    let x = if col + w <= screen_w { col } else { screen_w.saturating_sub(w) };
    let y = if row + h <= screen_h { row } else { screen_h.saturating_sub(h) };
    Rect::new(x, y, w, h)
}

/// Left-column for a submenu of width `sub_w`: to the right of the parent
/// normally, but flipped to the parent's left (no overlap) when the right side
/// has no room.
pub fn submenu_origin(parent_col: u16, parent_w: u16, sub_w: u16, screen_w: u16) -> u16 {
    if parent_col + parent_w + sub_w <= screen_w {
        parent_col + parent_w
    } else {
        parent_col.saturating_sub(sub_w)
    }
}

/// Classify a click against a popup `rect` containing `n_items` rows (between
/// the top and bottom border rows).
pub fn menu_hit(rect: Rect, col: u16, row: u16, n_items: u16) -> Hit {
    if col < rect.x || col >= rect.x + rect.width || row < rect.y || row >= rect.y + rect.height {
        return Hit::Outside;
    }
    if row == rect.y || row == rect.y + rect.height - 1 {
        return Hit::Border;
    }
    let idx = row - rect.y - 1;
    if idx < n_items {
        Hit::Item(idx as usize)
    } else {
        Hit::Border
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn place_fits_without_clamping() {
        let r = place(2, 3, 26, 12, 200, 50);
        assert_eq!((r.x, r.y, r.width, r.height), (2, 3, 26, 12));
    }

    #[test]
    fn place_pulls_in_at_right_and_bottom() {
        assert_eq!(place(190, 3, 26, 12, 200, 50).x, 174); // 200 - 26
        assert_eq!(place(2, 45, 26, 12, 200, 50).y, 38); // 50 - 12
    }

    #[test]
    fn submenu_opens_right_then_flips_left() {
        // Room on the right.
        assert_eq!(submenu_origin(10, 26, 26, 200), 36);
        // No room: flip to the left (ends exactly at parent_col, no overlap).
        assert_eq!(submenu_origin(180, 26, 26, 200), 154);
    }

    #[test]
    fn hit_classifies_rows() {
        let rect = Rect::new(5, 5, 26, 5); // rows 5..10; items at 6,7,8; borders 5 & 9
        assert_eq!(menu_hit(rect, 6, 6, 3), Hit::Item(0));
        assert_eq!(menu_hit(rect, 6, 8, 3), Hit::Item(2));
        assert_eq!(menu_hit(rect, 6, 5, 3), Hit::Border);
        assert_eq!(menu_hit(rect, 6, 9, 3), Hit::Border);
        assert_eq!(menu_hit(rect, 40, 6, 3), Hit::Outside);
    }
}

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;

/// Draw every level in the menu stack as an anchored, edge-clamped popup.
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let Some(cm) = app.context_menu.as_ref() else {
        return;
    };
    for level in &cm.levels {
        let h = level.items.len() as u16 + 2;
        let rect = place(level.origin_col, level.origin_row, MENU_W, h, area.width, area.height);
        let lines: Vec<Line> = level
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let style = if !it.enabled {
                    Style::default().fg(Color::DarkGray)
                } else if i == level.selected {
                    Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let suffix = if it.opens_submenu { " ▸" } else { "" };
                Line::from(Span::styled(format!(" {} {}{}", it.icon, it.label, suffix), style))
            })
            .collect();
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan))),
            rect,
        );
    }
}
