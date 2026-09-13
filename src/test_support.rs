//! Local SQLite-backed Hrana fixture. Exercises HTTP and conditional batches;
//! it is not a substitute for the optional real-sqld integration test.
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::fallible_iterator::FallibleIterator;
use rusqlite::Connection;
use serde_json::{json, Value};

pub const SCHEMA_SQL: &str = "
CREATE TABLE parents(id INTEGER PRIMARY KEY);
CREATE TABLE audit(message TEXT);
CREATE TABLE things(id INTEGER PRIMARY KEY AUTOINCREMENT,
    code TEXT COLLATE NOCASE UNIQUE, qty INTEGER CHECK(qty > 0),
    parent_id INTEGER REFERENCES parents(id) ON UPDATE CASCADE ON DELETE RESTRICT,
    stale TEXT, note TEXT DEFAULT 'new');
CREATE INDEX things_qty ON things(qty);
CREATE TRIGGER things_insert AFTER INSERT ON things BEGIN
    INSERT INTO audit VALUES (new.code);
END;
CREATE VIEW things_view AS SELECT code, qty FROM things;
CREATE TABLE children(id INTEGER PRIMARY KEY, thing_id INTEGER REFERENCES things(id));
INSERT INTO parents VALUES (1);
INSERT INTO things(code,qty,parent_id) VALUES ('Ada',2,1);
INSERT INTO children VALUES (1,1);
CREATE TABLE strict_keys(key TEXT PRIMARY KEY, value INTEGER) WITHOUT ROWID, STRICT;
INSERT INTO strict_keys VALUES ('key', 7);
";

pub fn assert_native_schema_safety(db: &dyn crate::db::DbLink) {
    use crate::db::PValue;
    let snapshot = || {
        db.query("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY name")
            .unwrap()
            .rows
    };
    db.apply_schema_changes(&["ALTER TABLE strict_keys RENAME COLUMN value TO amount".into()])
        .unwrap();
    assert_eq!(
        db.query("SELECT wr, strict FROM pragma_table_list WHERE name='strict_keys'")
            .unwrap()
            .rows[0],
        vec![PValue::Int(1), PValue::Int(1)]
    );
    assert_eq!(
        db.query("SELECT key, amount FROM strict_keys")
            .unwrap()
            .rows[0],
        vec![PValue::Text("key".into()), PValue::Int(7)]
    );
    assert!(
        db.execute("UPDATE strict_keys SET amount='invalid'")
            .is_err(),
        "STRICT survived"
    );
    db.apply_schema_changes(&["ALTER TABLE strict_keys RENAME COLUMN amount TO value".into()])
        .unwrap();
    // SQLite may quote the renamed identifier; capture its canonical form.
    let before = snapshot();
    // The first change succeeds, the second cannot drop an indexed column.
    assert!(db
        .apply_schema_changes(&[
            "ALTER TABLE things RENAME COLUMN code TO label".into(),
            "ALTER TABLE things DROP COLUMN qty".into(),
        ])
        .is_err());
    assert_eq!(snapshot(), before, "all ALTERs rolled back");
    db.apply_schema_changes(&[
        "ALTER TABLE things RENAME COLUMN qty TO amount".into(),
        "ALTER TABLE things RENAME TO goods".into(),
        "ALTER TABLE goods ADD COLUMN extra TEXT DEFAULT 'ready; yes'".into(),
        "ALTER TABLE goods DROP COLUMN stale".into(),
    ])
    .unwrap();
    assert_eq!(
        db.query("SELECT id,code,amount,parent_id,extra FROM goods")
            .unwrap()
            .rows[0],
        vec![
            PValue::Int(1),
            PValue::Text("Ada".into()),
            PValue::Int(2),
            PValue::Int(1),
            PValue::Text("ready; yes".into())
        ]
    );
    assert_eq!(
        db.query("SELECT amount FROM things_view").unwrap().rows[0][0],
        PValue::Int(2)
    );
    assert_eq!(
        db.query("SELECT count(*) FROM pragma_index_info('things_qty') WHERE name='amount'")
            .unwrap()
            .rows[0][0],
        PValue::Int(1)
    );
    assert!(
        db.execute("INSERT INTO goods(code,amount) VALUES ('ada',2)")
            .is_err(),
        "UNIQUE and collation survived"
    );
    assert!(
        db.execute("INSERT INTO goods(code,amount) VALUES ('bad',0)")
            .is_err(),
        "CHECK survived"
    );
    assert!(
        db.execute("DELETE FROM goods WHERE id=1").is_err(),
        "inbound FK survived rename"
    );
    db.execute("INSERT INTO goods(code,amount,parent_id) VALUES ('Grace',3,1)")
        .unwrap();
    assert_eq!(db.count("audit").unwrap(), 2, "trigger survived");
    db.execute("UPDATE parents SET id=2 WHERE id=1").unwrap();
    assert_eq!(
        db.query("SELECT parent_id FROM goods WHERE id=1")
            .unwrap()
            .rows[0][0],
        PValue::Int(2),
        "FK action survived"
    );
    assert_eq!(
        db.query("SELECT seq FROM sqlite_sequence WHERE name='goods'")
            .unwrap()
            .rows[0][0],
        PValue::Int(2)
    );
    assert_eq!(
        db.query("SELECT count(*) FROM pragma_foreign_key_check")
            .unwrap()
            .rows[0][0],
        PValue::Int(0)
    );
}

pub struct TestDb(std::path::PathBuf);

impl TestDb {
    pub fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Self(std::env::temp_dir().join(format!(
            "phosphor-test-{}-{}.db",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }

    pub fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
    pub fn connect(&self) -> Connection {
        Connection::open(&self.0).unwrap()
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub struct HranaFixture {
    pub db: TestDb,
    pub url: String,
    pub executed: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HranaFixture {
    pub fn new(reject_reads: bool) -> Self {
        let db = TestDb::new();
        db.connect();
        let path = db.path().to_owned();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let executed = Arc::new(Mutex::new(Vec::new()));
        let log = executed.clone();
        let thread = std::thread::spawn(move || {
            for socket in listener.incoming() {
                let mut socket = socket.unwrap();
                if done.load(Ordering::Relaxed) {
                    break;
                }
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut reader = std::io::BufReader::new(&mut socket);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                }
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                // Each pipeline is a fresh connection, like the production
                // client without a baton. No accidental transaction sharing.
                let conn = Connection::open(&path).unwrap();
                conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
                let mut results = Vec::new();
                for req in body["requests"].as_array().unwrap() {
                    let response = match req["type"].as_str().unwrap() {
                        "close" => json!({"type": "close"}),
                        "execute" => match execute(&conn, &req["stmt"], &log, reject_reads) {
                            Ok(result) => json!({"type": "execute", "result": result}),
                            Err(error) => {
                                results.push(json!({"type": "error", "error": {"message": error}}));
                                continue;
                            }
                        },
                        "batch" => {
                            let mut rows = Vec::new();
                            let mut errors = Vec::new();
                            for step in req["batch"]["steps"].as_array().unwrap() {
                                if !condition(&step["condition"], &rows, conn.is_autocommit()) {
                                    rows.push(Value::Null);
                                    errors.push(Value::Null);
                                    continue;
                                }
                                match execute(&conn, &step["stmt"], &log, reject_reads) {
                                    Ok(row) => {
                                        rows.push(row);
                                        errors.push(Value::Null);
                                    }
                                    Err(error) => {
                                        rows.push(Value::Null);
                                        errors.push(json!({"message": error}));
                                    }
                                }
                            }
                            json!({"type": "batch", "result": {"step_results": rows, "step_errors": errors}})
                        }
                        other => panic!("unsupported fixture request {other}"),
                    };
                    results.push(json!({"type": "ok", "response": response}));
                }
                let body = json!({"results": results}).to_string();
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        Self {
            db,
            url,
            executed,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for HranaFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.url.trim_start_matches("http://"));
        self.thread.take().unwrap().join().unwrap();
    }
}

fn condition(cond: &Value, rows: &[Value], autocommit: bool) -> bool {
    match cond["type"].as_str() {
        None => true,
        Some("ok") => !rows[cond["step"].as_u64().unwrap() as usize].is_null(),
        Some("not") => !condition(&cond["cond"], rows, autocommit),
        Some("and") => cond["conds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| condition(c, rows, autocommit)),
        Some("is_autocommit") => autocommit,
        other => panic!("unsupported fixture condition {other:?}"),
    }
}

fn execute(
    conn: &Connection,
    stmt: &Value,
    log: &Mutex<Vec<String>>,
    reject: bool,
) -> Result<Value, String> {
    let sql = stmt["sql"].as_str().unwrap();
    log.lock().unwrap().push(sql.to_owned());
    // Match sqld's connection-setting policy, which plain SQLite lacks.
    if sql.starts_with("PRAGMA query_only=")
        || sql.starts_with("PRAGMA query_only =")
        || sql.starts_with("PRAGMA legacy_alter_table=")
        || sql.starts_with("CREATE TEMP")
    {
        return Err("statement not supported by sqld".into());
    }
    if reject && sql.starts_with("SELECT * FROM (\n") {
        return Err("read unavailable".into());
    }
    let run = || -> rusqlite::Result<Value> {
        let mut batch = rusqlite::Batch::new(conn, sql);
        let mut prepared = batch.next()?.unwrap();
        if batch.next()?.is_some() {
            return Err(rusqlite::Error::MultipleStatement);
        }
        if let Some(args) = stmt["args"].as_array() {
            for (i, arg) in args.iter().enumerate() {
                prepared.raw_bind_parameter(i + 1, crate::remote::decode(arg).unwrap())?;
            }
        }
        let cols: Vec<Value> = prepared
            .column_names()
            .iter()
            .map(|name| json!({"name": name}))
            .collect();
        let mut rows = Vec::new();
        let mut cursor = prepared.raw_query();
        while let Some(row) = cursor.next()? {
            let mut cells = Vec::new();
            for i in 0..cols.len() {
                cells.push(crate::remote::encode(&crate::db::PValue::from_ref(
                    row.get_ref(i)?,
                )));
            }
            rows.push(cells);
        }
        Ok(
            json!({"cols": cols, "rows": rows, "affected_row_count": conn.changes(), "last_insert_rowid": conn.last_insert_rowid().to_string()}),
        )
    };
    run().map_err(|e| e.to_string())
}
