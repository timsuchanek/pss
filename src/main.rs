mod app;
mod collector;
mod config;
mod heuristics;
mod llm;
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

use crate::app::{App, AppEvent, Pane, ResizeTarget, Selection, SidebarSortKey, SortKey};
use crate::collector::Collector;
use crate::llm::LlmClient;

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = config::load();
    let has_llm = cfg.openrouter_api_key.is_some();
    let llm = LlmClient::new(cfg.openrouter_api_key.clone(), cfg.model.clone());

    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    // sampler task — prime sysinfo so first real sample has live cpu%
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut collector = Collector::new();
            let _ = collector.sample();
            tokio::time::sleep(Duration::from_millis(250)).await;
            let mut tick = interval(Duration::from_millis(1000));
            loop {
                tick.tick().await;
                let snap = collector.sample();
                let _ = tx.send(AppEvent::Snapshot(snap));
            }
        });
    }

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

    let mut app = App::new();
    app.has_llm = has_llm;
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
                            match llm.recommend(&digest).await {
                                Ok(recs) => {
                                    let _ = tx.send(AppEvent::Recommendations(recs));
                                }
                                Err(_) => {}
                            }
                        });
                    }
                }
                AppEvent::Recommendations(r) => app.set_recommendations(r),
            }
        }

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

                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Esc => {
                            if app.drilldown_pid.is_some() {
                                app.close_drilldown();
                            } else if app.show_help {
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
                            }
                        }
                        KeyCode::Char('j') | KeyCode::Down => app.nav_down(),
                        KeyCode::Char('k') | KeyCode::Up => app.nav_up(),
                        KeyCode::Char('h') | KeyCode::Left => app.collapse(),
                        KeyCode::Char('l') | KeyCode::Right => app.expand(),
                        KeyCode::Tab => app.cycle_pane(),
                        KeyCode::Char('K') => app.kill_selected(),
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
                        _ => {}
                    }
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
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
    let sidebar_list_top = body_top + 2;

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
    let gutter = crate::ui::sidebar::COL_GUTTER as u16;
    // Right edge of inner area: sidebar_w - 1 (account for block right border).
    let inner_right = sidebar_w.saturating_sub(1);
    let mem_right = inner_right;
    let mem_left = mem_right.saturating_sub(mem_col_w);
    let cpu_right = mem_left.saturating_sub(gutter);
    let cpu_left = cpu_right.saturating_sub(cpu_col_w);

    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
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
                            Selection::Process(_, pid) => app.open_drilldown(pid),
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
                return;
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            app.drag(ev.column, ev.row, term_w, term_h);
        }
        MouseEventKind::Up(MouseButton::Left) => {
            app.end_resize();
        }
        _ => {}
    }
}
