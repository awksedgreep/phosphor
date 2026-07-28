//! The banded report writer + label writer (DESIGN.md phase 4): page
//! header, group bands with subtotals, detail lines, grand totals,
//! page footer — the engine behind forty years of business paperwork.

use crate::db::{DbLink, DbResult, PValue};
use crate::store;

pub const PAGE_LINES: usize = 55;
const PAGE_WIDTH: usize = 100;

#[derive(Debug, Clone)]
pub struct ReportSpec {
    pub name: String,
    pub title: String,
    /// A table name or any SELECT (saved queries paste their SQL here).
    pub source: String,
    /// Column name to group on (adds group bands + subtotals).
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
        let base = if src.to_ascii_lowercase().starts_with("select")
            || src.to_ascii_lowercase().starts_with("with")
        {
            format!("({src})")
        } else {
            format!("\"{}\"", src.replace('"', "\"\""))
        };
        match &self.group_by {
            // Group bands need group-sorted rows; the report sorts, the
            // user doesn't have to know.
            Some(g) => format!(
                "SELECT * FROM {base} ORDER BY \"{}\"",
                g.replace('"', "\"\"")
            ),
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
    let mut out: String = s.chars().take(w).collect();
    if s.chars().count() > w && w > 0 {
        out.pop();
        out.push('…');
    }
    while out.chars().count() < w {
        out.push(' ');
    }
    out
}

fn rpad(s: &str, w: usize) -> String {
    let mut out = String::new();
    let len = s.chars().count().min(w);
    for _ in 0..w.saturating_sub(len) {
        out.push(' ');
    }
    out.extend(s.chars().take(w));
    out
}

/// Render the report to pageable text lines. Pure layout over DbLink
/// data — the same render drives the screen pager and the file writer.
pub fn render(db: &dyn DbLink, spec: &ReportSpec) -> DbResult<Vec<String>> {
    let q = db.query(&spec.source_sql())?;
    let ncols = q.columns.len();

    // Numeric columns (every non-NULL value Int/Real) get totals.
    let mut numeric = vec![!q.rows.is_empty(); ncols];
    for row in &q.rows {
        for (i, v) in row.iter().enumerate() {
            if !matches!(v, PValue::Int(_) | PValue::Real(_) | PValue::Null) {
                numeric[i] = false;
            }
        }
    }
    let group_idx = spec
        .group_by
        .as_ref()
        .and_then(|g| q.columns.iter().position(|c| c == g));
    if let Some(gi) = group_idx {
        numeric[gi] = false;
    }

    // Grand totals first: numeric columns must be wide enough for their
    // own SUM line, not just their data (a column of 3-digit ids has a
    // 6-digit total — found by test).
    let mut grand_precalc = vec![0f64; ncols];
    for row in &q.rows {
        for (i, v) in row.iter().enumerate() {
            if numeric[i] {
                match v {
                    PValue::Int(n) => grand_precalc[i] += *n as f64,
                    PValue::Real(f) => grand_precalc[i] += f,
                    _ => {}
                }
            }
        }
    }

    // Column widths from header + data + totals (numbers right-aligned).
    let widths: Vec<usize> = q
        .columns
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let mut w = name.chars().count();
            for row in &q.rows {
                w = w.max(row[i].render().chars().count());
            }
            if numeric[i] {
                w = w.max(fmt_num(grand_precalc[i]).chars().count());
            }
            w.clamp(3, 26)
        })
        .collect();

    let cell = |v: &PValue, i: usize| -> String {
        let text = v.render();
        if numeric[i] {
            rpad(&text, widths[i])
        } else {
            pad(&text, widths[i])
        }
    };
    let header_line = q
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| pad(c, widths[i]))
        .collect::<Vec<_>>()
        .join(" ");
    let rule = "─".repeat(header_line.chars().count().min(PAGE_WIDTH));

    let totals_line = |label: &str, sums: &[f64], count: usize| -> Vec<String> {
        let cells = (0..ncols)
            .map(|i| {
                if numeric[i] {
                    rpad(&fmt_num(sums[i]), widths[i])
                } else {
                    " ".repeat(widths[i])
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        vec![
            rule.clone(),
            format!("{label} ({count} rows)"),
            cells,
        ]
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

    let mut grand = vec![0f64; ncols];
    let mut grand_n = 0usize;
    let mut group_sums = vec![0f64; ncols];
    let mut group_n = 0usize;
    let mut current_group: Option<String> = None;

    for row in &q.rows {
        if let Some(gi) = group_idx {
            let g = row[gi].render();
            if current_group.as_deref() != Some(g.as_str()) {
                if current_group.is_some() {
                    for l in totals_line("  subtotal", &group_sums, group_n) {
                        emit(&mut out, l);
                    }
                    emit(&mut out, String::new());
                }
                emit(&mut out, format!("▌ {} = {g}", q.columns[gi]));
                current_group = Some(g);
                group_sums = vec![0f64; ncols];
                group_n = 0;
            }
        }
        let line = row
            .iter()
            .enumerate()
            .map(|(i, v)| cell(v, i))
            .collect::<Vec<_>>()
            .join(" ");
        emit(&mut out, line);
        for (i, v) in row.iter().enumerate() {
            if numeric[i] {
                match v {
                    PValue::Int(n) => {
                        group_sums[i] += *n as f64;
                        grand[i] += *n as f64;
                    }
                    PValue::Real(f) => {
                        group_sums[i] += f;
                        grand[i] += f;
                    }
                    _ => {}
                }
            }
        }
        group_n += 1;
        grand_n += 1;
    }
    if group_idx.is_some() && current_group.is_some() {
        for l in totals_line("  subtotal", &group_sums, group_n) {
            emit(&mut out, l);
        }
    }
    emit(&mut out, String::new());
    for l in totals_line("TOTAL", &grand, grand_n) {
        emit(&mut out, l);
    }
    if q.truncated {
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
            let mut line = String::new();
            for row in chunk {
                let text = if line_idx < q.columns.len() {
                    row[line_idx].render()
                } else {
                    String::new()
                };
                line.push_str(&pad(&text, LABEL_W - 2));
                line.push_str("  ");
            }
            out.push(line.trim_end().to_owned());
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
        let path = format!("{}.txt", self.file_stem);
        std::fs::write(&path, self.lines.join("\n")).map_err(|e| e.to_string())?;
        Ok(path)
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
        assert_eq!(first.matches("east").count() + first.matches("west").count(), 3);
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
}
