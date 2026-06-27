# pss Suggestions-Pane Context Menu Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the right-click / `x` context menu to the recommendations pane — reusing the existing process menu for live process-backed suggestions and a "Copy reason" menu otherwise — and fix the recs row→index model (filter + scroll) so the destructive menu always targets the suggestion the user sees.

**Architecture:** One shared *visible projection* (`visible_rec_indices` → `App::visible_recs` + stored `App::recs_offset`) drives the renderer, mouse hit-test, `x`, nav, and the kill paths; `selected_rec` becomes a consistent display index. The menu reuses the shipped pipeline: a suggestion resolves to `Selection::Process(label,pid)` (if the pid is live) or a one-item `CopyReason` menu. A pure `submenu_anchor` helper fixes the renice cascade near the right edge.

**Tech Stack:** Rust, ratatui/crossterm, sysinfo. macOS-first (same as the shipped context menu).

## Global Constraints

- **Destructive-safe mapping:** never map a screen row to a raw `recommendations` index. Resolve clicks/`x` through `App::visible_recs()` + `App::recs_offset`, both produced from the same filter the renderer draws. Bound recs hit-tests to data rows only (`ev.row < recs_list_end.saturating_sub(1)`).
- **`selected_rec` is a display index** (a position within `visible_recs()`), consistent across render, nav, `x`, and the kill paths.
- **Stale-pid guard:** a suggestion opens the process menu only when `rec.pid = Some(pid)` AND `app.process_by_pid(pid).is_some()`; otherwise the `Copy reason` menu.
- **Cached metadata only** via `App::process_by_pid` (no fresh `sysinfo::System` for reads).
- **Reuse, don't duplicate:** the process menu, `open`, `select`, `run_action`, `place`, `submenu_origin`, `menu_hit` are unchanged; this plan adds `CopyReason`, `build_info_rec`, `submenu_anchor`, `open_for_rec`, `bucket_label_for_pid`, the projection API, and wiring.
- **Conventional Commits.** Out of scope: process-table right-click; other rec verbs; the pre-existing short-terminal `recs_list_top` vs clamped-area geometry edge.

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `src/menu.rs` | `MenuAction::CopyReason` + `build_info_rec` | Modify |
| `src/ui/context_menu.rs` | pure `submenu_anchor` helper + test | Modify |
| `src/app.rs` | visible projection API + selection-consumer rewire + delegator | Modify |
| `src/ui/recommendations.rs` | render from projection, store `recs_offset` | Modify |
| `src/menu_dispatch.rs` | `open_for_rec`, `bucket_label_for_pid`, `CopyReason`, anchor fix | Modify |
| `src/main.rs` | recs left-click fix + right-click + `x` wiring | Modify |

---

### Task 1: `menu.rs` — `CopyReason` action + `build_info_rec`

**Files:**
- Modify: `src/menu.rs` (enum at line 27; add builder + test)

**Interfaces:**
- Consumes: `Caps`, `MenuItem`, the private `item(...)` helper, `Outcome` (its `_ => Close` arm covers the new variant).
- Produces: `MenuAction::CopyReason(String)`; `pub fn build_info_rec(caps: Caps, reason: String) -> Vec<MenuItem>`.

- [ ] **Step 1: Write the failing test**

Add to `src/menu.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn info_rec_menu_is_copy_reason_gated_by_clipboard() {
        let m = build_info_rec(ALL, "idle 90s".to_string());
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].action, MenuAction::CopyReason("idle 90s".to_string()));
        assert!(m[0].enabled);
        assert!(!m[0].opens_submenu);
        let m2 = build_info_rec(NO_GUI, "x".to_string());
        assert!(!m2[0].enabled, "Copy reason disabled without clipboard");
    }
```

(`ALL` and `NO_GUI` already exist in this test module; `NO_GUI` has `clipboard: false`.)

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test menu::tests::info_rec_menu -q`
Expected: FAIL — `build_info_rec` not found / `CopyReason` not a variant.

- [ ] **Step 3: Add the variant and builder**

In `src/menu.rs`, add a variant to `MenuAction` (e.g. after `CopyPath,`):

```rust
    CopyPath,
    CopyReason(String),
```

Add the builder after `build_all` (near line 204):

```rust
pub fn build_info_rec(caps: Caps, reason: String) -> Vec<MenuItem> {
    vec![item("⧉", "Copy reason", MenuAction::CopyReason(reason), caps.clipboard, false)]
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test menu:: -q`
Expected: PASS (all menu tests, including the new one).

- [ ] **Step 5: Commit**

```bash
git add src/menu.rs
git commit -m "feat(menu): CopyReason action + build_info_rec for info suggestions"
```

---

### Task 2: `ui/context_menu.rs` — pure `submenu_anchor` helper

**Files:**
- Modify: `src/ui/context_menu.rs` (add fn near `submenu_origin`; add tests)

**Interfaces:**
- Consumes: `MENU_W`, `place`, `submenu_origin`.
- Produces: `pub fn submenu_anchor(origin_col: u16, origin_row: u16, n_items: u16, selected: u16, sw: u16, sh: u16) -> (u16, u16)`.

- [ ] **Step 1: Write the failing tests**

Add to `src/ui/context_menu.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn submenu_anchor_opens_right_in_open_space() {
        // place(10,5,26,6,200,50) = (10,5); submenu_origin(10,26,26,200)=36; row=5+1
        assert_eq!(submenu_anchor(10, 5, 4, 1, 200, 50), (36, 6));
    }

    #[test]
    fn submenu_anchor_flips_left_at_right_edge() {
        // parent clamps to x=174; submenu has no room right → 174-26=148
        assert_eq!(submenu_anchor(180, 5, 4, 1, 200, 50), (148, 6));
    }

    #[test]
    fn submenu_anchor_follows_bottom_clamp() {
        // place(10,48,26,6,200,50).y → 50-6=44; row=44+selected
        assert_eq!(submenu_anchor(10, 48, 4, 1, 200, 50), (36, 45));
    }
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test context_menu::tests::submenu_anchor -q`
Expected: FAIL — `submenu_anchor` not found.

- [ ] **Step 3: Implement the helper**

In `src/ui/context_menu.rs`, add after `submenu_origin` (after line 40):

```rust
/// Top-left anchor for a submenu of width `MENU_W` opened from a parent level:
/// computed against the parent's *placed* (edge-clamped) rect so it stays
/// aligned to what was drawn, flipping left when the right side has no room.
pub fn submenu_anchor(
    origin_col: u16,
    origin_row: u16,
    n_items: u16,
    selected: u16,
    sw: u16,
    sh: u16,
) -> (u16, u16) {
    let placed = place(origin_col, origin_row, MENU_W, n_items + 2, sw, sh);
    (submenu_origin(placed.x, MENU_W, MENU_W, sw), placed.y + selected)
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test context_menu:: -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui/context_menu.rs
git commit -m "feat(ui): submenu_anchor — clamp-aware cascade placement"
```

---

### Task 3: shared visible projection + index-model fix

This is one cohesive change: it makes `selected_rec` a consistent display index everywhere, so render, nav, the kill paths, and the **left-click** handler agree. Splitting it would regress kill-after-left-click, so it lands together.

**Files:**
- Modify: `src/app.rs` (projection API + field + nav/kill/clamp rewire)
- Modify: `src/ui/recommendations.rs` (render from projection, store offset)
- Modify: `src/main.rs` (recs left-click handler; `recs_list_top` via helper)

**Interfaces:**
- Consumes: `crate::llm::Recommendation`, `App::{recommendations, search_query, pid_visible, term_size, recs_height, selected_rec}`.
- Produces: free fn `visible_rec_indices(...)`, `recs_list_top_geom(...)`; `App::{visible_recs, selected_rec_raw, recs_list_top, recs_offset}`.

- [ ] **Step 1: Write the failing tests (pure projection + geometry)**

Add a test module at the end of `src/app.rs` (or extend `ctxmenu_tests`):

```rust
#[cfg(test)]
mod recs_tests {
    use super::*;
    use crate::llm::Recommendation;

    fn rec(pid: Option<u32>, target: &str, reason: &str) -> Recommendation {
        Recommendation {
            pid,
            action: "kill".into(),
            target: target.into(),
            reason: reason.into(),
            confidence: 90,
            estimated_saved_mb: 0,
        }
    }

    #[test]
    fn empty_query_keeps_all_in_order() {
        let recs = vec![rec(Some(1), "a", "x"), rec(None, "b", "y")];
        assert_eq!(visible_rec_indices(&recs, "", |_| true), vec![0, 1]);
    }

    #[test]
    fn query_filters_by_target_reason_or_visible_pid() {
        let recs = vec![
            rec(Some(1), "node", "idle server"),   // 0: matches "idle"
            rec(Some(2), "claude", "busy"),        // 1: pid 2 visible
            rec(None, "esbuild", "compiling"),     // 2: no match
        ];
        // pid 2 is "visible"; query "idle" matches rec 0's reason.
        let got = visible_rec_indices(&recs, "idle", |pid| pid == 2);
        assert_eq!(got, vec![0, 1]);
    }

    #[test]
    fn recs_list_top_matches_geometry() {
        // recs_top = term_h - (1 + recs_height); list top = recs_top + 1
        assert_eq!(recs_list_top_geom(50, 8), 50 - 1 - 8 + 1);
        assert_eq!(recs_list_top_geom(4, 8), 1); // saturating: recs_top=0 → 1
    }
}
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test app::recs_tests -q` (or `cargo test recs_tests -q`)
Expected: FAIL — `visible_rec_indices` / `recs_list_top_geom` not found.

- [ ] **Step 3: Add the projection field + free functions + App methods**

In `src/app.rs`, add the field to the `App` struct (after `pub term_size: (u16, u16),` at line 196):

```rust
    pub term_size: (u16, u16),
    // scroll offset of the recs list (display index of the first drawn row)
    pub recs_offset: usize,
```

In `App::new()` (after `term_size: (80, 24),`) add:

```rust
            term_size: (80, 24),
            recs_offset: 0,
```

Add these free functions at module scope (near the other free fns like `app_ancestor`):

```rust
/// Raw indices of recommendations that pass the recs-pane filter, in display
/// order. Mirrors the renderer's predicate. Pure — unit testable.
pub fn visible_rec_indices(
    recs: &[crate::llm::Recommendation],
    query: &str,
    is_pid_visible: impl Fn(u32) -> bool,
) -> Vec<usize> {
    let q = query.to_ascii_lowercase();
    recs.iter()
        .enumerate()
        .filter_map(|(i, r)| {
            if query.is_empty() {
                return Some(i);
            }
            if let Some(pid) = r.pid {
                if is_pid_visible(pid) {
                    return Some(i);
                }
            }
            if r.target.to_ascii_lowercase().contains(&q)
                || r.reason.to_ascii_lowercase().contains(&q)
            {
                Some(i)
            } else {
                None
            }
        })
        .collect()
}

/// First recs data row: block top border at `term_h - (1 + recs_height)`, data
/// starts one below. Matches `handle_mouse`'s inline geometry.
pub fn recs_list_top_geom(term_h: u16, recs_height: u16) -> u16 {
    let recs_top = term_h.saturating_sub(1 + recs_height);
    recs_top + 1
}
```

Add these methods inside `impl App` (near `pid_visible`, line 851):

```rust
    /// Raw indices of currently-visible recommendations, in display order.
    pub fn visible_recs(&self) -> Vec<usize> {
        visible_rec_indices(&self.recommendations, &self.search_query, |pid| self.pid_visible(pid))
    }

    /// Raw index of the selected recommendation (selected_rec is a display
    /// index, clamped to the visible list); None when nothing is visible.
    pub fn selected_rec_raw(&self) -> Option<usize> {
        let vis = self.visible_recs();
        if vis.is_empty() {
            return None;
        }
        vis.get(self.selected_rec.min(vis.len() - 1)).copied()
    }

    pub fn recs_list_top(&self) -> u16 {
        recs_list_top_geom(self.term_size.1, self.recs_height)
    }
```

- [ ] **Step 4: Rewire the `selected_rec` consumers to the projection**

In `nav_down` Recommendations branch (line 1057) change the bound to the visible length:

```rust
            Pane::Recommendations => {
                if self.selected_rec + 1 < self.visible_recs().len() {
                    self.selected_rec += 1;
                }
            }
```

In `open_kill_menu`'s Recommendations branch (lines 474-479) resolve via the projection:

```rust
            Pane::Recommendations => {
                let rec = self.selected_rec_raw().and_then(|raw| self.recommendations.get(raw));
                let pid = rec.and_then(|r| r.pid);
                let name = rec.map(|r| r.target.clone()).unwrap_or_default();
                (pid, name)
            }
```

In `kill_selected`'s Recommendations branch (lines 1269-1272):

```rust
            Pane::Recommendations => self
                .selected_rec_raw()
                .and_then(|raw| self.recommendations.get(raw))
                .and_then(|r| r.pid),
```

In `set_recommendations` (lines 1303-1305) clamp against the visible length:

```rust
        let vis = self.visible_recs().len();
        if self.selected_rec >= vis {
            self.selected_rec = vis.saturating_sub(1);
        }
```

(`nav_up`'s `saturating_sub(1)` is already correct for a display index — leave it.)

- [ ] **Step 5: Render the recs pane from the projection and store the offset**

Replace the body of `pub fn render` in `src/ui/recommendations.rs` (signature becomes `&mut App`). Full replacement of the file's `render` function:

```rust
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
```

Update the call site in `src/ui/mod.rs:538` — it already passes `app` (which is `&mut App` in `render_body`), so only the callee signature changed; confirm it still reads `recommendations::render(f, right[3], app);` and compiles.

- [ ] **Step 6: Fix the left-click handler to use the projection**

In `src/main.rs`, change the `recs_list_top` local in `handle_mouse` (line 472) to the shared helper:

```rust
    let recs_list_top = app.recs_list_top();
```

Replace the "Recommendations row click" block (lines 580-598) with the projection mapping:

```rust
            // Recommendations row click (projection-mapped; data rows only).
            if ev.column > sidebar_w
                && ev.row >= recs_list_top
                && ev.row < recs_list_end.saturating_sub(1)
            {
                let vr = (ev.row - recs_list_top) as usize;
                if let Some(raw) = app.visible_recs().get(app.recs_offset + vr).copied() {
                    let display = app.recs_offset + vr;
                    let was_same = app.selected_rec == display && app.pane == Pane::Recommendations;
                    app.selected_rec = display;
                    app.pane = Pane::Recommendations;
                    if was_same {
                        if let Some(pid) = app.recommendations.get(raw).and_then(|r| r.pid) {
                            app.open_drilldown(pid);
                            app.ensure_drilldown_loaded_for_tab();
                        }
                    }
                    return;
                }
            }
```

- [ ] **Step 7: Build and run the tests**

Run: `cargo test -q`
Expected: compiles; `recs_tests::*` pass; all prior tests still pass.

- [ ] **Step 8: Commit**

```bash
git add src/app.rs src/ui/recommendations.rs src/main.rs
git commit -m "fix(recs): shared visible projection — consistent selected_rec, correct row mapping"
```

---

### Task 4: `menu_dispatch.rs` — `open_for_rec`, `bucket_label_for_pid`, `CopyReason`, anchor fix

**Files:**
- Modify: `src/menu_dispatch.rs` (imports; `open_for_rec`; `bucket_label_for_pid`; `CopyReason` arm; `open_submenu`; test)
- Modify: `src/app.rs` (delegator `open_context_menu_for_rec`)

**Interfaces:**
- Consumes: `menu::build_info_rec`, `actions::caps`, `ui::context_menu::submenu_anchor`, `App::{recommendations, process_by_pid, buckets, context_menu, term_size}`, `crate::app::Bucket`.
- Produces: `pub(crate) fn open_for_rec(app, raw_idx, col, row)`; `pub(crate) fn bucket_label_for_pid(&[Bucket], u32) -> Option<String>`; `App::open_context_menu_for_rec(idx, col, row)`.

- [ ] **Step 1: Write the failing test**

Add to `src/menu_dispatch.rs`'s `#[cfg(test)] mod tests`:

```rust
    use crate::app::{Bucket, BucketKey};

    fn bucket(key: BucketKey, pids: Vec<u32>) -> Bucket {
        Bucket { key, cpu: 0.0, mem: 0, net_rx: 0, net_tx: 0, pids }
    }

    #[test]
    fn bucket_label_for_pid_finds_owner() {
        let buckets = vec![
            bucket(BucketKey::System, vec![1, 2]),
            bucket(BucketKey::Bundle("Foo.app".into()), vec![9]),
        ];
        assert_eq!(bucket_label_for_pid(&buckets, 9), Some("Foo.app (bundle)".to_string()));
        assert_eq!(bucket_label_for_pid(&buckets, 1), Some("(system)".to_string()));
        assert_eq!(bucket_label_for_pid(&buckets, 42), None);
    }
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test menu_dispatch::tests::bucket_label_for_pid -q`
Expected: FAIL — `bucket_label_for_pid` not found.

- [ ] **Step 3: Update imports**

In `src/menu_dispatch.rs`, change the imports (lines 8-10):

```rust
use crate::actions::{self, ShellAction};
use crate::app::{app_ancestor, App, Bucket, BucketKey, Selection};
use crate::menu::{self, BucketKind, ContextMenu, MenuAction, Outcome};
use crate::ui::context_menu::{menu_hit, place, submenu_anchor, Hit, MENU_W};
```

(`submenu_origin` is replaced by `submenu_anchor`; `place` stays for `click`.)

- [ ] **Step 4: Add `open_for_rec` + `bucket_label_for_pid`**

Add after `open` (after line 38):

```rust
/// Open the context menu for recommendation `raw_idx`: the full process menu
/// when the suggestion targets a *live* process, else a `Copy reason` menu.
pub(crate) fn open_for_rec(app: &mut App, raw_idx: usize, col: u16, row: u16) {
    let (pid, reason) = match app.recommendations.get(raw_idx) {
        Some(r) => (r.pid, r.reason.clone()),
        None => return,
    };
    match pid {
        Some(pid) if app.process_by_pid(pid).is_some() => {
            let label = bucket_label_for_pid(&app.buckets, pid).unwrap_or_default();
            open(app, Selection::Process(label, pid), col, row);
        }
        _ => {
            let items = menu::build_info_rec(actions::caps(), reason);
            app.context_menu = Some(ContextMenu::new(Selection::All, items, col, row));
        }
    }
}

/// Label of the bucket that owns `pid`, if any.
pub(crate) fn bucket_label_for_pid(buckets: &[Bucket], pid: u32) -> Option<String> {
    buckets.iter().find(|b| b.pids.contains(&pid)).map(|b| b.key.label())
}
```

- [ ] **Step 5: Dispatch `CopyReason` and fix `open_submenu`**

In `run_action`, add an arm before the final `OpenKill | OpenReniceSubmenu` arm (after the `CollapseAll` arm, ~line 207):

```rust
        A::CopyReason(text) => copy(app, &text, "copied reason".into()),
        // Handled before dispatch.
        A::OpenKill | A::OpenReniceSubmenu => {}
```

Replace `open_submenu` (lines 94-104) to use the clamp-aware anchor:

```rust
fn open_submenu(app: &mut App) {
    let items = menu::build_renice();
    let (sw, sh) = app.term_size;
    if let Some(cm) = app.context_menu.as_mut() {
        if let Some(level) = cm.levels.last() {
            let (col, row) = submenu_anchor(
                level.origin_col,
                level.origin_row,
                level.items.len() as u16,
                level.selected as u16,
                sw,
                sh,
            );
            cm.push(items, col, row);
        }
    }
}
```

- [ ] **Step 6: Add the app delegator**

In `src/app.rs`, next to `open_context_menu` add:

```rust
    pub fn open_context_menu_for_rec(&mut self, idx: usize, col: u16, row: u16) {
        crate::menu_dispatch::open_for_rec(self, idx, col, row);
    }
```

- [ ] **Step 7: Build and run the tests**

Run: `cargo test -q`
Expected: compiles; `menu_dispatch::tests::bucket_label_for_pid_finds_owner` passes; all prior tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/menu_dispatch.rs src/app.rs
git commit -m "feat(menu): open_for_rec + bucket_label_for_pid + CopyReason; clamp-aware submenu"
```

---

### Task 5: `main.rs` — recs right-click + `x` wiring

**Files:**
- Modify: `src/main.rs` (recs branch in the `Down(MouseButton::Right)` arm; `x` Recommendations match arm)

**Interfaces:**
- Consumes: `App::{visible_recs, recs_offset, recs_list_top, selected_rec_raw, selected_rec, sidebar_width, open_context_menu_for_rec}`; locals `recs_list_top`, `recs_list_end`, `sidebar_w`.

- [ ] **Step 1: Add the recs branch to the right-click arm**

In `src/main.rs`, inside the `MouseEventKind::Down(MouseButton::Right) =>` arm, after the sidebar `if` block (after line 640, before the arm's closing brace at 641), add:

```rust
            // Recommendations row: open the menu for the suggestion under the cursor.
            if ev.column > sidebar_w
                && ev.row >= recs_list_top
                && ev.row < recs_list_end.saturating_sub(1)
            {
                let vr = (ev.row - recs_list_top) as usize;
                if let Some(raw) = app.visible_recs().get(app.recs_offset + vr).copied() {
                    app.selected_rec = app.recs_offset + vr;
                    app.pane = Pane::Recommendations;
                    app.open_context_menu_for_rec(raw, ev.column + 1, ev.row + 1);
                }
            }
```

(The modal guard at the top of this arm already covers the recs case.)

- [ ] **Step 2: Add the `x` Recommendations match arm**

In the normal-mode `match key.code` block, after the Sidebar `x` arm (after line 429), add a second guarded arm:

```rust
                        KeyCode::Char('x')
                            if app.pane == crate::app::Pane::Recommendations => {
                                if let Some(raw) = app.selected_rec_raw() {
                                    let vis_len = app.visible_recs().len();
                                    let display = app.selected_rec.min(vis_len.saturating_sub(1));
                                    let vr = display.saturating_sub(app.recs_offset) as u16;
                                    let row = app.recs_list_top() + vr;
                                    app.open_context_menu_for_rec(raw, app.sidebar_width + 2, row);
                                }
                            }
```

- [ ] **Step 3: Build and run the tests**

Run: `cargo build -q && cargo test -q`
Expected: compiles; all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): right-click + x context menu on the suggestions pane"
```

---

### Task 6: Verify + document

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Full test + feature-scoped clippy**

Run: `cargo test -q`
Expected: all pass.

Run: `cargo clippy --all-targets 2>&1 | grep -E "menu|recommendations|main\.rs|context_menu" | grep warning:`
Expected: no warnings attributable to the changed files. (The repo has ~5 pre-existing dead-code warnings — `kill_selected`, `kill_pid`, `ts`, `fd`, `size` — that are out of scope; do not fix them here.)

- [ ] **Step 2: Manual smoke**

Run: `cargo run`

1. Type a search that filters the recs, press Enter (commit — `search_active` clears, filter stays), then right-click a **visible** suggestion → the menu targets **that** suggestion (no raw-index mismatch). With enough recs to scroll, verify the scrolled rows still map correctly.
2. Right-click a live kill suggestion → full process menu; Inspect closes the menu and opens the drill-down.
3. Right-click an info-only or **stale** suggestion (pid no longer running) → `Copy reason`; selecting copies the reason and shows `copied reason`.
4. Focus the suggestions pane and press `x` → menu opens on the selected suggestion.
5. Open Renice ▸ on a suggestion → the cascade flips left near the right edge and aligns with the parent item.
6. With a modal open (drill-down/kill menu/thermal/active search), right-click the recs region → nothing opens.

- [ ] **Step 3: Update the README**

In `README.md`, extend the context-menu line(s) so they mention the suggestions pane, e.g. update the `actions` entry added previously to read: `x  / right-click   context menu for the focused project / process / suggestion`.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: note context menu on the suggestions pane"
```

---

## Self-Review

**Spec coverage:**
- Visible projection (filter + scroll) shared by render/click/right-click/`x`/nav → `visible_rec_indices`/`visible_recs`/`recs_offset` (T3), consumed in T3 (render, left-click, nav, kill) and T5 (right-click, `x`). ✓
- `selected_rec` as a consistent display index; `selected_rec_raw` for kill paths → T3. ✓
- Stale-pid guard (`process_by_pid(pid).is_some()` → process menu, else Copy reason) → `open_for_rec` (T4). ✓
- `CopyReason` + `build_info_rec` → T1; dispatch arm → T4. ✓
- Submenu anchor fix (placed-rect, `(sw,sh)` before borrow, pure helper + test) → `submenu_anchor` (T2), `open_submenu` (T4). ✓
- Trigger: right-click recs region (modal guard reused) + `x` guarded match arm using `app.sidebar_width` → T5. ✓
- `open_for_rec` no-ops on `None`; `x` bounds via `selected_rec_raw`/clamp → T4/T5. ✓
- Shared `recs_list_top()` used by `handle_mouse` + new code, tested via `recs_list_top_geom` → T3. ✓
- Data-row bound `ev.row < recs_list_end.saturating_sub(1)` on both recs hit-tests → T3, T5. ✓
- Non-goals (process table, other verbs, short-terminal geometry edge) → not implemented. ✓

**Placeholder scan:** none — every step carries complete code. ✓

**Type consistency:** `CopyReason(String)` identical in T1 (variant), T4 (dispatch). `build_info_rec(Caps, String)` T1↔T4. `submenu_anchor(u16×6) -> (u16,u16)` T2↔T4. `visible_recs() -> Vec<usize>`, `recs_offset: usize`, `selected_rec_raw() -> Option<usize>`, `recs_list_top() -> u16` defined T3, consumed T3/T4/T5. `open_for_rec(app, usize, u16, u16)` T4↔delegator↔T5. `bucket_label_for_pid(&[Bucket], u32)` T4 (def + test). `Bucket { key, cpu, mem, net_rx, net_tx, pids }` literal matches the struct used in the prior feature's tests. ✓
