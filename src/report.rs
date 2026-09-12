//! The banded report writer + label writer (DESIGN.md phase 4): page
//! header, group bands with subtotals, detail lines, grand totals,
//! page footer — the engine behind forty years of business paperwork.

use crate::db::{DbLink, DbResult, PValue};
use crate::store;

pub const PAGE_LINES: usize = 55;
const PAGE_WIDTH: usize = 100;
/// Synthetic column the grouping expression is selected as; stripped
/// back off before layout (it is a band key, not a report column).
const GROUP_ALIAS: &str = "__phosphor_group";

#[derive(Debug, Clone)]
pub struct ReportSpec {
    pub name: String,
    pub title: String,
    /// A table name or any SELECT (saved queries paste their SQL here).
    pub source: String,
    /// Column name OR SQL expression to group on (adds group bands +
    /// subtotals). `region` and `substr(city, 1, 1)` both work.
    pub group_by: Option<String>,
}

impl ReportSpec {
    pub fn for_table(table: &str) -> ReportSpec {
        ReportSpec {
            name: table.to_owned(),
            title: format!("{table} report"),
            source: table.to_owned(),
            group_by: None,
        }
    }

    fn source_sql(&self) -> String {
        let src = self.source.trim();
        let lower = src.to_ascii_lowercase();
        let base = if lower.starts_with("select") || lower.starts_with("with") {
            format!("({src})")
        } else {
            format!("\"{}\"", src.replace('"', "\"\""))
        };
        match &self.group_by {
            // Group bands need group-sorted rows; the report sorts, the
            // user doesn't have to know. The grouping key rides along as
            // a synthetic column so a full expression — not just a column
            // name — can drive the bands; `render` strips it back off.
            Some(g) => {
                let expr = g.trim().trim_end_matches(';').trim();
                format!("SELECT *, ({expr}) AS {GROUP_ALIAS} FROM {base} ORDER BY {GROUP_ALIAS}")
            }
            None => format!("SELECT * FROM {base}"),
        }
    }

    pub fn save(&self, db: &dyn DbLink) -> DbResult<()> {
        store::upsert(
            db,
            "_phosphor_reports",
            "name",
            &self.name,
            &[
                ("title", self.title.clone()),
                ("source_sql", self.source.clone()),
                ("group_by", self.group_by.clone().unwrap_or_default()),
            ],
        )
    }

    pub fn load(db: &dyn DbLink, name: &str) -> Option<ReportSpec> {
        let r = store::lookup(
            db,
            "_phosphor_reports",
            "name",
            name,
            &["title", "source_sql", "group_by"],
        )?;
        Some(ReportSpec {
            name: name.to_owned(),
            title: r[0].clone(),
            source: r[1].clone(),
            group_by: Some(r[2].clone()).filter(|g| !g.is_empty()),
        })
    }
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

fn pad(s: &str, w: usize) -> String {
    let count = s.chars().count();
    let mut out: String = s.chars().take(w).collect();
    if count > w && w > 0 {
        out.pop();
        out.push('…');
    }
    for _ in count.min(w)..w {
        out.push(' ');
    }
    out
}

fn rpad(s: &str, w: usize) -> String {
    let len = s.chars().count().min(w);
    let mut out = String::with_capacity(w);
    for _ in len..w {
        out.push(' ');
    }
    out.extend(s.chars().take(w));
    out
}

/// Render the report to pageable text lines. Pure layout over DbLink
/// data — the same render drives the screen pager and the file writer.
pub fn render(db: &dyn DbLink, spec: &ReportSpec) -> DbResult<Vec<String>> {
    let crate::db::QueryResult {
        mut columns,
        mut rows,
        truncated,
        ..
    } = db.query(&spec.source_sql())?;

    // Split the synthetic grouping column off before layout. `group_vals`
    // is then the band key per row; what remains are the real columns.
    let group_vals: Option<Vec<String>> =
        if spec.group_by.is_some() && columns.last().is_some_and(|c| c == GROUP_ALIAS) {
            let idx = columns.len() - 1;
            let vals = rows.iter().map(|r| r[idx].render()).collect();
            for r in rows.iter_mut() {
                r.pop();
            }
            columns.pop();
            Some(vals)
        } else {
            None
        };
    let ncols = columns.len();

    // Single pass over PValues: numeric detection + grand totals together
    // (was two full passes). Non-numeric cells mark the column; numeric
    // cells accumulate — exclusions below zero out anything disqualified.
    let mut numeric = vec![!rows.is_empty(); ncols];
    let mut grand = vec![0f64; ncols];
    for row in &rows {
        for (i, v) in row.iter().enumerate() {
            match v {
                PValue::Int(n) => grand[i] += *n as f64,
                PValue::Real(f) => grand[i] += f,
                PValue::Null => {}
                _ => numeric[i] = false,
            }
        }
    }
    // Grouping by a plain column: that column is a key, not a quantity.
    // (An expression group can't map to one column; its own synthetic
    // column was already stripped, and any numeric source columns it
    // groups over remain summable.)
    let group_col = spec.group_by.as_ref().and_then(|g| {
        let name = g.trim().trim_matches('"');
        columns.iter().position(|c| c.eq_ignore_ascii_case(name))
    });
    if let Some(gi) = group_col {
        numeric[gi] = false;
    }
    // Identifiers are not quantities: never total a table's PRIMARY KEY,
    // nor id-shaped columns from arbitrary SELECT sources ("TOTAL: 36"
    // over customer ids was proudly displayed nonsense — found on film).
    let src = spec.source.trim();
    if !src.to_ascii_lowercase().starts_with("select")
        && !src.to_ascii_lowercase().starts_with("with")
    {
        if let Ok(cols) = db.columns(src) {
            for c in cols.iter().filter(|c| c.pk) {
                if let Some(i) = columns.iter().position(|n| *n == c.name) {
                    numeric[i] = false;
                }
            }
        }
    }
    for (i, name) in columns.iter().enumerate() {
        let lower = name.to_ascii_lowercase();
        if lower == "id" || lower == "rowid" || lower.ends_with("_id") {
            numeric[i] = false;
        }
    }
    for (i, n) in numeric.iter().enumerate() {
        if !n {
            grand[i] = 0.0;
        }
    }

    // Render every cell ONCE into a string cache, measuring widths inline
    // (was: a widths pass re-rendering every cell + a detail pass
    // rendering them all again).
    let mut rendered: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    let mut widths: Vec<usize> = columns.iter().map(|c| c.chars().count()).collect();
    for row in &rows {
        let mut r = Vec::with_capacity(ncols);
        for (i, v) in row.iter().enumerate() {
            let s = v.render();
            widths[i] = widths[i].max(s.chars().count());
            r.push(s);
        }
        rendered.push(r);
    }
    // Totals can widen a column beyond its data (a column of 3-digit
    // amounts has a 6-digit total — found by test).
    for (i, w) in widths.iter_mut().enumerate() {
        if numeric[i] {
            *w = (*w).max(fmt_num(grand[i]).chars().count());
        }
        *w = (*w).clamp(3, 26);
    }

    let cell = |s: &str, i: usize| -> String {
        if numeric[i] {
            rpad(s, widths[i])
        } else {
            pad(s, widths[i])
        }
    };
    let header_line = columns
        .iter()
        .enumerate()
        .map(|(i, c)| pad(c, widths[i]))
        .collect::<Vec<_>>()
        .join(" ");
    let rule = "─".repeat(header_line.chars().count().min(PAGE_WIDTH));

    let totals_line = |label: &str, sums: &[f64], count: usize| -> Vec<String> {
        // One reserved String instead of Vec<String> + join per group.
        let mut cells = String::with_capacity(ncols * 8);
        for i in 0..ncols {
            if i > 0 {
                cells.push(' ');
            }
            cells.push_str(&if numeric[i] {
                rpad(&fmt_num(sums[i]), widths[i])
            } else {
                " ".repeat(widths[i])
            });
        }
        vec![rule.clone(), format!("{label} ({count} rows)"), cells]
    };

    let mut out: Vec<String> = Vec::new();
    let mut page = 0usize;
    let mut line_on_page = usize::MAX; // force header on first line

    let mut emit = |lines: &mut Vec<String>, s: String| {
        if line_on_page >= PAGE_LINES {
            if page > 0 {
                lines.push(format!(
                    "{}page {page}",
                    " ".repeat(PAGE_WIDTH.saturating_sub(9))
                ));
                lines.push("\u{c}".into()); // form feed between pages
            }
            page += 1;
            lines.push(format!("{}  ·  page {page}", spec.title));
            lines.push(header_line.clone());
            lines.push(rule.clone());
            line_on_page = 3;
        }
        lines.push(s);
        line_on_page += 1;
    };

    let mut group_sums = vec![0f64; ncols];
    let mut group_n = 0usize;
    let mut current_group: Option<String> = None;
    // The band caption: the real column name when grouping by one, else
    // the expression text the user typed (which is what they recognize).
    let group_label: Option<String> = spec.group_by.as_ref().map(|g| {
        group_col
            .map(|i| columns[i].clone())
            .unwrap_or_else(|| g.clone())
    });

    // Detail pass over the RENDERED cache: no PValue::render() here at
    // all (group keys and cells are reused strings). `grand` was already
    // accumulated in pass 1 — only group subtotals accrue here.
    for (ri, row) in rows.iter().enumerate() {
        let rrow = &rendered[ri];
        if let (Some(label), Some(vals)) = (&group_label, &group_vals) {
            let g = vals[ri].as_str();
            if current_group.as_deref() != Some(g) {
                if current_group.is_some() {
                    for l in totals_line("  subtotal", &group_sums, group_n) {
                        emit(&mut out, l);
                    }
                    emit(&mut out, String::new());
                }
                emit(&mut out, format!("▌ {label} = {g}"));
                current_group = Some(g.to_owned());
                group_sums.fill(0.0);
                group_n = 0;
            }
        }
        let mut line = String::with_capacity(ncols * 8);
        for (i, s) in rrow.iter().enumerate() {
            if i > 0 {
                line.push(' ');
            }
            line.push_str(&cell(s, i));
        }
        emit(&mut out, line);
        for (i, v) in row.iter().enumerate() {
            if numeric[i] {
                match v {
                    PValue::Int(n) => group_sums[i] += *n as f64,
                    PValue::Real(f) => group_sums[i] += f,
                    _ => {}
                }
            }
        }
        group_n += 1;
    }
    if group_label.is_some() && current_group.is_some() {
        for l in totals_line("  subtotal", &group_sums, group_n) {
            emit(&mut out, l);
        }
    }
    emit(&mut out, String::new());
    for l in totals_line("TOTAL", &grand, rows.len()) {
        emit(&mut out, l);
    }
    if truncated {
        emit(&mut out, "(source truncated at the 10k query cap)".into());
    }
    Ok(out)
}

/// The label writer: every visible column of each row becomes a line,
/// three labels across — Avery 5160 energy, zero configuration.
pub fn labels(db: &dyn DbLink, table: &str) -> DbResult<Vec<String>> {
    const ACROSS: usize = 3;
    const LABEL_W: usize = 32;
    let quoted = format!("\"{}\"", table.replace('"', "\"\""));
    let q = db.query(&format!("SELECT * FROM {quoted}"))?;
    let per_label = q.columns.len().max(1) + 1; // + blank separator
    let mut out = Vec::new();
    for chunk in q.rows.chunks(ACROSS) {
        for line_idx in 0..per_label {
            let mut line = String::with_capacity(ACROSS * LABEL_W);
            for row in chunk {
                let text = if line_idx < q.columns.len() {
                    row[line_idx].render()
                } else {
                    String::new()
                };
                line.push_str(&pad(&text, LABEL_W - 2));
                line.push_str("  ");
            }
            // Trim in place instead of trim_end().to_owned() (one copy saved per line).
            line.truncate(line.trim_end().len());
            out.push(line);
        }
    }
    Ok(out)
}

/// Scrollable pager over rendered lines; 'w' writes them to a file.
pub struct PagerState {
    pub title: String,
    pub lines: Vec<String>,
    pub offset: usize,
    pub file_stem: String,
}

impl PagerState {
    pub fn write_file(&self) -> Result<String, String> {
        use std::io::Write as _;
        let path = format!("{}.txt", self.file_stem);
        let f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        // Stream line-by-line: join() would spike 2x memory on big reports.
        let mut w = std::io::BufWriter::new(f);
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                w.write_all(b"\n").map_err(|e| e.to_string())?;
            }
            w.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        }
        w.flush().map_err(|e| e.to_string())?;
        Ok(path)
    }

    /// `p` in the pager: send the rendered report/labels to a printer.
    /// Writes the text file first (the artifact exists either way), then
    /// runs `$PHOSPHOR_PRINT` (default `lp`, falling back to `lpr`) with
    /// the path appended. A missing printer is a clear error, never a
    /// panic; the file stays for manual printing.
    pub fn print_via_lp(&self) -> Result<String, String> {
        let path = self.write_file()?;
        let configured = std::env::var("PHOSPHOR_PRINT").unwrap_or_default();
        let attempts: Vec<String> = if configured.trim().is_empty() {
            vec!["lp".to_owned(), "lpr".to_owned()]
        } else {
            vec![configured]
        };
        let mut last = String::new();
        for spec in attempts {
            let mut parts = spec.split_whitespace();
            let Some(bin) = parts.next() else { continue };
            let args: Vec<&str> = parts.collect();
            match std::process::Command::new(bin)
                .args(&args)
                .arg(&path)
                .stdin(std::process::Stdio::null())
                .output()
            {
                Ok(out) if out.status.success() => {
                    return Ok(format!("sent {path} to {bin}"));
                }
                Ok(out) => {
                    last = format!("{bin}: {}", String::from_utf8_lossy(&out.stderr).trim());
                }
                Err(e) => last = format!("{bin}: {e}"),
            }
        }
        Err(format!(
            "no printer available ({last}); wrote {path} — print it by hand"
        ))
    }
}

/// Designer state for a report: cursor over (title, source, group_by).
pub struct ReportState {
    pub spec: ReportSpec,
    pub cursor: usize, // 0 title, 1 source, 2 group_by
    pub editing: Option<String>,
    /// Columns of the current source (group_by cycles through these).
    pub columns: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::EmbeddedDb;

    fn db() -> EmbeddedDb {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE sales(region TEXT, rep TEXT, amount REAL);
             INSERT INTO sales VALUES
               ('east','ada',100.0),('east','grace',50.0),
               ('west','edsger',25.0),('west','ada',25.0);",
        )
        .unwrap();
        db
    }

    #[test]
    fn grouped_report_has_bands_and_totals() {
        let db = db();
        let spec = ReportSpec {
            name: "sales".into(),
            title: "Sales by region".into(),
            source: "sales".into(),
            group_by: Some("region".into()),
        };
        let lines = render(&db, &spec).unwrap();
        let text = lines.join("\n");
        assert!(text.contains("▌ region = east"));
        assert!(text.contains("▌ region = west"));
        // east subtotal 150, west 50, grand 200 — verify all present.
        assert_eq!(text.matches("subtotal (2 rows)").count(), 2);
        assert!(text.contains("150"));
        assert!(text.contains("TOTAL (4 rows)"));
        assert!(text.contains("200"));
        assert!(lines[0].contains("Sales by region"), "page header first");
    }

    /// #17: grouping by an arbitrary SQL expression (not just a column)
    /// still produces bands + subtotals, captioned with the expression.
    #[test]
    fn grouped_report_on_expression() {
        let db = db();
        let spec = ReportSpec {
            name: "sales".into(),
            title: "By initial".into(),
            source: "sales".into(),
            group_by: Some("substr(region, 1, 1)".into()),
        };
        let lines = render(&db, &spec).unwrap();
        let text = lines.join("\n");
        assert!(text.contains("▌ substr(region, 1, 1) = e"), "{text}");
        assert!(text.contains("▌ substr(region, 1, 1) = w"), "{text}");
        assert_eq!(text.matches("subtotal (2 rows)").count(), 2);
        assert!(text.contains("TOTAL (4 rows)"));
        // The amount column still totals across the expression groups.
        assert!(text.contains("200"));
    }

    #[test]
    fn spec_round_trips_through_store() {
        let db = db();
        let spec = ReportSpec {
            name: "r1".into(),
            title: "T".into(),
            source: "SELECT rep, amount FROM sales".into(),
            group_by: None,
        };
        spec.save(&db).unwrap();
        let back = ReportSpec::load(&db, "r1").unwrap();
        assert_eq!(back.title, "T");
        assert!(back.source.starts_with("SELECT"));
        assert_eq!(back.group_by, None);
    }

    #[test]
    fn labels_are_three_across() {
        let db = db();
        let lines = labels(&db, "sales").unwrap();
        // 4 rows → 2 banks of labels; first line holds 3 regions.
        let first = &lines[0];
        assert_eq!(
            first.matches("east").count() + first.matches("west").count(),
            3
        );
    }

    #[test]
    fn pager_writes_file() {
        let p = PagerState {
            title: "t".into(),
            lines: vec!["a".into(), "b".into()],
            offset: 0,
            file_stem: std::env::temp_dir()
                .join(format!("phosphor-pager-{}", std::process::id()))
                .to_string_lossy()
                .into_owned(),
        };
        let path = p.write_file().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a\nb");
        let _ = std::fs::remove_file(path);
    }

    /// #13: with a bogus printer command the pager reports cleanly and
    /// still leaves the text file behind for manual printing.
    #[test]
    fn pager_print_missing_printer_keeps_file() {
        let stem = std::env::temp_dir()
            .join(format!("phosphor-print-{}", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let p = PagerState {
            title: "t".into(),
            lines: vec!["hello".into()],
            offset: 0,
            file_stem: stem.clone(),
        };
        std::env::set_var("PHOSPHOR_PRINT", "/nonexistent/phosphor-printer-xyz");
        let err = p.print_via_lp().unwrap_err();
        std::env::remove_var("PHOSPHOR_PRINT");
        assert!(err.contains("no printer available"), "{err}");
        assert_eq!(
            std::fs::read_to_string(format!("{stem}.txt")).unwrap(),
            "hello"
        );
        let _ = std::fs::remove_file(format!("{stem}.txt"));
    }
}
