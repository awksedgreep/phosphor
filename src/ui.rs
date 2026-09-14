//! Rendering. Reads App, draws; the only state it writes back is the
//! measured viewport and the status panel's clamped scroll position.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use std::borrow::Cow;

use crate::app::{App, DetailState, Focus, Grid, GridSource, Overlay, ScriptTarget};
use crate::db::{DbLink, PValue};

#[derive(Default)]
pub struct Scroll {
    pub top: usize,
    pub rows: usize,
}

impl Scroll {
    fn follow(&mut self, selected: usize, count: usize, rows: usize) {
        self.rows = rows;
        let selected = selected.min(count.saturating_sub(1));
        self.top = self.top.min(count.saturating_sub(rows));
        if selected < self.top {
            self.top = selected;
        }
        if selected >= self.top + rows {
            self.top = selected.saturating_sub(rows.saturating_sub(1));
        }
    }
}

#[derive(Default)]
pub struct Viewports {
    pub sidebar: Scroll,
    pub create: Scroll,
    pub form: Scroll,
    pub apps: Scroll,
    pub menu: Scroll,
    pub qbe: Scroll,
    pub report: Scroll,
    pub edit: Scroll,
    pub picker: Scroll,
    pub script: Scroll,
    pub help_topics: Scroll,
    pub script_left: usize,
    pub prompt_left: usize,
    pub paint_x: u16,
    pub paint_y: u16,
}

/// Keep headers fixed and move the data window just far enough to show
/// the selection. Resizing follows the same rule as keyboard navigation.
fn draw_rows(
    f: &mut Frame,
    area: Rect,
    lines: Vec<Line<'_>>,
    headers: usize,
    selected: usize,
    scroll: &mut Scroll,
) {
    let headers = if area.height as usize > headers {
        headers
    } else {
        0
    };
    let rows = (area.height as usize).saturating_sub(headers);
    scroll.follow(
        selected.saturating_sub(headers),
        lines.len().saturating_sub(headers),
        rows,
    );
    let visible: Vec<_> = lines
        .iter()
        .take(headers)
        .cloned()
        .chain(lines.iter().skip(headers + scroll.top).take(rows).cloned())
        .collect();
    f.render_widget(Paragraph::new(visible), area);
}

fn dialog_area(area: Rect) -> Rect {
    Rect {
        height: area.height.saturating_sub(4),
        ..area
    }
}

fn text_from_column(text: &str, column: usize) -> String {
    let mut skipped = 0;
    for (byte, g) in text.grapheme_indices(true) {
        if skipped >= column {
            return text[byte..].to_owned();
        }
        skipped += g.width();
        if skipped > column {
            return format!(
                "{}{}",
                " ".repeat(skipped - column),
                &text[byte + g.len()..]
            );
        }
    }
    String::new()
}

/// Horizontal scrolling follows the caret in terminal cells, not bytes.
fn caret_line(
    text: &str,
    byte: usize,
    width: u16,
    left: &mut usize,
    th: &crate::theme::Theme,
) -> Line<'static> {
    let mut byte = byte.min(text.len());
    if let Some((start, _)) = text
        .grapheme_indices(true)
        .find(|(start, g)| *start <= byte && byte < start + g.len())
    {
        byte = start;
    }
    let (before, after) = text.split_at(byte);
    let caret = after.graphemes(true).next().unwrap_or(" ");
    let column = before.width();
    let room = width.max(1) as usize;
    if column < *left {
        *left = column;
    }
    if column + caret.width().max(1) > *left + room {
        *left = (column + caret.width().max(1)).saturating_sub(room);
    }
    let rest = after.get(caret.len()..).unwrap_or("");
    Line::from(vec![
        Span::styled(text_from_column(before, *left), th.base()),
        Span::styled(caret.to_owned(), th.cursor()),
        Span::styled(rest.to_owned(), th.base()),
    ])
}

pub fn draw(f: &mut Frame, app: &mut App) -> bool {
    // Returns true when the measured viewport changed: paging math
    // follows the new size, so the main loop redraws once more.
    let (old_rows, old_cols) = (app.visible_rows, app.visible_cols_width);
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

    // Mouse hit rects (content only, borders excluded).
    let inner1 = |r: ratatui::layout::Rect| -> Option<ratatui::layout::Rect> {
        (r.width >= 2 && r.height >= 2).then(|| ratatui::layout::Rect {
            x: r.x + 1,
            y: r.y + 1,
            width: r.width - 2,
            height: r.height - 2,
        })
    };
    // Split layout: side by side (default) or master stacked above the
    // detail pane (`H`), each with its own minimum viable size.
    let want_split = app.detail.is_some();
    let side_by_side = want_split && !app.split_horizontal && main.width >= 76;
    let stacked = want_split && app.split_horizontal && main.height >= 10;
    let (master_area, detail_area) = if side_by_side {
        let [m, d] = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
            .areas(main);
        (m, Some(d))
    } else if stacked {
        let [m, d] =
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(main);
        (m, Some(d))
    } else {
        (main, None)
    };
    app.hit = crate::app::HitRects {
        sidebar: inner1(sidebar),
        master: inner1(master_area),
        detail: detail_area.and_then(inner1),
        prompt: Some(prompt_line),
    };
    draw_sidebar(f, app, sidebar);
    draw_main(f, app, master_area);
    if let (Some(d_area), Some(_)) = (detail_area, &app.detail) {
        let state = app.detail.as_ref().expect("split checked");
        draw_detail_panel(f, app, state, d_area);
    }
    draw_prompt(f, app, prompt_line);

    match &app.overlay {
        Overlay::Busy(_) => draw_busy(f, app),
        Overlay::SaveDatabase(_) => draw_save_database(f, app),
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
        Overlay::Create(_) => draw_create(f, app),
        Overlay::ScriptEditor(_) => draw_script_editor(f, app),
        Overlay::None => {}
    }
    draw_editor_input(f, app);
    // The FK value picker draws over the record form (same overlay).
    if let Overlay::Edit(ed) = &app.overlay {
        if ed.picker.is_some() {
            draw_picker(f, app);
        }
    }
    // Outcomes stay visible even when a designer fills the screen.
    draw_status(f, app, status_line);
    if app.status_details.is_some() {
        draw_status_details(f, app);
    }
    // Optional CRT scanlines (DESIGN.md "CRT affectations"): dim every
    // other screen row. A no-op unless the user opted in.
    if app.shimmer {
        let buf = f.buffer_mut();
        let area = buf.area;
        for y in (1..area.height).step_by(2) {
            for x in 0..area.width {
                if let Some(cell) = buf.cell_mut((area.x + x, area.y + y)) {
                    cell.modifier |= ratatui::style::Modifier::DIM;
                }
            }
        }
    }
    app.visible_rows != old_rows || app.visible_cols_width != old_cols
}

/// A full-width input line keeps the caret available even when a table
/// designer's fixed columns cannot all fit in a narrow terminal.
fn draw_editor_input(f: &mut Frame, app: &App) {
    let input = match &app.overlay {
        Overlay::Edit(ed) if ed.picker.is_none() => ed.editing.as_deref().map(|buf| {
            (
                format!(
                    "{} · {}",
                    ed.table,
                    ed.labels
                        .get(ed.cursor)
                        .map(String::as_str)
                        .unwrap_or("field")
                ),
                buf,
            )
        }),
        Overlay::Qbe(st) => st.editing.as_deref().map(|buf| {
            (
                if st.naming {
                    app.design_name_action.label().into()
                } else {
                    format!("{} · filter", st.spec.cols[st.cursor].name)
                },
                buf,
            )
        }),
        Overlay::Report(st) => st.editing.as_deref().map(|buf| {
            (
                if st.naming {
                    app.design_name_action.label().into()
                } else {
                    ["report title", "report source", "report grouping"][st.cursor.min(2)].into()
                },
                buf,
            )
        }),
        Overlay::Form(st) => st.editing.as_deref().map(|buf| {
            (
                format!(
                    "{} · {}",
                    st.spec
                        .fields
                        .get(st.cursor)
                        .map(|fl| fl.column.as_str())
                        .unwrap_or("field"),
                    if st.editing_mask {
                        "mask"
                    } else if st.editing_computed {
                        "computed expression"
                    } else {
                        "label"
                    }
                ),
                buf,
            )
        }),
        Overlay::Apps(st) => st.editing.as_deref().map(|buf| {
            (
                (if st.renaming_app {
                    "application name"
                } else if st.editing_ref {
                    "menu target"
                } else {
                    "menu label"
                })
                .into(),
                buf,
            )
        }),
        Overlay::Create(st) => st.editing.as_deref().map(|buf| {
            (
                format!(
                    "{} · {}",
                    st.draft.table,
                    if st.cursor == 0 {
                        "table name"
                    } else {
                        match st.slot {
                            crate::creator::EditSlot::Name => "field name",
                            crate::creator::EditSlot::Default => "default",
                            crate::creator::EditSlot::Refs => "foreign key",
                        }
                    }
                ),
                buf,
            )
        }),
        Overlay::Paint(st) => st
            .editing
            .as_deref()
            .map(|buf| ("painted text".into(), buf)),
        _ => None,
    };
    let Some((label, buf)) = input else { return };
    let area = Rect {
        y: f.area().bottom().saturating_sub(4),
        height: 2.min(f.area().height),
        ..f.area()
    };
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format!(
                    " {label} · Enter {} · Esc cancels input",
                    if matches!(app.overlay, Overlay::Edit(_)) {
                        "saves"
                    } else {
                        "applies"
                    }
                ),
                app.theme.dim(),
            ),
            Line::from(editing_span(buf, area.width.saturating_sub(1), app.theme)),
        ])
        .style(app.theme.base()),
        area,
    );
}

fn draw_save_database(f: &mut Frame, app: &App) {
    let Overlay::SaveDatabase(path) = &app.overlay else {
        return;
    };
    let area = centered(dialog_area(f.area()), 72, 13);
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .style(app.theme.base())
        .border_style(app.theme.bright())
        .title(" SAVE DATABASE ")
        .title_bottom(" Enter save · Esc cancel · F1 help ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines = vec![
        Line::raw("Keep this scratch database in a file."),
        Line::raw("Saved records, tables, and designs travel together."),
        Line::raw("Future saved changes will go to that file."),
        Line::raw("Existing files are never replaced."),
    ];
    let [description, input] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(2)]).areas(inner);
    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
        description,
    );
    f.render_widget(
        Paragraph::new(vec![
            Line::styled("New filename:", app.theme.dim()),
            Line::from(editing_span(path, inner.width.saturating_sub(1), app.theme)),
        ]),
        input,
    );
}

fn draw_busy(f: &mut Frame, app: &App) {
    let Overlay::Busy(b) = &app.overlay else {
        return;
    };
    let area = centered(dialog_area(f.area()), 66, 10);
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .style(app.theme.base())
        .border_style(app.theme.bright())
        .title(format!(" {} ", b.label))
        .title_bottom(" Esc cancel · F1 help · Ctrl-Q quit ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines = vec![
        Line::raw(if b.control.cancelled() {
            "Cancelling; waiting for the database to finish."
        } else {
            "Working; your previous screen is kept."
        }),
        Line::raw(format!(
            "{} rows read · {}s elapsed",
            b.control.rows(),
            b.started.elapsed().as_secs()
        )),
        Line::raw(""),
        Line::raw("F1 Help and F12 details remain available."),
    ];
    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
        inner,
    );
}

fn draw_create(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    let Overlay::Create(st) = &app.overlay else {
        return;
    };
    let is_editor = st.original.is_some();
    let title_text = if is_editor {
        format!(" TABLE EDITOR · {} ", st.draft.table)
    } else {
        format!(" TABLE DESIGNER · {} ", st.draft.table)
    };
    let footer_text = if is_editor {
        " F3 type F4 pk F5 null F6 uniq F7 dflt F10 fk F8 ins F9 del [] move F2 apply "
    } else {
        " F3 type F4 pk F5 null F6 uniq F7 dflt F10 fk F8 ins F9 del [] move F2 create "
    };
    let area = centered(
        dialog_area(f.area()),
        86,
        (st.draft.fields.len() as u16 + 12).min(f.area().height),
    );
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(title_text, th.bright()))
        .title_bottom(Line::styled(footer_text, th.dim()));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    // Row 0: the table name.
    let name_selected = st.cursor == 0;
    use crate::creator::EditSlot;
    let name_span: Span = match (name_selected, st.slot, &st.editing) {
        (true, EditSlot::Name, Some(buf)) => editing_span(buf, inner.width.saturating_sub(19), th),
        _ => Span::styled(
            st.draft.table.clone(),
            if name_selected {
                th.cursor()
            } else {
                th.bright()
            },
        ),
    };
    lines.push(Line::from(vec![
        Span::styled(
            pad("NAME", 17),
            if name_selected { th.bright() } else { th.dim() },
        ),
        name_span,
    ]));
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(pad("FIELD", 16), th.dim()),
        Span::styled(pad("TYPE", 9), th.dim()),
        Span::styled(pad("PK", 4), th.dim()),
        Span::styled(pad("NULL?", 6), th.dim()),
        Span::styled(pad("UNIQ", 5), th.dim()),
        Span::styled(pad("DEFAULT", 13), th.dim()),
        Span::styled("FK →", th.dim()),
    ]));
    for (i, fld) in st.draft.fields.iter().enumerate() {
        let selected = st.cursor == i + 1;
        let style = if selected { th.cursor() } else { th.base() };
        let name: Span = match (selected, st.slot, &st.editing) {
            (true, EditSlot::Name, Some(buf)) => editing_span(buf, 16, th),
            _ => Span::styled(pad(&fld.name, 16), style),
        };
        let default: Span = match (selected, st.slot, &st.editing) {
            (true, EditSlot::Default, Some(buf)) => editing_span(buf, 13, th),
            _ => Span::styled(pad(fld.default.text(), 13), style),
        };
        let refs: Span = match (selected, st.slot, &st.editing) {
            (true, EditSlot::Refs, Some(buf)) => {
                editing_span(buf, inner.width.saturating_sub(60), th)
            }
            _ => Span::styled(fld.references.clone(), style),
        };
        lines.push(Line::from(vec![
            name,
            Span::styled(pad(fld.ftype.as_str(), 9), style),
            Span::styled(pad(if fld.pk { "¶" } else { " " }, 4), style),
            Span::styled(pad(if fld.notnull { "*" } else { " " }, 6), style),
            Span::styled(pad(if fld.unique { "u" } else { " " }, 5), style),
            default,
            refs,
        ]));
    }
    let rows_h = (lines.len() as u16 + 1).min(inner.height.saturating_sub(4).max(1));
    let [rows_area, sql_area] =
        Layout::vertical([Constraint::Length(rows_h), Constraint::Fill(1)]).areas(inner);
    draw_rows(
        f,
        rows_area,
        lines,
        3,
        if st.cursor == 0 { 0 } else { st.cursor + 2 },
        &mut app.viewports.create,
    );
    // The SQL that F2 will run: CREATE for a new table, the compiled
    // native ALTERs for an existing one — or the
    // reason it can't be applied.
    let empty_schema = crate::creator::EditorSchema {
        table: String::new(),
        columns: Vec::new(),
        fks: Vec::new(),
    };
    let default_schema = &empty_schema;
    let schema = st.original.as_ref().unwrap_or(default_schema);
    let sql_lines: Vec<Line> = if st.original.is_some() {
        // TABLE EDITOR: preview native ALTERs or the refusal reason.
        match st.draft.apply_script(schema) {
            Ok(stmts) => {
                let mut v = vec![Line::styled("CHANGES:", th.dim())];
                if stmts.is_empty() {
                    v.push(Line::styled("(no changes)", th.dim()));
                }
                for s in &stmts {
                    v.push(Line::styled(format!("{s};"), th.bright()));
                }
                v
            }
            Err(e) => vec![
                Line::styled("CHANGES:", th.dim()),
                Line::styled(e, th.error()),
            ],
        }
    } else {
        vec![
            Line::styled("SQL:", th.dim()),
            Line::styled(st.draft.sql(), th.bright()),
        ]
    };
    f.render_widget(
        Paragraph::new(sql_lines).wrap(ratatui::widgets::Wrap { trim: false }),
        sql_area,
    );
}

/// Draw one canvas line through a clipped viewport without allocating a
/// buffer the size of a saved form. Coordinates remain design coordinates.
fn canvas_line(
    f: &mut Frame,
    inner: Rect,
    origin: (u16, u16),
    x: u16,
    y: u16,
    line: Line<'_>,
    width: u16,
) {
    let dy = y as i64 - origin.1 as i64;
    if dy < 0 || dy >= inner.height as i64 {
        return;
    }
    let dx = x as i64 - origin.0 as i64;
    let clip = (-dx).max(0) as u16;
    let start = dx.max(0) as u16;
    if clip >= width || start >= inner.width {
        return;
    }
    let rect = Rect {
        x: inner.x + start,
        y: inner.y + dy as u16,
        width: (width - clip).min(inner.width - start),
        height: 1,
    };
    f.render_widget(Paragraph::new(line).scroll((0, clip)), rect);
}

#[allow(clippy::too_many_arguments)]
fn paint_spec(
    f: &mut Frame,
    inner: Rect,
    spec: &crate::forms::FormSpec,
    th: &crate::theme::Theme,
    selected_col: Option<&str>,
    origin: (u16, u16),
    mut value_of: impl FnMut(&str) -> Option<Span<'static>>,
) {
    for b in &spec.boxes {
        if b.w < 2 || b.h < 2 {
            continue;
        }
        let edge = "─".repeat(b.w.saturating_sub(2) as usize);
        canvas_line(
            f,
            inner,
            origin,
            b.x,
            b.y,
            Line::styled(format!("┌{edge}┐"), th.dim()),
            b.w,
        );
        canvas_line(
            f,
            inner,
            origin,
            b.x,
            b.y.saturating_add(b.h - 1),
            Line::styled(format!("└{edge}┘"), th.dim()),
            b.w,
        );
        for y in b.y.saturating_add(1).max(origin.1)
            ..b.y
                .saturating_add(b.h - 1)
                .min(origin.1.saturating_add(inner.height))
        {
            canvas_line(f, inner, origin, b.x, y, Line::styled("│", th.dim()), 1);
            canvas_line(
                f,
                inner,
                origin,
                b.x.saturating_add(b.w - 1),
                y,
                Line::styled("│", th.dim()),
                1,
            );
        }
    }
    for t in &spec.texts {
        canvas_line(
            f,
            inner,
            origin,
            t.x,
            t.y,
            Line::styled(t.text.clone(), th.bright()),
            t.text.width().min(u16::MAX as usize) as u16,
        );
    }
    // Draw the selection last so an overlapping saved layout cannot hide it.
    let fields = spec
        .fields
        .iter()
        .filter(|fl| fl.include && selected_col != Some(fl.column.as_str()))
        .chain(
            spec.fields
                .iter()
                .filter(|fl| fl.include && selected_col == Some(fl.column.as_str())),
        );
    for field in fields {
        let Some((x, y)) = field.pos else { continue };
        let selected = selected_col == Some(field.column.as_str());
        let label = format!("{}:", field.label);
        let lw = label.width().min(u16::MAX as usize) as u16;
        canvas_line(
            f,
            inner,
            origin,
            x,
            y,
            Line::styled(label, if selected { th.cursor() } else { th.base() }),
            lw,
        );
        let value = value_of(&field.column)
            .unwrap_or_else(|| Span::styled("_".repeat(field.width as usize), th.dim()));
        canvas_line(
            f,
            inner,
            origin,
            x.saturating_add(lw).saturating_add(1),
            y,
            Line::from(value),
            field.width,
        );
    }
}

fn follow_canvas(offset: &mut u16, position: u16, size: u16) {
    if position < *offset {
        *offset = position;
    }
    if position as u32 >= *offset as u32 + size as u32 {
        *offset = position.saturating_sub(size.saturating_sub(1));
    }
}

fn draw_paint(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    let Overlay::Paint(st) = &app.overlay else {
        return;
    };
    let (cw, ch) = st.spec.size;
    let area = centered(
        dialog_area(f.area()),
        cw.saturating_add(2),
        ch.saturating_add(2),
    );
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

    follow_canvas(&mut app.viewports.paint_x, st.cursor.0, inner.width);
    follow_canvas(&mut app.viewports.paint_y, st.cursor.1, inner.height);
    let origin = (app.viewports.paint_x, app.viewports.paint_y);
    let selected_col = st.spec.fields.get(st.selected).map(|fl| fl.column.clone());
    paint_spec(
        f,
        inner,
        &st.spec,
        th,
        selected_col.as_deref(),
        origin,
        |_| None,
    );
    if let Some((bx, by)) = st.pending_box {
        canvas_line(f, inner, origin, bx, by, Line::styled("┌", th.bright()), 1);
    }
    if let Some(buf) = &st.editing {
        let available = inner
            .width
            .saturating_sub(st.cursor.0.saturating_sub(origin.0));
        canvas_line(
            f,
            inner,
            origin,
            st.cursor.0,
            st.cursor.1,
            Line::from(editing_span(buf, available.saturating_sub(1), th)),
            available,
        );
    } else {
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
        canvas_line(
            f,
            inner,
            origin,
            st.cursor.0,
            st.cursor.1,
            Line::styled(shown.to_string(), th.cursor()),
            1,
        );
    }
}

fn draw_form(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    let Overlay::Form(st) = &app.overlay else {
        return;
    };
    let area = centered(
        dialog_area(f.area()),
        88,
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
            " Space show · n add · x del · Enter label · m mask · c computed · [ ] order · F2 PAINT · F6 save ",
            th.dim(),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = vec![Line::from(vec![
        Span::styled(pad("FIELD", 16), th.dim()),
        Span::styled(pad("SHOW", 5), th.dim()),
        Span::styled(pad("REQ", 4), th.dim()),
        Span::styled(pad("PICTURE", 14), th.dim()),
        Span::styled(pad("COMPUTED", 18), th.dim()),
        Span::styled("LABEL", th.dim()),
    ])];
    for (i, field) in st.spec.fields.iter().enumerate() {
        let selected = i == st.cursor;
        let style = if selected { th.cursor() } else { th.base() };
        let label: Span = match (
            selected && !st.editing_mask && !st.editing_computed,
            &st.editing,
        ) {
            (true, Some(buf)) => editing_span(buf, inner.width.saturating_sub(63), th),
            _ => Span::styled(field.label.clone(), style),
        };
        let mask: Span = match (selected && st.editing_mask, &st.editing) {
            (true, Some(buf)) => editing_span(buf, 14, th),
            _ => Span::styled(pad(&field.mask, 14), style),
        };
        let computed: Span = match (selected && st.editing_computed, &st.editing) {
            (true, Some(buf)) => editing_span(buf, 18, th),
            _ => Span::styled(pad(&field.computed, 18), style),
        };
        lines.push(Line::from(vec![
            Span::styled(pad(&field.column, 16), style),
            Span::styled(pad(if field.include { "▪" } else { " " }, 5), style),
            Span::styled(pad(if field.required { "*" } else { " " }, 4), style),
            mask,
            computed,
            label,
        ]));
    }
    draw_rows(f, inner, lines, 1, st.cursor + 1, &mut app.viewports.form);
}

/// One-line preview of a possibly-multiline menu target (`⏎` when the
/// full source is longer — press E to edit it).
fn ref_preview(s: &str) -> String {
    let first = s.lines().next().unwrap_or("");
    if s.lines().count() > 1 {
        format!("{first} ⏎")
    } else {
        first.to_owned()
    }
}

fn draw_apps(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    let Overlay::Apps(st) = &app.overlay else {
        return;
    };
    let area = centered(
        dialog_area(f.area()),
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
            " n new · x del · Enter label · e target · E script · r name · c kind · F2 run ",
            th.dim(),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();
    if st.renaming_app {
        lines.push(Line::from(vec![
            Span::styled("app name: ", th.bright()),
            editing_span(
                st.editing.as_deref().unwrap_or(""),
                inner.width.saturating_sub(11),
                th,
            ),
        ]));
        lines.push(Line::raw(""));
    }
    lines.push(Line::from(vec![
        Span::styled(pad("LABEL", 24), th.dim()),
        Span::styled(pad("KIND", 8), th.dim()),
        Span::styled("TARGET (table · query/report · SQL · Lua)", th.dim()),
    ]));
    for (i, item) in st.items.iter().enumerate() {
        let selected = i == st.cursor;
        let style = if selected { th.cursor() } else { th.base() };
        let (label, target): (Span, Span) = match (selected, &st.editing) {
            (true, Some(buf)) if st.editing_ref => (
                Span::styled(pad(&item.label, 24), style),
                editing_span(buf, inner.width.saturating_sub(35), th),
            ),
            (true, Some(buf)) => (
                editing_span(buf, 24, th),
                Span::styled(ref_preview(&item.action_ref), style),
            ),
            _ => (
                Span::styled(pad(&item.label, 24), style),
                Span::styled(ref_preview(&item.action_ref), style),
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
    draw_rows(
        f,
        inner,
        lines,
        if st.renaming_app { 3 } else { 1 },
        if st.renaming_app { 0 } else { st.cursor + 1 },
        &mut app.viewports.apps,
    );
}

fn draw_app_menu(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    let Overlay::AppMenu(st) = &app.overlay else {
        return;
    };
    let width = 46u16;
    let area = centered(
        dialog_area(f.area()),
        width,
        (st.items.len() as u16 * 2 + 6).min(f.area().height),
    );
    f.render_widget(Clear, area);
    let title = if st.version > 1 {
        format!(" ▓▓ {} v{} ▓▓ ", st.app.to_uppercase(), st.version)
    } else {
        format!(" ▓▓ {} ▓▓ ", st.app.to_uppercase())
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(title, th.bright()))
        .title_alignment(Alignment::Center)
        .title_bottom(Line::styled(
            " ↑↓ + Enter · hotkey letters · Esc ",
            th.dim(),
        ));
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
            Span::styled(first, if selected { th.cursor() } else { th.bright() }),
            Span::styled(pad(&rest, width.saturating_sub(8)), style),
        ]));
        lines.push(Line::raw(""));
    }
    draw_rows(
        f,
        inner,
        lines,
        0,
        st.cursor * 2 + 1,
        &mut app.viewports.menu,
    );
}

/// The editing cell: buffer + caret, PADDED to the column width so the
/// columns to its right hold still while you type (they used to slide
/// with every keystroke — the designer's "drifting type" bug).
fn editing_span(buf: &str, width: u16, th: &crate::theme::Theme) -> Span<'static> {
    if width == 0 {
        return Span::styled("", th.cursor());
    }
    let room = width.saturating_sub(1) as usize;
    let mut suffix = String::new();
    let mut used = 0;
    for g in buf.graphemes(true).rev() {
        let w = g.width();
        if used + w > room {
            break;
        }
        suffix.insert_str(0, g);
        used += w;
    }
    Span::styled(
        format!("{suffix}▏{} ", " ".repeat(room - used)),
        th.cursor(),
    )
}

fn draw_qbe(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    let Overlay::Qbe(st) = &app.overlay else {
        return;
    };
    let area = centered(
        dialog_area(f.area()),
        76,
        (st.spec.cols.len() as u16 + 12).min(f.area().height.saturating_sub(2)),
    );
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(
            format!(
                " QUERY BY EXAMPLE · {} · {} ",
                st.spec.table,
                st.original_name.as_deref().unwrap_or("unsaved")
            ),
            th.bright(),
        ))
        .title_bottom(Line::styled(
            " Space show · Enter filter · s sort · J join · g group · F1 help ",
            th.dim(),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = vec![Line::from(vec![
        Span::styled(pad("COLUMN", 18), th.dim()),
        Span::styled(pad("SHOW", 5), th.dim()),
        Span::styled(pad("SORT", 5), th.dim()),
        Span::styled("FILTER (> 100 · like 'a%' · bare = equals)", th.dim()),
    ])];
    for (i, col) in st.spec.cols.iter().enumerate() {
        let selected = i == st.cursor;
        let row_style = if selected { th.cursor() } else { th.base() };
        let filter: Span = match (selected && !st.naming, &st.editing) {
            (true, Some(buf)) => editing_span(buf, inner.width.saturating_sub(32), th),
            _ => Span::styled(col.filter.clone(), row_style),
        };
        lines.push(Line::from(vec![
            Span::styled(pad(&col.name, 18), row_style),
            Span::styled(pad(if col.show { "▪" } else { " " }, 5), row_style),
            Span::styled(pad(col.sort.glyph(), 5), row_style),
            filter,
        ]));
    }
    let column_lines = std::mem::take(&mut lines);
    if let Some(j) = &st.spec.join {
        lines.push(Line::from(vec![
            Span::styled("JOIN  ", th.dim()),
            Span::styled(
                format!(
                    "{} ON {}.{} = {}.{}",
                    j.table, st.spec.table, j.left, j.table, j.right
                ),
                th.bright(),
            ),
        ]));
    }
    if let Some(g) = &st.spec.group_by {
        lines.push(Line::from(vec![
            Span::styled("GROUP ", th.dim()),
            Span::styled(format!("{g}  · count(*) AS n"), th.bright()),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "F2 run · F6 save · F7 save as · F8 rename · Esc close",
        th.dim(),
    ));
    if st.naming {
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", app.design_name_action.label()), th.bright()),
            editing_span(
                st.editing.as_deref().unwrap_or(""),
                inner
                    .width
                    .saturating_sub(app.design_name_action.label().len() as u16 + 3),
                th,
            ),
        ]));
    }
    let controls_h = lines.len() as u16;
    let rows_h =
        (column_lines.len() as u16).min(inner.height.saturating_sub(controls_h + 3).max(1));
    let [rows_area, controls_area, sql_area] = Layout::vertical([
        Constraint::Length(rows_h),
        Constraint::Length(controls_h),
        Constraint::Fill(1),
    ])
    .areas(inner);
    draw_rows(
        f,
        rows_area,
        column_lines,
        1,
        st.cursor + 1,
        &mut app.viewports.qbe,
    );
    f.render_widget(Paragraph::new(lines), controls_area);
    // The generated SQL WRAPS — on film it silently clipped at the box
    // edge and the ORDER BY was never visible. Showing the SQL is the
    // whole point of QBE; it must never be cut off.
    f.render_widget(
        Paragraph::new(vec![
            Line::styled("SQL:", th.dim()),
            Line::styled(st.spec.sql(), th.bright()),
        ])
        .wrap(ratatui::widgets::Wrap { trim: false }),
        sql_area,
    );
}

fn draw_report(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    let Overlay::Report(st) = &app.overlay else {
        return;
    };
    let area = centered(dialog_area(f.area()), 76, 12);
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
            " F2 preview · F6 save · F7 save as · F8 rename · Esc close ",
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
        let selected = i == st.cursor && !st.naming;
        let value_span = match (selected, &st.editing) {
            (true, Some(buf)) => editing_span(buf, inner.width.saturating_sub(13), th),
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
    if st.naming {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} : ", app.design_name_action.label()),
                th.bright(),
            ),
            editing_span(
                st.editing.as_deref().unwrap_or(""),
                inner
                    .width
                    .saturating_sub(app.design_name_action.label().len() as u16 + 4),
                th,
            ),
        ]));
    } else {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Enter edits · Space cycles group · numeric columns total automatically",
            th.dim(),
        ));
    }
    draw_rows(
        f,
        inner,
        lines,
        0,
        if st.naming { 4 } else { st.cursor },
        &mut app.viewports.report,
    );
}

fn draw_pager(f: &mut Frame, app: &App) {
    let th = app.theme;
    let Overlay::Pager(p) = &app.overlay else {
        return;
    };
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
    let Overlay::Health(hv) = &app.overlay else {
        return;
    };
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
            Span::styled(advice.chars().take(advice_w).collect::<String>(), th.dim()),
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

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(focus_style(app, Focus::Sidebar))
        .title(Span::styled(" Data ", focus_style(app, Focus::Sidebar)));
    let visible = app.visible_tables();
    let hidden = app.tables.len() - visible.len();
    let items: Vec<Line> = visible
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
            Line::styled(format!("{marker}{}", t.name), style)
        })
        .chain(
            (hidden > 0 && !app.show_internals)
                .then(|| Line::styled(format!("  … {hidden} internal (i)"), th.dim())),
        )
        .collect();
    let empty = visible.is_empty();
    let inner = block.inner(area);
    f.render_widget(block, area);
    draw_rows(
        f,
        inner,
        items,
        0,
        app.sidebar_idx,
        &mut app.viewports.sidebar,
    );
    if empty {
        let hint = Rect {
            x: area.x + 2,
            y: area.y + 2,
            width: area.width.saturating_sub(4),
            height: 3.min(area.height.saturating_sub(2)),
        };
        f.render_widget(
            Paragraph::new(vec![
                Line::styled("No tables yet", th.dim()),
                Line::styled("C  Create table", th.bright()),
            ]),
            hint,
        );
    }
}

fn draw_main(f: &mut Frame, app: &mut App, area: Rect) {
    draw_master_panel(f, app, area);
}

fn master_title(g: &Grid) -> String {
    match &g.source {
        GridSource::Table { name, editable } => {
            if *editable {
                format!(" BROWSE {name} ")
            } else {
                format!(" BROWSE {name} (read-only) ")
            }
        }
        GridSource::Query { truncated } => {
            if *truncated {
                " QUERY (capped at 10k rows) ".to_owned()
            } else {
                " QUERY ".to_owned()
            }
        }
        GridSource::Detail { .. } => " DETAIL ".to_owned(),
    }
}

fn draw_master_panel(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let title = app
        .grid
        .as_ref()
        .map(master_title)
        .unwrap_or_else(|| " phosphor ".to_owned());
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
        let empty = app.visible_tables().is_empty();
        let mut lines = vec![Line::raw("")];
        if app.scratch() {
            lines.extend([
                Line::styled("TEMPORARY SCRATCH DATABASE", th.bright()),
                Line::raw("Work here disappears when you quit."),
                Line::raw("F9  Save Database to a new file"),
                Line::raw(""),
            ]);
        } else if empty {
            lines.extend([
                Line::styled("Your database is ready.", th.bright()),
                Line::raw(if app.db.backend() == "embedded" {
                    "Saved changes are kept in this file."
                } else {
                    "Saved changes are kept on the server."
                }),
                Line::raw(""),
            ]);
        }
        if empty {
            lines.extend([
                Line::styled("C  Create your first table", th.bright()),
                Line::raw("Name its fields, then F2 builds it."),
                Line::raw("Next: a adds your first record."),
            ]);
        } else {
            lines.push(Line::raw("Enter on a table to BROWSE"));
        }
        lines.extend([
            Line::raw(""),
            Line::raw(".  SQL and commands"),
            Line::raw("F1 Help and the manual"),
        ]);
        f.render_widget(
            Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
            inner,
        );
        return;
    };

    // Visible column window: frozen leading columns always render, then
    // the scrolling window from max(col_off, frozen) (issue: 1988 BROWSE
    // "freeze").
    let frozen = g.frozen.min(g.columns.len());
    let start = g.col_off.max(frozen);
    let mut cols: Vec<usize> = Vec::new();
    let mut used: u16 = 0;
    for c in 0..frozen {
        used = used.saturating_add(g.widths[c] + 1);
        cols.push(c);
    }
    for c in start..g.columns.len() {
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
            .map(|&c| Span::styled(pad(&g.columns[c], g.widths[c]), th.bright()))
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
                    let text = row.get(c).map(PValue::render).unwrap_or_default();
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

fn draw_detail_panel(f: &mut Frame, app: &App, state: &DetailState, area: Rect) {
    let th = app.theme;
    let (child, child_col, key_sql) = match &state.grid.source {
        GridSource::Detail {
            child,
            child_col,
            key_sql,
            ..
        } => (child.as_str(), child_col.as_str(), key_sql.as_str()),
        _ => ("", "", ""),
    };
    let title = format!(
        " {child} · {child_col} = {key_sql} · {} rows ",
        state.grid.total
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(focus_style(app, Focus::Detail))
        .title(Span::styled(title, focus_style(app, Focus::Detail)))
        .title_bottom(Line::styled(
            if app.db.link().readonly() || state.grid.rowids.is_none() {
                " Tab master · v close · read-only "
            } else {
                " Enter edit · a add · x delete · Tab master "
            },
            th.dim(),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let visible = inner.height.saturating_sub(1).max(1) as i64; // minus header

    if state.grid.columns.is_empty() {
        f.render_widget(Paragraph::new(Line::styled("…", th.dim())), inner);
        return;
    }

    // Visible column window (pane is ~half the screen).
    let mut cols: Vec<usize> = Vec::new();
    let mut used: u16 = 0;
    for c in state.grid.col_off..state.grid.columns.len() {
        let w = state.grid.widths[c] + 1;
        if used + w > inner.width && !cols.is_empty() {
            break;
        }
        used += w;
        cols.push(c);
    }

    let g = &state.grid;
    let mut lines: Vec<Line> = Vec::with_capacity(inner.height as usize);
    lines.push(Line::from(
        cols.iter()
            .map(|&c| Span::styled(pad(&g.columns[c], g.widths[c]), th.bright()))
            .collect::<Vec<_>>(),
    ));
    for vis in 0..visible {
        let abs = g.row_off + vis;
        if abs >= g.total {
            break;
        }
        let spans: Vec<Span> = match g.row(abs) {
            Some(row) => cols
                .iter()
                .map(|&c| {
                    let text = row.get(c).map(PValue::render).unwrap_or_default();
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
    let clipped = s.width() > width;
    let room = width.saturating_sub(usize::from(clipped));
    let mut out = String::new();
    let mut used = 0;
    for g in s.graphemes(true) {
        if used + g.width() > room {
            break;
        }
        out.push_str(g);
        used += g.width();
    }
    if clipped && width > 0 {
        out.push('…');
        used += 1;
    }
    out.push_str(&" ".repeat(width - used + 1));
    out
}

fn draw_prompt(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Prompt && matches!(app.overlay, Overlay::None);
    let dot = Span::styled(" . ", if focused { th.bright() } else { th.dim() });
    let [prefix, content] =
        Layout::horizontal([Constraint::Length(3), Constraint::Fill(1)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(dot)), prefix);
    let line = if focused {
        caret_line(
            &app.prompt.input,
            app.prompt.cursor,
            content.width,
            &mut app.viewports.prompt_left,
            th,
        )
    } else {
        Line::styled(app.prompt.input.as_str(), th.dim())
    };
    f.render_widget(Paragraph::new(line), content);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(th.base()), area);
    let hint = " F12 details ";
    let hint_w = (hint.len() as u16).min(area.width);
    let content = Rect {
        width: area.width - hint_w,
        ..area
    };
    f.render_widget(
        Paragraph::new(hint).style(th.dim()),
        Rect {
            x: area.x + content.width,
            width: hint_w,
            ..area
        },
    );
    // A message owns the available width. Connection/position telemetry
    // must never hide a validation failure or destructive confirmation.
    if let Some((msg, error)) = &app.status {
        let style = if *error { th.error() } else { th.bright() };
        let text = format!(" {}", msg.replace(['\n', '\r', '\t'], " "));
        if let Some(g) = app.grid.as_ref().filter(|g| g.total > 0) {
            let position = format!(" row {}/{} ", g.cur_row + 1, g.total);
            let width = position.len() as u16;
            if Line::raw(&text).width() + (width as usize) < content.width as usize {
                f.render_widget(
                    Paragraph::new(position).style(th.dim()),
                    Rect {
                        x: content.x + content.width - width,
                        width,
                        ..content
                    },
                );
            }
        }
        let clipped = Line::raw(&text).width() > content.width as usize;
        let text_area = Rect {
            width: content.width.saturating_sub(u16::from(clipped)),
            ..content
        };
        f.render_widget(Paragraph::new(text).style(style), text_area);
        if clipped && content.width > 0 {
            f.render_widget(
                Paragraph::new("…").style(style),
                Rect {
                    x: content.x + content.width - 1,
                    width: 1,
                    ..content
                },
            );
        }
        return;
    }
    let name = app.db.name();
    let short_name = if name.contains("://") {
        name.split_once("://")
            .map(|(_, rest)| rest.split('/').next().unwrap_or(rest))
            .unwrap_or(name)
    } else {
        std::path::Path::new(name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(name)
    };
    let left = if app.scratch() {
        " SCRATCH · F9 Save Database".to_owned()
    } else {
        format!(" {short_name} [{}]", app.db.backend())
    };
    // Borrowed mid/message (were format!+clone per frame): the spans
    // borrow from `app`, which outlives the frame render.
    // The source name is deliberately omitted: the pane title above
    // already says BROWSE <table> / QUERY / DETAIL, and the status line
    // needs its width for the message and the row position.
    let mid: Cow<'_, str> = match &app.grid {
        Some(g) if g.total > 0 => Cow::Owned(format!(
            "row {}/{} · {}",
            g.cur_row + 1,
            g.total,
            g.columns.get(g.cur_col).map(String::as_str).unwrap_or("")
        )),
        _ => Cow::Borrowed(""),
    };
    let ms = app.last_ms.map(|m| format!("{m:.1}ms")).unwrap_or_default();
    let (dot, dot_style) = match &app.health {
        Some(s) => ("●", th.health(s)),
        None => ("○", th.dim()),
    };

    let right = format!("{}  {ms} {dot} ", mid.as_ref());
    let right_w = (Line::raw(&right).width() as u16).min(content.width);
    let split = content.width.saturating_sub(right_w);
    let left_area = Rect {
        width: split,
        ..area
    };
    let right_area = Rect {
        x: area.x + split,
        width: right_w,
        ..area
    };
    f.render_widget(Paragraph::new(left).style(th.dim()), left_area);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(mid.into_owned(), th.dim()),
            Span::styled(format!("  {ms} "), th.dim()),
            Span::styled(dot, dot_style),
            Span::styled(" ", th.dim()),
        ]))
        .alignment(Alignment::Right),
        right_area,
    );
}

fn draw_status_details(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    let area = f.area().inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .style(th.base())
        .border_style(th.bright())
        .title(" STATUS & CONNECTION ")
        .title_bottom(" ↑↓ scroll · PgUp/PgDn · Esc/F12 return ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let (msg, error) = app
        .status
        .as_ref()
        .map(|(s, e)| (s.as_str(), *e))
        .unwrap_or(("No status message.", false));
    let mut text: Vec<Line> = msg
        .lines()
        .map(|line| Line::styled(line, if error { th.error() } else { th.bright() }))
        .collect();
    text.extend([
        Line::raw(""),
        Line::styled("Connection", th.bright()),
        Line::raw(app.db.name()),
        Line::raw(format!("Backend: {}", app.db.backend())),
    ]);
    let paragraph = Paragraph::new(text).wrap(ratatui::widgets::Wrap { trim: false });
    let max = paragraph
        .line_count(inner.width)
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    let offset = app.status_details.unwrap_or(0).min(max);
    app.status_details = Some(offset);
    f.render_widget(paragraph.scroll((offset, 0)), inner);
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

/// Render the SET RELATION panes: one titled mini-list per child link.
fn link_pane_lines(ed: &crate::app::EditState, th: &crate::theme::Theme) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    for (i, link) in ed.links.iter().enumerate() {
        let key = ["F4", "F5", "F6"].get(i).copied().unwrap_or("");
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(format!("─ {} ({}) ", link.child, link.total), th.bright()),
            Span::styled(
                format!("{key} opens ").to_string() + &"─".repeat(30),
                th.dim(),
            ),
        ]));
        if link.rows.is_empty() {
            lines.push(Line::styled("  (none yet)", th.dim()));
            continue;
        }
        lines.push(Line::styled(
            format!(
                "  {}",
                link.header
                    .iter()
                    .map(|h| format!("{h:<14.14}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            th.dim(),
        ));
        for row in &link.rows {
            lines.push(Line::styled(
                format!(
                    "  {}",
                    row.iter()
                        .map(|v| format!("{v:<14.14}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
                th.base(),
            ));
        }
        if link.total > link.rows.len() as i64 {
            lines.push(Line::styled(
                format!("  … {} more ({key})", link.total - link.rows.len() as i64),
                th.dim(),
            ));
        }
    }
    lines
}

fn draw_edit(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    let Overlay::Edit(ed) = &app.overlay else {
        return;
    };
    // Painted forms render CREATE SCREEN style; unpainted stay a list.
    let fits = |spec: &&crate::forms::FormSpec| {
        let area = dialog_area(f.area());
        spec.size.0.saturating_add(2) <= area.width
            && spec.size.1.saturating_add(2) <= area.height
            && ed.fields.iter().all(|(c, _)| {
                spec.fields.iter().any(|fl| {
                    fl.column == c.name
                        && fl.pos.is_some_and(|(x, y)| {
                            x as usize + fl.label.width() + 2 + fl.width as usize
                                <= spec.size.0 as usize
                                && y < spec.size.1
                        })
                })
            })
    };
    if let Some(spec) = ed.painted.as_ref().filter(fits) {
        let (cw, ch) = spec.size;
        let panes = link_pane_lines(ed, th);
        let area = centered(
            dialog_area(f.area()),
            cw.saturating_add(2).max(50),
            (ch.saturating_add(2).saturating_add(panes.len() as u16)).min(f.area().height),
        );
        f.render_widget(Clear, area);
        let dirty = if ed.dirty() { " *" } else { "" };
        let title = if ed.inserting {
            format!(" NEW {} record{dirty} ", ed.table)
        } else {
            format!(
                " EDIT {} · {}/{}{dirty} ",
                ed.table,
                ed.row_abs + 1,
                if ed.relation.is_some() {
                    app.detail.as_ref().map(|d| d.grid.total)
                } else {
                    app.grid.as_ref().map(|g| g.total)
                }
                .unwrap_or(0)
            )
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(th.bright())
            .style(th.base())
            .title(Span::styled(title, th.bright()))
            .title_bottom(Line::styled(
                " type to edit · ↑↓/Tab field · Enter save · PgUp/PgDn record · Esc ",
                th.dim(),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let selected_col = ed.fields.get(ed.cursor).map(|(c, _)| c.name.clone());
        app.viewports.edit.rows = inner.height as usize;
        paint_spec(f, inner, spec, th, selected_col.as_deref(), (0, 0), |col| {
            let i = ed.fields.iter().position(|(c, _)| c.name == col)?;
            let selected = i == ed.cursor;
            Some(match (selected, &ed.editing) {
                (true, Some(buf)) => editing_span(
                    buf,
                    spec.fields
                        .iter()
                        .find(|fl| fl.column == col)
                        .map_or(1, |fl| fl.width.saturating_sub(1)),
                    th,
                ),
                _ => {
                    let (text, edited) = match &ed.inputs[i] {
                        Some(t) => (note_display(t), true),
                        None if ed.automatic(i) => ("(automatic)".into(), false),
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
        if !panes.is_empty() && inner.height > ch {
            let below = Rect {
                x: inner.x,
                y: inner.y + ch,
                width: inner.width,
                height: inner.height - ch,
            };
            f.render_widget(Paragraph::new(panes), below);
        }
        return;
    }
    let label_w = ed
        .labels
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(4)
        .clamp(4, 20);
    let panes = link_pane_lines(ed, th);
    let area = centered(
        dialog_area(f.area()),
        if panes.is_empty() { 62 } else { 66 },
        (ed.fields.len() as u16 + 4 + panes.len() as u16).min(f.area().height),
    );
    f.render_widget(Clear, area);
    let dirty = if ed.dirty() { " *" } else { "" };
    let title = if ed.inserting {
        format!(" NEW {} record{dirty} ", ed.table)
    } else {
        format!(
            " EDIT {} · {}/{}{dirty} ",
            ed.table,
            ed.row_abs + 1,
            if ed.relation.is_some() {
                app.detail.as_ref().map(|d| d.grid.total)
            } else {
                app.grid.as_ref().map(|g| g.total)
            }
            .unwrap_or(0)
        )
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
        // PICTURE-clause energy: ¶ pk, * required, ƒ computed.
        let marker = if ed.parent_field(i) {
            "↳"
        } else if ed.read_only(i) {
            "ƒ"
        } else if col.pk {
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
            editing_span(buf, inner.width.saturating_sub(label_w as u16 + 5), th)
        } else {
            let (text, edited) = match &ed.inputs[i] {
                Some(t) => (note_display(t), true),
                None if ed.automatic(i) => ("(automatic)".into(), false),
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
    lines.extend(panes);
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "type to edit · Tab next · Enter save · PgUp/PgDn record · Esc cancel",
        th.dim(),
    ));
    draw_rows(f, inner, lines, 0, ed.cursor, &mut app.viewports.edit);
}

/// The F7 foreign-key value picker: a scrollable list of parent rows;
/// the first column is the key that gets written into the field.
fn draw_picker(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    let Overlay::Edit(ed) = &app.overlay else {
        return;
    };
    let Some(p) = &ed.picker else { return };
    let area = centered(dialog_area(f.area()), 84, 24);
    f.render_widget(Clear, area);
    let position = if p.loading.is_some() {
        "loading…".into()
    } else if p.error.is_some() {
        "lookup failed · F5 retry".into()
    } else {
        format!(
            "{} / {} matches",
            if p.total == 0 { 0 } else { p.cursor + 1 },
            p.total
        )
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(format!(" PICK · {} · {position} ", p.parent))
        .title_bottom(" / search · Enter pick · Esc close · PgUp/PgDn · Home/End · ←→ columns ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [search_area, rows_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(inner);
    let mut search = vec![Span::styled("Search: ", th.bright())];
    search.push(match &p.editing {
        Some(buf) => editing_span(buf, inner.width.saturating_sub(9), th),
        None => Span::styled(
            if p.search.is_empty() {
                "(all records)"
            } else {
                &p.search
            },
            th.base(),
        ),
    });
    f.render_widget(
        Paragraph::new(vec![Line::from(search), Line::styled(position, th.dim())]),
        search_area,
    );
    if p.loading.is_some() {
        f.render_widget(
            Paragraph::new("Loading matching records…").style(th.dim()),
            rows_area,
        );
        return;
    }
    if let Some(error) = &p.error {
        f.render_widget(Paragraph::new(format!("Lookup failed: {error}\nF5 retries; / changes the search; Esc returns to the form."))
            .wrap(ratatui::widgets::Wrap { trim: false }).style(th.error()), rows_area);
        return;
    }
    if p.rows.is_empty() {
        f.render_widget(Paragraph::new("No matching records.\nPress / to change or clear the search; Esc returns to the form.")
            .wrap(ratatui::widgets::Wrap { trim: false }).style(th.dim()), rows_area);
        return;
    }
    // Freeze the key; arrow horizontally through the descriptive fields.
    let count = ((inner.width as usize / 20).max(2)).min(p.columns.len());
    let indexes: Vec<_> = std::iter::once(0)
        .chain((p.column + 1..p.columns.len()).take(count.saturating_sub(1)))
        .collect();
    let width = inner.width / indexes.len().max(1) as u16;
    let mut lines = vec![Line::from(
        indexes
            .iter()
            .map(|&i| Span::styled(pad(&p.columns[i], width.saturating_sub(1)), th.dim()))
            .collect::<Vec<_>>(),
    )];
    for (i, row) in p.rows.iter().enumerate() {
        let style = if p.start + i == p.cursor {
            th.cursor()
        } else {
            th.base()
        };
        lines.push(Line::from(
            indexes
                .iter()
                .map(|&c| {
                    Span::styled(
                        pad(
                            &row.get(c).map(PValue::render).unwrap_or_default(),
                            width.saturating_sub(1),
                        ),
                        style,
                    )
                })
                .collect::<Vec<_>>(),
        ));
    }
    draw_rows(
        f,
        rows_area,
        lines,
        1,
        p.cursor.saturating_sub(p.start) + 1,
        &mut app.viewports.picker,
    );
}

/// The multi-line Lua editor: line numbers, an inverse caret, dirty
/// marker, and a hint footer. Full screen so long scripts breathe.
/// A note's newlines show as ␤ in the one-line EDIT field; the full
/// note editor (F3) is where they are real.
fn note_display(text: &str) -> String {
    text.replace('\n', "␤")
}

fn draw_script_editor(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    let Overlay::ScriptEditor(st) = &app.overlay else {
        return;
    };
    let area = f.area().inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });
    f.render_widget(Clear, area);
    let dirty = if st.dirty { " *" } else { "" };
    let hint = if matches!(&st.target, ScriptTarget::Memo { .. }) {
        " type · Enter newline · Tab indent · F6 keep · Esc cancel "
    } else {
        " type · Enter newline · Tab indent · F6 save · Esc close "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(th.bright())
        .style(th.base())
        .title(Span::styled(format!("{}{dirty} ", st.title()), th.bright()))
        .title_bottom(Line::styled(hint, th.dim()));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let number_width = (st.lines.len().to_string().len().max(3) + 1) as u16;
    let [numbers, content] =
        Layout::horizontal([Constraint::Length(number_width), Constraint::Fill(1)]).areas(inner);
    app.viewports
        .script
        .follow(st.row, st.lines.len(), inner.height as usize);
    let start = app.viewports.script.top;
    let current = st.lines.get(st.row).map(String::as_str).unwrap_or("");
    let byte = current
        .char_indices()
        .nth(st.col)
        .map(|(b, _)| b)
        .unwrap_or(current.len());
    let current_line = caret_line(
        current,
        byte,
        content.width,
        &mut app.viewports.script_left,
        th,
    );
    let mut nums = Vec::new();
    let mut lines = Vec::new();
    for (i, text) in st
        .lines
        .iter()
        .enumerate()
        .skip(start)
        .take(inner.height as usize)
    {
        nums.push(Line::styled(
            format!(
                "{:>w$} ",
                i + 1,
                w = number_width.saturating_sub(1) as usize
            ),
            th.dim(),
        ));
        lines.push(if i == st.row {
            current_line.clone()
        } else {
            Line::styled(text_from_column(text, app.viewports.script_left), th.base())
        });
    }
    f.render_widget(Paragraph::new(nums), numbers);
    f.render_widget(Paragraph::new(lines), content);
}

fn draw_help(f: &mut Frame, app: &mut App) {
    let th = app.theme;
    let Overlay::Help(st) = &mut app.overlay else {
        return;
    };
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

    let (toc, body) = if inner.width >= 64 {
        let [toc, _, body] = Layout::horizontal([
            Constraint::Length(16),
            Constraint::Length(2),
            Constraint::Fill(1),
        ])
        .areas(inner);
        (toc, body)
    } else {
        (Rect::default(), inner)
    };

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
    draw_rows(
        f,
        toc,
        toc_lines,
        0,
        st.topic,
        &mut app.viewports.help_topics,
    );

    let mut body_lines: Vec<Line> = vec![
        Line::styled(topic.title.to_uppercase(), th.bright()),
        Line::raw(""),
    ];
    body_lines.extend(topic.body.lines().map(|l| Line::styled(l, th.base())));
    let paragraph = Paragraph::new(body_lines).wrap(ratatui::widgets::Wrap { trim: false });
    let max = paragraph
        .line_count(body.width)
        .saturating_sub(body.height as usize)
        .min(u16::MAX as usize) as u16;
    st.scroll = st.scroll.min(max);
    f.render_widget(paragraph.scroll((st.scroll, 0)), body);
}
