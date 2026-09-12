//! Query By Example — the 1988 low-code, done honestly: fill in a grid,
//! phosphor writes the SQL and SHOWS it to you (DESIGN.md phase 4).

use crate::db::{ColumnInfo, DbLink, DbResult};
use crate::store;

/// Every FK-driven join reachable from `table`: children that point at
/// it, and parents it points at. The parent's primary key stands in for
/// a bare `REFERENCES parent`.
fn discover_relations(db: &dyn DbLink, table: &str, cols: &[ColumnInfo]) -> Vec<Join> {
    let pk_of = |cols: &[ColumnInfo]| cols.iter().find(|c| c.pk).map(|c| c.name.clone());
    let base_pk = pk_of(cols).unwrap_or_else(|| "rowid".into());
    let mut out = Vec::new();
    // base is the parent: the child table joins on its FK column.
    for (child, child_col, parent_col) in db.child_links(table) {
        let right = if parent_col.is_empty() {
            base_pk.clone()
        } else {
            parent_col
        };
        out.push(Join {
            table: child,
            left: right,
            right: child_col,
        });
    }
    // base is the child: it joins to each declared parent.
    for (from_col, to_table, to_col) in db.outgoing_fks(table) {
        let right = to_col.or_else(|| db.columns(&to_table).ok().and_then(|c| pk_of(&c)));
        out.push(Join {
            table: to_table,
            left: from_col,
            right: right.unwrap_or_else(|| "rowid".into()),
        });
    }
    out
}

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

/// One FK-driven join available to a QBE session: `base.left` matches
/// `table.right`. `left` is always a column of the QBE's base table.
#[derive(Debug, Clone, PartialEq)]
pub struct Join {
    pub table: String,
    pub left: String,
    pub right: String,
}

#[derive(Debug, Clone)]
pub struct QbeSpec {
    pub table: String,
    pub cols: Vec<QbeCol>,
    /// An optional FK join, cycled in the designer (`J`).
    pub join: Option<Join>,
    /// Optional GROUP BY column (`g`): turns the projection into an
    /// aggregate (`col, count(*) AS n`) so QBE can teach grouping too.
    pub group_by: Option<String>,
    /// Every join the declared foreign keys make reachable.
    pub relations: Vec<Join>,
}

fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Does the fragment already start with a SQL operator/keyword?
/// Allocation-free: single-char ops by byte, keywords by ASCII-folded
/// prefix compare (no to_lowercase temporary per filter per frame).
fn starts_with_op(f: &str) -> bool {
    let b = f.as_bytes();
    if b.is_empty() {
        return false;
    }
    if matches!(b[0], b'=' | b'!' | b'<' | b'>') {
        return true;
    }
    for kw in ["like ", "not ", "in ", "in(", "between ", "is ", "glob "] {
        let kb = kw.as_bytes();
        if b.len() >= kb.len() && b[..kb.len()].eq_ignore_ascii_case(kb) {
            return true;
        }
    }
    false
}

impl QbeSpec {
    pub fn new(db: &dyn DbLink, table: &str) -> DbResult<QbeSpec> {
        let info = db.columns(table)?;
        let relations = discover_relations(db, table, &info);
        let cols = info
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
            join: None,
            group_by: None,
            relations,
        })
    }

    /// A column reference qualified with the base table, needed once a
    /// JOIN can make bare names ambiguous.
    fn base_col(&self, name: &str) -> String {
        format!("{}.{}", quote_ident(&self.table), quote_ident(name))
    }

    /// The generated SQL — always visible in the designer, because the
    /// point is to teach, not to hide.
    pub fn sql(&self) -> String {
        let q = quote_ident(&self.table);
        let from = match &self.join {
            Some(j) => format!(
                "{q} JOIN {} ON {}.{} = {}.{}",
                quote_ident(&j.table),
                q,
                quote_ident(&j.left),
                quote_ident(&j.table),
                quote_ident(&j.right)
            ),
            None => q,
        };

        // Projection. GROUP BY collapses to the grouped column + a count
        // (the classic teaching shape); a JOIN qualifies every base
        // column so the result is unambiguous.
        let select = if let Some(g) = &self.group_by {
            let gcol = if self.join.is_some() {
                self.base_col(g)
            } else {
                quote_ident(g)
            };
            format!("{gcol}, count(*) AS n")
        } else {
            let shown: Vec<&QbeCol> = self.cols.iter().filter(|c| c.show).collect();
            let project_all = shown.is_empty() || shown.len() == self.cols.len();
            if self.join.is_some() {
                let list: Vec<String> = if project_all {
                    self.cols.iter().map(|c| self.base_col(&c.name)).collect()
                } else {
                    shown.iter().map(|c| self.base_col(&c.name)).collect()
                };
                list.join(", ")
            } else if project_all {
                "*".to_owned()
            } else {
                shown
                    .iter()
                    .map(|c| quote_ident(&c.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        };
        let mut sql = format!("SELECT {select} FROM {from}");

        // Filters always target the base table's columns; qualifying is
        // only needed (and only shown) once a JOIN makes names ambiguous.
        let colref = |name: &str| {
            if self.join.is_some() {
                self.base_col(name)
            } else {
                quote_ident(name)
            }
        };
        let preds: Vec<String> = self
            .cols
            .iter()
            .filter(|c| !c.filter.trim().is_empty())
            .map(|c| {
                let f = c.filter.trim();
                let col = colref(&c.name);
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

        if let Some(g) = &self.group_by {
            sql.push_str(&format!(" GROUP BY {}", colref(g)));
        }

        let orders: Vec<String> = self
            .cols
            .iter()
            .filter(|c| c.sort != Sort::None)
            .map(|c| {
                format!(
                    "{}{}",
                    colref(&c.name),
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
        let join = self
            .join
            .as_ref()
            .map(|j| serde_json::json!({"table": j.table, "left": j.left, "right": j.right}));
        serde_json::json!({
            "table": self.table, "cols": cols,
            "join": join, "group_by": self.group_by, "v": 1,
        })
        .to_string()
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

    /// #16: declared FKs become cycleable joins in both directions, and
    /// a projection is qualified once a JOIN makes names ambiguous.
    #[test]
    fn fk_joins_both_directions() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE orders(id INTEGER PRIMARY KEY,
                 customer_id INTEGER REFERENCES customers(id), amount REAL);",
        )
        .unwrap();
        // base = child: join up to the parent.
        let mut s = QbeSpec::new(&db, "orders").unwrap();
        assert_eq!(
            s.relations,
            [Join {
                table: "customers".into(),
                left: "customer_id".into(),
                right: "id".into()
            }]
        );
        s.join = Some(s.relations[0].clone());
        let sql = s.sql();
        assert!(
            sql.contains(
                r#"FROM "orders" JOIN "customers" ON "orders"."customer_id" = "customers"."id""#
            ),
            "{sql}"
        );
        assert!(
            sql.starts_with(
                r#"SELECT "orders"."id", "orders"."customer_id", "orders"."amount" FROM"#
            ),
            "{sql}"
        );
        // base = parent: join down to the child.
        let s2 = QbeSpec::new(&db, "customers").unwrap();
        assert_eq!(
            s2.relations,
            [Join {
                table: "orders".into(),
                left: "id".into(),
                right: "customer_id".into()
            }]
        );
    }

    /// #16: GROUP BY collapses the projection to the key + a count.
    #[test]
    fn group_by_teaches_aggregation() {
        let mut s = spec();
        s.group_by = Some("who".into());
        assert_eq!(
            s.sql(),
            r#"SELECT "who", count(*) AS n FROM "o" GROUP BY "who""#
        );
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
