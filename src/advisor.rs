//! The index / vacuum advisor (DESIGN.md: "this browse full-scans;
//! create index?"). Reads the schema through `DbLink` — no backend
//! knowledge — and produces plain-language suggestions:
//!
//! * foreign-key columns with no index (the classic join / child-pane
//!   full scan), with the exact `CREATE INDEX` to fix it;
//! * a VACUUM nudge when the dbhealth report flags file bloat.
//!
//! It never changes anything: suggestions are text the user can run.

use crate::db::{DbLink, DbResult, PValue};

/// Columns carried by every index on `table` (one row per index column).
/// Literal SQL: `DbLink` is the ad-hoc path with no bind parameters.
fn indexed_columns(db: &dyn DbLink, table: &str) -> Vec<String> {
    let Ok(q) = db.query(&format!(
        "SELECT name FROM pragma_index_list({})",
        crate::store::q(table)
    )) else {
        return Vec::new();
    };
    let mut cols = Vec::new();
    for row in &q.rows {
        let Some(PValue::Text(index)) = row.first() else {
            continue;
        };
        let Ok(info) = db.query(&format!(
            "SELECT name FROM pragma_index_info({})",
            crate::store::q(index)
        )) else {
            continue;
        };
        for r in &info.rows {
            if let Some(PValue::Text(c)) = r.first() {
                cols.push(c.to_ascii_lowercase());
            }
        }
    }
    cols
}

/// All suggestions, newest concern first. Empty table list is fine.
pub fn advise(db: &dyn DbLink) -> DbResult<Vec<String>> {
    let mut out = vec!["INDEX & VACUUM ADVISOR".to_owned(), "".to_owned()];
    let mut suggestions = 0usize;

    let tables = db.tables()?;
    for t in tables.iter().filter(|t| !t.is_view) {
        // Only user tables: skip phosphor's catalogs, engine shadow
        // tables, and sqlite's own.
        if t.name.starts_with('_') || t.name.starts_with("sqlite_") {
            continue;
        }
        let fks = db.outgoing_fks(&t.name);
        if fks.is_empty() {
            continue;
        }
        let indexed = indexed_columns(db, &t.name);
        for (from, to_table, _) in fks {
            if indexed.iter().any(|c| c == &from.to_ascii_lowercase()) {
                continue;
            }
            suggestions += 1;
            out.push(format!(
                "＊ {}.{} has no index (FK -> {})",
                t.name, from, to_table
            ));
            out.push(format!(
                "   CREATE INDEX \"idx_{}_{}\" ON \"{}\"(\"{}\");",
                t.name, from, t.name, from
            ));
            out.push(String::new());
        }
    }

    // Vacuum nudge from dbhealth, if it is present and complains about
    // file size / bloat. The report view is discovered generically.
    if let Some(line) = vacuum_advice(db) {
        out.push(line);
        out.push(String::new());
    } else if suggestions == 0 {
        out.push("No missing foreign-key indexes found.".to_owned());
        out.push(String::new());
        out.push("Browse freely — the advisor will shout if a table".to_owned());
        out.push("needs an index.".to_owned());
    }
    Ok(out)
}

/// Look for the dbhealth report and return a VACUUM suggestion when a
/// bloat-ish check is present.
fn vacuum_advice(db: &dyn DbLink) -> Option<String> {
    let view: String = db
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'view' \
             AND name LIKE '%\\_report' ESCAPE '\\' ORDER BY name LIMIT 1",
        )
        .ok()
        .and_then(|q| q.rows.into_iter().next())
        .and_then(|r| r.into_iter().next())
        .and_then(|v| match v {
            PValue::Text(t) => Some(t),
            _ => None,
        })?;
    let q = db
        .query(&format!(
            "SELECT \"check\", status, advice FROM \"{}\"",
            view.replace('"', "\"\"")
        ))
        .ok()?;
    for row in &q.rows {
        let check = row
            .first()
            .map(crate::db::PValue::render)
            .unwrap_or_default();
        let advice = row
            .get(2)
            .map(crate::db::PValue::render)
            .unwrap_or_default();
        let lower = check.to_ascii_lowercase();
        if lower.contains("bloat")
            || lower.contains("file")
            || lower.contains("freelist")
            || advice.to_ascii_lowercase().contains("vacuum")
        {
            return Some(format!("＊ {check}: {advice}\n   Consider: VACUUM;"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::EmbeddedDb;

    #[test]
    fn flags_unindexed_foreign_keys() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE orders(id INTEGER PRIMARY KEY,
                 customer_id INTEGER REFERENCES customers(id), note TEXT);
             CREATE INDEX ix_note ON orders(note);",
        )
        .unwrap();
        let lines = advise(&db).unwrap();
        let text = lines.join("\n");
        assert!(text.contains("orders.customer_id has no index"), "{text}");
        assert!(
            text.contains(r#"CREATE INDEX "idx_orders_customer_id" ON "orders"("customer_id");"#),
            "{text}"
        );
        // A column that IS indexed is not flagged.
        assert!(!text.contains("orders.note has no index"), "{text}");
    }

    #[test]
    fn quiet_when_everything_is_indexed() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE customers(id INTEGER PRIMARY KEY);
             CREATE TABLE orders(id INTEGER PRIMARY KEY,
                 customer_id INTEGER REFERENCES customers(id));
             CREATE INDEX ix ON orders(customer_id);",
        )
        .unwrap();
        let lines = advise(&db).unwrap();
        assert!(lines.iter().any(|l| l.contains("No missing")), "{lines:?}");
    }
}
