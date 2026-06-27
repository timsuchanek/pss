# pss — right-click context menu on suggestions (recommendations pane)

Date: 2026-06-26
Status: approved (design)

## Why

The left sidebar got a right-click/`x` context menu (shipped). The bottom
"recommendations" pane lists actionable suggestions ("kill next-server [3850] —
dev server idle, holding 2434M"), but the only way to act on one is to drill in
or remember the pid and act elsewhere. Extending the same menu to suggestion
rows closes the loop: point at a suggestion, do the thing. It also delivers the
"extend right-click to the recs pane" follow-up the prior feature's review
flagged, and lets us fix a deferred submenu-anchor nit that only bites on the
right edge — exactly where this pane lives.

## Scope

- **In scope:** right-click (and `x` when the suggestions pane is focused) on a
  recommendation row → the same anchored context menu. For a suggestion that
  targets a process, reuse the existing process menu verbatim; for an info-only
  suggestion, a one-item "Copy reason" menu. Plus the `open_submenu` anchor fix.
- **Out of scope (YAGNI):** right-click on the middle process table; any new
  recommendation-specific actions beyond Copy reason (no "Apply suggestion",
  "Dismiss"); reworking the pre-existing recommendation filter/index mapping
  (the existing click handler maps a screen row to a raw `recommendations`
  index without accounting for visibility filtering — we mirror that behavior
  rather than fix it here).

## Background (current state)

- `Recommendation { pid: Option<u32>, action: String, target: String, reason: String, confidence: u8, estimated_saved_mb: u64 }` (`src/llm.rs:67`).
- `App.recommendations: Vec<Recommendation>`, `App.selected_rec: usize`, pane
  `Pane::Recommendations`.
- The left-click handler already selects a rec row and double-click drills into
  its pid (`src/main.rs`, "Recommendations row click"): it computes
  `idx = ev.row - recs_list_top` and treats `idx` as a raw index into
  `app.recommendations`.
- `handle_mouse` already computes `recs_list_top` and `recs_list_end`.
- The context-menu machinery exists: `menu.rs` (model/builders), `actions.rs`
  (side effects), `ui/context_menu.rs` (geometry + render), `menu_dispatch.rs`
  (`open`/`select`/`click`/`run_action` over `&mut App`), `app.rs` delegators,
  `main.rs` wiring. The menu's target is `crate::app::Selection`.

## Behavior

### Trigger
- **Mouse:** `MouseEventKind::Down(MouseButton::Right)` on a row in the recs
  region (`recs_list_top..recs_list_end`). First bail if any modal is open
  (`kill_menu` / `show_thermal` / `search_active` / `drilldown_pid`) — same
  guard as the sidebar arm. Select the rec (`app.selected_rec = idx`,
  `app.pane = Pane::Recommendations`) and open the menu anchored at
  `(ev.column + 1, ev.row + 1)`.
- **Keyboard:** `x` in normal mode, when `app.pane == Pane::Recommendations`,
  opens the menu for `app.selected_rec`, anchored at
  `(sidebar_width + 2, recs_list_top() + selected_rec)`.

The `x` handler keeps its existing sidebar branch; this adds a recs branch.

### Target resolution
For the chosen recommendation index:
- `rec.pid = Some(pid)` → resolve the bucket label that contains `pid`
  (the same lookup the process-table click uses; `""` fallback if none) → build
  `Selection::Process(label, pid)` → call the existing `open_context_menu`,
  which produces the full **process** menu. No new menu content.
- `rec.pid = None` → build a one-item menu via `build_info_rec(caps, reason)`:
  `⧉ Copy reason`, enabled iff `caps.clipboard`. The `ContextMenu` is created
  with target `Selection::All` (a placeholder — the `CopyReason` action carries
  its own text, so the target is unused for it).

Navigation, cascade submenu, close-on-activate, and the disabled-item rules are
all inherited unchanged.

### Submenu anchor fix (rides along)
`open_submenu` currently anchors the renice cascade at the parent level's raw
`origin_col`/`origin_row`. On the right-edge recs pane, the parent popup is
edge-clamped leftward by `place`, so the cascade would misalign. Fix: compute
the parent's **placed** rect (`place(origin_col, origin_row, MENU_W, n+2, sw, sh)`)
and anchor the submenu at `submenu_origin(placed.x, MENU_W, MENU_W, sw)` /
`placed.y + selected`. This also benefits the sidebar.

## Menu contents

### Process-backed suggestion (`rec.pid = Some`)
Exactly the existing process menu (no changes): Inspect · Kill… · Suspend ·
Resume · Renice ▸ · Copy PID · Copy command · Reveal exe in Finder · Open cwd in
editor · Open cwd in terminal — with the same capability/availability gating.

### Info-only suggestion (`rec.pid = None`)
- `⧉ Copy reason` → `MenuAction::CopyReason(reason)`; enabled iff `caps.clipboard`.

## Architecture

### `src/menu.rs`
- Add `MenuAction::CopyReason(String)`.
- Add `build_info_rec(caps: Caps, reason: String) -> Vec<MenuItem>` returning the
  single Copy-reason item (`enabled: caps.clipboard`, `opens_submenu: false`).
- `outcome_for(CopyReason(_)) => Outcome::Close` (covered by the catch-all arm —
  no change needed beyond the new variant compiling through it).

### `src/menu_dispatch.rs`
- `pub(crate) fn open_for_rec(app: &mut App, rec_index: usize, col: u16, row: u16)`:
  reads `app.recommendations.get(rec_index)`; if `pid` is Some, resolves the
  label and calls `open(app, Selection::Process(label, pid), col, row)`; else
  builds the info menu and sets `app.context_menu` directly with target
  `Selection::All`.
- `fn bucket_label_for_pid(buckets: &[crate::app::Bucket], pid: u32) -> Option<String>`
  — pure: the first bucket whose `pids` contains `pid`, mapped to `key.label()`.
- `run_action` gains `MenuAction::CopyReason(text) => copy(app, &text, "copied reason".into())`.
- `open_submenu` updated to the placed-rect anchoring described above.

### `src/app.rs`
- `pub fn recs_list_top(&self) -> u16` — `term_size.1 - 1 (footer) - recs_height + 1`,
  matching `handle_mouse`'s geometry (saturating).
- `pub fn open_context_menu_for_rec(&mut self, idx: usize, col: u16, row: u16)` —
  thin delegator to `crate::menu_dispatch::open_for_rec`.

### `src/main.rs`
- Extend the `Down(MouseButton::Right)` arm: after the sidebar branch, add a recs
  branch — same modal guard, `idx = ev.row - recs_list_top`, bounds-check against
  `app.recommendations.len()`, then `app.open_context_menu_for_rec(idx, ev.column + 1, ev.row + 1)`.
- Extend the `x` arm: add an `else if app.pane == Pane::Recommendations` branch
  that opens for `app.selected_rec` at `(sidebar_width + 2, recs_list_top() + selected_rec)`.

## Testing

- **Unit (pure):**
  - `build_info_rec`: one item, `CopyReason` action, enabled iff `caps.clipboard`.
  - `bucket_label_for_pid`: returns the right label for a pid present in a
    bucket; `None` when absent. Built from constructed `Bucket` values (no
    `App`/collector).
  - The submenu anchor fix is exercised by the existing `place` /
    `submenu_origin` tests (the change feeds a placed `x` into the already-tested
    helpers).
- **Manual:**
  - Right-click a kill suggestion (has pid) → full process menu; Inspect closes
    the menu and opens the drill-down.
  - Right-click an info-only suggestion → `Copy reason`; selecting it copies the
    reason and shows `copied reason` in the status line.
  - `x` with the suggestions pane focused opens the menu on the selected rec.
  - Open Renice ▸ on a right-edge suggestion → the cascade flips left and aligns
    with the parent item (anchor fix).
  - With a modal open, right-clicking the recs region opens nothing (guard).

## Non-goals / future

- Right-click on the process table (the prior feature's other documented
  follow-up).
- "Apply suggestion" / "Dismiss" or any recommendation-specific verbs beyond
  Copy reason.
- Fixing the pre-existing recs filter-vs-index mismatch in the click handler.
