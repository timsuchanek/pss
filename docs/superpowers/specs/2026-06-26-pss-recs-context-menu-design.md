# pss — right-click context menu on suggestions (recommendations pane)

Date: 2026-06-26
Status: approved (design, rev 2 — incorporates spec review)

## Why

The left sidebar got a right-click/`x` context menu (shipped). The bottom
"recommendations" pane lists actionable suggestions ("kill next-server [3850] —
dev server idle, holding 2434M"), but the only way to act on one is to drill in
or remember the pid and act elsewhere. Extending the same menu to suggestion
rows closes the loop.

Doing this safely forces a real fix: the recs pane's row→recommendation mapping
is currently **broken** under filtering and scrolling, and `selected_rec` is
used inconsistently. That was tolerable for read-only select/drill, but a
context menu drives **destructive** actions (kill/suspend/renice), so a
wrong-row mapping would mean killing the wrong process. We fix the index model
with one shared *visible projection* — which also repairs the pre-existing
wrong-row left-click/drill bug.

## The bug we must fix (verified in source)

- The renderer draws a **filtered** subset (`src/ui/recommendations.rs:26-42`,
  skips recs whose pid isn't visible / text doesn't match the search query) and
  uses a `ListState`, so the list **scrolls** to keep the selection visible.
- The left-click handler maps a screen row straight into the **raw** vector:
  `idx = ev.row - recs_list_top; app.recommendations[idx]` (`src/main.rs`), and
  sets `selected_rec = idx` as a **raw** index.
- `selected_rec` is read three inconsistent ways: the renderer treats it as a
  **display** index (`selected_rec.min(filtered.len()-1)`), while nav
  (`src/app.rs:1057,1082`) and `kill_selected` / `open_kill_menu`
  (`src/app.rs:1271,475`) treat it as a **raw** index.
- The modal guard alone can't paper over this: `exit_search_commit`
  (`src/app.rs:779`) sets `search_active = false` but leaves the filter applied,
  so after committing a search the list is filtered while `search_active` is
  false — a guard on `search_active` passes, and a raw-index right-click targets
  the wrong recommendation.

## Scope

- **In scope:** right-click (and `x` when the suggestions pane is focused) on a
  recommendation row → the anchored context menu. For a suggestion that targets
  a **live** process, reuse the existing process menu verbatim; otherwise a
  one-item "Copy reason" menu. **Plus** the shared visible-projection fix (render
  + left-click + right-click + `x` + nav all consume it; `selected_rec` becomes a
  consistent display index) and the `open_submenu` right-edge anchor fix.
- **Out of scope (YAGNI):** right-click on the middle process table; any
  recommendation-specific verbs beyond Copy reason ("Apply", "Dismiss"); fixing
  the pre-existing short-terminal geometry edge where `recs_list_top` (derived
  from `recs_height`) can disagree with the clamped render area
  (`src/ui/mod.rs:515`) — `handle_mouse` already has this; we stay consistent
  with it.

## Recommendations visible projection (index-model fix)

One source of truth for "what the recs pane shows", consumed everywhere:

- **`visible_rec_indices(recs, query, is_pid_visible) -> Vec<usize>`** — a pure
  free function returning the **raw** indices of recommendations that pass the
  same filter the renderer uses, in display order. (Filter: empty query ⇒ all;
  else keep if `pid` is visible, or `target`/`reason` contains the lowercased
  query.)
- **`App::visible_recs(&self) -> Vec<usize>`** — calls the pure fn with
  `self.search_query` and `|pid| self.pid_visible(pid)`.
- **`App::recs_offset: usize`** (new field) — the list scroll offset, written by
  the renderer each frame so it reflects exactly what was drawn.
- **`App::selected_rec` is redefined as a display index** (position within
  `visible_recs()`), used consistently by render, nav, and the kill paths.
- **`App::selected_rec_raw(&self) -> Option<usize>`** — `visible_recs().get(self.selected_rec).copied()`; the raw index of the selected rec, or `None`.

Row mapping (used by mouse + `x`): a visible data row `vr = row - recs_list_top`
maps to display index `recs_offset + vr`, and to raw index
`visible_recs().get(recs_offset + vr)`.

### Renderer changes (`src/ui/recommendations.rs`)
`render` takes `&mut App`. It builds the item list from `app.visible_recs()`
(mapping each raw index to `&app.recommendations[i]`), computes the scroll
**offset** that keeps the clamped `selected_rec` visible within the inner area
height, stores it in `app.recs_offset`, sets the `ListState` offset + selected,
and renders. (Computing and setting the offset ourselves — rather than reading
it back — keeps the stored value exactly equal to what is drawn.)

### Selection-consumer updates (`src/app.rs`)
- `nav` bounds (1057/1082): `selected_rec + 1 < self.visible_recs().len()` /
  `saturating_sub(1)`.
- `set_recommendations` clamp (1303): clamp `selected_rec` to
  `visible_recs().len().saturating_sub(1)`.
- `open_kill_menu` Recommendations branch (475) and `kill_selected` (1271): use
  `self.selected_rec_raw()` → `self.recommendations[raw]` (None ⇒ no-op).

## Behavior (context menu)

### Trigger
- **Mouse:** `Down(MouseButton::Right)` on a row in `recs_list_top..recs_list_end`.
  First bail if any modal is open (`kill_menu` / `show_thermal` /
  `search_active` / `drilldown_pid`) — same guard as the sidebar arm. (No need to
  bail on a committed filter: the projection maps correctly.) Compute the raw
  index via the projection; if present, set `selected_rec` to that display index,
  `pane = Recommendations`, and open the menu at `(ev.column + 1, ev.row + 1)`.
- **Keyboard:** a second guarded match arm
  `KeyCode::Char('x') if app.pane == Pane::Recommendations =>` opens the menu for
  `selected_rec_raw()` (None ⇒ nothing), anchored at
  `(app.sidebar_width + 2, recs_list_top() + (selected_rec - recs_offset))`.

### Target resolution (`open_for_rec(app, raw_idx, col, row)`)
- `recommendations.get(raw_idx)` is `None` ⇒ do nothing.
- `rec.pid = Some(pid)` **and** `process_by_pid(pid).is_some()` (the pid is live
  in the latest snapshot) ⇒ resolve the bucket label via
  `bucket_label_for_pid(&app.buckets, pid)` (`""` if the live pid is in no
  bucket — harmless, since process-menu actions key off the pid, not the label)
  ⇒ `open(app, Selection::Process(label, pid), col, row)` (the existing process
  menu, unchanged).
- otherwise (no pid, or a **stale** pid no longer in the snapshot) ⇒ build the
  info menu `build_info_rec(caps, rec.reason)` and set `context_menu` with a
  `Selection::All` placeholder target (unused: `CopyReason` carries its own text).

This staleness check prevents a stale LLM/heuristic pid from opening Kill/Inspect
on a dead process (Inspect would otherwise open an empty drill-down).

### Submenu anchor fix (rides along; right edge needs it)
`open_submenu` currently anchors the renice cascade at the parent level's
**unclamped** `origin_col`/raw `origin_row` (it applies `submenu_origin` to
`origin_col`, ignoring `place`'s clamping). On the right-edge recs pane the
parent popup is clamped leftward, so the cascade would misalign. Replace the
inline math with a pure helper:

```
ui::context_menu::submenu_anchor(origin_col, origin_row, n_items, selected, sw, sh) -> (u16, u16)
  let placed = place(origin_col, origin_row, MENU_W, n_items + 2, sw, sh);
  ( submenu_origin(placed.x, MENU_W, MENU_W, sw), placed.y + selected )
```

`open_submenu` reads `let (sw, sh) = app.term_size;` **before** taking the
`context_menu.as_mut()` borrow, then calls `submenu_anchor(level.origin_col,
level.origin_row, level.items.len() as u16, level.selected as u16, sw, sh)`.
This also changes shipped sidebar behavior at the bottom edge (y now follows the
clamp) — intended.

## Menu contents

- **Live process-backed suggestion:** the existing process menu, unchanged.
- **Info / stale suggestion:** `⧉ Copy reason` → `MenuAction::CopyReason(reason)`;
  enabled iff `caps.clipboard`; activating copies the reason and sets the status
  line to `copied reason`.

## Architecture summary

- `src/menu.rs` — `MenuAction::CopyReason(String)`; `build_info_rec(caps, reason)`.
- `src/ui/context_menu.rs` — pure `submenu_anchor(...)` + unit test.
- `src/menu_dispatch.rs` — `open_for_rec`; pure `bucket_label_for_pid(&[Bucket], pid)`;
  `CopyReason` dispatch arm; `open_submenu` rewritten to use `submenu_anchor`.
- `src/app.rs` — pure `visible_rec_indices(...)`; `visible_recs`, `selected_rec_raw`,
  `recs_offset` field, `recs_list_top()`; nav/clamp/kill-path updates;
  `open_context_menu_for_rec(idx, col, row)` delegator.
- `src/ui/recommendations.rs` — `render(&mut App)`, consumes `visible_recs()`,
  stores `recs_offset`.
- `src/main.rs` — recs left-click handler rewritten to the projection mapping
  (fixes the pre-existing bug); recs right-click arm (modal guard + projection);
  `x` recs match arm. `handle_mouse` uses `app.recs_list_top()`.

## Testing

- **Unit (pure):**
  - `visible_rec_indices`: empty query ⇒ all indices; with a query ⇒ only
    matching raw indices in order; pid-visible inclusion path covered.
  - `selected_rec_raw` mapping: display index → expected raw index (via a small
    constructed projection).
  - `submenu_anchor`: near the right edge the col flips left; near the bottom the
    row follows the clamped `placed.y`; in open space it sits to the right at
    `origin_row + selected`.
  - `bucket_label_for_pid`: correct label when the pid is in a bucket; `None`
    otherwise (built from constructed `Bucket` values — no `App`).
  - `build_info_rec`: one `CopyReason` item, enabled iff `caps.clipboard`.
  - `recs_list_top()`: equals the inline `handle_mouse` formula for sample
    `(term_h, recs_height)` (shared helper ⇒ no drift).
- **Manual:**
  - Type a search that filters the recs, press Enter (commit), then right-click a
    visible suggestion → the menu targets **that** suggestion (not a raw-index
    mismatch). Repeat with enough recs to scroll.
  - Right-click a live kill suggestion → full process menu; Inspect closes the
    menu and opens the drill-down.
  - Right-click an info-only or **stale** suggestion → `Copy reason`; selecting
    copies the reason and shows `copied reason`.
  - `x` with the suggestions pane focused opens on the selected rec.
  - Renice ▸ on a right-edge suggestion → the cascade flips left and aligns with
    the parent item.
  - With a modal open, right-clicking the recs region opens nothing.

## Non-goals / future

- Right-click on the process table.
- "Apply suggestion" / "Dismiss" or other recommendation verbs.
- The short-terminal `recs_list_top` vs clamped-render-area geometry edge
  (pre-existing in `handle_mouse`; we stay consistent with it).
