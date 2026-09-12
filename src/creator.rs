//! The TABLE DESIGNER — dBASE's CREATE structure screen, reborn.
//! Define fields as rows (name · type · pk · null · unique · default),
//! watch the CREATE TABLE write itself underneath (the QBE philosophy:
//! teach the SQL, never hide it), F2 builds the table. The dot prompt's
//! raw `CREATE TABLE ...` remains first-class; this is the screen for
//! people who think in fields, not clauses.

use crate::db::{DbLink, DbResult};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FType {
    Integer,
    Text,
    Real,
    Blob,
    Numeric,
}

impl FType {
    pub fn cycle(self) -> FType {
        match self {
            FType::Integer => FType::Text,
            FType::Text => FType::Real,
            FType::Real => FType::Blob,
            FType::Blob => FType::Numeric,
            FType::Numeric => FType::Integer,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FType::Integer => "INTEGER",
            FType::Text => "TEXT",
            FType::Real => "REAL",
            FType::Blob => "BLOB",
            FType::Numeric => "NUMERIC",
        }
    }

    /// The closest designer type for a declared column type (same
    /// affinity rules as PValue::parse).
    pub fn of_decl(decl: &str) -> FType {
        let d = decl.to_ascii_uppercase();
        if d.contains("INT") {
            FType::Integer
        } else if d.contains("REAL") || d.contains("FLOA") || d.contains("DOUB") {
            FType::Real
        } else if d.contains("BLOB") {
            FType::Blob
        } else if d.contains("NUM") || d.contains("DEC") {
            FType::Numeric
        } else {
            FType::Text
        }
    }
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub ftype: FType,
    pub pk: bool,
    pub notnull: bool,
    pub unique: bool,
    pub default: String,
    /// Raw REFERENCES target: "customers" or "customers(id)". A bare
    /// table name points at that table's PRIMARY KEY (SQLite rules).
    pub references: String,
}

impl FieldDef {
    fn new(name: &str, ftype: FType) -> FieldDef {
        FieldDef {
            name: name.to_owned(),
            ftype,
            pk: false,
            notnull: false,
            unique: false,
            default: String::new(),
            references: String::new(),
        }
    }
}

/// The SQL-expr text of a field's DEFAULT, exactly as the schema
/// stores it (pragma dflt_value shape) — for round-trip comparison.
fn raw_default_of(f: &FieldDef) -> String {
    if f.default.trim().is_empty() {
        String::new()
    } else {
        default_sql(&f.default)
    }
}

/// The normalized FK target of a field: "table" or "table(col)" —
/// matches how EditorSchema.fks records outgoing targets.
fn fk_of(f: &FieldDef) -> Option<(String, Option<String>)> {
    let raw = f.references.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.split_once('(') {
        Some((t, rest)) => Some((t.trim().to_owned(), Some(rest.trim_end_matches(')').trim().to_owned()))),
        None => Some((raw.to_owned(), None)),
    }
}

pub fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Render a DEFAULT clause value: numbers, NULL, CURRENT_* and
/// parenthesized expressions pass through; everything else is quoted.
fn default_sql(raw: &str) -> String {
    let v = raw.trim();
    // No uppercased temporary per field per frame: exact and prefix
    // comparisons fold ASCII case in place.
    if v.parse::<f64>().is_ok()
        || v.eq_ignore_ascii_case("NULL")
        || v.eq_ignore_ascii_case("TRUE")
        || v.eq_ignore_ascii_case("FALSE")
        || (v.len() >= 8 && v.as_bytes()[..8].eq_ignore_ascii_case(b"CURRENT_"))
        || (v.starts_with('(') && v.ends_with(')'))
    {
        v.to_owned()
    } else {
        format!("'{}'", v.replace('\'', "''"))
    }
}

/// Render a REFERENCES target: quote the table ident (and column, if
/// given as `table(col)`); pass already-quoted input through.
fn references_sql(raw: &str) -> String {
    if raw.starts_with('"') {
        return raw.to_owned();
    }
    match raw.split_once('(') {
        Some((table, rest)) => {
            let col = rest.trim_end_matches(')');
            format!("{}({})", quote_ident(table.trim()), quote_ident(col.trim()))
        }
        None => quote_ident(raw),
    }
}

#[derive(Debug, Clone)]
pub struct TableDraft {
    pub table: String,
    pub fields: Vec<FieldDef>,
}

/// The live schema a TABLE EDITOR session opened with: introspected
/// columns plus this table's outgoing FK targets. F2 diffs the draft
/// against it and produces either ALTER statements or a rebuild.
#[derive(Debug, Clone)]
pub struct EditorSchema {
    pub table: String,
    pub columns: Vec<crate::db::ColumnInfo>,
    /// (from_col, to_table, to_col) — to_col None = references the
    /// parent's pk.
    pub fks: Vec<(String, String, Option<String>)>,
}

impl EditorSchema {
    /// The normalized FK target declared on `column`, if any.
    fn fk_of(&self, column: &str) -> Option<(String, Option<String>)> {
        self.fks
            .iter()
            .find(|(from, _, _)| from.eq_ignore_ascii_case(column))
            .map(|(_, to_table, to_col)| (to_table.clone(), to_col.clone()))
    }
}

impl TableDraft {
    pub fn new(name: &str) -> TableDraft {
        // Start with the field almost every table wants; delete it if not.
        let mut id = FieldDef::new("id", FType::Integer);
        id.pk = true;
        TableDraft {
            table: name.to_owned(),
            fields: vec![id],
        }
    }

    /// A draft preloaded with a table's LIVE schema (TABLE EDITOR):
    /// declared type maps to the closest designer type; pk/notnull/
    /// default/FK round-trip from introspection.
    pub fn from_live(schema: &EditorSchema) -> TableDraft {
        let fields = schema
            .columns
            .iter()
            .map(|c| {
                let mut f = FieldDef::new(&c.name, FType::of_decl(&c.decl_type));
                f.pk = c.pk;
                f.notnull = c.notnull;
                if let Some(d) = &c.dflt_value {
                    f.default = d.clone();
                }
                if let Some((_, to_table, to_col)) = schema
                    .fks
                    .iter()
                    .find(|(from, _, _)| from.eq_ignore_ascii_case(&c.name))
                {
                    f.references = match to_col {
                        Some(col) => format!("{to_table}({col})"),
                        None => to_table.clone(),
                    };
                }
                f
            })
            .collect();
        TableDraft {
            table: schema.table.clone(),
            fields,
        }
    }

    /// The CREATE TABLE this draft will run — always on screen.
    ///
    /// A single INTEGER pk becomes the inline `INTEGER PRIMARY KEY`
    /// (the rowid alias — what EDIT/paging key on); any other pk shape
    /// becomes a table-level PRIMARY KEY(...) clause.
    pub fn sql(&self) -> String {
        self.sql_for(&self.table)
    }

    /// The CREATE TABLE for `name` — same shape as sql(), used by the
    /// rebuild script to create the shadow table before swapping.
    pub fn sql_for(&self, name: &str) -> String {
        let pks: Vec<&FieldDef> = self.fields.iter().filter(|f| f.pk).collect();
        let inline_pk = pks.len() == 1 && pks[0].ftype == FType::Integer;
        let mut cols: Vec<String> = Vec::new();
        for f in &self.fields {
            let mut c = format!("{} {}", quote_ident(&f.name), f.ftype.as_str());
            if f.pk && inline_pk {
                c.push_str(" PRIMARY KEY");
            }
            if f.notnull && !(f.pk && inline_pk) {
                c.push_str(" NOT NULL");
            }
            if f.unique && !f.pk {
                c.push_str(" UNIQUE");
            }
            if !f.default.trim().is_empty() {
                c.push_str(&format!(" DEFAULT {}", default_sql(&f.default)));
            }
            if !f.references.trim().is_empty() {
                c.push_str(&format!(" REFERENCES {}", references_sql(f.references.trim())));
            }
            cols.push(c);
        }
        if !inline_pk && !pks.is_empty() {
            cols.push(format!(
                "PRIMARY KEY ({})",
                pks.iter()
                    .map(|f| quote_ident(&f.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        format!(
            "CREATE TABLE {} (\n  {}\n)",
            quote_ident(name),
            cols.join(",\n  ")
        )
    }

    /// Append a field at the end (tests build drafts this way; the
    /// designer itself always inserts relative to the cursor).
    #[cfg(test)]
    pub fn add_field(&mut self) -> usize {
        self.insert_field(self.fields.len().checked_sub(1))
    }

    /// Insert a fresh TEXT field AFTER position `after` (None → at the
    /// top, i.e. the cursor was on the NAME row) — dBASE-style: the
    /// field you forgot goes where you're standing, not at the end.
    pub fn insert_field(&mut self, after: Option<usize>) -> usize {
        let at = after.map_or(0, |i| i + 1).min(self.fields.len());
        let name = self.fresh_name();
        self.fields.insert(at, FieldDef::new(&name, FType::Text));
        at
    }

    /// First unused `field{n}` placeholder (inserts + deletes can make
    /// plain len()+1 collide; validate() rejects duplicates, so don't).
    fn fresh_name(&self) -> String {
        let mut n = self.fields.len() + 1;
        loop {
            let name = format!("field{n}");
            if !self
                .fields
                .iter()
                .any(|f| f.name.eq_ignore_ascii_case(&name))
            {
                return name;
            }
            n += 1;
        }
    }

    /// The SQL script that turns the schema the editor opened with
    /// into this draft. Cheap structural changes (add / rename / drop
    /// column, rename table) compile to ALTER statements; anything
    /// that SQLite can't ALTER in place — type or constraint changes
    /// on existing columns — compiles to the full REBUILD procedure
    /// (new table, copy, drop, swap) wrapped in a transaction.
    /// Empty result = no changes. `index_sql`: captured CREATE INDEX
    /// statements for the table, re-run after a rebuild.
    pub fn apply_script(&self, schema: &EditorSchema, index_sql: &[String]) -> DbResult<Vec<String>> {
        let orig_table = schema.table.as_str();
        let renamed_table = !self.table.eq_ignore_ascii_case(orig_table);

        // Pair draft fields to original columns by name first, then
        // positionally (a rename keeps a column's place). Unpaired
        // originals = dropped; unpaired fields = added.
        let mut src_of: Vec<Option<usize>> = vec![None; self.fields.len()];
        for (ni, f) in self.fields.iter().enumerate() {
            if let Some(oi) = schema
                .columns
                .iter()
                .position(|o| o.name.eq_ignore_ascii_case(&f.name))
            {
                src_of[ni] = Some(oi);
            }
        }
        let mut unmatched_orig: Vec<usize> = (0..schema.columns.len())
            .filter(|oi| !src_of.contains(&Some(*oi)))
            .collect();
        let unmatched_new: Vec<usize> = (0..self.fields.len())
            .filter(|ni| src_of[*ni].is_none())
            .collect();
        // Positional pairs are renames; leftover unpaired originals
        // are DROPS — but a positional drop that lost its name-mate
        // (rename to an unrelated name) must not silently lose data,
        // so a drop pair only forms when types agree too.
        for &ni in &unmatched_new {
            let f = &self.fields[ni];
            if let Some(pos) = unmatched_orig
                .iter()
                .position(|&oi| {
                    schema.columns[oi].decl_type.eq_ignore_ascii_case(f.ftype.as_str())
                        && schema.columns[oi].pk == f.pk
                })
            {
                let oi = unmatched_orig[pos];
                if !self
                    .fields
                    .iter()
                    .any(|g| g.name.eq_ignore_ascii_case(&schema.columns[oi].name))
                {
                    src_of[ni] = Some(oi);
                    unmatched_orig.retain(|&x| x != oi);
                }
            }
        }

        // What kind of change set is this?
        let mut needs_rebuild = false;
        let mut alter_lines: Vec<String> = Vec::new();
        let mut drop_lines: Vec<String> = Vec::new();
        let mut add_lines: Vec<String> = Vec::new();
        let q_old = quote_ident(orig_table);

        // Table rename is always an ALTER.
        if renamed_table {
            alter_lines.push(format!(
                "ALTER TABLE {q_old} RENAME TO {}",
                quote_ident(&self.table)
            ));
        }
        // Drop columns that no draft field claims.
        for &oi in &unmatched_orig {
            drop_lines.push(format!(
                "ALTER TABLE {q_old} DROP COLUMN {}",
                quote_ident(&schema.columns[oi].name)
            ));
        }
        // Paired columns: renames are ALTERs; constraint/type/default
        // drift forces the rebuild.
        for (ni, f) in self.fields.iter().enumerate() {
            let Some(oi) = src_of[ni] else {
                // Added: ALTER-able unless it needs rebuild-level features.
                if f.pk {
                    return Err(format!(
                        "adding {} as a PRIMARY KEY needs a table rebuild — leave the id field alone instead", f.name
                    ));
                }
                if f.unique {
                    return Err(format!(
                        "adding {} as UNIQUE needs a table rebuild — add it plain, then UNIQUE-index it", f.name
                    ));
                }
                if f.notnull && f.default.trim().is_empty() {
                    return Err(format!(
                        "adding NOT NULL {} needs a DEFAULT for the existing rows", f.name
                    ));
                }
                let mut c = format!("{} {}", quote_ident(&f.name), f.ftype.as_str());
                if f.notnull {
                    c.push_str(" NOT NULL");
                }
                if !f.default.trim().is_empty() {
                    c.push_str(&format!(" DEFAULT {}", default_sql(&f.default)));
                }
                if !f.references.trim().is_empty() {
                    c.push_str(&format!(
                        " REFERENCES {}",
                        references_sql(f.references.trim())
                    ));
                }
                add_lines.push(format!("ALTER TABLE {q_old} ADD COLUMN {c}"));
                continue;
            };
            let orig = &schema.columns[oi];
            if !f.name.eq_ignore_ascii_case(&orig.name) {
                let q_cur = if renamed_table {
                    quote_ident(&self.table)
                } else {
                    q_old.clone()
                };
                alter_lines.push(format!(
                    "ALTER TABLE {q_cur} RENAME COLUMN {} TO {}",
                    quote_ident(&orig.name),
                    quote_ident(&f.name)
                ));
            }
            if f.ftype.as_str() != FType::of_decl(&orig.decl_type).as_str()
                || f.pk != orig.pk
                || f.notnull != orig.notnull
                || raw_default_of(f) != orig.dflt_value.as_deref().unwrap_or("")
                || fk_of(f) != schema.fk_of(&orig.name)
            {
                needs_rebuild = true;
            }
        }

        if needs_rebuild {
            // The SQLite canonical rebuild, wrapped in a transaction.
            // FK enforcement is suspended for the copy (the canonical
            // procedure) and restored at the end — existing parent
            // tables stay referenced and named correctly after the
            // final swap. Lines are semicolon-free; the caller joins.
            const SHADOW: &str = "__phosphor_rebuild";
            let mut lines = vec![
                "PRAGMA foreign_keys = OFF".to_owned(),
                "BEGIN".to_owned(),
            ];
            lines.push(self.sql_for(SHADOW));
            let targets: Vec<(String, String)> = self
                .fields
                .iter()
                .enumerate()
                .filter_map(|(ni, f)| {
                    src_of[ni]
                        .as_ref()
                        .map(|oi| {
                            (
                                quote_ident(&f.name).to_string(),
                                quote_ident(&schema.columns[*oi].name).to_string(),
                            )
                        })
                        .or_else(|| {
                            // Added columns get their DEFAULT (or NULL).
                            let d = if f.default.trim().is_empty() {
                                "NULL".to_owned()
                            } else {
                                default_sql(&f.default)
                            };
                            Some((quote_ident(&f.name).to_string(), d))
                        })
                })
                .collect();
            let ins: Vec<String> = targets.iter().map(|(a, _)| a.clone()).collect();
            let sel: Vec<String> = targets.iter().map(|(_, b)| b.clone()).collect();
            lines.push(format!(
                "INSERT INTO \"{SHADOW}\" ({}) SELECT {} FROM {q_old}",
                ins.join(", "),
                sel.join(", ")
            ));
            lines.push(format!("DROP TABLE {q_old}"));
            lines.push(format!(
                "ALTER TABLE \"{SHADOW}\" RENAME TO {}",
                quote_ident(&self.table)
            ));
            for ix in index_sql {
                lines.push(format!("{ix};"));
            }
            lines.push("COMMIT".to_owned());
            lines.push("PRAGMA foreign_key_check".to_owned());
            lines.push("PRAGMA foreign_keys = ON".to_owned());
            // Semicolon-free lines; the caller joins with ";\n".
            return Ok(lines.into_iter().map(|l| l.trim_end_matches(';').to_owned()).collect());
        }

        let mut out = alter_lines;
        out.extend(add_lines);
        out.extend(drop_lines);
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.table.trim().is_empty() {
            return Err("the table needs a name (Enter on the NAME row)".into());
        }
        if self.fields.is_empty() {
            return Err("a table needs at least one field (F8 adds one)".into());
        }
        let mut seen = std::collections::HashSet::new();
        for f in &self.fields {
            if f.name.trim().is_empty() {
                return Err("a field has no name".into());
            }
            if !seen.insert(f.name.to_ascii_lowercase()) {
                return Err(format!("duplicate field name {:?}", f.name));
            }
        }
        Ok(())
    }

    pub fn create(&self, db: &dyn DbLink) -> DbResult<()> {
        self.validate()?;
        db.execute(&self.sql()).map(|_| ())
    }
}

/// Designer state. Cursor row 0 is the table NAME; rows 1.. are fields.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditSlot {
    Name,
    Default,
    Refs,
}

pub struct CreateState {
    pub draft: TableDraft,
    pub cursor: usize,
    pub editing: Option<String>,
    /// Which text cell of the row the buffer edits.
    pub slot: EditSlot,
    /// Set when editing an EXISTING table: the live schema it was
    /// opened with. F2 then applies apply_script(schema) — ALTERs or
    /// the full rebuild — instead of CREATE TABLE.
    pub original: Option<EditorSchema>,
}

impl CreateState {
    pub fn new(name: &str) -> CreateState {
        CreateState {
            draft: TableDraft::new(name),
            cursor: 1, // the id field; Enter on row 0 renames the table
            editing: None,
            slot: EditSlot::Name,
            original: None,
        }
    }

    /// The TABLE EDITOR: an existing table's live schema, ready to
    /// be changed. F2 applies the diff.
    pub fn edit_existing(schema: EditorSchema) -> CreateState {
        let count = schema.columns.len();
        CreateState {
            draft: TableDraft::from_live(&schema),
            cursor: 1,
            editing: None,
            slot: EditSlot::Name,
            original: Some(schema),
        }
        .with_cursor(count)
    }

    fn with_cursor(mut self, cursor: usize) -> Self {
        self.cursor = cursor.min(self.draft.fields.len());
        self
    }

    pub fn field_idx(&self) -> Option<usize> {
        self.cursor.checked_sub(1)
    }
}

#[cfg(test)]
mod editor_tests {
    use super::*;
    use crate::db::ColumnInfo;

    fn col(name: &str, decl: &str, pk: bool, notnull: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.to_owned(),
            decl_type: decl.to_owned(),
            pk,
            notnull,
            dflt_value: None,
        }
    }

    fn schema(cols: Vec<ColumnInfo>) -> EditorSchema {
        EditorSchema {
            table: "t".into(),
            columns: cols,
            fks: Vec::new(),
        }
    }

    /// Adding a column compiles to ADD COLUMN (with its default).
    #[test]
    fn editor_add_column() {
        let cols = vec![col("id", "INTEGER", true, false), col("name", "TEXT", false, false)];
        let sch = schema(cols.clone());
        let mut d = TableDraft::from_live(&sch);
        let mut bal = FieldDef::new("balance", FType::Real);
        bal.default = "0".into();
        d.fields.push(bal);
        let script = d.apply_script(&sch, &[]).unwrap();
        assert_eq!(
            script,
            ["ALTER TABLE \"t\" ADD COLUMN \"balance\" REAL DEFAULT 0"]
        );
    }

    /// Dropping compiles to DROP COLUMN; renaming (same type/flags,
    /// new name, same position) compiles to RENAME COLUMN.
    #[test]
    fn editor_drop_and_rename() {
        let cols = vec![
            col("id", "INTEGER", true, false),
            col("city", "TEXT", false, false),
            col("zip", "TEXT", false, false),
        ];
        let sch = schema(cols.clone());
        let mut d = TableDraft::from_live(&sch);
        d.fields.remove(2); // drop zip
        d.fields[1].name = "locality".into(); // rename city -> locality
        let script = d.apply_script(&sch, &[]).unwrap();
        assert_eq!(
            script,
            [
                "ALTER TABLE \"t\" RENAME COLUMN \"city\" TO \"locality\"",
                "ALTER TABLE \"t\" DROP COLUMN \"zip\"",
            ]
        );
    }

    /// Changing an existing column's type now compiles to the REBUILD
    /// procedure (previously declined) — data-preserving by design.
    #[test]
    fn editor_type_change_rebuilds() {
        let cols = vec![col("score", "INTEGER", false, false)];
        let sch = schema(cols.clone());
        let mut d = TableDraft::from_live(&sch);
        d.fields[0].ftype = FType::Text;
        let script = d.apply_script(&sch, &[]).unwrap();
        assert!(script.iter().any(|l| l.contains("INSERT INTO")), "{script:?}");
        assert!(script.iter().any(|l| l.contains("DROP TABLE")), "{script:?}");
    }

    /// The editor round-trips a table's real schema: open a live
    /// table, change nothing, and the script is empty.
    #[test]
    fn editor_round_trips_live_columns() {
        let (db, _) = crate::db::EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE c(id INTEGER PRIMARY KEY, name TEXT NOT NULL, balance REAL DEFAULT 0);",
        )
        .unwrap();
        let cols = db.columns("c").unwrap();
        let sch = EditorSchema {
            table: "c".into(),
            columns: cols.clone(),
            fks: Vec::new(),
        };
        let d = TableDraft::from_live(&sch);
        assert!(d.apply_script(&sch, &[]).unwrap().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::EmbeddedDb;

    #[test]
    fn single_integer_pk_is_the_rowid_alias() {
        let mut d = TableDraft::new("people");
        let f = d.add_field();
        d.fields[f].name = "name".into();
        d.fields[f].notnull = true;
        assert_eq!(
            d.sql(),
            "CREATE TABLE \"people\" (\n  \"id\" INTEGER PRIMARY KEY,\n  \"name\" TEXT NOT NULL\n)"
        );
    }

    #[test]
    fn composite_pk_defaults_and_unique() {
        let mut d = TableDraft::new("t");
        d.fields[0].ftype = FType::Text; // pk id, but TEXT → table-level
        let b = d.add_field();
        d.fields[b].name = "region".into();
        d.fields[b].pk = true;
        d.fields[b].default = "east".into();
        let c = d.add_field();
        d.fields[c].name = "score".into();
        d.fields[c].ftype = FType::Real;
        d.fields[c].unique = true;
        d.fields[c].default = "0".into();
        let sql = d.sql();
        assert!(sql.contains("\"region\" TEXT DEFAULT 'east'"), "{sql}");
        assert!(sql.contains("\"score\" REAL UNIQUE DEFAULT 0"), "{sql}");
        assert!(sql.contains("PRIMARY KEY (\"id\", \"region\")"), "{sql}");
    }

    #[test]
    fn references_emit_and_enforce() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        let p = TableDraft::new("customers");
        p.create(&db).unwrap();
        let mut d = TableDraft::new("orders");
        let f = d.add_field();
        d.fields[f].name = "customer_id".into();
        d.fields[f].ftype = FType::Integer;
        d.fields[f].references = "customers(id)".into();
        let g = d.add_field();
        d.fields[g].name = "note".into();
        d.fields[g].references = "customers".into(); // bare → pk
        let sql = d.sql();
        assert!(sql.contains("REFERENCES \"customers\"(\"id\")"), "{sql}");
        assert!(sql.contains("\"note\" TEXT REFERENCES \"customers\"\n"), "{sql}");
        d.create(&db).unwrap();
        // The declared FK is discoverable — the fuel for linked forms.
        let q = db
            .query("SELECT \"table\", \"from\" FROM pragma_foreign_key_list('orders')")
            .unwrap();
        assert_eq!(q.rows.len(), 2);
    }

    #[test]
    fn insert_lands_after_the_cursor_with_a_fresh_name() {
        let mut d = TableDraft::new("addr");
        let a = d.add_field();
        d.fields[a].name = "street_num".into();
        let b = d.add_field();
        d.fields[b].name = "city".into();
        // Forgot street_name: standing on street_num, insert — it must
        // land between street_num and city, not at the end.
        let i = d.insert_field(Some(a));
        assert_eq!(i, a + 1);
        let names: Vec<&str> = d.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["id", "street_num", "field4", "city"]);
        // NAME row (None) inserts at the top; placeholders never collide.
        d.insert_field(None);
        assert_eq!(d.fields[0].name, "field5");
        assert_eq!(d.validate(), Ok(()), "no duplicate placeholders");
    }

    #[test]
    fn validation_catches_the_obvious() {
        let mut d = TableDraft::new("");
        assert!(d.validate().is_err());
        d.table = "x".into();
        let f = d.add_field();
        d.fields[f].name = "ID".into(); // duplicate of id, case-insensitive
        assert!(d.validate().unwrap_err().contains("duplicate"));
    }

    #[test]
    fn created_table_is_real_and_editable() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        let mut d = TableDraft::new("crew");
        let f = d.add_field();
        d.fields[f].name = "name".into();
        d.fields[f].notnull = true;
        let g = d.add_field();
        d.fields[g].name = "rank".into();
        d.fields[g].default = "ensign".into();
        d.create(&db).unwrap();
        // Defaults apply; the inline pk keeps it rowid-editable.
        db.execute("INSERT INTO crew(name) VALUES ('Saavik')").unwrap();
        let q = db.query("SELECT rank FROM crew").unwrap();
        assert_eq!(q.rows[0][0], crate::db::PValue::Text("ensign".into()));
        assert!(db.has_rowid("crew"));
    }
}
