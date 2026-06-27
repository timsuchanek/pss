mod actions;
mod app;
mod collector;
mod config;
mod details;
mod heuristics;
mod llm;
mod menu;
mod menu_dispatch;
mod netmon;
mod thermal;
mod ui;

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
    MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;
use tokio::time::{Instant, interval};
// `interval` only needed for the recommender ticker below; sampler uses sleep().

use crate::app::{
    App, AppEvent, DrilldownTab, Pane, ResizeTarget, Selection, SidebarSortKey, SortKey,
};
use crate::collector::Collector;
use crate::llm::LlmClient;

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = config::load();
    let has_llm = cfg.openrouter_api_key.is_some();
    let llm = LlmClient::new(cfg.openrouter_api_key.clone(), cfg.model.clone());

    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    // sampler task — prime sysinfo so first real sample has live cpu%
    let mut app = App::new();
    app.has_llm = has_llm;
    app.apply_config(&cfg);
    let sampler = app.sampler.clone();
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            use std::sync::atomic::Ordering;
            let mut collector = Collector::new();
            let _ = collector.sample();
            tokio::time::sleep(Duration::from_millis(250)).await;
            loop {
                let interval_ms = sampler.interval_ms.load(Ordering::Relaxed).max(100);
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
                if sampler.paused.load(Ordering::Relaxed) {
                    continue;
                }
                let snap = collector.sample();
                let _ = tx.send(AppEvent::Snapshot(snap));
            }
        });
    }

    // thermal sampler — IOHID on macOS, no-op elsewhere. IOHID internally
    // rate-limits at ~1Hz so faster polling is wasted.
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let Some(reader) = crate::thermal::ThermalReader::new() else {
                return;
            };
            loop {
                let sensors = reader.read();
                let snap = crate::thermal::ThermalSnapshot { sensors };
                if tx.send(AppEvent::Thermal(snap)).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1000));
            }
        });
    }

    // net sampler — aggregate rx/tx rates from getifaddrs, 1 Hz.
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let Some(mut mon) = crate::netmon::NetMon::new() else {
                return;
            };
            // Prime so first published sample has a real delta.
            let _ = mon.sample();
            loop {
                std::thread::sleep(Duration::from_millis(1000));
                if let Some(r) = mon.sample() {
                    if tx.send(AppEvent::Net(r)).is_err() {
                        break;
                    }
                }
            }
        });
    }

    // per-PID network rates via a streaming `nettop -d` child. Kept alive
    // for the lifetime of `_per_pid_sampler`; Drop kills the child.
    let _per_pid_sampler = {
        let tx = tx.clone();
        crate::netmon::PerPidSampler::spawn(move |r| {
            let _ = tx.send(AppEvent::PerPidNet(r));
        })
    };

    // recommender task — only runs when a key is configured
    if has_llm {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(10));
            loop {
                tick.tick().await;
                let _ = tx.send(AppEvent::RequestRecommendations);
            }
        });
    }

    // terminal setup
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut last_render = Instant::now();

    loop {
        // drain pending events
        while let Ok(ev) = rx.try_recv() {
            match ev {
                AppEvent::Snapshot(s) => app.push_snapshot(s),
                AppEvent::RequestRecommendations => {
                    if let Some(digest) = app.digest_for_llm() {
                        let tx = tx.clone();
                        let llm = llm.clone();
                        tokio::spawn(async move {
                            if let Ok(recs) = llm.recommend(&digest).await {
                                let _ = tx.send(AppEvent::Recommendations(recs));
                            }
                        });
                    }
                }
                AppEvent::Recommendations(r) => app.set_recommendations(r),
                AppEvent::Thermal(t) => app.set_thermal(t),
                AppEvent::Net(r) => app.set_net(r),
                AppEvent::PerPidNet(r) => app.set_per_pid_net(r),
            }
        }

        app.expire_status();
        if last_render.elapsed() >= Duration::from_millis(100) {
            terminal.draw(|f| ui::render(f, &mut app))?;
            last_render = Instant::now();
        }

        if event::poll(Duration::from_millis(50))? {
            let ev = event::read()?;
            let (tw, th) = crossterm::terminal::size().unwrap_or((80, 24));
            if let Event::Mouse(me) = ev {
                handle_mouse(&mut app, me, tw, th);
                continue;
            }
            if let Event::Key(key) = ev {
                if key.kind == KeyEventKind::Press {
                    if key.modifiers.contains(event::KeyModifiers::CONTROL)
                        && matches!(key.code, KeyCode::Char('c'))
                    {
                        break;
                    }

                    // Search-mode input hijacks most keys.
                    if app.search_active {
                        match key.code {
                            KeyCode::Esc => app.exit_search_cancel(),
                            KeyCode::Enter => app.exit_search_commit(),
                            KeyCode::Backspace => app.search_pop(),
                            KeyCode::Char(c)
                                if !key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                            {
                                app.search_push(c);
                            }
                            KeyCode::Up => app.nav_up(),
                            KeyCode::Down => app.nav_down(),
                            _ => {}
                        }
                        continue;
                    }

                    // Thermal overlay hijacks input while open.
                    if app.show_thermal {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('T') => {
                                app.show_thermal = false;
                                continue;
                            }
                            KeyCode::Char('j') | KeyCode::Down => {
                                app.thermal_scroll_by(1);
                                continue;
                            }
                            KeyCode::Char('k') | KeyCode::Up => {
                                app.thermal_scroll_by(-1);
                                continue;
                            }
                            KeyCode::PageDown => {
                                app.thermal_scroll_by(10);
                                continue;
                            }
                            KeyCode::PageUp => {
                                app.thermal_scroll_by(-10);
                                continue;
                            }
                            _ => {
                                continue;
                            }
                        }
                    }

                    // Kill menu hijacks input while open.
                    if app.kill_menu.is_some() {
                        use sysinfo::Signal;
                        match key.code {
                            KeyCode::Esc => {
                                app.close_kill_menu();
                                continue;
                            }
                            KeyCode::Enter | KeyCode::Char('t') => {
                                app.kill_with_signal(Signal::Term);
                                continue;
                            }
                            KeyCode::Char('k') => {
                                app.kill_with_signal(Signal::Kill);
                                continue;
                            }
                            KeyCode::Char('h') => {
                                app.kill_with_signal(Signal::Hangup);
                                continue;
                            }
                            KeyCode::Char('i') => {
                                app.kill_with_signal(Signal::Interrupt);
                                continue;
                            }
                            KeyCode::Char('s') => {
                                app.kill_with_signal(Signal::Stop);
                                continue;
                            }
                            KeyCode::Char('c') => {
                                app.kill_with_signal(Signal::Continue);
                                continue;
                            }
                            KeyCode::Char('q') => {
                                app.kill_with_signal(Signal::Quit);
                                continue;
                            }
                            KeyCode::Char('1') => {
                                app.kill_with_signal(Signal::User1);
                                continue;
                            }
                            KeyCode::Char('2') => {
                                app.kill_with_signal(Signal::User2);
                                continue;
                            }
                            _ => {
                                continue;
                            }
                        }
                    }

                    // Context menu hijacks input while open.
                    if app.context_menu.is_some() {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') => {
                                app.context_menu_back();
                                continue;
                            }
                            KeyCode::Char('j') | KeyCode::Down => {
                                app.context_menu_nav(1);
                                continue;
                            }
                            KeyCode::Char('k') | KeyCode::Up => {
                                app.context_menu_nav(-1);
                                continue;
                            }
                            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                                app.context_menu_select();
                                continue;
                            }
                            KeyCode::Char('h') | KeyCode::Left => {
                                app.context_menu_pop_only();
                                continue;
                            }
                            _ => {
                                continue;
                            }
                        }
                    }

                    // Drill-down modal hijacks a few keys when it's open.
                    if app.drilldown_pid.is_some() {
                        match key.code {
                            KeyCode::Esc => {
                                app.close_drilldown();
                                continue;
                            }
                            KeyCode::Char('r') => {
                                app.refresh_drilldown();
                                continue;
                            }
                            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                                app.drilldown_next_tab();
                                continue;
                            }
                            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                                app.drilldown_prev_tab();
                                continue;
                            }
                            KeyCode::Char('1') => {
                                app.drilldown_set_tab(DrilldownTab::Facts);
                                continue;
                            }
                            KeyCode::Char('2') => {
                                app.drilldown_set_tab(DrilldownTab::Env);
                                continue;
                            }
                            KeyCode::Char('3') => {
                                app.drilldown_set_tab(DrilldownTab::Files);
                                continue;
                            }
                            KeyCode::Char('4') => {
                                app.drilldown_set_tab(DrilldownTab::Sockets);
                                continue;
                            }
                            KeyCode::Char('5') => {
                                app.drilldown_set_tab(DrilldownTab::Tree);
                                continue;
                            }
                            KeyCode::Char('j') | KeyCode::Down => {
                                app.drilldown_scroll_by(1);
                                continue;
                            }
                            KeyCode::Char('k') | KeyCode::Up => {
                                app.drilldown_scroll_by(-1);
                                continue;
                            }
                            KeyCode::PageDown => {
                                app.drilldown_scroll_by(10);
                                continue;
                            }
                            KeyCode::PageUp => {
                                app.drilldown_scroll_by(-10);
                                continue;
                            }
                            KeyCode::Char('K') => {
                                app.open_kill_menu();
                                continue;
                            }
                            _ => {
                                continue;
                            }
                        }
                    }

                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Esc => {
                            if app.show_help {
                                app.show_help = false;
                            } else if !app.search_query.is_empty() {
                                app.exit_search_cancel();
                            } else {
                                break;
                            }
                        }
                        KeyCode::Char('/') => app.enter_search(),
                        KeyCode::Char('?') => app.toggle_help(),
                        KeyCode::Enter => {
                            if let Some(pid) = app.selected_pid() {
                                app.open_drilldown(pid);
                                app.ensure_drilldown_loaded_for_tab();
                            }
                        }
                        KeyCode::Char('j') | KeyCode::Down => app.nav_down(),
                        KeyCode::Char('k') | KeyCode::Up => app.nav_up(),
                        KeyCode::Char('h') | KeyCode::Left => app.collapse(),
                        KeyCode::Char('l') | KeyCode::Right => app.expand(),
                        KeyCode::Tab => app.cycle_pane(),
                        KeyCode::Char('K') => app.open_kill_menu(),
                        KeyCode::Char(' ') => app.toggle_paused(),
                        KeyCode::Char('[') => app.sampling_faster(),
                        KeyCode::Char(']') => app.sampling_slower(),
                        KeyCode::Char('H') => app.toggle_hide_kernel(),
                        KeyCode::Char('U') => app.toggle_only_my_uid(),
                        KeyCode::Char('S') => app.toggle_hide_self(),
                        KeyCode::Char('c') => {
                            if app.pane == crate::app::Pane::Sidebar {
                                app.sidebar_toggle_sort(SidebarSortKey::Cpu);
                            } else {
                                app.set_sort(SortKey::Cpu);
                            }
                        }
                        KeyCode::Char('m') => {
                            if app.pane == crate::app::Pane::Sidebar {
                                app.sidebar_toggle_sort(SidebarSortKey::Mem);
                            } else {
                                app.set_sort(SortKey::Mem);
                            }
                        }
                        KeyCode::Char('n') => app.set_sort(SortKey::Name),
                        KeyCode::Char('w') => app.set_sort(SortKey::Net),
                        KeyCode::Char('T') => app.toggle_thermal_overlay(),
                        KeyCode::Char('x')
                            if app.pane == crate::app::Pane::Sidebar => {
                                if let Some(idx) = app.selected_tree_index() {
                                    let sel = app.selection.clone();
                                    let row = app.sidebar_list_top() + idx as u16;
                                    app.open_context_menu(sel, 2, row);
                                }
                            }
                        _ => {}
                    }
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    // Persist UI state (best-effort; swallow errors).
    let _ = config::save(&app.to_config_patch(&cfg));
    Ok(())
}

fn handle_mouse(app: &mut App, ev: MouseEvent, term_w: u16, term_h: u16) {
    // Layout geometry — must match ui::mod::render_body.
    let sidebar_w = app.sidebar_width;
    let header = 1u16;
    let footer = 1u16;
    let search_h: u16 = if app.search_active || !app.search_query.is_empty() {
        1
    } else {
        0
    };
    let body_top = header + search_h;
    let chart_top = body_top;
    let chart_bottom = chart_top + app.chart_height; // first row after chart
    let recs_top = term_h.saturating_sub(footer + app.recs_height);

    // Sidebar rows: block top border at body_top, header row at body_top+1,
    // list rows start at body_top+2.
    let sidebar_list_top = app.sidebar_list_top();

    // Process table: block top border at chart_bottom, header row at chart_bottom+1,
    // data rows start at chart_bottom+2. Data ends at recs_top - detail(2) - 1.
    let processes_list_top = chart_bottom + 2;
    let detail_h: u16 = 2;
    let processes_list_end = recs_top.saturating_sub(detail_h); // exclusive

    // Recs block: top border at recs_top, data rows start at recs_top + 1.
    let recs_list_top = recs_top + 1;
    let recs_list_end = term_h.saturating_sub(footer); // exclusive

    // Sidebar header row lives at: body_top + block_top(1).
    let sidebar_header_y = body_top + 1;
    let cpu_col_w = crate::ui::sidebar::CPU_COL_W as u16;
    let mem_col_w = crate::ui::sidebar::MEM_COL_W as u16;
    let net_pair_w = crate::ui::sidebar::NET_PAIR_W as u16;
    let gutter = crate::ui::sidebar::COL_GUTTER as u16;
    // Right edge of inner area: sidebar_w - 1 (account for block right border).
    let inner_right = sidebar_w.saturating_sub(1);
    // Treat the ↑/↓ pair as one hit region — clicking either header cycles
    // net sort direction.
    let net_right = inner_right;
    let net_left = net_right.saturating_sub(net_pair_w);
    let mem_right = net_left.saturating_sub(gutter);
    let mem_left = mem_right.saturating_sub(mem_col_w);
    let cpu_right = mem_left.saturating_sub(gutter);
    let cpu_left = cpu_right.saturating_sub(cpu_col_w);

    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // An open context menu consumes the click: pick an item or close.
            if app.context_menu.is_some() {
                app.context_menu_click(ev.column, ev.row);
                return;
            }
            // Close the drill-down modal if the user clicks outside it.
            if app.drilldown_pid.is_some() {
                app.close_drilldown();
                return;
            }

            // Sidebar header clicks: sort by cpu / mem.
            if ev.row == sidebar_header_y && ev.column < sidebar_w {
                if ev.column >= cpu_left && ev.column < cpu_right {
                    app.sidebar_toggle_sort(SidebarSortKey::Cpu);
                    return;
                }
                if ev.column >= mem_left && ev.column < mem_right {
                    app.sidebar_toggle_sort(SidebarSortKey::Mem);
                    return;
                }
                if ev.column >= net_left && ev.column < net_right {
                    app.sidebar_toggle_sort(SidebarSortKey::Net);
                    return;
                }
            }
            // Sidebar tree row click.
            if ev.column < sidebar_w && ev.row >= sidebar_list_top && ev.row < term_h - footer {
                let idx = (ev.row - sidebar_list_top) as usize;
                let rows = app.tree_rows();
                if let Some(row) = rows.get(idx) {
                    let was_same = row.selection == app.selection;
                    app.selection = row.selection.clone();
                    app.pane = Pane::Sidebar;
                    // second click on same row: expand/collapse or drill into process
                    if was_same {
                        match row.selection.clone() {
                            Selection::Bucket(label) => {
                                if app.collapsed.contains(&label) {
                                    app.collapsed.remove(&label);
                                } else {
                                    app.collapsed.insert(label);
                                }
                            }
                            Selection::Process(_, pid) => {
                                app.open_drilldown(pid);
                                app.ensure_drilldown_loaded_for_tab();
                            }
                            Selection::All => {}
                        }
                    }
                    return;
                }
            }
            // Process table row click.
            if ev.column > sidebar_w
                && ev.row >= processes_list_top
                && ev.row < processes_list_end
            {
                let idx = (ev.row - processes_list_top) as usize;
                let picked_pid: Option<u32> = {
                    let procs = app.procs_in_selected();
                    procs.get(idx).map(|p| p.pid)
                };
                if let Some(pid) = picked_pid {
                    let was_same = Some(pid) == app.selected_pid();
                    let bucket_label: Option<String> = app
                        .selected_bucket_ref()
                        .map(|b| b.key.label())
                        .or_else(|| {
                            app.buckets
                                .iter()
                                .find(|b| b.pids.contains(&pid))
                                .map(|b| b.key.label())
                        });
                    if let Some(lab) = bucket_label {
                        app.selection = Selection::Process(lab, pid);
                    }
                    app.pane = Pane::Processes;
                    if was_same {
                        app.open_drilldown(pid);
                        app.ensure_drilldown_loaded_for_tab();
                    }
                    return;
                }
            }
            // Recommendations row click.
            if ev.column > sidebar_w
                && ev.row >= recs_list_top
                && ev.row < recs_list_end
            {
                let idx = (ev.row - recs_list_top) as usize;
                if idx < app.recommendations.len() {
                    let was_same = app.selected_rec == idx && app.pane == Pane::Recommendations;
                    app.selected_rec = idx;
                    app.pane = Pane::Recommendations;
                    if was_same {
                        if let Some(pid) = app.recommendations.get(idx).and_then(|r| r.pid) {
                            app.open_drilldown(pid);
                            app.ensure_drilldown_loaded_for_tab();
                        }
                    }
                    return;
                }
            }
            // Vertical border between sidebar and right pane?
            if ev.column == sidebar_w && ev.row >= header && ev.row < term_h - footer {
                app.begin_resize(ResizeTarget::SidebarWidth);
                return;
            }
            // Horizontal border: chart / processes
            if ev.row == chart_bottom && ev.column > sidebar_w {
                app.begin_resize(ResizeTarget::ChartHeight);
                return;
            }
            // Horizontal border: detail / recs
            if ev.row == recs_top && ev.column > sidebar_w {
                app.begin_resize(ResizeTarget::RecsHeight);
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            app.drag(ev.column, ev.row, term_w, term_h);
        }
        MouseEventKind::Up(MouseButton::Left) => {
            app.end_resize();
        }
        MouseEventKind::Down(MouseButton::Right) => {
            // Don't stack the menu under another modal.
            if app.kill_menu.is_some()
                || app.show_thermal
                || app.search_active
                || app.drilldown_pid.is_some()
            {
                return;
            }
            if ev.column < sidebar_w
                && ev.row >= sidebar_list_top
                && ev.row < term_h.saturating_sub(footer)
            {
                let idx = (ev.row - sidebar_list_top) as usize;
                let rows = app.tree_rows();
                if let Some(row) = rows.get(idx) {
                    app.selection = row.selection.clone();
                    app.pane = Pane::Sidebar;
                    let sel = row.selection.clone();
                    app.open_context_menu(sel, ev.column + 1, ev.row + 1);
                }
            }
        }
        _ => {}
    }
}
