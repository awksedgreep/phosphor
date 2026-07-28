//! Forms users craft themselves (DESIGN.md phase 5), now with the real
//! thing: CREATE SCREEN. Layout v2 adds 2D positions, static texts, and
//! boxes — `@ SAY/GET`, reborn. v1 (list) layouts load compatibly and
//! keep rendering as a vertical form until painted.

use crate::db::{DbLink, DbResult};
use crate::store;

pub const DEFAULT_FIELD_WIDTH: u16 = 20;
pub const DEFAULT_CANVAS: (u16, u16) = (64, 18);

#[derive(Debug, Clone)]
pub struct FormField {
    pub column: String,
    pub label: String,
    pub include: bool,
    pub required: bool,
    /// Painted position of the label (canvas coords); None = unplaced.
    pub pos: Option<(u16, u16)>,
    /// Width of the value cell in painted mode.
    pub width: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextItem {
    pub x: u16,
    pub y: u16,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoxItem {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

#[derive(Debug, Clone)]
pub struct FormSpec {
    pub table: String,
    pub fields: Vec<FormField>,
    pub texts: Vec<TextItem>,
    pub boxes: Vec<BoxItem>,
    pub size: (u16, u16),
}

impl FormSpec {
    /// A default form mirroring the table's columns (unpainted).
    pub fn new(db: &dyn DbLink, table: &str) -> DbResult<FormSpec> {
        let fields = db
            .columns(table)?
            .into_iter()
            .map(|c| FormField {
                label: c.name.clone(),
                column: c.name,
                include: true,
                required: false,
                pos: None,
                width: DEFAULT_FIELD_WIDTH,
            })
            .collect();
        Ok(FormSpec {
            table: table.to_owned(),
            fields,
            texts: Vec::new(),
            boxes: Vec::new(),
            size: DEFAULT_CANVAS,
        })
    }

    /// Has this form been painted (2D mode), or is it still a list?
    pub fn painted(&self) -> bool {
        self.fields.iter().any(|f| f.pos.is_some())
            || !self.texts.is_empty()
            || !self.boxes.is_empty()
    }

    /// Give every included-but-unplaced field a default spot (a neat
    /// column) so the painter always starts from something visible.
    pub fn auto_place(&mut self) {
        let mut y = 1u16;
        let (w, h) = self.size;
        for f in self.fields.iter_mut().filter(|f| f.include) {
            if f.pos.is_none() {
                f.pos = Some((2.min(w.saturating_sub(1)), y.min(h.saturating_sub(1))));
                y += 2;
            }
        }
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
                let mut v = serde_json::json!({
                    "column": f.column, "label": f.label,
                    "include": f.include, "required": f.required,
                    "width": f.width,
                });
                if let Some((x, y)) = f.pos {
                    v["x"] = x.into();
                    v["y"] = y.into();
                }
                v
            })
            .collect();
        let texts: Vec<serde_json::Value> = self
            .texts
            .iter()
            .map(|t| serde_json::json!({"x": t.x, "y": t.y, "text": t.text}))
            .collect();
        let boxes: Vec<serde_json::Value> = self
            .boxes
            .iter()
            .map(|b| serde_json::json!({"x": b.x, "y": b.y, "w": b.w, "h": b.h}))
            .collect();
        serde_json::json!({
            "fields": fields, "texts": texts, "boxes": boxes,
            "size": {"w": self.size.0, "h": self.size.1}, "v": 2,
        })
        .to_string()
    }

    fn from_json(table: &str, json: &str) -> Option<FormSpec> {
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        let u16_of = |j: &serde_json::Value| j.as_u64().map(|n| n.min(500) as u16);
        let fields = v["fields"]
            .as_array()?
            .iter()
            .map(|f| FormField {
                column: f["column"].as_str().unwrap_or("").to_owned(),
                label: f["label"].as_str().unwrap_or("").to_owned(),
                include: f["include"].as_bool().unwrap_or(true),
                required: f["required"].as_bool().unwrap_or(false),
                // v1 layouts have no coords: pos stays None, list mode.
                pos: match (u16_of(&f["x"]), u16_of(&f["y"])) {
                    (Some(x), Some(y)) => Some((x, y)),
                    _ => None,
                },
                width: u16_of(&f["width"]).unwrap_or(DEFAULT_FIELD_WIDTH).max(1),
            })
            .filter(|f| !f.column.is_empty())
            .collect();
        let texts = v["texts"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| {
                        Some(TextItem {
                            x: u16_of(&t["x"])?,
                            y: u16_of(&t["y"])?,
                            text: t["text"].as_str()?.to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let boxes = v["boxes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|b| {
                        Some(BoxItem {
                            x: u16_of(&b["x"])?,
                            y: u16_of(&b["y"])?,
                            w: u16_of(&b["w"])?.max(2),
                            h: u16_of(&b["h"])?.max(2),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let size = (
            u16_of(&v["size"]["w"]).unwrap_or(DEFAULT_CANVAS.0).max(20),
            u16_of(&v["size"]["h"]).unwrap_or(DEFAULT_CANVAS.1).max(5),
        );
        Some(FormSpec {
            table: table.to_owned(),
            fields,
            texts,
            boxes,
            size,
        })
    }
}

/// List designer state (labels, include, required, order).
pub struct FormState {
    pub spec: FormSpec,
    pub cursor: usize,
    pub editing: Option<String>,
}

/// The painter: a canvas cursor, a selected field, optional text-input
/// and box-corner modes. CREATE SCREEN, 2026 edition.
pub struct PaintState {
    pub spec: FormSpec,
    pub cursor: (u16, u16),
    /// Index into spec.fields of the selected (movable) field.
    pub selected: usize,
    /// Some(buffer) while typing a static text (placed at cursor).
    pub editing: Option<String>,
    /// First corner of a box being drawn ('b' pressed once).
    pub pending_box: Option<(u16, u16)>,
}

impl PaintState {
    pub fn new(mut spec: FormSpec) -> Self {
        spec.auto_place();
        let selected = spec.fields.iter().position(|f| f.include).unwrap_or(0);
        PaintState {
            spec,
            cursor: (2, 1),
            selected,
            editing: None,
            pending_box: None,
        }
    }

    /// Cycle selection to the next included field.
    pub fn select_next(&mut self) {
        let n = self.spec.fields.len();
        if n == 0 {
            return;
        }
        for step in 1..=n {
            let idx = (self.selected + step) % n;
            if self.spec.fields[idx].include {
                self.selected = idx;
                // Snap the cursor to the newly selected field.
                if let Some(pos) = self.spec.fields[idx].pos {
                    self.cursor = pos;
                }
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::EmbeddedDb;

    fn db() -> EmbeddedDb {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE c(id INTEGER PRIMARY KEY, email TEXT NOT NULL, notes TEXT)")
            .unwrap();
        db
    }

    #[test]
    fn round_trip_and_defaults() {
        let db = db();
        let mut spec = FormSpec::new(&db, "c").unwrap();
        assert_eq!(spec.fields.len(), 3);
        assert!(!spec.painted());
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
        assert!(!back.painted(), "no coords yet: still a list form");
    }

    #[test]
    fn painted_layout_round_trips() {
        let db = db();
        let mut spec = FormSpec::new(&db, "c").unwrap();
        spec.fields[0].pos = Some((4, 2));
        spec.fields[1].pos = Some((4, 5));
        spec.fields[1].width = 32;
        spec.texts.push(TextItem {
            x: 20,
            y: 0,
            text: "CUSTOMER ENTRY".into(),
        });
        spec.boxes.push(BoxItem {
            x: 1,
            y: 1,
            w: 50,
            h: 8,
        });
        spec.size = (60, 14);
        spec.save(&db).unwrap();

        let back = FormSpec::load(&db, "c").unwrap();
        assert!(back.painted());
        assert_eq!(back.fields[0].pos, Some((4, 2)));
        assert_eq!(back.fields[1].width, 32);
        assert_eq!(back.texts[0].text, "CUSTOMER ENTRY");
        assert_eq!(back.boxes[0], BoxItem { x: 1, y: 1, w: 50, h: 8 });
        assert_eq!(back.size, (60, 14));
    }

    #[test]
    fn v1_layout_json_still_loads() {
        // Hand-written v1 JSON (as phase 5 shipped it): no coords.
        let json = r#"{"fields":[{"column":"id","label":"ID","include":true,"required":false},
                        {"column":"email","label":"Mail","include":true,"required":true}],"v":1}"#;
        let spec = FormSpec::from_json("c", json).unwrap();
        assert_eq!(spec.fields.len(), 2);
        assert_eq!(spec.fields[1].label, "Mail");
        assert!(spec.fields[1].required);
        assert!(!spec.painted());
        assert_eq!(spec.size, DEFAULT_CANVAS);
    }

    #[test]
    fn auto_place_fills_a_column() {
        let db = db();
        let mut spec = FormSpec::new(&db, "c").unwrap();
        spec.fields[1].include = false;
        spec.auto_place();
        assert_eq!(spec.fields[0].pos, Some((2, 1)));
        assert_eq!(spec.fields[1].pos, None, "excluded fields stay unplaced");
        assert_eq!(spec.fields[2].pos, Some((2, 3)));
    }
}
