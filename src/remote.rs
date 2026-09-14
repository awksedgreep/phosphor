//! RemoteDb — the sqld backend: Hrana over HTTP (`POST /v3/pipeline`),
//! same `DbLink` trait as the embedded file backend (DESIGN.md phase 2).
//!
//! Wire facts (verified against sqld 0.24.x in the timeless-libsql
//! docs work): integers are JSON *strings* to preserve 64-bit
//! precision; blobs are base64; each pipeline without a baton lands on
//! a fresh pooled connection. Keep the baton while a transaction is open.
//!
//! PHOSPHOR_TOKEN adds `Authorization: Bearer …` — the only difference
//! between self-hosted sqld and Turso-hosted URLs.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value as Json};

use crate::db::{ColumnInfo, DbLink, DbResult, PValue, Page, QueryResult, TableInfo, QUERY_CAP};

pub struct RemoteDb {
    agent: ureq::Agent,
    pipeline_url: String,
    display: String,
    auth: Option<String>,
    readonly: bool,
    rowid_cache: Mutex<HashMap<String, Option<String>>>,
    stream: Mutex<StreamState>,
}

#[derive(Default)]
struct StreamState {
    active: Option<Stream>,
    // A lost response may hide a COMMIT or a rotated baton. Never reconnect
    // and continue an import on a different connection after that failure.
    lost: bool,
}

#[derive(Clone)]
struct Stream {
    baton: String,
    url: String,
}

struct StmtOut {
    cols: Vec<String>,
    rows: Vec<Vec<PValue>>,
    affected: i64,
    last_rowid: Option<i64>,
}

impl RemoteDb {
    pub fn open(url: &str) -> DbResult<Self> {
        Self::open_with_mode(url, false)
    }

    pub fn open_with_mode(url: &str, readonly: bool) -> DbResult<Self> {
        let base = url.trim_end_matches('/');
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(15))
            .build();
        let db = RemoteDb {
            agent,
            pipeline_url: format!("{base}/v3/pipeline"),
            display: base.to_owned(),
            readonly,
            // Preformatted once (was format! per HTTP request).
            auth: std::env::var("PHOSPHOR_TOKEN")
                .ok()
                .filter(|t| !t.is_empty())
                .map(|t| format!("Bearer {t}")),
            rowid_cache: Mutex::new(HashMap::new()),
            stream: Mutex::new(StreamState::default()),
        };
        // Fail at open, not at first keystroke.
        db.pipeline(&[("SELECT 1", vec![])])?;
        Ok(db)
    }

    fn pipeline(&self, stmts: &[(&str, Vec<Json>)]) -> DbResult<Vec<StmtOut>> {
        self.run_statements(
            stmts,
            stmts
                .iter()
                .any(|(sql, _)| crate::sql::changes_transaction(sql)),
        )
    }

    fn run_statements(
        &self,
        stmts: &[(&str, Vec<Json>)],
        may_change_transaction: bool,
    ) -> DbResult<Vec<StmtOut>> {
        let mut state = self.stream.lock().unwrap();
        if state.lost {
            return Err("remote transaction outcome is unknown; reopen the database and check before retrying".into());
        }
        let previous = state.active.clone();
        let track = previous.is_some() || may_change_transaction;
        let mut steps = Vec::new();
        if previous.is_none() && !self.readonly {
            steps.push(json!({"stmt": {"sql": "PRAGMA foreign_keys=ON"}}));
        }
        let prefix = steps.len();
        for (sql, args) in stmts {
            // The server parses read-only requests as SELECT subqueries and
            // rejects multiple statements, preventing escapes into a script.
            let sql = if self.readonly {
                format!("SELECT * FROM (\n{}\n)", sql.trim().trim_end_matches(';'))
            } else {
                (*sql).to_owned()
            };
            let mut step = json!({"stmt": {"sql": sql, "args": args}});
            if !steps.is_empty() {
                step["condition"] = json!({"type": "ok", "step": steps.len()-1});
            } else if previous.is_some() {
                // A server timeout can roll back a transaction while leaving
                // its stream alive. Skip writes if ownership was lost.
                step["condition"] = json!({"type": "not", "cond": {"type": "is_autocommit"}});
            }
            steps.push(step);
        }
        let requests = vec![
            json!({"type": "batch", "batch": {"steps": steps}}),
            if track {
                json!({"type": "get_autocommit"})
            } else {
                json!({"type": "close"})
            },
        ];
        let url = previous
            .as_ref()
            .map_or(self.pipeline_url.as_str(), |s| &s.url);
        let body = match self.send_to(url, previous.as_ref().map(|s| s.baton.as_str()), requests) {
            Ok(body) => body,
            Err(e) => {
                state.lost = track;
                return Err(if track {
                    format!("{e}; transaction outcome unknown; reopen and check before retrying")
                } else {
                    e
                });
            }
        };
        let result = decode_batch(&body["results"][0], steps.len());
        if track {
            let continuation = (|| {
                let baton = body["baton"]
                    .as_str()
                    .ok_or("sqld closed the transaction stream")?
                    .to_owned();
                let url = match body["base_url"].as_str() {
                    Some(base) if base.starts_with("http://") || base.starts_with("https://") => {
                        format!("{}/v3/pipeline", base.trim_end_matches('/'))
                    }
                    Some(_) => return Err("sqld returned an invalid stream URL"),
                    None => url.to_owned(),
                };
                let status = &body["results"][1];
                if status["type"] != "ok" || status["response"]["type"] != "get_autocommit" {
                    return Err("sqld did not report transaction state");
                }
                let autocommit = status["response"]["is_autocommit"]
                    .as_bool()
                    .ok_or("sqld omitted transaction state")?;
                Ok((Stream { baton, url }, autocommit))
            })();
            let (stream, autocommit) = match continuation {
                Ok(value) => value,
                Err(e) => {
                    state.lost = true;
                    return Err(format!(
                        "{e}; reopen the database and check before retrying"
                    ));
                }
            };
            if autocommit {
                // COMMIT/ROLLBACK is already acknowledged. A close failure
                // must not turn a completed write into a retryable error.
                self.close_stream(&stream);
                state.active = None;
            } else if previous.is_none() && result.is_err() {
                // This failed script started its own transaction. Conditional
                // steps skipped COMMIT; closing the stream rolls it back.
                let rollback = self.send_to(
                    &stream.url,
                    Some(&stream.baton),
                    vec![
                        json!({"type": "execute", "stmt": {"sql": "ROLLBACK"}}),
                        json!({"type": "close"}),
                    ],
                );
                state.active = None;
                if !rollback
                    .as_ref()
                    .is_ok_and(|body| body["results"][0]["type"] == "ok")
                {
                    state.lost = true;
                    return Err(format!(
                        "{}; rollback could not be confirmed; reopen and check the database",
                        result.err().unwrap()
                    ));
                }
            } else {
                state.active = Some(stream);
            }
        }
        result
            .map(|out| out.into_iter().skip(prefix).collect())
            .map_err(|e| {
                if self.readonly {
                    format!("read-only query: {e}")
                } else {
                    e
                }
            })
    }

    // Dedicated schema batches own a complete transaction and must never
    // run beside an interactive transaction on another connection.
    fn send(&self, requests: Vec<Json>) -> DbResult<Json> {
        let mut state = self.stream.lock().unwrap();
        if state.lost {
            return Err("remote transaction outcome unknown; reopen and check the database".into());
        }
        if state.active.is_some() {
            return Err("finish the current transaction before editing the table".into());
        }
        self.send_to(&self.pipeline_url, None, requests)
            .inspect_err(|_| state.lost = true)
    }

    fn send_to(&self, url: &str, baton: Option<&str>, requests: Vec<Json>) -> DbResult<Json> {
        let mut req = self.agent.post(url);
        if let Some(a) = &self.auth {
            req = req.set("Authorization", a);
        }
        req.send_json(json!({"baton": baton, "requests": requests}))
            .map_err(|e| match e {
                ureq::Error::Status(code, resp) => format!(
                    "sqld HTTP {code}: {}",
                    resp.into_string().unwrap_or_default()
                ),
                other => format!("sqld unreachable: {other}"),
            })?
            .into_json()
            .map_err(|e| format!("sqld response was not JSON: {e}"))
    }

    fn close_stream(&self, stream: &Stream) {
        let mut req = self.agent.post(&stream.url).timeout(Duration::from_secs(1));
        if let Some(a) = &self.auth {
            req = req.set("Authorization", a);
        }
        let _ = req.send_json(json!({"baton": stream.baton, "requests": [{"type": "close"}]}));
    }

    fn one(&self, sql: &str, args: Vec<Json>) -> DbResult<StmtOut> {
        let mut v = self.pipeline(&[(sql, args)])?;
        v.pop().ok_or_else(|| "sqld returned no result".into())
    }

    fn read_cursor(
        &self,
        sql: &str,
        cap: Option<usize>,
        may_stop: bool,
        mut sink: crate::db::RowSink,
    ) -> DbResult<usize> {
        use std::io::BufRead;
        let mut state = self.stream.lock().unwrap();
        if state.lost {
            return Err("remote transaction outcome unknown; reopen and check the database".into());
        }
        let previous = state.active.clone();
        let pipeline_url = previous
            .as_ref()
            .map_or(self.pipeline_url.as_str(), |s| &s.url);
        let cursor_url = format!("{}/cursor", pipeline_url.trim_end_matches("/pipeline"));
        let mut step = json!({"stmt": {"sql": sql, "want_rows": true}});
        if previous.is_some() {
            step["condition"] = json!({"type": "not", "cond": {"type": "is_autocommit"}});
        }
        let mut req = self.agent.post(&cursor_url);
        if let Some(auth) = &self.auth {
            req = req.set("Authorization", auth);
        }
        let mut continuation = None;
        let mut complete_response = false;
        let result = (|| {
            let response = req
                .send_json(json!({
                    "baton": previous.as_ref().map(|s| &s.baton),
                    "batch": {"steps": [step]}
                }))
                .map_err(|e| format!("sqld output request: {e}"))?;
            let mut reader = std::io::BufReader::new(response.into_reader());
            let mut line = String::new();
            reader.read_line(&mut line).map_err(|e| e.to_string())?;
            let header: Json = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            if let Some(baton) = header["baton"].as_str() {
                let url = match header["base_url"].as_str() {
                    Some(base) if base.starts_with("http://") || base.starts_with("https://") => {
                        format!("{}/v3/pipeline", base.trim_end_matches('/'))
                    }
                    Some(_) => return Err("sqld returned an invalid stream URL".into()),
                    None => pipeline_url.to_owned(),
                };
                continuation = Some(Stream {
                    baton: baton.to_owned(),
                    url,
                });
            } else if previous.is_some() {
                return Err("sqld closed the transaction stream during output".into());
            }
            let (mut started, mut ended, mut width, mut count) = (false, false, 0, 0);
            let mut error = None;
            let mut stopped = false;
            loop {
                line.clear();
                if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                    complete_response = true;
                    break;
                }
                let entry: Json = serde_json::from_str(&line).map_err(|e| e.to_string())?;
                let event = match entry["type"].as_str() {
                    Some("step_begin") if !started && entry["step"] == 0 => {
                        started = true;
                        let columns = entry["cols"]
                            .as_array()
                            .ok_or("missing output columns")?
                            .iter()
                            .map(|c| {
                                c["name"]
                                    .as_str()
                                    .map(str::to_owned)
                                    .ok_or_else(|| "missing column name".to_owned())
                            })
                            .collect::<DbResult<Vec<_>>>()?;
                        width = columns.len();
                        Some(crate::db::QueryEvent::Columns(columns))
                    }
                    Some("row") if started && !ended => {
                        let row = entry["row"]
                            .as_array()
                            .ok_or("missing output row")?
                            .iter()
                            .map(decode)
                            .collect::<DbResult<Vec<_>>>()?;
                        if row.len() != width {
                            return Err("incomplete output row".into());
                        }
                        count += 1;
                        if cap.is_none_or(|cap| count <= cap) {
                            Some(crate::db::QueryEvent::Row(row))
                        } else {
                            None
                        }
                    }
                    Some("step_end") if started && !ended => {
                        ended = true;
                        None
                    }
                    Some("step_error" | "error") => {
                        error.get_or_insert_with(|| remote_error(&entry["error"]));
                        None
                    }
                    // sqld adds replication metadata after the result.
                    Some("replication_index") => None,
                    _ => return Err("unexpected sqld output response".into()),
                };
                // Drain after a disk/SQL failure so a caller-owned stream can
                // still be used or rolled back once this response completes.
                if error.is_none() {
                    if let Some(event) = event {
                        error = sink(event).err();
                    }
                }
                // An autocommit read owns this cursor. Stop after the lookahead
                // row and close its stream; never abandon a caller's transaction.
                if error.is_some() && previous.is_none() {
                    break; // failed/cancelled output owns this stream; close it
                }
                if may_stop
                    && previous.is_none()
                    && error.is_none()
                    && cap.is_some_and(|cap| count >= cap)
                {
                    stopped = true;
                    break;
                }
            }
            if let Some(error) = error {
                return Err(error);
            }
            if !ended && !stopped {
                return Err("output incomplete: server did not finish the SELECT".into());
            }
            sink(crate::db::QueryEvent::End)?;
            Ok(count)
        })();
        if previous.is_some() {
            if let Some(stream) = continuation {
                state.active = Some(stream);
            }
            if !complete_response {
                state.lost = true;
                return Err(format!(
                    "{}; remote transaction outcome unknown; reopen and check the database",
                    result
                        .err()
                        .unwrap_or_else(|| "output response incomplete".into())
                ));
            }
        } else if let Some(stream) = continuation {
            self.close_stream(&stream);
        }
        result
    }

    fn quote(ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    fn str_lit(s: &str) -> String {
        format!("'{}'", s.replace('\'', "''"))
    }
}

impl Drop for RemoteDb {
    fn drop(&mut self) {
        let stream = self
            .stream
            .get_mut()
            .ok()
            .and_then(|state| state.active.take());
        if let Some(stream) = stream {
            self.close_stream(&stream);
        }
    }
}

fn decode_batch(response: &Json, expected: usize) -> DbResult<Vec<StmtOut>> {
    if response["type"] == "error" {
        return Err(remote_error(&response["error"]));
    }
    if response["response"]["type"] != "batch" {
        return Err("sqld did not return the statement batch".into());
    }
    let result = &response["response"]["result"];
    let rows = result["step_results"]
        .as_array()
        .ok_or("missing statement results")?;
    let errors = result["step_errors"]
        .as_array()
        .ok_or("missing statement errors")?;
    if rows.len() != expected || errors.len() != expected {
        return Err("incomplete statement response".into());
    }
    for error in errors {
        if !error.is_null() {
            return Err(remote_error(error));
        }
    }
    if rows.iter().any(Json::is_null) {
        return Err("transaction ended on the server; statements were skipped".into());
    }
    rows.iter().map(decode_result).collect()
}

fn remote_error(error: &Json) -> String {
    error["message"]
        .as_str()
        .unwrap_or("unknown sqld error")
        .to_owned()
}

pub(crate) fn encode(v: &PValue) -> Json {
    match v {
        PValue::Null => json!({"type": "null"}),
        // 64-bit precision survives only as a string on the wire.
        PValue::Int(i) => json!({"type": "integer", "value": i.to_string()}),
        PValue::Real(f) => json!({"type": "float", "value": f}),
        PValue::Text(t) => json!({"type": "text", "value": t}),
        PValue::Blob(b) => json!({
            "type": "blob",
            "base64": base64::engine::general_purpose::STANDARD.encode(b)
        }),
    }
}

pub(crate) fn decode(v: &Json) -> DbResult<PValue> {
    Ok(match v["type"].as_str().unwrap_or("") {
        "null" => PValue::Null,
        "integer" => PValue::Int(
            v["value"]
                .as_str()
                .ok_or("integer without string value")?
                .parse::<i64>()
                .map_err(|e| format!("bad integer from sqld: {e}"))?,
        ),
        "float" => PValue::Real(v["value"].as_f64().ok_or("float without value")?),
        "text" => PValue::Text(v["value"].as_str().unwrap_or("").to_owned()),
        "blob" => {
            let b64 = v["base64"].as_str().unwrap_or("");
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(b64))
                .map_err(|e| format!("bad blob base64 from sqld: {e}"))?;
            PValue::Blob(bytes)
        }
        other => return Err(format!("unknown hrana value type {other:?}")),
    })
}

fn decode_result(result: &Json) -> DbResult<StmtOut> {
    let cols = result["cols"]
        .as_array()
        .map(|cols| {
            cols.iter()
                .map(|c| c["name"].as_str().unwrap_or("").to_owned())
                .collect()
        })
        .unwrap_or_default();
    let mut rows = Vec::new();
    if let Some(json_rows) = result["rows"].as_array() {
        rows.reserve(json_rows.len());
        for row in json_rows {
            let cells = row.as_array().ok_or("row is not an array")?;
            let mut out = Vec::with_capacity(cells.len());
            for c in cells {
                out.push(decode(c)?);
            }
            rows.push(out);
        }
    }
    let affected = result["affected_row_count"].as_i64().unwrap_or(0);
    // Hrana sends last_insert_rowid as a string (64-bit precision).
    let last_rowid = result["last_insert_rowid"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok());
    Ok(StmtOut {
        cols,
        rows,
        affected,
        last_rowid,
    })
}

impl DbLink for RemoteDb {
    fn stream_query(&self, sql: &str, sink: crate::db::RowSink) -> DbResult<usize> {
        self.read_cursor(&crate::sql::select_source(sql)?, None, false, sink)
    }

    fn readonly(&self) -> bool {
        self.readonly
    }
    fn backend(&self) -> &'static str {
        "sqld"
    }

    fn name(&self) -> &str {
        &self.display
    }

    fn tables(&self) -> DbResult<Vec<TableInfo>> {
        let out = self.one(
            "SELECT name, type FROM sqlite_master \
             WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' \
             ORDER BY type = 'view', name",
            vec![],
        )?;
        Ok(out
            .rows
            .into_iter()
            .filter_map(|r| match (&r[0], &r[1]) {
                (PValue::Text(name), PValue::Text(t)) => Some(TableInfo {
                    name: name.clone(),
                    is_view: t == "view",
                }),
                _ => None,
            })
            .collect())
    }

    fn columns(&self, table: &str) -> DbResult<Vec<ColumnInfo>> {
        let out = self.one(
            crate::db::COLUMN_INFO_SQL,
            vec![encode(&PValue::Text(table.to_owned()))],
        )?;
        Ok(out
            .rows
            .into_iter()
            .map(|r| ColumnInfo {
                name: match &r[1] {
                    PValue::Text(t) => t.clone(),
                    v => v.render(),
                },
                decl_type: match &r[2] {
                    PValue::Text(t) => t.clone(),
                    _ => String::new(),
                },
                notnull: matches!(&r[3], PValue::Int(n) if *n != 0),
                pk: matches!(&r[5], PValue::Int(n) if *n != 0),
                generated: matches!(&r[6], PValue::Int(n) if *n >= 2),
                dflt_value: match &r[4] {
                    PValue::Text(t) => Some(t.clone()),
                    _ => None,
                },
            })
            .collect())
    }

    fn count(&self, table: &str) -> DbResult<i64> {
        let out = self.one(
            &format!("SELECT count(*) FROM {}", Self::quote(table)),
            vec![],
        )?;
        match out.rows.first().and_then(|r| r.first()) {
            Some(PValue::Int(n)) => Ok(*n),
            _ => Err("count(*) did not return an integer".into()),
        }
    }

    fn rowid_column(&self, table: &str) -> DbResult<Option<String>> {
        if let Some(known) = self.rowid_cache.lock().unwrap().get(table) {
            return Ok(known.clone());
        }
        let alias = crate::db::resolve_rowid_column(self, table)?;
        self.rowid_cache
            .lock()
            .unwrap()
            .insert(table.to_owned(), alias.clone());
        Ok(alias)
    }

    fn page(&self, table: &str, offset: i64, limit: i64) -> DbResult<Page> {
        let q = Self::quote(table);
        if let Some(alias) = self.rowid_column(table)? {
            let id = Self::quote(&alias);
            let out = self.one(
                &format!(
                    "SELECT {q}.{id}, * FROM {q} ORDER BY {q}.{id} LIMIT {limit} OFFSET {offset}"
                ),
                vec![],
            )?;
            let mut rows = out.rows;
            let mut rowids = Vec::with_capacity(rows.len());
            for row in &mut rows {
                match row.remove(0) {
                    PValue::Int(id) => rowids.push(id),
                    _ => return Err("rowid was not an integer".into()),
                }
            }
            Ok(Page {
                rows,
                rowids: Some(rowids),
            })
        } else {
            let out = self.one(
                &format!("SELECT * FROM {q} LIMIT {limit} OFFSET {offset}"),
                vec![],
            )?;
            Ok(Page {
                rows: out.rows,
                rowids: None,
            })
        }
    }

    /// Single-pipeline open_window (total rides along, stripped here).
    fn open_window(&self, table: &str, offset: i64, limit: i64) -> DbResult<(Page, i64)> {
        let fallback = || {
            let page = self.page(table, offset, limit)?;
            let total = self.count(table)?;
            Ok((page, total))
        };
        let q = Self::quote(table);
        let alias = self.rowid_column(table)?;
        let with_rowid = alias.is_some();
        let select = if let Some(alias) = alias {
            let id = Self::quote(&alias);
            format!("SELECT {q}.{id}, *, count(*) OVER () AS _total FROM {q} ORDER BY {q}.{id}")
        } else {
            format!("SELECT *, count(*) OVER () AS _total FROM {q}")
        };
        let out = match self.one(&format!("{select} LIMIT {limit} OFFSET {offset}"), vec![]) {
            Ok(o) => o,
            Err(_) => return fallback(),
        };
        let rows = out.rows;
        if rows.is_empty() {
            if offset == 0 && limit > 0 {
                let page = Page {
                    rows: Vec::new(),
                    rowids: with_rowid.then(Vec::new),
                };
                return Ok((page, 0));
            }
            return fallback();
        }
        match crate::db::strip_window_total(rows, with_rowid) {
            Ok(ok) => Ok(ok),
            Err(_) => fallback(),
        }
    }

    fn query(&self, sql: &str) -> DbResult<QueryResult> {
        let start = Instant::now();
        let sql = crate::sql::single_query(sql)?;
        let may_stop = crate::sql::read_query(sql);
        if !may_stop && crate::sql::head(sql) != "PRAGMA" {
            // Statements with RETURNING must finish their write. The ordinary
            // pipeline also preserves FK enforcement and transaction ownership.
            let out = self.one(sql, vec![])?;
            let truncated = out.rows.len() > QUERY_CAP;
            let mut rows = out.rows;
            rows.truncate(QUERY_CAP);
            return Ok(QueryResult {
                columns: out.cols,
                rows,
                truncated,
                elapsed: start.elapsed(),
            });
        }
        let source = if self.readonly {
            crate::sql::select_source(sql)?
        } else {
            sql.to_owned()
        };
        let data = std::sync::Arc::new(Mutex::new((Vec::new(), Vec::new())));
        let output = data.clone();
        self.read_cursor(
            &source,
            Some(QUERY_CAP + 1),
            may_stop,
            Box::new(move |event| {
                let mut data = output.lock().unwrap();
                match event {
                    crate::db::QueryEvent::Columns(columns) => data.0 = columns,
                    crate::db::QueryEvent::Row(row) => data.1.push(row),
                    crate::db::QueryEvent::End => (),
                }
                Ok(())
            }),
        )?;
        let (columns, mut rows) = std::mem::take(&mut *data.lock().unwrap());
        let truncated = rows.len() > QUERY_CAP;
        rows.truncate(QUERY_CAP);
        Ok(QueryResult {
            columns,
            rows,
            truncated,
            elapsed: start.elapsed(),
        })
    }

    fn apply_schema_changes(&self, statements: &[String]) -> DbResult<Duration> {
        self.require_writable()?;
        let start = Instant::now();
        // One stream, with every step conditional on its predecessor. A
        // failed ALTER or FK check skips COMMIT and takes the rollback path.
        // CASE evaluates the overflow expression only when FK violations
        // exist, turning validation into a server-side step failure before
        // COMMIT. sqld forbids temporary tables.
        let mut sql = vec!["PRAGMA foreign_keys=ON".to_owned(), "BEGIN".to_owned()];
        sql.extend_from_slice(statements);
        let check_step = sql.len();
        sql.push(
            "SELECT CASE WHEN EXISTS(SELECT 1 FROM pragma_foreign_key_check) \
             THEN abs(-9223372036854775808) ELSE 0 END"
                .into(),
        );
        sql.push("COMMIT".into());
        let commit_step = sql.len() - 1;
        let mut steps: Vec<Json> = sql
            .iter()
            .enumerate()
            .map(|(i, sql)| {
                let mut step = json!({"stmt": {"sql": sql}});
                if i > 0 {
                    step["condition"] = json!({"type": "ok", "step": i - 1});
                }
                step
            })
            .collect();
        steps.push(json!({"condition": {"type": "and", "conds": [
            {"type": "not", "cond": {"type": "ok", "step": commit_step}},
            {"type": "not", "cond": {"type": "is_autocommit"}}
        ]}, "stmt": {"sql": "ROLLBACK"}}));
        self.rowid_cache.lock().unwrap().clear();
        let body = self.send(vec![
            json!({"type": "batch", "batch": {"steps": steps}}),
            json!({"type": "close"}),
        ])?;
        let response = &body["results"][0];
        if response["type"] == "error" {
            return Err(remote_error(&response["error"]));
        }
        if response["response"]["type"] != "batch" {
            return Err("sqld did not return the schema batch".into());
        }
        let result = &response["response"]["result"];
        let results = result["step_results"]
            .as_array()
            .ok_or("missing schema results")?;
        let errors = result["step_errors"]
            .as_array()
            .ok_or("missing schema errors")?;
        if results.len() != steps.len() || errors.len() != steps.len() {
            return Err("incomplete schema response; refresh to check the database".into());
        }
        for (i, error) in errors.iter().enumerate() {
            if !error.is_null() {
                let message = if i == check_step {
                    format!("foreign key check failed: {}", remote_error(error))
                } else {
                    remote_error(error)
                };
                let rollback = &errors[commit_step + 1];
                return Err(if rollback.is_null() {
                    message
                } else {
                    format!("{message}; rollback: {}", remote_error(rollback))
                });
            }
        }
        if results[..=commit_step].iter().any(Json::is_null) {
            return Err("server skipped a schema step; refresh to check the database".into());
        }
        Ok(start.elapsed())
    }

    fn execute(&self, sql: &str) -> DbResult<(i64, Duration)> {
        self.require_writable()?;
        let start = Instant::now();
        let stmts = crate::sql::split(sql)?;
        if stmts.is_empty() {
            return Ok((0, start.elapsed()));
        }
        let calls: Vec<(&str, Vec<Json>)> = stmts.iter().map(|s| (*s, Vec::new())).collect();
        self.rowid_cache.lock().unwrap().clear();
        let outs = self.pipeline(&calls)?;
        let n = if outs.len() == 1 {
            outs[0].affected
        } else {
            -1
        };
        // Any statement may have changed schema/rowid-ness.
        self.rowid_cache.lock().unwrap().clear();
        Ok((n, start.elapsed()))
    }

    fn execute_params(&self, sql: &str, params: &[PValue]) -> DbResult<(i64, Duration)> {
        self.require_writable()?;
        let start = Instant::now();
        self.rowid_cache.lock().unwrap().clear();
        let out = self.one(sql, params.iter().map(encode).collect())?;
        Ok((out.affected, start.elapsed()))
    }

    fn update_row(&self, table: &str, rowid: i64, changes: &[(String, PValue)]) -> DbResult<i64> {
        self.require_writable()?;
        if changes.is_empty() {
            return Ok(rowid);
        }
        let alias = self
            .rowid_column(table)?
            .ok_or("no unambiguous row identity; editing is read-only")?;
        let sets: Vec<String> = changes
            .iter()
            .enumerate()
            .map(|(i, (col, _))| format!("{} = ?{}", Self::quote(col), i + 1))
            .collect();
        let sql = format!(
            "UPDATE {} SET {} WHERE {} RETURNING {}",
            Self::quote(table),
            sets.join(", "),
            crate::db::rowid_predicate(table, &alias, &format!("?{}", changes.len() + 1)),
            Self::quote(&alias)
        );
        let mut args: Vec<Json> = changes.iter().map(|(_, v)| encode(v)).collect();
        args.push(encode(&PValue::Int(rowid)));
        let out = self.one(&sql, args)?;
        if out.affected == 1 {
            match out.rows.as_slice() {
                [row] => match row.as_slice() {
                    [PValue::Int(id)] => Ok(*id),
                    _ => Err("updated record has no usable identity".into()),
                },
                _ => Err("sqld did not return the updated record identity".into()),
            }
        } else {
            Err(format!(
                "expected to update 1 row, updated {}",
                out.affected
            ))
        }
    }

    fn insert_row(&self, table: &str, changes: &[(String, PValue)]) -> DbResult<i64> {
        self.require_writable()?;
        let sql = if changes.is_empty() {
            format!("INSERT INTO {} DEFAULT VALUES", Self::quote(table))
        } else {
            let cols: Vec<String> = changes.iter().map(|(c, _)| Self::quote(c)).collect();
            let marks: Vec<String> = (1..=changes.len()).map(|i| format!("?{i}")).collect();
            format!(
                "INSERT INTO {} ({}) VALUES ({})",
                Self::quote(table),
                cols.join(", "),
                marks.join(", ")
            )
        };
        let args: Vec<Json> = changes.iter().map(|(_, v)| encode(v)).collect();
        let out = self.one(&sql, args)?;
        out.last_rowid
            .ok_or_else(|| "sqld did not report last_insert_rowid".into())
    }

    fn delete_row(&self, table: &str, rowid: i64) -> DbResult<()> {
        self.require_writable()?;
        let alias = self
            .rowid_column(table)?
            .ok_or("no unambiguous row identity; deleting is read-only")?;
        let out = self.one(
            &format!(
                "DELETE FROM {} WHERE {}",
                Self::quote(table),
                crate::db::rowid_predicate(table, &alias, "?1")
            ),
            vec![encode(&PValue::Int(rowid))],
        )?;
        if out.affected == 1 {
            Ok(())
        } else {
            Err(format!(
                "expected to delete 1 row, deleted {}",
                out.affected
            ))
        }
    }

    fn health(&self) -> Option<String> {
        let view = self
            .one(
                "SELECT name FROM sqlite_master \
                 WHERE type = 'view' AND name LIKE '%\\_report' ESCAPE '\\' \
                 ORDER BY name LIMIT 1",
                vec![],
            )
            .ok()?
            .rows
            .into_iter()
            .next()?;
        let PValue::Text(view) = &view[0] else {
            return None;
        };
        let out = self
            .one(
                &format!("SELECT status FROM {} LIMIT 1", Self::quote(view)),
                vec![],
            )
            .ok()?;
        match out.rows.into_iter().next()?.into_iter().next()? {
            PValue::Text(s) => Some(s),
            _ => None,
        }
    }

    /// Batched override: all pragma_foreign_key_list probes go out in
    /// ONE pipeline (one HTTP round-trip) instead of N.
    fn child_links(&self, parent: &str) -> Vec<(String, String, String)> {
        let tables = match self.tables() {
            Ok(t) => t,
            Err(_) => return Vec::new(),
        };
        let stmts: Vec<(String, Vec<Json>)> = tables
            .iter()
            .filter(|t| !t.name.eq_ignore_ascii_case(parent))
            .map(|t| {
                (
                    format!(
                        "SELECT \"table\", \"from\", \"to\" FROM pragma_foreign_key_list({})",
                        Self::str_lit(&t.name)
                    ),
                    Vec::new(),
                )
            })
            .collect();
        if stmts.is_empty() {
            return Vec::new();
        }
        let refs: Vec<(&str, Vec<Json>)> =
            stmts.iter().map(|(s, a)| (s.as_str(), a.clone())).collect();
        let outs = match self.pipeline(&refs) {
            Ok(o) => o,
            Err(_) => {
                // A pipeline aborts on its FIRST error — one quirky
                // table must not empty every relation pane. Isolate
                // per table instead (the old default's behavior).
                let mut out = Vec::new();
                for t in tables
                    .iter()
                    .filter(|t| !t.name.eq_ignore_ascii_case(parent))
                {
                    if let Ok(o) = self.one(
                        &format!(
                            "SELECT \"table\", \"from\", \"to\" FROM pragma_foreign_key_list({})",
                            Self::str_lit(&t.name)
                        ),
                        vec![],
                    ) {
                        for row in &o.rows {
                            let (PValue::Text(to_table), PValue::Text(from_col)) =
                                (&row[0], &row[1])
                            else {
                                continue;
                            };
                            if !to_table.eq_ignore_ascii_case(parent) {
                                continue;
                            }
                            let to_col = match &row[2] {
                                PValue::Text(c) => c.clone(),
                                _ => String::new(),
                            };
                            out.push((t.name.clone(), from_col.clone(), to_col));
                        }
                    }
                }
                return out;
            }
        };
        let mut out = Vec::new();
        for (t, stmt_out) in tables
            .iter()
            .filter(|t| !t.name.eq_ignore_ascii_case(parent))
            .zip(outs.iter())
        {
            for row in &stmt_out.rows {
                let (PValue::Text(to_table), PValue::Text(from_col)) = (&row[0], &row[1]) else {
                    continue;
                };
                if !to_table.eq_ignore_ascii_case(parent) {
                    continue;
                }
                let to_col = match &row[2] {
                    PValue::Text(c) => c.clone(),
                    _ => String::new(),
                };
                out.push((t.name.clone(), from_col.clone(), to_col));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lost_output_reply_reports_unknown_transaction_and_is_not_replayed() {
        let server = crate::test_support::HranaFixture::new(false);
        let db = RemoteDb::open(&server.url).unwrap();
        db.execute("CREATE TABLE lost_output(n); BEGIN; INSERT INTO lost_output VALUES(1)")
            .unwrap();
        server
            .drop_next
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let error = db
            .stream_query("SELECT * FROM lost_output", Box::new(|_| Ok(())))
            .unwrap_err();
        assert!(error.contains("transaction outcome unknown"), "{error}");
        let before = server.executed.lock().unwrap().len();
        assert!(db
            .query("VALUES(42)")
            .unwrap_err()
            .contains("outcome unknown"));
        assert_eq!(
            server.executed.lock().unwrap().len(),
            before,
            "an uncertain transaction was retried"
        );
    }

    #[test]
    fn remote_complete_outputs_include_rows_beyond_the_preview_cap() {
        let server = crate::test_support::HranaFixture::new(false);
        let db = RemoteDb::open(&server.url).unwrap();
        crate::test_support::assert_complete_output(&db);
    }

    #[test]
    fn unfinished_remote_output_does_not_replace_a_completed_file() {
        let server = crate::test_support::HranaFixture::new(false);
        let db = RemoteDb::open(&server.url).unwrap();
        db.execute("CREATE TABLE t(n); INSERT INTO t VALUES(1),(2)")
            .unwrap();
        let file = crate::test_support::TestDb::new();
        std::fs::write(file.path(), "previous output").unwrap();
        server
            .truncate_output
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(crate::csv_io::export_csv(&db, "t", file.path())
            .unwrap_err()
            .contains("incomplete"));
        assert_eq!(
            std::fs::read_to_string(file.path()).unwrap(),
            "previous output"
        );
        db.begin_transaction().unwrap();
        db.execute("INSERT INTO t VALUES(3)").unwrap();
        assert_eq!(db.query_complete("SELECT * FROM t").unwrap().rows.len(), 3);
        assert!(db.query_complete("SELECT * FROM absent").is_err());
        db.rollback_transaction().unwrap();
        assert_eq!(db.count("t").unwrap(), 2);
        let readonly = RemoteDb::open_with_mode(&server.url, true).unwrap();
        assert_eq!(
            readonly
                .query_complete("SELECT * FROM t")
                .unwrap()
                .rows
                .len(),
            2
        );
    }

    #[test]
    fn remote_transactions_and_saved_scripts_round_trip() {
        let server = crate::test_support::HranaFixture::new(false);
        let db = RemoteDb::open(&server.url).unwrap();
        crate::test_support::assert_transaction_and_script_workflows(&db);
    }

    #[test]
    fn expired_transaction_skips_new_writes() {
        let server = crate::test_support::HranaFixture::new(false);
        let db = RemoteDb::open(&server.url).unwrap();
        db.execute("CREATE TABLE t(id INTEGER)").unwrap();
        db.begin_transaction().unwrap();
        db.insert_row("t", &[("id".into(), PValue::Int(1))])
            .unwrap();
        server
            .expire_next
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let error = db
            .insert_row("t", &[("id".into(), PValue::Int(2))])
            .unwrap_err();
        assert!(error.contains("transaction ended"), "{error}");
        assert_eq!(db.count("t").unwrap(), 0);
    }

    #[test]
    fn lost_transaction_response_never_reconnects_and_continues_writing() {
        let server = crate::test_support::HranaFixture::new(false);
        let db = RemoteDb::open(&server.url).unwrap();
        db.execute("CREATE TABLE t(id INTEGER)").unwrap();
        db.begin_transaction().unwrap();
        server
            .drop_next
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(db
            .insert_row("t", &[("id".into(), PValue::Int(1))])
            .is_err());
        let count = server.executed.lock().unwrap().len();
        assert!(db
            .insert_row("t", &[("id".into(), PValue::Int(2))])
            .unwrap_err()
            .contains("unknown"));
        assert!(db.commit_transaction().is_err());
        assert_eq!(
            server.executed.lock().unwrap().len(),
            count,
            "no automatic reconnect or replay"
        );
    }

    #[test]
    fn dropping_remote_connection_rolls_back_open_transaction() {
        let server = crate::test_support::HranaFixture::new(false);
        let db = RemoteDb::open(&server.url).unwrap();
        db.execute("CREATE TABLE t(id INTEGER)").unwrap();
        db.begin_transaction().unwrap();
        db.insert_row("t", &[("id".into(), PValue::Int(1))])
            .unwrap();
        drop(db);
        let db = RemoteDb::open(&server.url).unwrap();
        assert_eq!(db.count("t").unwrap(), 0);
    }

    #[test]
    fn lost_commit_response_reports_uncertainty_without_replaying() {
        let server = crate::test_support::HranaFixture::new(false);
        let db = RemoteDb::open(&server.url).unwrap();
        db.execute("CREATE TABLE t(id INTEGER)").unwrap();
        db.begin_transaction().unwrap();
        db.insert_row("t", &[("id".into(), PValue::Int(1))])
            .unwrap();
        server
            .drop_next
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(db.commit_transaction().unwrap_err().contains("unknown"));
        assert!(db.execute("INSERT INTO t VALUES(2)").is_err());
        assert_eq!(
            server
                .db
                .connect()
                .query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn remote_schema_changes_preserve_constraints_and_roll_back_failures() {
        let server = crate::test_support::HranaFixture::new(false);
        server
            .db
            .connect()
            .execute_batch(crate::test_support::SCHEMA_SQL)
            .unwrap();
        let db = RemoteDb::open(&server.url).unwrap();
        crate::test_support::assert_native_schema_safety(&db);
    }

    #[test]
    fn remote_schema_changes_check_foreign_keys_before_commit() {
        let server = crate::test_support::HranaFixture::new(false);
        server
            .db
            .connect()
            .execute_batch(
                "PRAGMA foreign_keys=OFF;
            CREATE TABLE p(id INTEGER PRIMARY KEY); CREATE TABLE c(pid INTEGER REFERENCES p(id));
            INSERT INTO c VALUES (7)",
            )
            .unwrap();
        let db = RemoteDb::open(&server.url).unwrap();
        let error = db
            .apply_schema_changes(&["ALTER TABLE c ADD COLUMN note TEXT".into()])
            .unwrap_err();
        assert!(error.contains("foreign key check"), "{error}");
        assert_eq!(db.columns("c").unwrap().len(), 1);
        assert_eq!(db.count("c").unwrap(), 1);
        let log = server.executed.lock().unwrap();
        assert!(log.iter().any(|sql| sql == "ROLLBACK"));
        assert!(!log.iter().any(|sql| sql == "COMMIT"));
    }

    #[test]
    fn readonly_remote_queries_are_selects_on_each_connection() {
        let server = crate::test_support::HranaFixture::new(false);
        server.db.connect().execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT); INSERT INTO t VALUES (1, 'Ada')").unwrap();
        let db = RemoteDb::open_with_mode(&server.url, true).unwrap();
        assert!(db.readonly());
        assert_eq!(db.tables().unwrap().len(), 1);
        assert_eq!(db.columns("t").unwrap().len(), 2);
        assert_eq!(db.open_window("t", 0, 10).unwrap().1, 1);
        assert_eq!(
            db.query("WITH x AS (SELECT name FROM t) SELECT * FROM x")
                .unwrap()
                .rows[0][0],
            PValue::Text("Ada".into())
        );
        for sql in [
            "WITH x AS (SELECT 1) INSERT INTO t(name) SELECT 'bad' FROM x RETURNING * -- limit",
            "UPDATE t SET name='bad' RETURNING * -- limit",
            "DELETE FROM t RETURNING * -- limit",
            "PRAGMA query_only=OFF -- limit",
            "PRAGMA user_version=99 -- limit",
            "ATTACH ':memory:' AS extra -- limit",
            "SELECT 1); DELETE FROM t; SELECT (1 -- limit",
        ] {
            assert!(db.query(sql).is_err(), "allowed {sql}");
        }
        assert!(db.execute("DELETE FROM t").is_err());
        assert!(db
            .update_row("t", 1, &[("name".into(), PValue::Null)])
            .is_err());
        assert!(db.insert_row("t", &[]).is_err());
        assert!(db.delete_row("t", 1).is_err());
        assert!(db
            .apply_schema_changes(&["ALTER TABLE t ADD COLUMN extra TEXT".into()])
            .is_err());
        crate::store::pref_set(&db, "theme", "amber");
        assert_eq!(
            db.query("SELECT name FROM t").unwrap().rows,
            vec![vec![PValue::Text("Ada".into())]]
        );
        assert_eq!(db.tables().unwrap().len(), 1);
    }

    #[test]
    fn readonly_remote_does_not_retry_rejected_reads_as_raw_sql() {
        let server = crate::test_support::HranaFixture::new(true);
        assert!(RemoteDb::open_with_mode(&server.url, true).is_err());
        assert_eq!(
            *server.executed.lock().unwrap(),
            ["SELECT * FROM (\nSELECT 1\n)"]
        );
    }

    #[test]
    fn hrana_value_round_trip() {
        for v in [
            PValue::Null,
            PValue::Int(i64::MAX),
            PValue::Int(-42),
            PValue::Real(1.5),
            PValue::Text("héllo ; -- '".into()),
            PValue::Blob(vec![0, 1, 2, 255]),
        ] {
            assert_eq!(decode(&encode(&v)).unwrap(), v, "{v:?}");
        }
    }

    #[test]
    fn hrana_decodes_unpadded_blob_base64() {
        let j = json!({"type": "blob", "base64": "AAEC"}); // no padding
        assert_eq!(decode(&j).unwrap(), PValue::Blob(vec![0, 1, 2]));
    }

    /// Real end-to-end against PHOSPHOR_SQLD_BIN or ~/.cargo/bin/sqld.
    /// Optional locally; tools/test_sqld.py supplies a verified release in CI.
    #[test]
    fn against_real_sqld_when_available() {
        let home = std::env::var("HOME").unwrap_or_default();
        let sqld = std::env::var("PHOSPHOR_SQLD_BIN")
            .unwrap_or_else(|_| format!("{home}/.cargo/bin/sqld"));
        if !std::path::Path::new(&sqld).exists() {
            eprintln!("skipping: sqld not installed");
            return;
        }
        let server = crate::test_support::SqldFixture::new(&sqld);
        let url = server.url.as_str();
        let db = RemoteDb::open(url).unwrap();

        let run = || -> DbResult<()> {
            crate::test_support::assert_transaction_and_script_workflows(&db);
            db.execute(crate::test_support::SCHEMA_SQL)?;
            crate::test_support::assert_native_schema_safety(&db);
            crate::test_support::assert_complete_output(&db);
            db.execute(
                "CREATE TABLE IF NOT EXISTS crew(id INTEGER PRIMARY KEY, name TEXT); \
                 DELETE FROM crew",
            )?;
            db.execute("INSERT INTO crew(name) VALUES ('ada'), ('grace')")?;
            assert_eq!(db.count("crew")?, 2);
            assert!(db.has_rowid("crew"));
            let page = db.page("crew", 0, 10)?;
            assert_eq!(page.rows.len(), 2);
            let rowids = page.rowids.clone().unwrap();
            db.update_row(
                "crew",
                rowids[0],
                &[("name".into(), PValue::Text("ada lovelace".into()))],
            )?;
            let q = db.query("SELECT name FROM crew ORDER BY id")?;
            assert_eq!(q.rows[0][0], PValue::Text("ada lovelace".into()));
            let tables = db.tables()?;
            assert!(tables.iter().any(|t| t.name == "crew"));
            let readonly = RemoteDb::open_with_mode(url, true)?;
            assert_eq!(readonly.count("crew")?, 2);
            assert_eq!(readonly.query_complete("SELECT * FROM crew")?.rows.len(), 2);
            assert!(readonly
                .query("DELETE FROM crew RETURNING * -- limit")
                .is_err());
            assert_eq!(readonly.count("crew")?, 2);
            Ok(())
        };
        run().unwrap();
        crate::app::assert_related_record_workflow(Box::new(RemoteDb::open(url).unwrap()));
        crate::app::assert_builder_workflow(|| Box::new(RemoteDb::open(url).unwrap()));
        crate::app::assert_picker_workflow(Box::new(RemoteDb::open(url).unwrap()));
        crate::app::assert_query_preview_workflow(Box::new(RemoteDb::open(url).unwrap()));
        crate::app::assert_first_entry_workflow(Box::new(RemoteDb::open(url).unwrap()));
        crate::test_support::assert_cancelled_read_keeps_transaction(&RemoteDb::open(url).unwrap());
    }
}
