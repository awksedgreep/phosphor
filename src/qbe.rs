//! Query By Example — the 1988 low-code, done honestly: fill in a grid,
//! phosphor writes the SQL and SHOWS it to you (DESIGN.md phase 4).

use crate::db::{DbLink, DbResult};
use crate::store;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sort {
    None,
    Asc,
    Desc,
}

impl Sort {
    pub fn cycle(self) -> Sort {
        match self {
            Sort::None => Sort::Asc,
            Sort::Asc => Sort::Desc,
            Sort::Desc => Sort::None,
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Sort::None => " ",
            Sort::Asc => "▲",
            Sort::Desc => "▼",
        }
    }
}

#[derive(Debug, Clone)]
pub struct QbeCol {
    pub name: String,
    pub show: bool,
    /// A filter fragment: `> 100`, `like '%ada%'`, `between 1 and 9`,
    /// or a bare value (auto `=`, auto-quoted unless numeric).
    pub filter: String,
    pub sort: Sort,
}

#[derive(Debug, Clone)]
pub struct QbeSpec {
    pub table: String,
    pub cols: Vec<QbeCol>,
}

fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Does the fragment already start with a SQL operator/keyword?
fn starts_with_op(f: &str) -> bool {
    let lower = f.to_ascii_lowercase();
    ["=", "!", "<", ">"]
        .iter()
        .any(|op| lower.starts_with(op))
        || ["like ", "not ", "in ", "in(", "between ", "is ", "glob "]
            .iter()
            .any(|kw| lower.starts_with(kw))
}

impl QbeSpec {
    pub fn new(db: &dyn DbLink, table: &str) -> DbResult<QbeSpec> {
        let cols = db
            .columns(table)?
            .into_iter()
            .map(|c| QbeCol {
                name: c.name,
                show: true,
                filter: String::new(),
                sort: Sort::None,
            })
            .collect();
        Ok(QbeSpec {
            table: table.to_owned(),
            cols,
        })
    }

    /// The generated SQL — always visible in the designer, because the
    /// point is to teach, not to hide.
    pub fn sql(&self) -> String {
        let shown: Vec<String> = self
            .cols
            .iter()
            .filter(|c| c.show)
            .map(|c| quote_ident(&c.name))
            .collect();
        let select = if shown.is_empty() || shown.len() == self.cols.len() {
            "*".to_owned()
        } else {
            shown.join(", ")
        };
        let mut sql = format!("SELECT {select} FROM {}", quote_ident(&self.table));

        let preds: Vec<String> = self
            .cols
            .iter()
            .filter(|c| !c.filter.trim().is_empty())
            .map(|c| {
                let f = c.filter.trim();
                let col = quote_ident(&c.name);
                if starts_with_op(f) {
                    format!("{col} {f}")
                } else if f.parse::<f64>().is_ok() {
                    format!("{col} = {f}")
                } else {
                    format!("{col} = {}", store::q(f))
                }
            })
            .collect();
        if !preds.is_empty() {
            sql.push_str(&format!(" WHERE {}", preds.join(" AND ")));
        }

        let orders: Vec<String> = self
            .cols
            .iter()
            .filter(|c| c.sort != Sort::None)
            .map(|c| {
                format!(
                    "{}{}",
                    quote_ident(&c.name),
                    if c.sort == Sort::Desc { " DESC" } else { "" }
                )
            })
            .collect();
        if !orders.is_empty() {
            sql.push_str(&format!(" ORDER BY {}", orders.join(", ")));
        }
        sql
    }

    pub fn save(&self, db: &dyn DbLink, name: &str) -> DbResult<()> {
        store::upsert(
            db,
            "_phosphor_queries",
            "name",
            name,
            &[
                ("table_ref", self.table.clone()),
                ("qbe_json", self.to_json()),
                ("sql_text", self.sql()),
            ],
        )
    }

    pub fn saved_sql(db: &dyn DbLink, name: &str) -> Option<String> {
        store::lookup(db, "_phosphor_queries", "name", name, &["sql_text"])
            .and_then(|r| r.into_iter().next())
            .filter(|s| !s.is_empty())
    }

    fn to_json(&self) -> String {
        let cols: Vec<serde_json::Value> = self
            .cols
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "show": c.show,
                    "filter": c.filter,
                    "sort": match c.sort { Sort::None => "", Sort::Asc => "asc", Sort::Desc => "desc" },
                })
            })
            .collect();
        serde_json::json!({"table": self.table, "cols": cols, "v": 1}).to_string()
    }
}

/// Designer state: cursor over the column rows, optional filter editor.
pub struct QbeState {
    pub spec: QbeSpec,
    pub cursor: usize,
    /// Some(buffer) while typing a filter; the name-save prompt reuses
    /// the same buffer with `naming = true`.
    pub editing: Option<String>,
    pub naming: bool,
}

impl QbeState {
    pub fn new(spec: QbeSpec) -> Self {
        QbeState {
            spec,
            cursor: 0,
            editing: None,
            naming: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::EmbeddedDb;

    fn spec() -> QbeSpec {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE o(id INTEGER PRIMARY KEY, who TEXT, amt REAL)")
            .unwrap();
        QbeSpec::new(&db, "o").unwrap()
    }

    #[test]
    fn bare_spec_is_select_star() {
        assert_eq!(spec().sql(), r#"SELECT * FROM "o""#);
    }

    #[test]
    fn filters_sorts_and_projection() {
        let mut s = spec();
        s.cols[1].filter = "ada".into(); // bare text → = 'ada'
        s.cols[2].filter = "> 100".into(); // operator passes through
        s.cols[2].sort = Sort::Desc;
        s.cols[0].show = false;
        assert_eq!(
            s.sql(),
            r#"SELECT "who", "amt" FROM "o" WHERE "who" = 'ada' AND "amt" > 100 ORDER BY "amt" DESC"#
        );
    }

    #[test]
    fn bare_numeric_filter_is_unquoted() {
        let mut s = spec();
        s.cols[0].filter = "42".into();
        assert!(s.sql().contains(r#""id" = 42"#));
    }

    #[test]
    fn save_and_reload() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE o(id INTEGER PRIMARY KEY, who TEXT)")
            .unwrap();
        let mut s = QbeSpec::new(&db, "o").unwrap();
        s.cols[1].filter = "like 'a%'".into();
        s.save(&db, "a-people").unwrap();
        let sql = QbeSpec::saved_sql(&db, "a-people").unwrap();
        assert!(sql.contains("like 'a%'"));
        assert_eq!(store::names(&db, "_phosphor_queries", "name"), ["a-people"]);
    }
}
