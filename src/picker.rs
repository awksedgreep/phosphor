//! Bounded, searchable parent-record windows for foreign-key fields.
use crate::db::{DbLink, DbResult, PValue};

pub const PAGE_SIZE: usize = 100;

pub struct PickerState {
    pub parent: String,
    /// Key first, then descriptive columns; no duplicate key projection.
    pub columns: Vec<String>,
    pub rows: Vec<Vec<PValue>>,
    pub cursor: usize,
    pub start: usize,
    pub total: usize,
    pub field: usize,
    pub search: String,
    pub editing: Option<String>,
    pub loading: Option<crate::worker::Token>,
    pub error: Option<String>,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PickerPage {
    pub rows: Vec<Vec<PValue>>,
    pub start: usize,
    pub total: usize,
    pub cursor: usize,
}

pub fn fetch(
    db: &dyn DbLink,
    parent: &str,
    columns: &[String],
    search: &str,
    want: usize,
) -> DbResult<PickerPage> {
    let ident = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let key = columns.first().ok_or("picker has no key column")?;
    let mut predicate = format!("{} IS NOT NULL", ident(key));
    if !search.is_empty() {
        // instr treats %, _, quotes and backslashes as literal search text.
        let needle = crate::store::q(search);
        let matches = columns
            .iter()
            .map(|c| {
                format!(
                    "instr(lower(CAST({} AS TEXT)), lower({needle})) > 0",
                    ident(c)
                )
            })
            .collect::<Vec<_>>();
        predicate.push_str(&format!(" AND ({})", matches.join(" OR ")));
    }
    let count = db.query(&format!(
        "SELECT count(*) FROM {} WHERE {predicate}",
        ident(parent)
    ))?;
    let total = match count.rows.first().and_then(|r| r.first()) {
        Some(PValue::Int(n)) => (*n).max(0) as usize,
        _ => return Err("picker count was not returned".into()),
    };
    let cursor = want.min(total.saturating_sub(1));
    let start = cursor / PAGE_SIZE * PAGE_SIZE;
    let sql = format!(
        "SELECT {} FROM {} WHERE {predicate} ORDER BY {} LIMIT {PAGE_SIZE} OFFSET {start}",
        columns
            .iter()
            .map(|c| ident(c))
            .collect::<Vec<_>>()
            .join(", "),
        ident(parent),
        ident(key)
    );
    let rows = db.query(&sql)?.rows;
    Ok(PickerPage {
        rows,
        start,
        total,
        cursor,
    })
}
