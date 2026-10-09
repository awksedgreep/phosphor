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
        // The SQL blob literal x'…', so a re-import of this file parses
        // the column back to BLOB instead of TEXT (#76).
        PValue::Blob(b) => {
            use std::fmt::Write as _;
            let mut s = String::with_capacity(b.len() * 2 + 3);
            s.push_str("x'");
            for byte in b {
                let _ = write!(s, "{byte:02x}");
            }
            s.push('\'');
            s
        }
    }
}

/// Bind parameters per multi-row INSERT: well under SQLite's variable
/// limit even for wide tables, and every flush is ONE round-trip
/// regardless of backend (#53: a 10k-row import used to be 10k
/// round-trips).
const BATCH_PARAMS: usize = 2000;

fn q_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Multi-row `INSERT … VALUES (…),(…)`. On a batch failure the rows are
/// retried singly (still inside the doomed transaction) so the error
/// names the first offending row, as the per-row path used to.
fn insert_rows_batch(
    db: &dyn DbLink,
    table: &str,
    cols: &[(usize, String, String)],
    rows: &[Vec<PValue>],
    base: i64,
) -> DbResult<()> {
    let names: Vec<&str> = cols.iter().map(|(_, n, _)| n.as_str()).collect();
    let changes = |row: &Vec<PValue>| {
        names
            .iter()
            .cloned()
            .zip(row.iter().cloned())
            .map(|(c, v)| (c.to_owned(), v))
            .collect::<Vec<_>>()
    };
    if rows.len() == 1 {
        return db
            .insert_row(table, &changes(&rows[0]))
            .map(|_| ())
            .map_err(|e| format!("import: row {}: {e}", base + 1));
    }
    let mut marks: Vec<String> = Vec::with_capacity(rows.len());
    let mut mark: i64 = 0;
    for _row in rows {
        let mut list = String::new();
        for (i, _) in cols.iter().enumerate() {
            if i > 0 {
                list.push(',');
            }
            mark += 1;
            use std::fmt::Write as _;
            let _ = write!(list, "?{mark}");
        }
        marks.push(format!("({list})"));
    }
    let sql = format!(
        "INSERT INTO {} ({}) VALUES {}",
        q_ident(table),
        names
            .iter()
            .map(|n| q_ident(n))
            .collect::<Vec<_>>()
            .join(", "),
        marks.join(", ")
    );
    let params: Vec<PValue> = rows.iter().flat_map(|r| r.iter().cloned()).collect();
    match db.execute_params(&sql, &params) {
        Ok(_) => Ok(()),
        Err(batch_err) => {
            for (i, row) in rows.iter().enumerate() {
                if let Err(row_err) = db.insert_row(table, &changes(row)) {
                    return Err(format!("import: row {}: {row_err}", base + i as i64 + 1));
                }
            }
            Err(format!("import: batch: {batch_err}"))
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
#[cfg(test)]
pub fn import_csv(db: &dyn DbLink, table: &str, path: &str) -> DbResult<String> {
    import_controlled(db, table, path, &crate::operation::Control::default())
}

/// `import_csv` with cooperative cancellation + row progress, for the
/// async prompt path (#53): the UI showed a frozen screen for minutes
/// on large remote imports.
pub fn import_controlled(
    db: &dyn DbLink,
    table: &str,
    path: &str,
    control: &crate::operation::Control,
) -> DbResult<String> {
    control.check()?;
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
        let mut batch: Vec<Vec<PValue>> = Vec::new();
        for result in rdr.records() {
            control.check()?;
            let record = result.map_err(|e| format!("import: csv record: {e}"))?;
            let row_no = inserted + batch.len() as i64 + 1;
            let mut row: Vec<PValue> = Vec::with_capacity(hdr_to_col.len());
            for (hi, col_name, decl) in &hdr_to_col {
                // Empty field → NULL; NOT NULL constraints still apply.
                // No trim: CSV quoting already preserves spaces (#57).
                let raw = record.get(*hi).unwrap_or("");
                row.push(match PValue::parse_strict(raw, decl) {
                    Ok(v) => v,
                    Err(e) => {
                        return Err(format!("import: row {row_no}: column {col_name:?}: {e}"))
                    }
                });
            }
            control.row()?;
            batch.push(row);
            if batch.len() * hdr_to_col.len().max(1) >= BATCH_PARAMS {
                insert_rows_batch(db, table, &hdr_to_col, &batch, inserted)?;
                inserted += batch.len() as i64;
                batch.clear();
            }
        }
        if !batch.is_empty() {
            insert_rows_batch(db, table, &hdr_to_col, &batch, inserted)?;
            inserted += batch.len() as i64;
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

    /// #76: a BLOB column must survive export→import as a BLOB, not TEXT.
    /// Export writes the SQL blob literal x'…'; re-importing parses it
    /// back to the original bytes.
    #[test]
    fn blob_column_round_trips_through_csv() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, data BLOB)")
            .unwrap();
        db.execute("INSERT INTO t(name, data) VALUES ('Ada', x'0001abcdef00ff')")
            .unwrap();
        let out = tmp_path("blob-out");
        export_csv(&db, "t", &out).unwrap();
        let content = fs::read_to_string(&out).unwrap();
        assert!(
            content.contains("x'0001abcdef00ff'"),
            "export must write the blob literal, got: {content:?}"
        );

        db.execute("CREATE TABLE u(id INTEGER PRIMARY KEY, name TEXT, data BLOB)")
            .unwrap();
        let msg = import_csv(&db, "u", &out).unwrap();
        assert!(msg.contains("1 row(s)"), "{msg}");
        let q = db.query("SELECT name, data FROM u").unwrap();
        assert_eq!(
            q.rows[0],
            vec![
                PValue::Text("Ada".into()),
                PValue::Blob(vec![0x00, 0x01, 0xab, 0xcd, 0xef, 0x00, 0xff])
            ],
            "a blob column must round-trip through CSV as a BLOB"
        );
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

    /// #53: wide-enough files flush as multi-row INSERTs (1500 rows x
    /// 3 cols = 4500 bind params spans three batches), progress counts
    /// every row, and a constraint hit in a later batch still names
    /// its row.
    #[test]
    fn import_batches_multirow_and_names_the_offending_row() {
        use crate::operation::Control;
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE bulk(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, c INTEGER)")
            .unwrap();
        let path = tmp_path("bulk");
        {
            let mut w = csv::Writer::from_path(&path).unwrap();
            w.write_record(["a", "b", "c"]).unwrap();
            for i in 0..1500 {
                w.write_record([i.to_string(), (i * 2).to_string(), (i * 3).to_string()])
                    .unwrap();
            }
            w.flush().unwrap();
        }
        let control = Control::default();
        let msg = import_controlled(&db, "bulk", &path, &control).unwrap();
        assert!(msg.contains("1500 row(s)"), "{msg}");
        assert_eq!(control.rows(), 1500, "progress counts every CSV row");
        assert_eq!(db.count("bulk").unwrap(), 1500);
        let q = db
            .query("SELECT a, b, c FROM bulk WHERE id = 1500")
            .unwrap();
        assert_eq!(
            q.rows[0],
            vec![PValue::Int(1499), PValue::Int(2998), PValue::Int(4497)]
        );
        fs::remove_file(&path).unwrap();

        // Duplicate b at row 1400 (third batch) — the error names it
        // and the whole import rolls back.
        db.execute("CREATE TABLE uniq(a INTEGER, b INTEGER UNIQUE)")
            .unwrap();
        let path2 = tmp_path("bulk-err");
        {
            let mut w = csv::Writer::from_path(&path2).unwrap();
            w.write_record(["a", "b"]).unwrap();
            for i in 0..1500 {
                let b = if i == 1399 { 4 } else { i }; // dup of row 5's b
                w.write_record([i.to_string(), b.to_string()]).unwrap();
            }
            w.flush().unwrap();
        }
        let error = import_controlled(&db, "uniq", &path2, &Control::default()).unwrap_err();
        assert!(error.contains("row 1400"), "{error}");
        assert_eq!(db.count("uniq").unwrap(), 0, "batched import rolled back");
        fs::remove_file(&path2).unwrap();
    }

    /// #57: a value that does not fit the declared type fails the
    /// import (naming row and column) instead of quietly landing as
    /// TEXT, and field whitespace is preserved verbatim.
    #[test]
    fn import_enforces_declared_types_and_keeps_whitespace() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE t(a INTEGER, r REAL, name TEXT)")
            .unwrap();
        let path = tmp_path("types");
        fs::write(&path, "a,r,name\n1,2.5, Ada \n").unwrap();
        import_csv(&db, "t", &path).unwrap();
        let q = db.query("SELECT a, r, name FROM t").unwrap();
        assert_eq!(
            q.rows[0],
            vec![
                PValue::Int(1),
                PValue::Real(2.5),
                PValue::Text(" Ada ".into())
            ],
            "declared types and field whitespace survive the import"
        );
        for (csv, needle) in [
            ("a,r,name\nabc,,x\n", "not an integer"),
            ("a,r,name\n12.5,,x\n", "not an integer"),
            ("a,r,name\n5,oops,x\n", "not a real"),
        ] {
            db.execute("DELETE FROM t").unwrap();
            fs::write(&path, csv).unwrap();
            let error = import_csv(&db, "t", &path).unwrap_err();
            assert!(error.contains(needle), "{csv:?}: {error}");
            assert!(error.contains("row 1"), "{csv:?}: {error}");
            assert_eq!(
                db.count("t").unwrap(),
                0,
                "{csv:?}: partial import survived"
            );
        }
        fs::remove_file(&path).unwrap();
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
