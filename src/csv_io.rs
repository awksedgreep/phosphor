//! CSV import/export via the dot prompt (issue #14).
//!
//! `import <table> <path>` reads a headered CSV and inserts rows into an
//! existing table. `export <source> <path>` writes a table or `SELECT` to
//! CSV. Both go through `DbLink` so they work embedded or over sqld.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use crate::db::{DbLink, DbResult, PValue};

fn to_csv_string(v: &PValue) -> String {
    match v {
        PValue::Null => String::new(),
        PValue::Int(i) => i.to_string(),
        PValue::Real(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{f:.1}")
            } else {
                format!("{f}")
            }
        }
        PValue::Text(t) => t.clone(),
        PValue::Blob(b) => {
            use std::fmt::Write as _;
            let mut s = String::with_capacity(b.len() * 2);
            for byte in b {
                let _ = write!(s, "{byte:02x}");
            }
            s
        }
    }
}

/// `import <table> <path>` — headered CSV into an existing table.
///
/// * Header names must match column names (case-insensitive).
/// * Extra CSV columns → error.
/// * Missing columns → omitted (DB default/NULL).
/// * Empty field → `PValue::Null` (via `PValue::parse`).
/// * Wrapped in `BEGIN`/`COMMIT` for speed; on error `ROLLBACK`.
pub fn import_csv(db: &dyn DbLink, table: &str, path: &str) -> DbResult<String> {
    let p = Path::new(path);
    let file = File::open(p).map_err(|e| format!("import: cannot open {path:?}: {e}"))?;
    let mut rdr = csv::Reader::from_reader(BufReader::new(file));

    let headers = rdr
        .headers()
        .map_err(|e| format!("import: bad header: {e}"))?
        .clone();

    let cols = db.columns(table)?;
    // Header name (lowercased) -> (column name, declared type).
    let col_map: std::collections::HashMap<String, (&str, &str)> = cols
        .iter()
        .map(|c| {
            (
                c.name.to_ascii_lowercase(),
                (c.name.as_str(), c.decl_type.as_str()),
            )
        })
        .collect();

    // (header_index, target_column, declared_type) for each CSV header.
    let mut hdr_to_col: Vec<(usize, String, String)> = Vec::new();
    for (hi, h) in headers.iter().enumerate() {
        let key = h.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        let Some((col_name, decl)) = col_map.get(&key) else {
            return Err(format!("import: unknown column {h:?} for table {table:?}"));
        };
        hdr_to_col.push((hi, (*col_name).to_owned(), (*decl).to_owned()));
    }
    if hdr_to_col.is_empty() {
        return Err("import: no matching columns".into());
    }

    // Begin transaction (best-effort; RemoteDb will batch as separate pipelines but still correct)
    let _ = db.execute("BEGIN");

    let mut inserted: i64 = 0;
    for result in rdr.records() {
        let record = result.map_err(|e| format!("import: csv record: {e}"))?;
        let mut changes: Vec<(String, PValue)> = Vec::with_capacity(hdr_to_col.len());
        for (hi, col_name, decl) in &hdr_to_col {
            // Empty field → NULL, the dBASE contract; NOT NULL constraints
            // then fire, which is exactly what the user wants to know.
            let raw = record.get(*hi).unwrap_or("").trim();
            changes.push((col_name.clone(), PValue::parse(raw, decl)));
        }
        db.insert_row(table, &changes).map_err(|e| {
            let _ = db.execute("ROLLBACK");
            format!("import: row {}: {e}", inserted + 1)
        })?;
        inserted += 1;
    }

    let _ = db.execute("COMMIT");
    Ok(format!("imported {inserted} row(s) into {table:?}"))
}

/// `export <source> <path>` — `source` is a table name or a SELECT/WITH.
///
/// Writes a headered CSV. Uses `csv` crate for quoting.
pub fn export_csv(db: &dyn DbLink, source: &str, path: &str) -> DbResult<String> {
    let src = source.trim();
    if src.is_empty() {
        return Err("export: missing source".into());
    }
    let sql = if src.to_ascii_lowercase().starts_with("select")
        || src.to_ascii_lowercase().starts_with("with")
    {
        src.to_owned()
    } else {
        format!("SELECT * FROM \"{}\"", src.replace('"', "\"\""))
    };

    let q = db.query(&sql)?;
    let p = Path::new(path);
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("export: cannot create dir {parent:?}: {e}"))?;
        }
    }
    let file = File::create(p).map_err(|e| format!("export: cannot create {path:?}: {e}"))?;
    let mut wtr = csv::Writer::from_writer(BufWriter::new(file));
    wtr.write_record(&q.columns)
        .map_err(|e| format!("export: write header: {e}"))?;
    for row in &q.rows {
        let rec: Vec<String> = row.iter().map(to_csv_string).collect();
        wtr.write_record(&rec)
            .map_err(|e| format!("export: write row: {e}"))?;
    }
    wtr.flush().map_err(|e| format!("export: flush: {e}"))?;
    Ok(format!(
        "exported {} row(s) from {src:?} to {path:?}",
        q.rows.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::EmbeddedDb;
    use std::fs;

    fn tmp_path(name: &str) -> String {
        let mut p = std::env::temp_dir();
        p.push(format!("phosphor-csv-{}-{}.csv", name, std::process::id()));
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn round_trip() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, score REAL)")
            .unwrap();
        let path = tmp_path("rt");
        // write a CSV with header + 2 rows
        {
            let mut w = csv::Writer::from_path(&path).unwrap();
            w.write_record(["name", "score"]).unwrap();
            w.write_record(["Ada", "99.5"]).unwrap();
            w.write_record(["Grace", "100"]).unwrap();
            w.flush().unwrap();
        }
        let msg = import_csv(&db, "t", &path).unwrap();
        assert!(msg.contains("2 row(s)"));
        let q = db.query("SELECT name, score FROM t ORDER BY name").unwrap();
        assert_eq!(q.rows.len(), 2);
        // export
        let out = tmp_path("rt-out");
        let msg2 = export_csv(&db, "t", &out).unwrap();
        assert!(msg2.contains("2 row(s)"));
        let content = fs::read_to_string(&out).unwrap();
        assert!(content.contains("Ada"));
        assert!(content.contains("Grace"));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&out);
    }

    #[test]
    fn import_unknown_column_errors() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT)")
            .unwrap();
        let path = tmp_path("bad");
        {
            let mut w = csv::Writer::from_path(&path).unwrap();
            w.write_record(["name", "unknown"]).unwrap();
            w.write_record(["Ada", "x"]).unwrap();
            w.flush().unwrap();
        }
        let err = import_csv(&db, "t", &path).unwrap_err();
        assert!(err.contains("unknown column"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn export_select() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT)")
            .unwrap();
        db.execute("INSERT INTO t(name) VALUES ('Ada'), ('Grace')")
            .unwrap();
        let out = tmp_path("sel");
        export_csv(&db, "SELECT name FROM t WHERE name='Ada'", &out).unwrap();
        let content = fs::read_to_string(&out).unwrap();
        assert!(content.contains("Ada"));
        assert!(!content.contains("Grace"));
        let _ = fs::remove_file(&out);
    }
}
