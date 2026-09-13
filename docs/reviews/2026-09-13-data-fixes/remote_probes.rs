// Run in an isolated source copy against the review's local Hrana fixture.
// PHOSPHOR_TEST_URL must name that disposable fixture, never a live database.
use crate::app::{App, Command, Overlay};
use crate::db::{DbLink, PValue};
use crate::remote::RemoteDb;

#[test]
fn remote_identity_handles_shadowed_aliases_and_refuses_ambiguous_tables() {
    let db = RemoteDb::open(&std::env::var("PHOSPHOR_TEST_URL").unwrap()).unwrap();
    db.execute("CREATE TABLE remote_identity(rowid INTEGER, _rowid_ INTEGER, name TEXT)")
        .unwrap();
    db.execute("INSERT INTO remote_identity VALUES(7,7,'Alice'),(7,7,'Bob')")
        .unwrap();
    assert_eq!(
        db.rowid_column("remote_identity").unwrap().as_deref(),
        Some("oid")
    );
    assert_eq!(
        db.open_window("remote_identity", 0, 10).unwrap().0.rowids,
        Some(vec![1, 2])
    );
    db.update_row(
        "remote_identity",
        1,
        &[("name".into(), PValue::Text("Alicia".into()))],
    )
    .unwrap();
    db.delete_row("remote_identity", 2).unwrap();
    assert_eq!(
        db.query("SELECT name FROM remote_identity").unwrap().rows,
        vec![vec![PValue::Text("Alicia".into())]]
    );
    db.execute("ALTER TABLE remote_identity ADD COLUMN oid INTEGER DEFAULT 7")
        .unwrap();
    assert!(!db.has_rowid("remote_identity"));
    assert!(db
        .update_row(
            "remote_identity",
            7,
            &[("name".into(), PValue::Text("wrong".into()))]
        )
        .is_err());
    assert!(db.delete_row("remote_identity", 7).is_err());
    assert_eq!(db.count("remote_identity").unwrap(), 1);
}

#[test]
fn remote_generated_values_and_explicit_insert_identity_reach_the_form() {
    let db = RemoteDb::open(&std::env::var("PHOSPHOR_TEST_URL").unwrap()).unwrap();
    db.execute(
        "CREATE TABLE remote_generated(id INTEGER PRIMARY KEY,
        size INTEGER AS (length(name)) VIRTUAL, name TEXT,
        shout TEXT AS (upper(name)) STORED)",
    )
    .unwrap();
    db.execute("INSERT INTO remote_generated(id,name) VALUES(1,'Alice'),(100,'Bob')")
        .unwrap();
    let cols = db.columns("remote_generated").unwrap();
    assert_eq!(
        cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["id", "size", "name", "shout"]
    );
    assert!(cols[1].generated && cols[3].generated);
    assert_eq!(
        db.page("remote_generated", 0, 1).unwrap().rows[0],
        vec![
            PValue::Int(1),
            PValue::Int(5),
            PValue::Text("Alice".into()),
            PValue::Text("ALICE".into())
        ]
    );
    let mut app = App::new(Box::new(db), None);
    app.apply(Command::OpenTable("remote_generated".into()));
    app.sync();
    app.apply(Command::OpenInsert);
    app.apply(Command::EditBegin);
    for c in "50".chars() {
        app.apply(Command::EditChar(c));
    }
    app.apply(Command::EditCommitField);
    app.sync();
    let Overlay::Edit(ed) = &app.overlay else {
        panic!()
    };
    assert_eq!(ed.rowid, 50);
    assert_eq!(
        ed.row_abs, 2,
        "the competing insert shifted the grid position"
    );
    assert_eq!(ed.cursor, 2, "Enter skips the generated size");
    app.apply(Command::EditBegin);
    for c in "Charlie".chars() {
        app.apply(Command::EditChar(c));
    }
    app.apply(Command::EditCommitField);
    app.sync();
    let Overlay::Edit(ed) = &app.overlay else {
        panic!()
    };
    assert_eq!(ed.rowid, 50);
    assert_eq!(ed.fields[1].1, PValue::Int(7));
    assert_eq!(ed.fields[3].1, PValue::Text("CHARLIE".into()));
    assert_eq!(
        app.db
            .query("SELECT name FROM remote_generated WHERE id = 100")
            .unwrap()
            .rows[0][0],
        PValue::Text("Bob".into())
    );
}
