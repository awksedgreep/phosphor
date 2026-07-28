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
        Overlay::Help => draw_help(f, app),
        Overlay::Edit(_) => draw_edit(f, app),
        Overlay::None => {}
    }
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
    let items: Vec<ListItem> = app
        .tables
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let marker = if t.is_view { "◇ " } else { "▪ " };
            let style = if i == app.sidebar_idx {
                th.cursor()
            } else if t.is_view {
                th.dim()
            } else {
                th.base()
            };
            ListItem::new(Line::styled(format!("{marker}{}", t.name), style))
        })
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
            source: GridSource::Query { .. },
            ..
        }) => " QUERY ".to_owned(),
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
    let label_w = ed
        .fields
        .iter()
        .map(|(c, _)| c.name.chars().count())
        .max()
        .unwrap_or(4)
        .clamp(4, 20);
    let area = centered(f.area(), 62, (ed.fields.len() as u16 + 4).min(f.area().height));
    f.render_widget(Clear, area);
    let dirty = if ed.dirty() { " *" } else { "" };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(
            format!(" EDIT {} · rowid {}{dirty} ", ed.table, ed.rowid),
            th.bright(),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();
    for (i, (col, original)) in ed.fields.iter().enumerate() {
        // PICTURE-clause energy: ¶ marks the primary key, * NOT NULL.
        let marker = if col.pk {
            "¶"
        } else if col.notnull {
            "*"
        } else {
            " "
        };
        let label = format!(
            "{marker}{:>w$} : ",
            col.name.chars().take(label_w).collect::<String>(),
            w = label_w
        );
        let selected = i == ed.cursor;
        let value_span = if selected && ed.editing.is_some() {
            let buf = ed.editing.as_ref().unwrap();
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
    let area = centered(f.area(), 58, 20);
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(" Help ", th.bright()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = [
        ("Tab / Esc", "cycle focus / back out"),
        (".", "dot prompt (SQL + commands)"),
        ("Enter", "open table · edit row · run"),
        ("↑↓←→ or hjkl", "move"),
        ("PgUp/PgDn g G", "page · top · bottom"),
        ("Home/End", "first / last column"),
        ("F5 / r", "refresh"),
        ("F10 / Ctrl-S", "save (in EDIT)"),
        ("q / Ctrl-Q", "quit"),
        ("", ""),
        ("prompt:", "any SQL · help · tables"),
        ("", "set theme green|amber|paper|blue"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!(" {k:>14}  "), th.bright()),
                Span::styled(*v, th.base()),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}
