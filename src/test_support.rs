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

pub struct SqldFixture {
    child: std::process::Child,
    dir: std::path::PathBuf,
    pub url: String,
}

impl SqldFixture {
    pub fn new(bin: &str) -> Self {
        let dir = TestDb::new().0.with_extension("sqld");
        std::fs::create_dir(&dir).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        drop(listener);
        let log = std::fs::File::create(dir.join("server.log")).unwrap();
        let child = std::process::Command::new(bin)
            .current_dir(&dir)
            .args(["--db-path", "test.sqld", "--http-listen-addr", &address])
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let mut fixture = Self {
            child,
            dir,
            url: format!("http://{address}"),
        };
        for _ in 0..50 {
            assert!(
                fixture.child.try_wait().unwrap().is_none(),
                "sqld exited: {}",
                std::fs::read_to_string(fixture.dir.join("server.log")).unwrap()
            );
            if crate::remote::RemoteDb::open(&fixture.url).is_ok() {
                return fixture;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!(
            "sqld did not start: {}",
            std::fs::read_to_string(fixture.dir.join("server.log")).unwrap()
        );
    }
}

impl Drop for SqldFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn assert_transaction_and_script_workflows(db: &dyn crate::db::DbLink) {
    use crate::db::PValue;
    db.execute("CREATE TABLE remote_text(id INTEGER PRIMARY KEY, name TEXT UNIQUE); CREATE TABLE remote_log(note TEXT)").unwrap();
    db.execute("INSERT INTO remote_text VALUES(1, 'Ada; O''Brien; 雨'); -- trailing ;")
        .unwrap();
    assert_eq!(
        db.query("SELECT name FROM remote_text").unwrap().rows[0][0],
        PValue::Text("Ada; O'Brien; 雨".into())
    );
    db.execute(
        "CREATE TRIGGER remote_insert AFTER INSERT ON remote_text BEGIN
        INSERT INTO remote_log VALUES ('first;');
        INSERT INTO remote_log VALUES (CASE WHEN new.id>0 THEN 'second;' ELSE 'no;' END);
        END; INSERT INTO remote_text VALUES(2, 'Grace; Hopper')",
    )
    .unwrap();
    assert_eq!(db.count("remote_log").unwrap(), 2);
    db.execute("DELETE FROM remote_text; DELETE FROM remote_log")
        .unwrap();

    let file = TestDb::new();
    for (contents, marker) in [
        ("id,name\n1,Ada\n1,Grace\n", "row 2"),
        ("id,name\n1,Ada\n2\n", "csv record"),
    ] {
        std::fs::write(file.path(), contents).unwrap();
        let error = crate::csv_io::import_csv(db, "remote_text", file.path()).unwrap_err();
        assert!(error.contains(marker), "{error}");
        assert_eq!(db.count("remote_text").unwrap(), 0);
        assert_eq!(
            db.count("remote_log").unwrap(),
            0,
            "trigger side effects rolled back"
        );
    }
    db.execute("CREATE TABLE remote_parent(id INTEGER PRIMARY KEY);
        CREATE TABLE remote_child(id INTEGER REFERENCES remote_parent(id) DEFERRABLE INITIALLY DEFERRED)").unwrap();
    std::fs::write(file.path(), "id\n9\n").unwrap();
    let error = crate::csv_io::import_csv(db, "remote_child", file.path()).unwrap_err();
    assert!(error.contains("commit"), "{error}");
    assert_eq!(db.count("remote_child").unwrap(), 0);
    std::fs::write(file.path(), "id,name\n2,Grace\n").unwrap();
    db.begin_transaction().unwrap();
    db.execute("INSERT INTO remote_text VALUES(1,'pending')")
        .unwrap();
    let error = crate::csv_io::import_csv(db, "remote_text", file.path()).unwrap_err();
    assert!(error.contains("begin"), "{error}");
    assert_eq!(
        db.count("remote_text").unwrap(),
        1,
        "caller's work retained"
    );
    assert!(db
        .apply_schema_changes(&["ALTER TABLE remote_text ADD COLUMN extra TEXT".into()])
        .is_err());
    db.rollback_transaction().unwrap();
    assert_eq!(db.count("remote_text").unwrap(), 0);
    assert!(db.execute("BEGIN; INSERT INTO remote_text VALUES(1,'a'); INSERT INTO remote_text VALUES(1,'b'); COMMIT").is_err());
    // Embedded execute_batch leaves a failed caller script open for explicit
    // recovery; RemoteDb closes the transaction it created in that batch.
    let _ = db.rollback_transaction();
    assert_eq!(db.count("remote_text").unwrap(), 0);
    assert!(db
        .execute("INSERT INTO missing_table VALUES(1); INSERT INTO remote_text VALUES(1,'bad')")
        .is_err());
    assert_eq!(
        db.count("remote_text").unwrap(),
        0,
        "later statements skipped"
    );
    assert!(crate::csv_io::import_csv(db, "remote_text", file.path())
        .unwrap()
        .contains("imported 1"));
    assert_eq!(db.count("remote_text").unwrap(), 1);

    db.begin_transaction().unwrap();
    db.execute("INSERT INTO remote_text VALUES(3,'transaction; row')")
        .unwrap();
    assert_eq!(db.count("remote_text").unwrap(), 2);
    db.commit_transaction().unwrap();
    assert_eq!(db.count("remote_text").unwrap(), 2);

    // Releasing an inner savepoint must retain the outer transaction's stream.
    db.execute("SAVEPOINT outer_scope").unwrap();
    db.execute("INSERT INTO remote_text VALUES(4,'outer')")
        .unwrap();
    db.execute("SAVEPOINT inner_scope; INSERT INTO remote_text VALUES(5,'inner'); ROLLBACK TO inner_scope; RELEASE inner_scope")
        .unwrap();
    assert_eq!(db.count("remote_text").unwrap(), 3);
    db.execute("ROLLBACK TO outer_scope; RELEASE outer_scope")
        .unwrap();
    assert_eq!(db.count("remote_text").unwrap(), 2);

    let text = "O'Brien; 雨\nsecond line; -- literal comment";
    let source = "local greeting = \"hello; O'Brien\";\nmessage(greeting); -- saved; script\n";
    let mut form =
        crate::forms::FormSpec::from_columns("remote_text", db.columns("remote_text").unwrap());
    form.fields[1].label = text.into();
    form.save(db).unwrap();
    assert_eq!(
        crate::forms::FormSpec::load(db, "remote_text")
            .unwrap()
            .fields[1]
            .label,
        text
    );
    crate::script::set_script(db, "remote_text", "on_load", source).unwrap();
    assert_eq!(
        crate::script::get_script(db, "remote_text", "on_load").unwrap(),
        source
    );
    crate::appsgen::add_item(db, "Remote; app", text).unwrap();
    let mut item = crate::appsgen::items(db, "Remote; app").remove(0);
    item.kind = crate::appsgen::ActionKind::Script;
    item.action_ref = source.into();
    crate::appsgen::update_item(db, &item).unwrap();
    crate::appsgen::set_item_ref(db, item.id, source).unwrap();
    assert_eq!(
        crate::appsgen::items(db, "Remote; app")[0].action_ref,
        source
    );
    assert_eq!(crate::appsgen::items(db, "Remote; app")[0].label, text);
    crate::store::pref_set(db, "theme;test", text);
    assert_eq!(crate::store::pref_get(db, "theme;test").unwrap(), text);
    // Quotes, semicolons and NULs are values, never SQL text.
    db.execute_params(
        "UPDATE remote_text SET name=?1 WHERE id=?2",
        &[PValue::Text("bound;\0text".into()), PValue::Int(2)],
    )
    .unwrap();
    assert_eq!(
        db.query("SELECT name FROM remote_text WHERE id=2")
            .unwrap()
            .rows[0][0],
        PValue::Text("bound;\0text".into())
    );

    db.apply_schema_changes(&["ALTER TABLE remote_text RENAME COLUMN name TO label".into()])
        .unwrap();
    let schema = db
        .query("SELECT sql FROM sqlite_schema WHERE name='remote_text'")
        .unwrap()
        .rows;
    assert!(db
        .apply_schema_changes(&[
            "ALTER TABLE remote_text RENAME TO discarded".into(),
            "ALTER TABLE discarded DROP COLUMN id".into()
        ])
        .is_err());
    assert_eq!(
        db.query("SELECT sql FROM sqlite_schema WHERE name='remote_text'")
            .unwrap()
            .rows,
        schema
    );
    assert_eq!(db.count("remote_text").unwrap(), 2);
}

pub fn assert_complete_output(db: &dyn crate::db::DbLink) {
    use crate::db::PValue;
    db.execute(
        "CREATE TABLE output_rows(n INTEGER PRIMARY KEY, label TEXT, amount INTEGER);
        WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<10005)
        INSERT INTO output_rows SELECT n, printf('recipient-%05d',n), 2 FROM seq",
    )
    .unwrap();
    assert!(db.query("SELECT * FROM output_rows").unwrap().truncated);
    let file = TestDb::new();
    let message = crate::csv_io::export_csv(db, "output_rows", file.path()).unwrap();
    assert!(message.contains("10005 row(s)"), "{message}");
    let mut csv = csv::Reader::from_path(file.path()).unwrap();
    let rows = csv.records().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(rows.len(), 10005);
    assert_eq!(&rows.last().unwrap()[1], "recipient-10005");
    let limited = db.query_complete("WITH selected AS (SELECT n FROM output_rows) SELECT n FROM selected ORDER BY n DESC LIMIT 3;").unwrap();
    assert_eq!(
        limited.rows,
        vec![
            vec![PValue::Int(10005)],
            vec![PValue::Int(10004)],
            vec![PValue::Int(10003)]
        ]
    );
    assert_eq!(
        db.query_complete("SELECT n FROM output_rows -- LIMIT is only a comment")
            .unwrap()
            .rows
            .len(),
        10005
    );
    let labels = crate::report::labels(db, "output_rows").unwrap().join("\n");
    assert_eq!(labels.matches("recipient-").count(), 10005);
    assert!(labels.contains("recipient-10005"));
    let report = crate::report::render(db, &crate::report::ReportSpec::for_table("output_rows"))
        .unwrap()
        .join("\n");
    assert!(report.contains("TOTAL (10005 rows)"));
    assert!(report.contains("20010"));
    assert!(report.contains("recipient-10005"));
    let grouped = crate::report::ReportSpec {
        name: "grouped output".into(),
        title: "Complete totals".into(),
        source: "SELECT n%2 AS cohort, amount FROM output_rows; -- complete source".into(),
        group_by: Some("cohort".into()),
    };
    let text = crate::report::render(db, &grouped).unwrap().join("\n");
    assert!(text.contains("subtotal (5002 rows)"));
    assert!(text.contains("subtotal (5003 rows)"));
    assert!(text.contains("TOTAL (10005 rows)"));

    for sql in [
        "SELECT * FROM missing_output_table",
        "SELECT CASE WHEN n=10005 THEN abs(-9223372036854775808) ELSE n END FROM output_rows",
        "WITH x AS (SELECT 1) DELETE FROM output_rows RETURNING n",
    ] {
        std::fs::write(file.path(), "previous completed output").unwrap();
        assert!(
            crate::csv_io::export_csv(db, sql, file.path()).is_err(),
            "{sql}"
        );
        assert_eq!(
            std::fs::read_to_string(file.path()).unwrap(),
            "previous completed output"
        );
        let mut spec = crate::report::ReportSpec::for_table("unused");
        spec.source = sql.into();
        assert!(crate::report::render(db, &spec).is_err(), "{sql}");
    }
    assert_eq!(db.count("output_rows").unwrap(), 10005);
    db.begin_transaction().unwrap();
    db.execute("INSERT INTO output_rows VALUES(10006,'pending',2)")
        .unwrap();
    assert_eq!(
        db.query_complete("SELECT n FROM output_rows WHERE n=10006")
            .unwrap()
            .rows,
        vec![vec![PValue::Int(10006)]]
    );
    let error = db
        .stream_query(
            "SELECT n FROM output_rows",
            Box::new(|event| {
                if matches!(event, crate::db::QueryEvent::Row(_)) {
                    Err("output disk failure".into())
                } else {
                    Ok(())
                }
            }),
        )
        .unwrap_err();
    assert!(error.contains("output disk failure"));
    db.rollback_transaction().unwrap();
    assert_eq!(db.count("output_rows").unwrap(), 10005);
    crate::csv_io::export_csv(db, "SELECT n FROM output_rows WHERE 0", file.path()).unwrap();
    assert_eq!(std::fs::read_to_string(file.path()).unwrap(), "n\n");
}

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
    pub expire_next: Arc<AtomicBool>,
    pub drop_next: Arc<AtomicBool>,
    pub truncate_output: Arc<AtomicBool>,
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
        let expire_next = Arc::new(AtomicBool::new(false));
        let expire = expire_next.clone();
        let drop_next = Arc::new(AtomicBool::new(false));
        let lose = drop_next.clone();
        let truncate_output = Arc::new(AtomicBool::new(false));
        let truncate = truncate_output.clone();
        let thread = std::thread::spawn(move || {
            let mut streams = std::collections::HashMap::<String, Connection>::new();
            let mut serial = 0;
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
                let connection = if let Some(baton) = body["baton"].as_str() {
                    streams.remove(baton)
                } else {
                    let conn = Connection::open(&path).unwrap();
                    conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
                    Some(conn)
                };
                let mut connection = connection;
                if expire.swap(false, Ordering::Relaxed) {
                    if let Some(conn) = &connection {
                        if !conn.is_autocommit() {
                            conn.execute_batch("ROLLBACK").unwrap();
                        }
                    }
                }
                if body["batch"].is_object() {
                    serial += 1;
                    let baton = format!("stream-{serial}");
                    let conn = connection.as_ref().unwrap();
                    let mut entries = vec![json!({"baton":baton, "base_url":null})];
                    for (i, step) in body["batch"]["steps"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .enumerate()
                    {
                        if !condition(&step["condition"], &[], conn.is_autocommit()) {
                            continue;
                        }
                        match execute(conn, &step["stmt"], &log, reject_reads) {
                            Ok(result) => {
                                entries.push(
                                    json!({"type":"step_begin", "step":i, "cols":result["cols"]}),
                                );
                                for row in result["rows"].as_array().unwrap() {
                                    entries.push(json!({"type":"row", "row":row}));
                                }
                                entries.push(json!({"type":"step_end", "affected_row_count":0, "last_insert_rowid":null}));
                            }
                            Err(error) => entries.push(
                                json!({"type":"step_error", "step":i, "error":{"message":error}}),
                            ),
                        }
                    }
                    if truncate.swap(false, Ordering::Relaxed) {
                        entries.truncate(3);
                    }
                    entries.push(json!({"type":"replication_index", "replication_index":null}));
                    streams.insert(baton, connection.take().unwrap());
                    let body = entries.iter().map(|v| format!("{v}\n")).collect::<String>();
                    let _ = write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    continue;
                }
                let mut results = Vec::new();
                for req in body["requests"].as_array().unwrap() {
                    if req["type"] == "close" {
                        connection.take();
                        results.push(json!({"type":"ok", "response":{"type":"close"}}));
                        continue;
                    }
                    let Some(conn) = &connection else {
                        results.push(json!({"type":"error", "error":{"message":"invalid baton"}}));
                        continue;
                    };
                    let response = match req["type"].as_str().unwrap() {
                        "get_autocommit" => {
                            json!({"type":"get_autocommit", "is_autocommit":conn.is_autocommit()})
                        }
                        "execute" => match execute(conn, &req["stmt"], &log, reject_reads) {
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
                                match execute(conn, &step["stmt"], &log, reject_reads) {
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
                serial += 1;
                let baton = connection.map(|conn| {
                    let baton = format!("stream-{serial}");
                    streams.insert(baton.clone(), conn);
                    baton
                });
                if lose.swap(false, Ordering::Relaxed) {
                    continue;
                }
                let body = json!({"baton":baton, "base_url":null, "results": results}).to_string();
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        Self {
            db,
            url,
            executed,
            expire_next,
            drop_next,
            truncate_output,
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
