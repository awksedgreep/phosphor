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
    // Header name (lowercased) -> metadata, including generated columns.
    let col_map: std::collections::HashMap<String, _> = cols
        .iter()
        .map(|c| (c.name.to_ascii_lowercase(), c))
        .collect();

    // (header_index, target_column, declared_type) for each CSV header.
    let mut hdr_to_col: Vec<(usize, String, String)> = Vec::new();
    for (hi, h) in headers.iter().enumerate() {
        let key = h.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        let Some(col) = col_map.get(&key) else {
            return Err(format!("import: unknown column {h:?} for table {table:?}"));
        };
        if col.generated {
            return Err(format!(
                "import: column {h:?} is generated; omit it from the CSV"
            ));
        }
        hdr_to_col.push((hi, col.name.clone(), col.decl_type.clone()));
    }
    if hdr_to_col.is_empty() {
        return Err("import: no matching columns".into());
    }

    // Own this transaction: a failed BEGIN must never let the import
    // join, commit, or roll back an existing caller's work.
    db.begin_transaction()
        .map_err(|e| format!("import: begin: {e}"))?;
    let result = (|| {
        let mut inserted: i64 = 0;
        for result in rdr.records() {
            let record = result.map_err(|e| format!("import: csv record: {e}"))?;
            let mut changes: Vec<(String, PValue)> = Vec::with_capacity(hdr_to_col.len());
            for (hi, col_name, decl) in &hdr_to_col {
                // Empty field → NULL; NOT NULL constraints still apply.
                let raw = record.get(*hi).unwrap_or("").trim();
                changes.push((col_name.clone(), PValue::parse(raw, decl)));
            }
            db.insert_row(table, &changes)
                .map_err(|e| format!("import: row {}: {e}", inserted + 1))?;
            inserted += 1;
        }
        db.commit_transaction()
            .map_err(|e| format!("import: commit: {e}"))?;
        Ok(format!("imported {inserted} row(s) into {table:?}"))
    })();
    match result {
        Ok(message) => Ok(message),
        Err(error) => match db.rollback_transaction() {
            Ok(_) => Err(error),
            Err(rollback) => Err(format!("{error}; rollback: {rollback}")),
        },
    }
}

/// `export <source> <path>` — `source` is a table name or a SELECT/WITH.
///
/// Writes a headered CSV. Uses `csv` crate for quoting.
#[cfg(test)]
pub fn export_csv(db: &dyn DbLink, source: &str, path: &str) -> DbResult<String> {
    export_controlled(db, source, path, &crate::operation::Control::default())
}

pub fn export_controlled(
    db: &dyn DbLink,
    source: &str,
    path: &str,
    control: &crate::operation::Control,
) -> DbResult<String> {
    control.check()?;
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

    let (output, file) = crate::output::AtomicOutput::create(Path::new(path))
        .map_err(|e| format!("export: cannot create {path:?}: {e}"))?;
    let mut wtr = csv::Writer::from_writer(BufWriter::new(file));
    let progress = control.clone();
    let count = db
        .stream_query(
            &sql,
            Box::new(move |event| {
                progress.check()?;
                match event {
                    crate::db::QueryEvent::Columns(columns) => {
                        wtr.write_record(columns).map_err(|e| e.to_string())
                    }
                    crate::db::QueryEvent::Row(row) => {
                        progress.row()?;
                        wtr.write_record(row.iter().map(to_csv_string))
                            .map_err(|e| e.to_string())
                    }
                    crate::db::QueryEvent::End => {
                        wtr.flush().map_err(|e| e.to_string())?;
                        wtr.get_ref()
                            .get_ref()
                            .sync_all()
                            .map_err(|e| e.to_string())
                    }
                }
            }),
        )
        .map_err(|e| format!("export incomplete; destination unchanged: {e}"))?;
    control.publish()?;
    output
        .publish()
        .map_err(|e| format!("export: cannot publish {path:?}: {e}"))?;
    Ok(format!("exported {count} row(s) from {src:?} to {path:?}"))
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
    fn failed_import_rolls_back_rows_and_releases_its_transaction() {
        for (case, csv, ddl, expected_error) in [
            ("short", "name,city\nAda,London\nGrace\n", "CREATE TABLE t(name TEXT, city TEXT)", "csv record"),
            ("constraint", "name,city\nAda,London\nAda,Arlington\n", "CREATE TABLE t(name TEXT UNIQUE, city TEXT)", "row 2"),
            ("commit", "name,city\nAda,London\n", "CREATE TABLE cities(name TEXT PRIMARY KEY); CREATE TABLE t(name TEXT, city TEXT REFERENCES cities(name) DEFERRABLE INITIALLY DEFERRED)", "commit"),
        ] {
            let (db, _) = EmbeddedDb::open(":memory:").unwrap();
            db.execute(ddl).unwrap();
            let path = tmp_path(case);
            fs::write(&path, csv).unwrap();
            let error = import_csv(&db, "t", &path).unwrap_err();
            fs::remove_file(path).unwrap();
            assert!(error.contains(expected_error), "{case}: {error}");
            assert_eq!(db.count("t").unwrap(), 0, "{case}: partial import survived");
            db.execute("BEGIN").expect("the import must release its transaction");
            db.execute("ROLLBACK").unwrap();
        }
    }

    #[test]
    fn import_refuses_to_join_or_finish_an_existing_transaction() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE t(name TEXT); BEGIN; INSERT INTO t VALUES ('existing')")
            .unwrap();
        let path = tmp_path("existing-transaction");
        fs::write(&path, "name\nimported\n").unwrap();
        let error = import_csv(&db, "t", &path).unwrap_err();
        fs::remove_file(path).unwrap();
        assert!(error.contains("begin"), "{error}");
        assert_eq!(db.count("t").unwrap(), 1);
        // Our caller's work is still pending and under its control.
        db.execute("ROLLBACK").unwrap();
        assert_eq!(db.count("t").unwrap(), 0);
    }

    #[test]
    fn import_omits_generated_columns_and_rejects_explicit_values() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE t(name TEXT, size INTEGER AS (length(name)))")
            .unwrap();
        let path = tmp_path("generated");
        fs::write(&path, "name\nAlice\n").unwrap();
        import_csv(&db, "t", &path).unwrap();
        assert_eq!(
            db.query("SELECT size FROM t").unwrap().rows[0][0],
            PValue::Int(5)
        );
        fs::write(&path, "name,size\nGrace,5\n").unwrap();
        assert!(import_csv(&db, "t", &path).unwrap_err().contains("omit it"));
        fs::remove_file(path).unwrap();
        assert_eq!(db.count("t").unwrap(), 1);
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
