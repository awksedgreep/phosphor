//! The Applications Generator (DESIGN.md phase 5): users craft menus
//! wired to browses, saved queries, reports, and SQL — then hand the
//! database to their team as an APPLICATION (`phosphor --app crm.db`).
//! Definitions are rows in `_phosphor_apps` / `_phosphor_items`;
//! hotkeys are the first letter of each label, dBASE-style.

use crate::db::{DbLink, DbResult};
use crate::store;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActionKind {
    Browse,
    Query,
    Report,
    Sql,
}

impl ActionKind {
    pub fn cycle(self) -> ActionKind {
        match self {
            ActionKind::Browse => ActionKind::Query,
            ActionKind::Query => ActionKind::Report,
            ActionKind::Report => ActionKind::Sql,
            ActionKind::Sql => ActionKind::Browse,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ActionKind::Browse => "browse",
            ActionKind::Query => "query",
            ActionKind::Report => "report",
            ActionKind::Sql => "sql",
        }
    }

    pub fn parse(s: &str) -> ActionKind {
        match s {
            "query" => ActionKind::Query,
            "report" => ActionKind::Report,
            "sql" => ActionKind::Sql,
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

/// Find-or-create an app by name; returns its id.
pub fn ensure_app(db: &dyn DbLink, name: &str) -> DbResult<i64> {
    store::ensure(db)?;
    db.execute(&format!(
        "INSERT OR IGNORE INTO _phosphor_apps(name) VALUES ({})",
        store::q(name)
    ))?;
    app_id(db, name).ok_or_else(|| format!("app {name:?} not found after insert"))
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

pub fn items(db: &dyn DbLink, app: &str) -> Vec<AppItem> {
    let Some(id) = app_id(db, app) else {
        return Vec::new();
    };
    db.query(&format!(
        "SELECT id, label, action_kind, action_ref, seq \
         FROM _phosphor_items WHERE app_id = {id} ORDER BY seq, id"
    ))
    .map(|out| {
        out.rows
            .into_iter()
            .map(|r| AppItem {
                id: store::int(r.first()),
                label: store::text(r.get(1)),
                kind: ActionKind::parse(&store::text(r.get(2))),
                action_ref: store::text(r.get(3)),
                seq: store::int(r.get(4)),
            })
            .collect()
    })
    .unwrap_or_default()
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
    db.execute(&format!(
        "UPDATE _phosphor_items SET label = {}, action_kind = {}, action_ref = {}, seq = {} \
         WHERE id = {}",
        store::q(&item.label),
        store::q(item.kind.as_str()),
        store::q(&item.action_ref),
        item.seq,
        item.id
    ))
    .map(|_| ())
}

pub fn delete_item(db: &dyn DbLink, item_id: i64) -> DbResult<()> {
    db.execute(&format!("DELETE FROM _phosphor_items WHERE id = {item_id}"))
        .map(|_| ())
}

/// Swap the seq of two items (reordering in the designer).
pub fn swap_items(db: &dyn DbLink, a: &AppItem, b: &AppItem) -> DbResult<()> {
    let (mut a2, mut b2) = (a.clone(), b.clone());
    std::mem::swap(&mut a2.seq, &mut b2.seq);
    update_item(db, &a2)?;
    update_item(db, &b2)
}

/// Designer state: items of one app, immediate persistence.
pub struct AppDesignState {
    pub app: String,
    pub items: Vec<AppItem>,
    pub cursor: usize,
    pub editing: Option<String>,
    /// true → editing action_ref, false → editing label.
    pub editing_ref: bool,
}

/// Runtime state: the menu end users drive.
pub struct AppMenuState {
    pub app: String,
    pub items: Vec<AppItem>,
    pub cursor: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::EmbeddedDb;

    #[test]
    fn app_crud_and_ordering() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        ensure_app(&db, "crm").unwrap();
        assert_eq!(list_apps(&db), ["crm"]);
        add_item(&db, "crm", "Customers").unwrap();
        add_item(&db, "crm", "Aging report").unwrap();
        let mut its = items(&db, "crm");
        assert_eq!(its.len(), 2);
        assert_eq!(its[0].label, "Customers");
        assert_eq!(its[0].kind, ActionKind::Browse);

        its[1].kind = ActionKind::Report;
        its[1].action_ref = "aging".into();
        update_item(&db, &its[1]).unwrap();

        let its = items(&db, "crm");
        swap_items(&db, &its[0], &its[1]).unwrap();
        let its = items(&db, "crm");
        assert_eq!(its[0].label, "Aging report");
        assert_eq!(its[0].kind, ActionKind::Report);

        delete_item(&db, its[1].id).unwrap();
        assert_eq!(items(&db, "crm").len(), 1);
    }
}
