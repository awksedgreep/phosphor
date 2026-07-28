//! Rendering. Reads App, draws; the only state it writes back is the
//! measured viewport (visible_rows / visible_cols_width) for paging math.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::{App, Focus, Grid, GridSource, Overlay};
use crate::db::PValue;

pub fn draw(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    // Paint the whole screen in theme colors first (paper needs the bg).
    f.render_widget(Block::default().style(th.base()), f.area());

    let [body, prompt_line, status_line] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    let [sidebar, main] =
        Layout::horizontal([Constraint::Length(24), Constraint::Fill(1)]).areas(body);

    draw_sidebar(f, app, sidebar);
    draw_main(f, app, main);
    draw_prompt(f, app, prompt_line);
    draw_status(f, app, status_line);

    match &app.overlay {
        Overlay::Help(_) => draw_help(f, app),
        Overlay::Edit(_) => draw_edit(f, app),
        Overlay::Health(_) => draw_health(f, app),
        Overlay::Qbe(_) => draw_qbe(f, app),
        Overlay::Report(_) => draw_report(f, app),
        Overlay::Pager(_) => draw_pager(f, app),
        Overlay::Form(_) => draw_form(f, app),
        Overlay::Paint(_) => draw_paint(f, app),
        Overlay::Apps(_) => draw_apps(f, app),
        Overlay::AppMenu(_) => draw_app_menu(f, app),
        Overlay::None => {}
    }
}

/// A Rect at canvas coords (x,y) inside `inner`, clamped so partially
/// off-canvas elements truncate instead of panicking.
fn canvas_rect(inner: Rect, x: u16, y: u16, w: u16, h: u16) -> Option<Rect> {
    if x >= inner.width || y >= inner.height {
        return None;
    }
    Some(Rect {
        x: inner.x + x,
        y: inner.y + y,
        width: w.min(inner.width - x),
        height: h.min(inner.height - y),
    })
}

/// Paint a form's boxes, texts, and fields into `inner`. Shared by the
/// painter canvas and the painted EDIT form — designers must never
/// drift from the runtime.
#[allow(clippy::too_many_arguments)]
fn paint_spec(
    f: &mut Frame,
    inner: Rect,
    spec: &crate::forms::FormSpec,
    th: &crate::theme::Theme,
    selected_col: Option<&str>,
    mut value_of: impl FnMut(&str) -> Option<Span<'static>>,
) {
    for b in &spec.boxes {
        if let Some(r) = canvas_rect(inner, b.x, b.y, b.w, b.h) {
            f.render_widget(
                Block::default().borders(Borders::ALL).border_style(th.dim()),
                r,
            );
        }
    }
    for t in &spec.texts {
        let w = t.text.chars().count() as u16;
        if let Some(r) = canvas_rect(inner, t.x, t.y, w, 1) {
            f.render_widget(Paragraph::new(Line::styled(t.text.clone(), th.bright())), r);
        }
    }
    for field in spec.fields.iter().filter(|fl| fl.include) {
        let Some((x, y)) = field.pos else { continue };
        let selected = selected_col == Some(field.column.as_str());
        let label = format!("{}:", field.label);
        let lw = label.chars().count() as u16;
        if let Some(r) = canvas_rect(inner, x, y, lw, 1) {
            f.render_widget(
                Paragraph::new(Line::styled(
                    label,
                    if selected { th.cursor() } else { th.base() },
                )),
                r,
            );
        }
        let vx = x + lw + 1;
        let value = value_of(&field.column)
            .unwrap_or_else(|| Span::styled("_".repeat(field.width as usize), th.dim()));
        if let Some(r) = canvas_rect(inner, vx, y, field.width, 1) {
            f.render_widget(Paragraph::new(Line::from(value)), r);
        }
    }
}

fn draw_paint(f: &mut Frame, app: &App) {
    let th = app.theme;
    let Overlay::Paint(st) = &app.overlay else { return };
    let (cw, ch) = st.spec.size;
    let area = centered(f.area(), cw + 2, ch + 2);
    f.render_widget(Clear, area);
    let sel_name = st
        .spec
        .fields
        .get(st.selected)
        .map(|fl| fl.label.as_str())
        .unwrap_or("-");
    let mode = if st.editing.is_some() {
        "text: type + Enter".to_owned()
    } else if st.pending_box.is_some() {
        "box: move + b for the far corner".to_owned()
    } else {
        format!("field: {sel_name}")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(
            format!(" FORM PAINTER · {} · {mode} ", st.spec.table),
            th.bright(),
        ))
        .title_bottom(Line::styled(
            " Tab field · Space place · t text · b box · x del · +/- width · F6 save ",
            th.dim(),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let selected_col = st.spec.fields.get(st.selected).map(|fl| fl.column.clone());
    paint_spec(f, inner, &st.spec, th, selected_col.as_deref(), |_| None);

    // Pending box corner marker, then the cursor cell on top.
    if let Some((bx, by)) = st.pending_box {
        if let Some(r) = canvas_rect(inner, bx, by, 1, 1) {
            f.render_widget(Paragraph::new(Line::styled("┌", th.bright())), r);
        }
    }
    if let Some(buf) = &st.editing {
        // Live text entry rendered at the cursor.
        let w = (buf.chars().count() as u16 + 1).max(1);
        if let Some(r) = canvas_rect(inner, st.cursor.0, st.cursor.1, w, 1) {
            f.render_widget(
                Paragraph::new(Line::from(editing_span(buf, th))),
                r,
            );
        }
    } else if let Some(r) = canvas_rect(inner, st.cursor.0, st.cursor.1, 1, 1) {
        // Invert the glyph under the cursor instead of blotting it out
        // ("▒ame:" on film). Look up what lives at this cell.
        let (cx, cy) = st.cursor;
        let mut under = ' ';
        for t in &st.spec.texts {
            if t.y == cy && cx >= t.x {
                if let Some(ch) = t.text.chars().nth((cx - t.x) as usize) {
                    under = ch;
                }
            }
        }
        for fl in st.spec.fields.iter().filter(|fl| fl.include) {
            if let Some((fx, fy)) = fl.pos {
                if fy == cy && cx >= fx {
                    let label = format!("{}:", fl.label);
                    if let Some(ch) = label.chars().nth((cx - fx) as usize) {
                        under = ch;
                    }
                }
            }
        }
        let shown = if under == ' ' { '▒' } else { under };
        f.render_widget(
            Paragraph::new(Line::styled(shown.to_string(), th.cursor())),
            r,
        );
    }
}

fn draw_form(f: &mut Frame, app: &App) {
    let th = app.theme;
    let Overlay::Form(st) = &app.overlay else { return };
    let area = centered(
        f.area(),
        66,
        (st.spec.fields.len() as u16 + 6).min(f.area().height),
    );
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(
            format!(" FORM · {} ", st.spec.table),
            th.bright(),
        ))
        .title_bottom(Line::styled(
            " Space show · Enter label · r req · [ ] order · F2 PAINT · F6 save ",
            th.dim(),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = vec![Line::from(vec![
        Span::styled(pad("FIELD", 16), th.dim()),
        Span::styled(pad("SHOW", 5), th.dim()),
        Span::styled(pad("REQ", 4), th.dim()),
        Span::styled("LABEL", th.dim()),
    ])];
    for (i, field) in st.spec.fields.iter().enumerate() {
        let selected = i == st.cursor;
        let style = if selected { th.cursor() } else { th.base() };
        let label: Span = match (selected, &st.editing) {
            (true, Some(buf)) => editing_span(buf, th),
            _ => Span::styled(field.label.clone(), style),
        };
        lines.push(Line::from(vec![
            Span::styled(pad(&field.column, 16), style),
            Span::styled(pad(if field.include { "▪" } else { " " }, 5), style),
            Span::styled(pad(if field.required { "*" } else { " " }, 4), style),
            label,
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_apps(f: &mut Frame, app: &App) {
    let th = app.theme;
    let Overlay::Apps(st) = &app.overlay else { return };
    let area = centered(
        f.area(),
        72,
        (st.items.len() as u16 + 7).max(10).min(f.area().height),
    );
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(
            format!(" APPLICATIONS GENERATOR · {} ", st.app),
            th.bright(),
        ))
        .title_bottom(Line::styled(
            " n new · x delete · Enter label · e ref · c kind · [ ] order · F2 run ",
            th.dim(),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = vec![Line::from(vec![
        Span::styled(pad("LABEL", 24), th.dim()),
        Span::styled(pad("KIND", 8), th.dim()),
        Span::styled("TARGET (table · saved query/report · SQL)", th.dim()),
    ])];
    for (i, item) in st.items.iter().enumerate() {
        let selected = i == st.cursor;
        let style = if selected { th.cursor() } else { th.base() };
        let (label, target): (Span, Span) = match (selected, &st.editing) {
            (true, Some(buf)) if st.editing_ref => (
                Span::styled(pad(&item.label, 24), style),
                editing_span(buf, th),
            ),
            (true, Some(buf)) => (
                editing_span(buf, th),
                Span::styled(item.action_ref.clone(), style),
            ),
            _ => (
                Span::styled(pad(&item.label, 24), style),
                Span::styled(item.action_ref.clone(), style),
            ),
        };
        lines.push(Line::from(vec![
            label,
            Span::styled(pad(item.kind.as_str(), 8), style),
            target,
        ]));
    }
    if st.items.is_empty() {
        lines.push(Line::styled("  n adds the first menu item", th.dim()));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_app_menu(f: &mut Frame, app: &App) {
    let th = app.theme;
    let Overlay::AppMenu(st) = &app.overlay else { return };
    let width = 46u16;
    let area = centered(
        f.area(),
        width,
        (st.items.len() as u16 * 2 + 6).min(f.area().height),
    );
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(
            format!(" ▓▓ {} ▓▓ ", st.app.to_uppercase()),
            th.bright(),
        ))
        .title_alignment(Alignment::Center)
        .title_bottom(Line::styled(" ↑↓ + Enter · hotkey letters · Esc ", th.dim()));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = vec![Line::raw("")];
    for (i, item) in st.items.iter().enumerate() {
        let selected = i == st.cursor;
        let style = if selected { th.cursor() } else { th.base() };
        // dBASE-style: the hotkey is the bright first letter.
        let mut chars = item.label.chars();
        let first: String = chars.next().map(|c| c.to_string()).unwrap_or_default();
        let rest: String = chars.collect();
        lines.push(Line::from(vec![
            Span::styled("   ", style),
            Span::styled(
                first,
                if selected {
                    th.cursor()
                } else {
                    th.bright()
                },
            ),
            Span::styled(pad(&rest, width.saturating_sub(8)), style),
        ]));
        lines.push(Line::raw(""));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn editing_span<'a>(buf: &'a str, th: &crate::theme::Theme) -> Span<'a> {
    Span::styled(format!("{buf}▏"), th.cursor())
}

fn draw_qbe(f: &mut Frame, app: &App) {
    let th = app.theme;
    let Overlay::Qbe(st) = &app.overlay else { return };
    let area = centered(f.area(), 76, (st.spec.cols.len() as u16 + 8).min(f.area().height));
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(
            format!(" QUERY BY EXAMPLE · {} ", st.spec.table),
            th.bright(),
        ))
        .title_bottom(Line::styled(
            " Space show · Enter filter · s sort · F2 run · F6 save · Esc ",
            th.dim(),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = vec![Line::from(vec![
        Span::styled(pad("COLUMN", 18), th.dim()),
        Span::styled(pad("SHOW", 5), th.dim()),
        Span::styled(pad("SORT", 5), th.dim()),
        Span::styled("FILTER (e.g. > 100, like 'a%', or a bare value)", th.dim()),
    ])];
    for (i, col) in st.spec.cols.iter().enumerate() {
        let selected = i == st.cursor;
        let row_style = if selected { th.cursor() } else { th.base() };
        let filter: Span = match (selected && !st.naming, &st.editing) {
            (true, Some(buf)) => editing_span(buf, th),
            _ => Span::styled(col.filter.clone(), row_style),
        };
        lines.push(Line::from(vec![
            Span::styled(pad(&col.name, 18), row_style),
            Span::styled(pad(if col.show { "▪" } else { " " }, 5), row_style),
            Span::styled(pad(col.sort.glyph(), 5), row_style),
            filter,
        ]));
    }
    lines.push(Line::raw(""));
    if st.naming {
        lines.push(Line::from(vec![
            Span::styled("save as: ", th.bright()),
            editing_span(st.editing.as_deref().unwrap_or(""), th),
        ]));
    }
    lines.push(Line::styled("SQL:", th.dim()));
    lines.push(Line::styled(st.spec.sql(), th.bright()));
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_report(f: &mut Frame, app: &App) {
    let th = app.theme;
    let Overlay::Report(st) = &app.overlay else { return };
    let area = centered(f.area(), 76, 12);
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(
            format!(" REPORT · {} ", st.spec.name),
            th.bright(),
        ))
        .title_bottom(Line::styled(
            " Enter edit · Space cycle group · F2 preview · F6 save · Esc ",
            th.dim(),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let fields = [
        ("title", st.spec.title.clone()),
        ("source", st.spec.source.clone()),
        (
            "group by",
            st.spec.group_by.clone().unwrap_or_else(|| "(none)".into()),
        ),
    ];
    let mut lines = Vec::new();
    for (i, (label, value)) in fields.iter().enumerate() {
        let selected = i == st.cursor;
        let value_span = match (selected, &st.editing) {
            (true, Some(buf)) => editing_span(buf, th),
            _ => Span::styled(
                value.clone(),
                if selected { th.cursor() } else { th.base() },
            ),
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{label:>9} : "),
                if selected { th.bright() } else { th.dim() },
            ),
            value_span,
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "numeric columns total automatically; grouping adds bands + subtotals",
        th.dim(),
    ));
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_pager(f: &mut Frame, app: &App) {
    let th = app.theme;
    let Overlay::Pager(p) = &app.overlay else { return };
    let area = f.area().inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });
    f.render_widget(Clear, area);
    let pos = format!(
        " {}/{} · w write file · Esc ",
        (p.offset + 1).min(p.lines.len()),
        p.lines.len()
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(format!(" {} ", p.title), th.bright()))
        .title_bottom(Line::styled(pos, th.dim()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines: Vec<Line> = p
        .lines
        .iter()
        .skip(p.offset)
        .take(inner.height as usize)
        .map(|l| {
            if l == "\u{c}" {
                Line::styled("· · · · · · · · page break · · · · · · · ·", th.dim())
            } else if l.starts_with('▌') {
                Line::styled(l.clone(), th.bright())
            } else {
                Line::styled(l.clone(), th.base())
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

const SPARK_BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// (rendered bars, is_flat). Flat series render as a LOW line — a
/// solid mid-height bar for constant zero looked identical to real
/// data on film, which is exactly what a chart must never do.
fn sparkline(values: &[f64], width: usize) -> (String, bool) {
    let take = values.len().min(width);
    let window = &values[values.len() - take..];
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in window {
        lo = lo.min(*v);
        hi = hi.max(*v);
    }
    let flat = hi <= lo;
    let bars = window
        .iter()
        .map(|v| {
            let idx = if flat {
                0
            } else {
                (((v - lo) / (hi - lo)) * 7.0).round() as usize
            };
            SPARK_BARS[idx.min(7)]
        })
        .collect();
    (bars, flat)
}

fn draw_health(f: &mut Frame, app: &App) {
    let th = app.theme;
    let Overlay::Health(hv) = &app.overlay else { return };
    let area = f.area().inner(ratatui::layout::Margin {
        horizontal: 3,
        vertical: 1,
    });
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(
            format!(" DBHEALTH · {} ", hv.table),
            th.bright(),
        ))
        .title_bottom(Line::styled(
            " LIVE · sampling every 5s while open · s now · Esc close ",
            th.dim(),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let report_h = (hv.report.len() as u16 + 1).min(inner.height / 2);
    let [report_area, _, sparks_area] = Layout::vertical([
        Constraint::Length(report_h),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);

    let mut lines = vec![Line::from(vec![
        Span::styled(pad("STATUS", 9), th.dim()),
        Span::styled(pad("CHECK", 19), th.dim()),
        Span::styled(pad("VALUE", 22), th.dim()),
        Span::styled("ADVICE", th.dim()),
    ])];
    for row in &hv.report {
        let [check, status, value, advice] = row;
        let advice_w = (inner.width as usize).saturating_sub(56).max(8);
        lines.push(Line::from(vec![
            Span::styled(pad(&format!("● {status}"), 8), th.health(status)),
            Span::styled(pad(check, 19), th.bright()),
            Span::styled(pad(value, 22), th.base()),
            Span::styled(
                advice.chars().take(advice_w).collect::<String>(),
                th.dim(),
            ),
        ]));
    }
    f.render_widget(Paragraph::new(lines), report_area);

    let spark_w = (sparks_area.width as usize).saturating_sub(34).max(10);
    let mut lines = vec![Line::styled("trends (oldest → newest)", th.dim())];
    for (name, values, latest) in &hv.sparks {
        let (bars, flat) = sparkline(values, spark_w);
        lines.push(Line::from(vec![
            Span::styled(pad(name, 19), th.base()),
            Span::styled(bars, if flat { th.dim() } else { th.bright() }),
            Span::styled(format!("  {latest}"), th.dim()),
        ]));
    }
    if hv.sparks.is_empty() {
        lines.push(Line::styled(
            "no series yet — press s to take a sample",
            th.dim(),
        ));
    }
    f.render_widget(Paragraph::new(lines), sparks_area);
}

fn focus_style(app: &App, mine: Focus) -> ratatui::style::Style {
    if app.focus == mine && matches!(app.overlay, Overlay::None) {
        app.theme.bright()
    } else {
        app.theme.dim()
    }
}

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(focus_style(app, Focus::Sidebar))
        .title(Span::styled(" Data ", focus_style(app, Focus::Sidebar)));
    let visible = app.visible_tables();
    let hidden = app.tables.len() - visible.len();
    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let marker = if t.is_view { "◇ " } else { "▪ " };
            let internal = crate::app::App::is_internal(t);
            let style = if i == app.sidebar_idx {
                th.cursor()
            } else if internal || t.is_view {
                th.dim()
            } else {
                th.base()
            };
            ListItem::new(Line::styled(format!("{marker}{}", t.name), style))
        })
        .chain((hidden > 0 && !app.show_internals).then(|| {
            ListItem::new(Line::styled(
                format!("  … {hidden} internal (i)"),
                th.dim(),
            ))
        }))
        .collect();
    let empty = app.tables.is_empty();
    f.render_widget(List::new(items).block(block), area);
    if empty {
        let hint = Rect {
            x: area.x + 2,
            y: area.y + 2,
            width: area.width.saturating_sub(4),
            height: 2,
        };
        f.render_widget(
            Paragraph::new(Line::styled("no tables yet —\ntry the . prompt", th.dim())),
            hint,
        );
    }
}

fn draw_main(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let title = match &app.grid {
        Some(Grid {
            source: GridSource::Table { name, editable },
            ..
        }) => {
            if *editable {
                format!(" BROWSE {name} ")
            } else {
                format!(" BROWSE {name} (read-only) ")
            }
        }
        Some(Grid {
            source: GridSource::Query { truncated },
            ..
        }) => {
            if *truncated {
                " QUERY (capped at 10k rows) ".to_owned()
            } else {
                " QUERY ".to_owned()
            }
        }
        None => " phosphor ".to_owned(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(focus_style(app, Focus::Grid))
        .title(Span::styled(title, focus_style(app, Focus::Grid)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Report the viewport so App paging math matches reality.
    app.visible_rows = inner.height.saturating_sub(1).max(1) as i64; // minus header
    app.visible_cols_width = inner.width;

    let Some(g) = &app.grid else {
        f.render_widget(
            Paragraph::new(vec![
                Line::raw(""),
                Line::styled("  Enter on a table to BROWSE", th.dim()),
                Line::styled("  .  for the dot prompt", th.dim()),
                Line::styled("  F1 for help", th.dim()),
            ]),
            inner,
        );
        return;
    };

    // Visible column window.
    let mut cols: Vec<usize> = Vec::new();
    let mut used: u16 = 0;
    for c in g.col_off..g.columns.len() {
        let w = g.widths[c] + 1;
        if used + w > inner.width && !cols.is_empty() {
            break;
        }
        used += w;
        cols.push(c);
    }

    let mut lines: Vec<Line> = Vec::with_capacity(inner.height as usize);
    let header = Line::from(
        cols.iter()
            .map(|&c| {
                Span::styled(pad(&g.columns[c], g.widths[c]), th.bright())
            })
            .collect::<Vec<_>>(),
    );
    lines.push(header);

    let visible = app.visible_rows;
    for vis in 0..visible {
        let abs = g.row_off + vis;
        if abs >= g.total {
            break;
        }
        let spans: Vec<Span> = match g.row(abs) {
            Some(row) => cols
                .iter()
                .map(|&c| {
                    let text = row
                        .get(c)
                        .map(PValue::render)
                        .unwrap_or_default();
                    let style = if abs == g.cur_row && c == g.cur_col {
                        th.cursor()
                    } else if abs == g.cur_row {
                        th.bright()
                    } else if matches!(row.get(c), Some(PValue::Null)) {
                        th.dim()
                    } else {
                        th.base()
                    };
                    Span::styled(pad(&text, g.widths[c]), style)
                })
                .collect(),
            None => vec![Span::styled("…", th.dim())],
        };
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn pad(s: &str, width: u16) -> String {
    let width = width as usize;
    let mut out: String = s.chars().take(width).collect();
    if s.chars().count() > width && width > 0 {
        out.pop();
        out.push('…');
    }
    while out.chars().count() < width {
        out.push(' ');
    }
    out.push(' ');
    out
}

fn draw_prompt(f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Prompt && matches!(app.overlay, Overlay::None);
    let dot = Span::styled(" . ", if focused { th.bright() } else { th.dim() });
    let mut spans = vec![dot];
    if focused {
        let chars: Vec<char> = app.prompt.input.chars().collect();
        let (before, at_after) = chars.split_at(app.prompt.cursor.min(chars.len()));
        spans.push(Span::styled(before.iter().collect::<String>(), th.base()));
        let cursor_char = at_after.first().copied().unwrap_or(' ');
        spans.push(Span::styled(cursor_char.to_string(), th.cursor()));
        spans.push(Span::styled(
            at_after.iter().skip(1).collect::<String>(),
            th.base(),
        ));
    } else {
        spans.push(Span::styled(app.prompt.input.clone(), th.dim()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    let left = format!(" {} [{}]", app.db.name(), app.db.backend());
    let mid = match &app.grid {
        Some(g) if g.total > 0 => format!(
            "{} · row {}/{} · {}",
            match &g.source {
                GridSource::Table { name, .. } => name.clone(),
                GridSource::Query { .. } => "query".into(),
            },
            g.cur_row + 1,
            g.total,
            g.columns.get(g.cur_col).cloned().unwrap_or_default()
        ),
        _ => String::new(),
    };
    let ms = app
        .last_ms
        .map(|m| format!("{m:.1}ms"))
        .unwrap_or_default();
    let (dot, dot_style) = match &app.health {
        Some(s) => ("●", th.health(s)),
        None => ("○", th.dim()),
    };

    let status_msg = app.status.clone();
    let (msg, msg_style) = match &status_msg {
        Some((m, true)) => (m.clone(), th.error()),
        Some((m, false)) => (m.clone(), th.dim()),
        None => (String::new(), th.dim()),
    };

    let line = Line::from(vec![
        Span::styled(left, th.dim()),
        Span::styled("  ", th.dim()),
        Span::styled(msg, msg_style),
        Span::styled("  ", th.dim()),
        Span::styled(mid, th.dim()),
        Span::styled(format!("  {ms} "), th.dim()),
        Span::styled(dot, dot_style),
        Span::styled(" ", th.dim()),
    ]);
    f.render_widget(Paragraph::new(line).alignment(Alignment::Left), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn draw_edit(f: &mut Frame, app: &App) {
    let th = app.theme;
    let Overlay::Edit(ed) = &app.overlay else { return };
    // Painted forms render CREATE SCREEN style; unpainted stay a list.
    if let Some(spec) = &ed.painted {
        let (cw, ch) = spec.size;
        let area = centered(f.area(), cw + 2, ch + 2);
        f.render_widget(Clear, area);
        let dirty = if ed.dirty() { " *" } else { "" };
        let title = if ed.inserting {
            format!(" NEW {} record{dirty} ", ed.table)
        } else {
            format!(" EDIT {} · rowid {}{dirty} ", ed.table, ed.rowid)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(th.bright())
            .style(th.base())
            .title(Span::styled(title, th.bright()))
            .title_bottom(Line::styled(
                " ↑↓ field · Enter edit · F10/Ctrl-S save · Esc ",
                th.dim(),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let selected_col = ed.fields.get(ed.cursor).map(|(c, _)| c.name.clone());
        paint_spec(f, inner, spec, th, selected_col.as_deref(), |col| {
            let i = ed.fields.iter().position(|(c, _)| c.name == col)?;
            let selected = i == ed.cursor;
            Some(match (selected, &ed.editing) {
                (true, Some(buf)) => Span::styled(format!("{buf}▏"), th.cursor()),
                _ => {
                    let (text, edited) = match &ed.inputs[i] {
                        Some(t) => (t.clone(), true),
                        None => (ed.fields[i].1.render(), false),
                    };
                    Span::styled(
                        text,
                        if selected {
                            th.cursor()
                        } else if edited {
                            th.bright()
                        } else {
                            th.base()
                        },
                    )
                }
            })
        });
        return;
    }
    let label_w = ed
        .labels
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(4)
        .clamp(4, 20);
    let area = centered(f.area(), 62, (ed.fields.len() as u16 + 4).min(f.area().height));
    f.render_widget(Clear, area);
    let dirty = if ed.dirty() { " *" } else { "" };
    let title = if ed.inserting {
        format!(" NEW {} record{dirty} ", ed.table)
    } else {
        format!(" EDIT {} · rowid {}{dirty} ", ed.table, ed.rowid)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(title, th.bright()));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();
    for (i, (col, original)) in ed.fields.iter().enumerate() {
        // PICTURE-clause energy: ¶ pk, * NOT NULL / form-required.
        let marker = if col.pk {
            "¶"
        } else if col.notnull || ed.required[i] {
            "*"
        } else {
            " "
        };
        let label = format!(
            "{marker}{:>w$} : ",
            ed.labels[i].chars().take(label_w).collect::<String>(),
            w = label_w
        );
        let selected = i == ed.cursor;
        let value_span = if let (true, Some(buf)) = (selected, &ed.editing) {
            Span::styled(format!("{buf}▏"), th.cursor())
        } else {
            let (text, edited) = match &ed.inputs[i] {
                Some(t) => (t.clone(), true),
                None => (original.render(), false),
            };
            let style = if selected {
                th.cursor()
            } else if edited {
                th.bright()
            } else {
                th.base()
            };
            Span::styled(text, style)
        };
        lines.push(Line::from(vec![
            Span::styled(label, if selected { th.bright() } else { th.dim() }),
            value_span,
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Enter edit · F10/Ctrl-S save · Esc cancel",
        th.dim(),
    ));
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_help(f: &mut Frame, app: &App) {
    let th = app.theme;
    let Overlay::Help(st) = &app.overlay else { return };
    let topics = crate::help::TOPICS;
    let topic = &topics[st.topic.min(topics.len() - 1)];

    let area = f.area().inner(ratatui::layout::Margin {
        horizontal: 4,
        vertical: 1,
    });
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(" HELP ", th.bright()))
        .title_bottom(Line::styled(
            " ←→ topics · ↑↓ scroll · Esc close ",
            th.dim(),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let [toc, _, body] = Layout::horizontal([
        Constraint::Length(16),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(inner);

    let toc_lines: Vec<Line> = topics
        .iter()
        .enumerate()
        .map(|(i, t)| {
            Line::styled(
                format!(" {} ", t.title),
                if i == st.topic { th.cursor() } else { th.dim() },
            )
        })
        .collect();
    f.render_widget(Paragraph::new(toc_lines), toc);

    let mut body_lines: Vec<Line> = vec![
        Line::styled(topic.title.to_uppercase(), th.bright()),
        Line::raw(""),
    ];
    body_lines.extend(topic.body.lines().map(|l| Line::styled(l, th.base())));
    let scroll = (st.scroll as usize)
        .min(body_lines.len().saturating_sub(body.height as usize / 2)) as u16;
    f.render_widget(Paragraph::new(body_lines).scroll((scroll, 0)), body);
}
