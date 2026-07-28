//! RemoteDb — the sqld backend: Hrana over HTTP (`POST /v3/pipeline`),
//! same `DbLink` trait as the embedded file backend (DESIGN.md phase 2).
//!
//! Wire facts (verified against sqld 0.24.x in the timeless-libsql
//! docs work): integers are JSON *strings* to preserve 64-bit
//! precision; blobs are base64; each pipeline without a baton lands on
//! a fresh pooled connection; end pipelines with a close request.
//!
//! PHOSPHOR_TOKEN adds `Authorization: Bearer …` — the only difference
//! between self-hosted sqld and Turso-hosted URLs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value as Json};

use crate::db::{
    ColumnInfo, DbLink, DbResult, Page, PValue, QueryResult, TableInfo, QUERY_CAP,
};

pub struct RemoteDb {
    agent: ureq::Agent,
    pipeline_url: String,
    display: String,
    token: Option<String>,
    rowid_cache: RefCell<HashMap<String, bool>>,
}

struct StmtOut {
    cols: Vec<String>,
    rows: Vec<Vec<PValue>>,
    affected: i64,
    last_rowid: Option<i64>,
}

impl RemoteDb {
    pub fn open(url: &str) -> DbResult<Self> {
        let base = url.trim_end_matches('/');
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(15))
            .build();
        let db = RemoteDb {
            agent,
            pipeline_url: format!("{base}/v3/pipeline"),
            display: base.to_owned(),
            token: std::env::var("PHOSPHOR_TOKEN").ok().filter(|t| !t.is_empty()),
            rowid_cache: RefCell::new(HashMap::new()),
        };
        // Fail at open, not at first keystroke.
        db.pipeline(&[("SELECT 1", vec![])])?;
        Ok(db)
    }

    fn pipeline(&self, stmts: &[(&str, Vec<Json>)]) -> DbResult<Vec<StmtOut>> {
        let mut requests: Vec<Json> = stmts
            .iter()
            .map(|(sql, args)| {
                json!({"type": "execute", "stmt": {"sql": sql, "args": args}})
            })
            .collect();
        requests.push(json!({"type": "close"}));

        let mut req = self.agent.post(&self.pipeline_url);
        if let Some(t) = &self.token {
            req = req.set("Authorization", &format!("Bearer {t}"));
        }
        let body: Json = req
            .send_json(json!({"requests": requests}))
            .map_err(|e| match e {
                ureq::Error::Status(code, resp) => format!(
                    "sqld HTTP {code}: {}",
                    resp.into_string().unwrap_or_default()
                ),
                other => format!("sqld unreachable: {other}"),
            })?
            .into_json()
            .map_err(|e| format!("sqld response was not JSON: {e}"))?;

        let results = body["results"]
            .as_array()
            .ok_or("sqld response missing results[]")?;
        let mut out = Vec::new();
        for r in results {
            match r["type"].as_str() {
                Some("ok") => {
                    let resp = &r["response"];
                    if resp["type"] == "execute" {
                        out.push(decode_result(&resp["result"])?);
                    } // close acks are skipped
                }
                Some("error") => {
                    return Err(r["error"]["message"]
                        .as_str()
                        .unwrap_or("unknown sqld error")
                        .to_owned());
                }
                _ => return Err("sqld result with unknown type".into()),
            }
        }
        Ok(out)
    }

    fn one(&self, sql: &str, args: Vec<Json>) -> DbResult<StmtOut> {
        let mut v = self.pipeline(&[(sql, args)])?;
        v.pop().ok_or_else(|| "sqld returned no result".into())
    }

    fn quote(ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }
}

fn encode(v: &PValue) -> Json {
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

fn decode(v: &Json) -> DbResult<PValue> {
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
                .or_else(|_| {
                    base64::engine::general_purpose::STANDARD_NO_PAD.decode(b64)
                })
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
        for row in json_rows {
            let cells = row.as_array().ok_or("row is not an array")?;
            rows.push(cells.iter().map(decode).collect::<DbResult<Vec<_>>>()?);
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
        let out = self.one(&format!("PRAGMA table_info({})", Self::quote(table)), vec![])?;
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
            })
            .collect())
    }

    fn count(&self, table: &str) -> DbResult<i64> {
        let out = self.one(&format!("SELECT count(*) FROM {}", Self::quote(table)), vec![])?;
        match out.rows.first().and_then(|r| r.first()) {
            Some(PValue::Int(n)) => Ok(*n),
            _ => Err("count(*) did not return an integer".into()),
        }
    }

    fn has_rowid(&self, table: &str) -> bool {
        if let Some(&known) = self.rowid_cache.borrow().get(table) {
            return known;
        }
        let ok = self
            .one(
                &format!("SELECT rowid FROM {} LIMIT 0", Self::quote(table)),
                vec![],
            )
            .is_ok();
        self.rowid_cache.borrow_mut().insert(table.to_owned(), ok);
        ok
    }

    fn page(&self, table: &str, offset: i64, limit: i64) -> DbResult<Page> {
        let q = Self::quote(table);
        if self.has_rowid(table) {
            let out = self.one(
                &format!("SELECT rowid, * FROM {q} LIMIT {limit} OFFSET {offset}"),
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

    fn query(&self, sql: &str) -> DbResult<QueryResult> {
        let start = Instant::now();
        let out = self.one(sql, vec![])?;
        let truncated = out.rows.len() > QUERY_CAP;
        let mut rows = out.rows;
        rows.truncate(QUERY_CAP);
        Ok(QueryResult {
            columns: out.cols,
            rows,
            truncated,
            elapsed: start.elapsed(),
        })
    }

    fn execute(&self, sql: &str) -> DbResult<(i64, Duration)> {
        let start = Instant::now();
        // Hrana takes one statement per execute request; split batches
        // naively on ';' (string literals with ';' will mis-split — the
        // dot prompt's multi-statement case is DDL, where that's rare).
        let stmts: Vec<&str> = sql
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if stmts.is_empty() {
            return Ok((0, start.elapsed()));
        }
        let calls: Vec<(&str, Vec<Json>)> =
            stmts.iter().map(|s| (*s, Vec::new())).collect();
        let outs = self.pipeline(&calls)?;
        let n = if outs.len() == 1 {
            outs[0].affected
        } else {
            -1
        };
        // Any statement may have changed schema/rowid-ness.
        self.rowid_cache.borrow_mut().clear();
        Ok((n, start.elapsed()))
    }

    fn update_row(
        &self,
        table: &str,
        rowid: i64,
        changes: &[(String, PValue)],
    ) -> DbResult<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let sets: Vec<String> = changes
            .iter()
            .enumerate()
            .map(|(i, (col, _))| format!("{} = ?{}", Self::quote(col), i + 1))
            .collect();
        let sql = format!(
            "UPDATE {} SET {} WHERE rowid = ?{}",
            Self::quote(table),
            sets.join(", "),
            changes.len() + 1
        );
        let mut args: Vec<Json> = changes.iter().map(|(_, v)| encode(v)).collect();
        args.push(encode(&PValue::Int(rowid)));
        let out = self.one(&sql, args)?;
        if out.affected == 1 {
            Ok(())
        } else {
            Err(format!("expected to update 1 row, updated {}", out.affected))
        }
    }

    fn insert_row(&self, table: &str, changes: &[(String, PValue)]) -> DbResult<i64> {
        let sql = if changes.is_empty() {
            format!("INSERT INTO {} DEFAULT VALUES", Self::quote(table))
        } else {
            let cols: Vec<String> = changes.iter().map(|(c, _)| Self::quote(c)).collect();
            let marks: Vec<String> =
                (1..=changes.len()).map(|i| format!("?{i}")).collect();
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
        let out = self.one(
            &format!("DELETE FROM {} WHERE rowid = ?1", Self::quote(table)),
            vec![encode(&PValue::Int(rowid))],
        )?;
        if out.affected == 1 {
            Ok(())
        } else {
            Err(format!("expected to delete 1 row, deleted {}", out.affected))
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
        let PValue::Text(view) = &view[0] else { return None };
        let out = self
            .one(&format!("SELECT status FROM {} LIMIT 1", Self::quote(view)), vec![])
            .ok()?;
        match out.rows.into_iter().next()?.into_iter().next()? {
            PValue::Text(s) => Some(s),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Real end-to-end against a spawned sqld, if one is installed
    /// (~/.cargo/bin/sqld). Skips silently otherwise so `cargo test`
    /// works on any machine.
    #[test]
    fn against_real_sqld_when_available() {
        let home = std::env::var("HOME").unwrap_or_default();
        let sqld = format!("{home}/.cargo/bin/sqld");
        if !std::path::Path::new(&sqld).exists() {
            eprintln!("skipping: sqld not installed");
            return;
        }
        let dir = std::env::temp_dir().join(format!("phosphor-sqld-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut child = std::process::Command::new(&sqld)
            .current_dir(&dir)
            .args(["--db-path", "t.sqld", "--http-listen-addr", "127.0.0.1:8871"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();

        let url = "http://127.0.0.1:8871";
        let mut db = None;
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(100));
            if let Ok(d) = RemoteDb::open(url) {
                db = Some(d);
                break;
            }
        }
        let db = db.expect("sqld did not come up");

        let run = || -> DbResult<()> {
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
            Ok(())
        };
        let result = run();
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
        result.unwrap();
    }
}
