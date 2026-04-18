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

use crate::app::{App, AppEvent, ResizeTarget, SortKey};
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
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Esc => {
                            if app.show_help {
                                app.show_help = false;
                            } else {
                                break;
                            }
                        }
                        KeyCode::Char('?') => app.toggle_help(),
                        KeyCode::Char('j') | KeyCode::Down => app.nav_down(),
                        KeyCode::Char('k') | KeyCode::Up => app.nav_up(),
                        KeyCode::Char('h') | KeyCode::Left => app.collapse(),
                        KeyCode::Char('l') | KeyCode::Right => app.expand(),
                        KeyCode::Tab => app.cycle_pane(),
                        KeyCode::Char('K') => app.kill_selected(),
                        KeyCode::Char('c') => app.set_sort(SortKey::Cpu),
                        KeyCode::Char('m') => app.set_sort(SortKey::Mem),
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
    // Header occupies row 0; footer occupies last row.
    let sidebar_w = app.sidebar_width;
    let header = 1u16;
    let footer = 1u16;
    let chart_top = header;
    let chart_bottom = chart_top + app.chart_height; // first row after chart
    let recs_top = term_h.saturating_sub(footer + app.recs_height);

    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
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
            // Horizontal border: detail / recs (recs top minus 0 = recs block border)
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
