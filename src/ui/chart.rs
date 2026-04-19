use std::collections::HashMap;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::App;
use crate::collector::ProcSample;

use super::titled_block;

const TOP_N: usize = 5;
const OTHER: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Cpu,
    Mem,
}

impl Metric {
    fn title(self) -> &'static str {
        match self {
            Metric::Cpu => "cpu over time",
            Metric::Mem => "mem over time",
        }
    }
    fn value(self, p: &ProcSample) -> f32 {
        match self {
            Metric::Cpu => p.cpu,
            // Work in MB to keep the numbers tame; we format nicely for display.
            Metric::Mem => p.mem as f32 / 1024.0 / 1024.0,
        }
    }
    /// Minimum top-of-chart value (in metric-native units) so small signals still read.
    fn min_scale(self) -> f32 {
        match self {
            Metric::Cpu => 100.0,
            Metric::Mem => 256.0, // 256 MB
        }
    }
}

pub fn render(f: &mut Frame, area: Rect, app: &App, metric: Metric) {
    let label_suffix = match app.selected_bucket_ref() {
        Some(b) => format!("— {}", b.key.label()),
        None => "— all".to_string(),
    };

    let mut selected_pids: Option<std::collections::HashSet<u32>> =
        app.selected_bucket_ref().map(|b| b.pids.iter().copied().collect());

    // Intersect with the fuzzy-search pid filter, if active.
    if let Some(fuzzy) = app.fuzzy_pids.as_ref() {
        selected_pids = Some(match selected_pids {
            Some(bucket) => bucket.intersection(fuzzy).copied().collect(),
            None => fuzzy.clone(),
        });
    }

    let (series_pids, series_names) = pick_top_series(app, &selected_pids, metric);
    let legend_w = area.width.saturating_sub(2) as usize;
    let (column_data, peak) = build_columns(
        app,
        &selected_pids,
        &series_pids,
        legend_w.saturating_sub(6),
        metric,
    );

    let peak_str = match metric {
        Metric::Cpu => format!("peak {:.1}%", peak),
        Metric::Mem => format!("peak {}", fmt_mem_mb(peak)),
    };
    let title = format!("{} {}   {}", metric.title(), label_suffix, peak_str);
    let block = titled_block(&title, false);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 3 || inner.width < 10 {
        return;
    }

    let chart_area = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
    let legend_area = Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(1),
        inner.width,
        1,
    );

    let chart = StackedChart {
        columns: &column_data,
        series_count: series_pids.len() + 1,
        min_scale: metric.min_scale(),
    };
    f.render_widget(chart, chart_area);

    // Legend — include per-series peak value in the metric's units.
    let peaks = per_series_peak(&column_data, &series_pids);
    let mut spans = Vec::new();
    for (i, (pid, name)) in series_pids.iter().zip(series_names.iter()).enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled("█ ", Style::default().fg(color_for(*pid, i))));
        spans.push(Span::styled(
            truncate(name, 14),
            Style::default().fg(Color::Gray),
        ));
        let peak = peaks.get(pid).copied().unwrap_or(0.0);
        let peak_fmt = match metric {
            Metric::Cpu => format!(" {:.0}%", peak),
            Metric::Mem => format!(" {}", fmt_mem_mb(peak)),
        };
        spans.push(Span::styled(peak_fmt, Style::default().fg(Color::DarkGray)));
    }
    if !series_pids.is_empty() {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled("█ ", Style::default().fg(Color::DarkGray)));
    spans.push(Span::styled("other", Style::default().fg(Color::Gray)));
    f.render_widget(ratatui::widgets::Paragraph::new(Line::from(spans)), legend_area);
}

fn pick_top_series(
    app: &App,
    filter: &Option<std::collections::HashSet<u32>>,
    metric: Metric,
) -> (Vec<u32>, Vec<String>) {
    let mut totals: HashMap<u32, (f32, String)> = HashMap::new();
    for snap in app.history.buf.iter() {
        for p in &snap.procs {
            if let Some(f) = filter {
                if !f.contains(&p.pid) {
                    continue;
                }
            }
            let entry = totals.entry(p.pid).or_insert_with(|| (0.0, p.name.clone()));
            entry.0 += metric.value(p);
        }
    }
    let mut v: Vec<_> = totals.into_iter().collect();
    v.sort_by(|a, b| b.1.0.partial_cmp(&a.1.0).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(TOP_N);
    let pids = v.iter().map(|(pid, _)| *pid).collect();
    let names = v.into_iter().map(|(_, (_, n))| n).collect();
    (pids, names)
}

fn build_columns(
    app: &App,
    filter: &Option<std::collections::HashSet<u32>>,
    series: &[u32],
    width: usize,
    metric: Metric,
) -> (Vec<Column>, f32) {
    let history = &app.history.buf;
    if history.is_empty() || width == 0 {
        return (Vec::new(), 0.0);
    }

    let skip = history.len().saturating_sub(width);
    let mut cols = Vec::with_capacity(width.min(history.len()));
    let mut peak = 0.0f32;

    for snap in history.iter().skip(skip) {
        let mut values: Vec<(u32, f32)> =
            series.iter().map(|pid| (*pid, 0.0f32)).collect();
        let mut other = 0.0f32;
        for p in &snap.procs {
            if let Some(f) = filter {
                if !f.contains(&p.pid) {
                    continue;
                }
            }
            let v = metric.value(p);
            if let Some(slot) = values.iter_mut().find(|(pid, _)| *pid == p.pid) {
                slot.1 += v;
            } else {
                other += v;
            }
        }
        values.push((OTHER, other));
        let total: f32 = values.iter().map(|(_, v)| *v).sum();
        if total > peak {
            peak = total;
        }
        cols.push(Column { values });
    }
    (cols, peak)
}

fn per_series_peak(cols: &[Column], series: &[u32]) -> HashMap<u32, f32> {
    let mut out: HashMap<u32, f32> = series.iter().map(|p| (*p, 0.0f32)).collect();
    for col in cols {
        for (pid, v) in &col.values {
            if let Some(slot) = out.get_mut(pid) {
                if *v > *slot {
                    *slot = *v;
                }
            }
        }
    }
    out
}

fn fmt_mem_mb(mb: f32) -> String {
    if mb >= 1024.0 {
        format!("{:.1}G", mb / 1024.0)
    } else if mb >= 1.0 {
        format!("{:.0}M", mb)
    } else {
        format!("{:.0}K", mb * 1024.0)
    }
}

#[derive(Clone)]
struct Column {
    values: Vec<(u32, f32)>, // in draw order (bottom to top stack)
}

struct StackedChart<'a> {
    columns: &'a [Column],
    #[allow(dead_code)]
    series_count: usize,
    min_scale: f32,
}

impl<'a> Widget for StackedChart<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.columns.is_empty() || area.width == 0 || area.height == 0 {
            return;
        }

        // Global scale: max stacked total across all columns, min metric-dependent.
        let max_total = self
            .columns
            .iter()
            .map(|c| c.values.iter().map(|(_, v)| *v).sum::<f32>())
            .fold(self.min_scale, f32::max);

        let rows = area.height as usize * 2; // half-block sub-rows

        let right = area.x + area.width;
        let n = self.columns.len();
        let offset = n.saturating_sub(area.width as usize);
        let cols = &self.columns[offset..];

        for (i, col) in cols.iter().enumerate() {
            let x = area.x + i as u16;
            if x >= right {
                break;
            }

            // fill bottom-up: convert cumulative values to sub-row counts
            let mut sub_fills: Vec<(u32, u16)> = Vec::with_capacity(col.values.len());
            let mut cumulative = 0.0f32;
            for (pid, v) in &col.values {
                cumulative += v;
                let sub = ((cumulative / max_total) * rows as f32).round() as u16;
                sub_fills.push((*pid, sub));
            }

            // determine owner of each sub-row
            let mut owners: Vec<Option<(u32, usize)>> = vec![None; rows];
            let mut last = 0u16;
            for (idx, (pid, up_to)) in sub_fills.iter().enumerate() {
                for r in last..*up_to.min(&(rows as u16)) {
                    owners[r as usize] = Some((*pid, idx));
                }
                last = *up_to;
            }

            for cell_row in 0..area.height {
                let sub_bottom = (area.height - 1 - cell_row) as usize * 2;
                let bottom = owners.get(sub_bottom).copied().flatten();
                let top = owners.get(sub_bottom + 1).copied().flatten();
                let cell = buf.cell_mut((x, area.y + cell_row));
                if let Some(c) = cell {
                    match (bottom, top) {
                        (Some((bpid, bi)), Some((tpid, ti))) => {
                            c.set_char('▀');
                            c.set_fg(color_for(tpid, ti));
                            c.set_bg(color_for(bpid, bi));
                        }
                        (Some((bpid, bi)), None) => {
                            c.set_char('▄');
                            c.set_fg(color_for(bpid, bi));
                            c.set_bg(Color::Reset);
                        }
                        (None, Some((tpid, ti))) => {
                            c.set_char('▀');
                            c.set_fg(color_for(tpid, ti));
                            c.set_bg(Color::Reset);
                        }
                        (None, None) => {
                            c.set_char(' ');
                        }
                    }
                }
            }
        }
    }
}

pub(super) fn color_for(pid: u32, idx: usize) -> Color {
    if pid == OTHER {
        return Color::DarkGray;
    }
    const PALETTE: &[Color] = &[
        Color::LightBlue,
        Color::LightRed,
        Color::LightMagenta,
        Color::LightGreen,
        Color::LightYellow,
        Color::Cyan,
        Color::Magenta,
    ];
    PALETTE[idx % PALETTE.len()]
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept)
}

#[allow(dead_code)]
fn _mod_styles() -> Modifier {
    Modifier::empty()
}
