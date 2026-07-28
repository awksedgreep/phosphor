//! Forms users craft themselves (DESIGN.md phase 5): field order,
//! labels, inclusion, and required-ness — stored in `_phosphor_forms`,
//! applied automatically whenever EDIT opens that table.

use crate::db::{DbLink, DbResult};
use crate::store;

#[derive(Debug, Clone)]
pub struct FormField {
    pub column: String,
    pub label: String,
    pub include: bool,
    pub required: bool,
}

#[derive(Debug, Clone)]
pub struct FormSpec {
    pub table: String,
    pub fields: Vec<FormField>,
}

impl FormSpec {
    /// A default form mirroring the table's columns.
    pub fn new(db: &dyn DbLink, table: &str) -> DbResult<FormSpec> {
        let fields = db
            .columns(table)?
            .into_iter()
            .map(|c| FormField {
                label: c.name.clone(),
                column: c.name,
                include: true,
                required: false,
            })
            .collect();
        Ok(FormSpec {
            table: table.to_owned(),
            fields,
        })
    }

    pub fn save(&self, db: &dyn DbLink) -> DbResult<()> {
        store::upsert(
            db,
            "_phosphor_forms",
            "table_ref",
            &self.table,
            &[("layout_json", self.to_json())],
        )
    }

    pub fn load(db: &dyn DbLink, table: &str) -> Option<FormSpec> {
        let r = store::lookup(db, "_phosphor_forms", "table_ref", table, &["layout_json"])?;
        Self::from_json(table, &r[0])
    }

    fn to_json(&self) -> String {
        let fields: Vec<serde_json::Value> = self
            .fields
            .iter()
            .map(|f| {
                serde_json::json!({
                    "column": f.column, "label": f.label,
                    "include": f.include, "required": f.required,
                })
            })
            .collect();
        serde_json::json!({"fields": fields, "v": 1}).to_string()
    }

    fn from_json(table: &str, json: &str) -> Option<FormSpec> {
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        let fields = v["fields"]
            .as_array()?
            .iter()
            .map(|f| FormField {
                column: f["column"].as_str().unwrap_or("").to_owned(),
                label: f["label"].as_str().unwrap_or("").to_owned(),
                include: f["include"].as_bool().unwrap_or(true),
                required: f["required"].as_bool().unwrap_or(false),
            })
            .filter(|f| !f.column.is_empty())
            .collect();
        Some(FormSpec {
            table: table.to_owned(),
            fields,
        })
    }
}

/// Designer state (list-based painter, v1).
pub struct FormState {
    pub spec: FormSpec,
    pub cursor: usize,
    pub editing: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::EmbeddedDb;

    #[test]
    fn round_trip_and_defaults() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE c(id INTEGER PRIMARY KEY, email TEXT NOT NULL, notes TEXT)")
            .unwrap();
        let mut spec = FormSpec::new(&db, "c").unwrap();
        assert_eq!(spec.fields.len(), 3);
        spec.fields[1].label = "E-Mail".into();
        spec.fields[1].required = true;
        spec.fields[2].include = false;
        spec.fields.swap(0, 1); // email first
        spec.save(&db).unwrap();

        let back = FormSpec::load(&db, "c").unwrap();
        assert_eq!(back.fields[0].column, "email");
        assert_eq!(back.fields[0].label, "E-Mail");
        assert!(back.fields[0].required);
        assert!(!back.fields[2].include);
    }
}
