//! The Applications Generator (DESIGN.md phase 5): users craft menus
//! wired to browses, saved queries, reports, and SQL — then hand the
//! database to their team as an APPLICATION (`phosphor --app crm.db`).
//! Definitions are rows in `_phosphor_apps` / `_phosphor_items`;
//! hotkeys are the first letter of each label, dBASE-style.

use crate::db::{DbLink, DbResult, PValue};
use crate::store;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActionKind {
    Browse,
    Query,
    Report,
    Sql,
    /// A one-line Lua script (docs: `src/script.rs`): `query`, `execute`,
    /// and `say` are the sandboxed surface.
    Script,
}

impl ActionKind {
    pub fn cycle(self) -> ActionKind {
        match self {
            ActionKind::Browse => ActionKind::Query,
            ActionKind::Query => ActionKind::Report,
            ActionKind::Report => ActionKind::Sql,
            ActionKind::Sql => ActionKind::Script,
            ActionKind::Script => ActionKind::Browse,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ActionKind::Browse => "browse",
            ActionKind::Query => "query",
            ActionKind::Report => "report",
            ActionKind::Sql => "sql",
            ActionKind::Script => "script",
        }
    }

    pub fn parse(s: &str) -> ActionKind {
        match s {
            "query" => ActionKind::Query,
            "report" => ActionKind::Report,
            "sql" => ActionKind::Sql,
            "script" => ActionKind::Script,
            _ => ActionKind::Browse,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppItem {
    pub id: i64,
    pub label: String,
    pub kind: ActionKind,
    pub action_ref: String,
    pub seq: i64,
}

pub fn list_apps(db: &dyn DbLink) -> Vec<String> {
    store::names(db, "_phosphor_apps", "name")
}

/// Find-or-create an app by name; returns its id. Also brings older
/// databases up to the current schema (the `version` column), since
/// `CREATE TABLE IF NOT EXISTS` cannot alter an existing table.
pub fn ensure_app(db: &dyn DbLink, name: &str) -> DbResult<i64> {
    store::ensure(db)?;
    // Explicit migration, not part of the idempotent DDL. Ignore the
    // "duplicate column name" error on databases that already have it.
    let _ = db.execute("ALTER TABLE _phosphor_apps ADD COLUMN version INTEGER DEFAULT 1");
    db.execute(&format!(
        "INSERT OR IGNORE INTO _phosphor_apps(name) VALUES ({})",
        store::q(name)
    ))?;
    app_id(db, name).ok_or_else(|| format!("app {name:?} not found after insert"))
}

/// The app's declared version (defaults to 1 for pre-version databases).
pub fn app_version(db: &dyn DbLink, name: &str) -> i64 {
    db.query(&format!(
        "SELECT version FROM _phosphor_apps WHERE name = {}",
        store::q(name)
    ))
    .ok()
    .and_then(|q| q.rows.into_iter().next())
    .and_then(|r| r.into_iter().next())
    .map(|v| store::int(Some(&v)))
    .filter(|v| *v > 0)
    .unwrap_or(1)
}

pub fn app_id(db: &dyn DbLink, name: &str) -> Option<i64> {
    let out = db
        .query(&format!(
            "SELECT id FROM _phosphor_apps WHERE name = {}",
            store::q(name)
        ))
        .ok()?;
    Some(store::int(out.rows.first()?.first()))
}

/// The app's items, or the query's error. A failure is propagated (not
/// mapped to an empty Vec) so a transient remote error surfaces as a
/// status message instead of a menu that looks empty (#75). An app that
/// genuinely has no items is `Ok([])`.
pub fn items(db: &dyn DbLink, app: &str) -> DbResult<Vec<AppItem>> {
    // Single JOIN (was: app_id lookup + items select = 2 round-trips
    // on every Apps open, worse over sqld).
    let out = db.query(&format!(
        "SELECT i.id, i.label, i.action_kind, i.action_ref, i.seq \
          FROM _phosphor_items i JOIN _phosphor_apps a ON a.id = i.app_id \
          WHERE a.name = {} ORDER BY i.seq, i.id",
        store::q(app)
    ))?;
    Ok(out
        .rows
        .into_iter()
        .map(|r| AppItem {
            id: store::int(r.first()),
            label: store::text(r.get(1)),
            kind: ActionKind::parse(&store::text(r.get(2))),
            action_ref: store::text(r.get(3)),
            seq: store::int(r.get(4)),
        })
        .collect())
}

pub fn add_item(db: &dyn DbLink, app: &str, label: &str) -> DbResult<()> {
    let id = ensure_app(db, app)?;
    db.execute(&format!(
        "INSERT INTO _phosphor_items(app_id, label, action_kind, action_ref, seq) \
         VALUES ({id}, {}, 'browse', '', \
                 COALESCE((SELECT max(seq) + 1 FROM _phosphor_items WHERE app_id = {id}), 0))",
        store::q(label)
    ))
    .map(|_| ())
}

pub fn update_item(db: &dyn DbLink, item: &AppItem) -> DbResult<()> {
    db.execute_params(
        "UPDATE _phosphor_items SET label = ?1, action_kind = ?2, action_ref = ?3, seq = ?4 WHERE id = ?5",
        &[PValue::Text(item.label.clone()), PValue::Text(item.kind.as_str().into()),
          PValue::Text(item.action_ref.clone()), PValue::Int(item.seq), PValue::Int(item.id)],
    )
    .map(|_| ())
}

/// Replace one item's target (used by the multi-line script editor).
pub fn set_item_ref(db: &dyn DbLink, item_id: i64, action_ref: &str) -> DbResult<()> {
    db.execute_params(
        "UPDATE _phosphor_items SET action_ref = ?1 WHERE id = ?2",
        &[PValue::Text(action_ref.into()), PValue::Int(item_id)],
    )
    .map(|_| ())
}

/// Rename an app (items link by id, so links survive). Errors if the
/// target name is already taken by a *different* app. A never-saved
/// default app (`old` has no row) is simply created under `new`.
pub fn rename_app(db: &dyn DbLink, old: &str, new: &str) -> DbResult<()> {
    if old == new {
        return Ok(());
    }
    store::ensure(db)?;
    let taken = db
        .query(&format!(
            "SELECT id FROM _phosphor_apps WHERE name = {}",
            store::q(new)
        ))
        .map(|q| !q.rows.is_empty())
        .unwrap_or(false);
    if taken {
        return Err(format!("an app named {new:?} already exists"));
    }
    let exists = app_id(db, old).is_some();
    if exists {
        db.execute(&format!(
            "UPDATE _phosphor_apps SET name = {} WHERE name = {}",
            store::q(new),
            store::q(old)
        ))
        .map(|_| ())
    } else {
        db.execute(&format!(
            "INSERT INTO _phosphor_apps(name) VALUES ({})",
            store::q(new)
        ))
        .map(|_| ())
    }
}

pub fn delete_item(db: &dyn DbLink, item_id: i64) -> DbResult<()> {
    db.execute(&format!("DELETE FROM _phosphor_items WHERE id = {item_id}"))
        .map(|_| ())
}

/// Swap the seq of two items (reordering in the designer): one
/// atomic UPDATE instead of two separate writes (was: clone both +
/// 2x UPDATE, non-atomic — a crash between them lost the order).
pub fn swap_items(db: &dyn DbLink, a: &AppItem, b: &AppItem) -> DbResult<()> {
    db.execute(&format!(
        "UPDATE _phosphor_items SET seq = CASE id WHEN {} THEN {} WHEN {} THEN {} ELSE seq END \
         WHERE id IN ({}, {})",
        a.id, b.seq, b.id, a.seq, a.id, b.id
    ))
    .map(|_| ())
}

/// True when a table or view with that name exists (case-insensitive) —
/// the target a Browse (or a report-on-table) item points at.
fn table_exists(db: &dyn DbLink, name: &str) -> bool {
    db.tables()
        .map(|ts| ts.iter().any(|t| t.name.eq_ignore_ascii_case(name)))
        .unwrap_or(false)
}

/// True when a saved QBE query with that name exists.
fn saved_query_exists(db: &dyn DbLink, name: &str) -> bool {
    store::names(db, "_phosphor_queries", "name")
        .iter()
        .any(|n| n.eq_ignore_ascii_case(name))
}

/// True when a saved report with that name exists.
fn saved_report_exists(db: &dyn DbLink, name: &str) -> bool {
    store::names(db, "_phosphor_reports", "name")
        .iter()
        .any(|n| n.eq_ignore_ascii_case(name))
}

/// Why a menu item's target is broken, if it is (`None` = valid), checked
/// against the live catalog so a broken target is caught before the app is
/// handed off rather than only at run time (#45).
pub fn item_target_error(db: &dyn DbLink, item: &AppItem) -> Option<String> {
    let broken = match item.kind {
        ActionKind::Browse => !table_exists(db, &item.action_ref),
        ActionKind::Query => !saved_query_exists(db, &item.action_ref),
        // A report item is valid if a saved report exists OR the target is a
        // table (start_report falls back to a report on that table).
        ActionKind::Report => {
            !saved_report_exists(db, &item.action_ref) && !table_exists(db, &item.action_ref)
        }
        ActionKind::Sql | ActionKind::Script => item.action_ref.trim().is_empty(),
    };
    broken.then(|| match item.kind {
        ActionKind::Browse => format!("no table named {:?}", item.action_ref),
        ActionKind::Query => format!("no saved query named {:?}", item.action_ref),
        ActionKind::Report => format!("no report or table named {:?}", item.action_ref),
        ActionKind::Sql => "empty SQL target".to_owned(),
        ActionKind::Script => "empty script target".to_owned(),
    })
}

/// Every broken target among the given items as `(item id, reason)` — the
/// check run before an app is handed off as the application (#45).
pub fn broken_targets_in(items: &[AppItem], db: &dyn DbLink) -> Vec<(i64, String)> {
    items
        .iter()
        .filter_map(|it| item_target_error(db, it).map(|e| (it.id, e)))
        .collect()
}

fn table_names(db: &dyn DbLink) -> Vec<String> {
    db.tables()
        .map(|ts| {
            ts.into_iter()
                .filter(|t| !crate::app::App::is_internal(t))
                .map(|t| t.name)
                .collect()
        })
        .unwrap_or_default()
}

/// The catalog a menu-item target can name, per its kind — what the
/// type-aware picker lists so a target is chosen, not remembered (#45).
fn target_options(kind: ActionKind, db: &dyn DbLink) -> Vec<String> {
    match kind {
        ActionKind::Browse => table_names(db),
        ActionKind::Query => store::names(db, "_phosphor_queries", "name"),
        ActionKind::Report => {
            let mut v = store::names(db, "_phosphor_reports", "name");
            for t in table_names(db) {
                if !v.iter().any(|r| r.eq_ignore_ascii_case(&t)) {
                    v.push(t);
                }
            }
            v
        }
        ActionKind::Sql | ActionKind::Script => Vec::new(),
    }
}

/// A searchable, in-memory name picker for choosing a menu-item target
/// (Browse→tables, Query→saved queries, Report→saved reports or tables).
pub struct TargetPickerState {
    pub kind: ActionKind,
    pub all: Vec<String>,
    pub search: String,
    pub cursor: usize,
    /// Index of the item being edited, into `AppDesignState::items`.
    pub item_index: usize,
}

impl TargetPickerState {
    pub fn open(kind: ActionKind, db: &dyn DbLink, item_index: usize) -> Self {
        let all = target_options(kind, db);
        TargetPickerState {
            kind,
            all,
            search: String::new(),
            cursor: 0,
            item_index,
        }
    }

    /// The catalog filtered by the current search text (case-insensitive).
    pub fn options(&self) -> Vec<String> {
        let s = self.search.to_lowercase();
        if s.is_empty() {
            self.all.clone()
        } else {
            self.all
                .iter()
                .filter(|o| o.to_lowercase().contains(&s))
                .cloned()
                .collect()
        }
    }
}

/// Designer state: items of one app, immediate persistence.
pub struct AppDesignState {
    pub app: String,
    pub items: Vec<AppItem>,
    pub cursor: usize,
    pub editing: Option<String>,
    /// true → editing action_ref, false → editing label.
    pub editing_ref: bool,
    /// true → the buffer edits the app's name (`r`).
    pub renaming_app: bool,
    /// Some → the type-aware target picker is open for `item_index` (#45).
    pub target_picker: Option<TargetPickerState>,
}

/// Runtime state: the menu end users drive.
pub struct AppMenuState {
    pub app: String,
    pub version: i64,
    pub items: Vec<AppItem>,
    pub cursor: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::EmbeddedDb;

    /// #21: an app table created before the `version` column existed is
    /// migrated in place, defaulting to version 1.
    #[test]
    fn legacy_app_table_migrates_version() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE _phosphor_apps (
                 id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL, description TEXT);",
        )
        .unwrap();
        ensure_app(&db, "legacy").unwrap();
        assert_eq!(app_version(&db, "legacy"), 1);
        let cols = db.columns("_phosphor_apps").unwrap();
        assert!(
            cols.iter().any(|c| c.name == "version"),
            "version column added"
        );
    }

    #[test]
    fn app_crud_and_ordering() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        ensure_app(&db, "crm").unwrap();
        assert_eq!(list_apps(&db), ["crm"]);
        add_item(&db, "crm", "Customers").unwrap();
        add_item(&db, "crm", "Aging report").unwrap();
        let mut its = items(&db, "crm").unwrap();
        assert_eq!(its.len(), 2);
        assert_eq!(its[0].label, "Customers");
        assert_eq!(its[0].kind, ActionKind::Browse);

        its[1].kind = ActionKind::Report;
        its[1].action_ref = "aging".into();
        update_item(&db, &its[1]).unwrap();

        let its = items(&db, "crm").unwrap();
        swap_items(&db, &its[0], &its[1]).unwrap();
        let its = items(&db, "crm").unwrap();
        assert_eq!(its[0].label, "Aging report");
        assert_eq!(its[0].kind, ActionKind::Report);

        delete_item(&db, its[1].id).unwrap();
        assert_eq!(items(&db, "crm").unwrap().len(), 1);
    }

    /// #45: menu targets are validated against the live catalog, per action
    /// kind, so a broken one is caught (and named) before handoff.
    #[test]
    fn menu_targets_validate_against_the_catalog() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        store::ensure(&db).unwrap();
        db.execute("CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT)")
            .unwrap();
        db.execute(
            "INSERT INTO _phosphor_queries(name, sql_text) \
             VALUES ('by city', 'SELECT * FROM customers')",
        )
        .unwrap();
        db.execute(
            "INSERT INTO _phosphor_reports(name, title, source_sql) \
             VALUES ('aging', 'Aging', 'customers')",
        )
        .unwrap();

        let mk = |kind: ActionKind, ref_: &str| AppItem {
            id: 0,
            label: String::new(),
            kind,
            action_ref: ref_.into(),
            seq: 0,
        };

        // Valid targets resolve (table names are case-insensitive; a report
        // may also name a table, matching start_report's fallback).
        assert_eq!(
            item_target_error(&db, &mk(ActionKind::Browse, "customers")),
            None
        );
        assert_eq!(
            item_target_error(&db, &mk(ActionKind::Browse, "CUSTOMERS")),
            None
        );
        assert_eq!(
            item_target_error(&db, &mk(ActionKind::Query, "by city")),
            None
        );
        assert_eq!(
            item_target_error(&db, &mk(ActionKind::Report, "aging")),
            None
        );
        assert_eq!(
            item_target_error(&db, &mk(ActionKind::Report, "customers")),
            None
        );
        assert_eq!(
            item_target_error(&db, &mk(ActionKind::Sql, "SELECT 1")),
            None
        );
        assert_eq!(
            item_target_error(&db, &mk(ActionKind::Script, "say('hi')")),
            None
        );

        // Broken targets are named, per kind.
        assert_eq!(
            item_target_error(&db, &mk(ActionKind::Browse, "ghost")).as_deref(),
            Some("no table named \"ghost\"")
        );
        assert_eq!(
            item_target_error(&db, &mk(ActionKind::Query, "ghost")).as_deref(),
            Some("no saved query named \"ghost\"")
        );
        assert_eq!(
            item_target_error(&db, &mk(ActionKind::Report, "ghost")).as_deref(),
            Some("no report or table named \"ghost\"")
        );
        assert_eq!(
            item_target_error(&db, &mk(ActionKind::Sql, "  ")).as_deref(),
            Some("empty SQL target")
        );
        assert_eq!(
            item_target_error(&db, &mk(ActionKind::Script, "")).as_deref(),
            Some("empty script target")
        );

        // The batch check names each broken item by id.
        let its = vec![
            AppItem {
                id: 11,
                label: "A".into(),
                kind: ActionKind::Browse,
                action_ref: "customers".into(),
                seq: 1,
            },
            AppItem {
                id: 12,
                label: "B".into(),
                kind: ActionKind::Query,
                action_ref: "ghost".into(),
                seq: 2,
            },
            AppItem {
                id: 13,
                label: "C".into(),
                kind: ActionKind::Report,
                action_ref: "aging".into(),
                seq: 3,
            },
        ];
        assert_eq!(
            broken_targets_in(&its, &db),
            vec![(12, "no saved query named \"ghost\"".into())]
        );
    }

    /// #45: the Browse/Report target picker lists only user tables —
    /// phosphor's own catalog and engine shadow tables stay out of the list.
    #[test]
    fn target_options_hides_internal_tables() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE orders(a INTEGER); \
             CREATE TABLE _phosphor_queries(name TEXT); \
             CREATE TABLE _chunks(x INT);",
        )
        .unwrap();
        let browse = target_options(ActionKind::Browse, &db);
        assert!(
            browse.iter().any(|n| n == "orders"),
            "user table listed: {browse:?}"
        );
        assert!(
            browse.iter().all(|n| !n.starts_with('_')),
            "internal tables hidden: {browse:?}"
        );
    }

    /// #75: a query failure is propagated as an error, not mapped to an
    /// empty Vec — so a transient remote error can't masquerade as an
    /// app that simply has no items.
    #[test]
    fn items_surfaces_query_errors() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        store::ensure(&db).unwrap();
        ensure_app(&db, "crm").unwrap();
        add_item(&db, "crm", "Customers").unwrap();
        assert_eq!(items(&db, "crm").unwrap().len(), 1);
        // Drop the items table: the JOIN now fails, and items() must
        // return Err rather than an empty Vec.
        db.execute("DROP TABLE _phosphor_items").unwrap();
        assert!(
            items(&db, "crm").is_err(),
            "a query failure must be an error, not an empty menu"
        );
    }
}
