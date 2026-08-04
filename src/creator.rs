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
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub ftype: FType,
    pub pk: bool,
    pub notnull: bool,
    pub unique: bool,
    pub default: String,
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
        }
    }
}

fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Render a DEFAULT clause value: numbers, NULL, CURRENT_* and
/// parenthesized expressions pass through; everything else is quoted.
fn default_sql(raw: &str) -> String {
    let v = raw.trim();
    let upper = v.to_ascii_uppercase();
    if v.parse::<f64>().is_ok()
        || upper == "NULL"
        || upper == "TRUE"
        || upper == "FALSE"
        || upper.starts_with("CURRENT_")
        || (v.starts_with('(') && v.ends_with(')'))
    {
        v.to_owned()
    } else {
        format!("'{}'", v.replace('\'', "''"))
    }
}

#[derive(Debug, Clone)]
pub struct TableDraft {
    pub table: String,
    pub fields: Vec<FieldDef>,
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

    pub fn add_field(&mut self) -> usize {
        let n = self.fields.len() + 1;
        self.fields.push(FieldDef::new(&format!("field{n}"), FType::Text));
        self.fields.len() - 1
    }

    /// The CREATE TABLE this draft will run — always on screen.
    ///
    /// A single INTEGER pk becomes the inline `INTEGER PRIMARY KEY`
    /// (the rowid alias — what EDIT/paging key on); any other pk shape
    /// becomes a table-level PRIMARY KEY(...) clause.
    pub fn sql(&self) -> String {
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
            quote_ident(&self.table),
            cols.join(",\n  ")
        )
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.table.trim().is_empty() {
            return Err("the table needs a name (Enter on the NAME row)".into());
        }
        if self.fields.is_empty() {
            return Err("a table needs at least one field (n adds one)".into());
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
pub struct CreateState {
    pub draft: TableDraft,
    pub cursor: usize,
    pub editing: Option<String>,
    /// true → editing the DEFAULT value, false → the (table/field) name.
    pub editing_default: bool,
}

impl CreateState {
    pub fn new(name: &str) -> CreateState {
        CreateState {
            draft: TableDraft::new(name),
            cursor: 1, // the id field; Enter on row 0 renames the table
            editing: None,
            editing_default: false,
        }
    }

    pub fn field_idx(&self) -> Option<usize> {
        self.cursor.checked_sub(1)
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
