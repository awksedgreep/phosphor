use crate::app::{App, Command, Overlay};
use crate::db::{DbLink, EmbeddedDb, PValue};
use crate::creator::{EditorSchema, TableDraft, FType};
use ratatui::{backend::TestBackend, Terminal};
use ratatui::crossterm::event::{KeyCode, KeyEvent};

fn db(sql: &str) -> EmbeddedDb {
 let (db, _) = EmbeddedDb::open(":memory:").unwrap();
 db.execute(sql).unwrap(); db
}
fn app(sql: &str, table: &str) -> App {
 let mut a = App::new(Box::new(db(sql)), None);
 a.apply(Command::OpenTable(table.into())); a.sync(); a
}
fn key(a: &mut App, code: KeyCode) {
 if let Some(c) = a.map_key(KeyEvent::from(code)) { a.apply(c); }
 a.sync();
}
fn screen(a: &mut App, width:u16, height:u16) -> String {
 let mut t=Terminal::new(TestBackend::new(width,height)).unwrap();
 t.draw(|f| {crate::ui::draw(f,a);}).unwrap();
 t.backend().buffer().content.iter().map(|c|c.symbol()).collect()
}
fn schema(db: &dyn DbLink, table:&str) -> EditorSchema {
 EditorSchema{table:table.into(), columns:db.columns(table).unwrap(), fks:db.outgoing_fks(table)}
}

#[test]
fn review_export_all_rows() {
 let d=db("CREATE TABLE t(id INTEGER PRIMARY KEY); WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<10001) INSERT INTO t SELECT x FROM n;");
 let path=std::env::temp_dir().join("phosphor-review-export.csv");
 let msg=crate::csv_io::export_csv(&d,"t",path.to_str().unwrap()).unwrap();
 let count=std::fs::read_to_string(path).unwrap().lines().count()-1;
 assert_eq!(count,10001,"{msg}");
}
#[test]
fn review_import_malformed_csv_rolls_back() {
 let d=db("CREATE TABLE t(name TEXT, city TEXT)");
 let path=std::env::temp_dir().join("phosphor-review-malformed.csv");
 std::fs::write(&path,"name,city\nAda,London\nGrace\n").unwrap();
 let err=crate::csv_io::import_csv(&d,"t",path.to_str().unwrap()).unwrap_err();
 assert_eq!(d.count("t").unwrap(),0,"import failed but left inserted rows in transaction: {err}");
}
#[test]
fn review_rebuild_preserves_schema() {
 let d=db("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT UNIQUE, n INTEGER CHECK(n>0)); CREATE TABLE audit(msg TEXT); CREATE TRIGGER t_audit AFTER INSERT ON t BEGIN INSERT INTO audit VALUES(new.name); END; INSERT INTO t VALUES(1,'Ada',1);");
 let s=schema(&d,"t"); let mut draft=TableDraft::from_live(&s);
 draft.fields[2].ftype=FType::Real;
 d.execute(&draft.apply_script(&s,&[]).unwrap().join(";\n")).unwrap();
 let trigger_count=d.query("SELECT count(*) FROM sqlite_master WHERE type='trigger' AND tbl_name='t'").unwrap().rows[0][0].clone();
 let duplicate_accepted=d.execute("INSERT INTO t VALUES(2,'Ada',1)").is_ok();
 let invalid_accepted=d.execute("INSERT INTO t VALUES(3,'Bob',-1)").is_ok();
 assert!(!duplicate_accepted && !invalid_accepted && trigger_count==PValue::Int(1),"duplicate accepted={duplicate_accepted}, CHECK violation accepted={invalid_accepted}, triggers={trigger_count:?}");
}
#[test]
fn review_unchanged_default_is_noop() {
 let d=db("CREATE TABLE t(id INTEGER PRIMARY KEY, status TEXT DEFAULT 'new')");
 let s=schema(&d,"t"); let draft=TableDraft::from_live(&s);
 let script=draft.apply_script(&s,&[]).unwrap();
 assert!(script.is_empty(),"unchanged table schedules rebuild: {script:?}");
}
#[test]
fn review_help_restores_draft() {
 let mut a=app("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT)","t");
 a.apply(Command::OpenCreate(Some("unsaved_draft".into())));
 key(&mut a,KeyCode::F(1)); key(&mut a,KeyCode::Esc);
 assert!(matches!(&a.overlay,Overlay::Create(st) if st.draft.table=="unsaved_draft"),"F1 then Esc discarded the table draft");
}
#[test]
fn review_report_preview_returns_to_designer() {
 let mut a=app("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT); INSERT INTO t VALUES(1,'Ada');","t");
 a.apply(Command::OpenReport(None));
 if let Overlay::Report(st)=&mut a.overlay { st.spec.title="My unsaved report".into(); }
 key(&mut a,KeyCode::F(2)); key(&mut a,KeyCode::Esc);
 assert!(matches!(&a.overlay,Overlay::Report(st) if st.spec.title=="My unsaved report"),"preview then Esc discarded report designer");
}
#[test]
fn review_generated_columns_align_with_values() {
 let mut a=app("CREATE TABLE t(id INTEGER PRIMARY KEY, size INTEGER GENERATED ALWAYS AS (length(name)) VIRTUAL, name TEXT); INSERT INTO t(id,name) VALUES(1,'Alice');","t");
 a.apply(Command::OpenEdit); a.sync();
 let Overlay::Edit(ed)=&a.overlay else {panic!("no edit")};
 assert_eq!(ed.fields.iter().find(|(c,_)|c.name=="name").unwrap().1,PValue::Text("Alice".into()),"generated column caused positional mismatch");
}
#[test]
fn review_picker_reaches_customer_201() {
 let mut a=app("CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT); WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<201) INSERT INTO customers SELECT x,'Customer '||x FROM n; CREATE TABLE orders(id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id));","orders");
 a.apply(Command::OpenInsert); a.apply(Command::EditMove(1)); a.apply(Command::EditPick);
 let Overlay::Edit(ed)=&a.overlay else {panic!("no edit")};
 assert!(ed.picker.as_ref().unwrap().rows.iter().any(|r|r[0]==PValue::Int(201)),"picker offers only {} customers, with no search or paging",ed.picker.as_ref().unwrap().rows.len());
}
#[test]
fn review_readonly_rejects_cte_write() {
 let mut a=app("CREATE TABLE t(id INTEGER PRIMARY KEY,name TEXT)","t"); a.readonly=true;
 a.prompt.input="WITH x AS (SELECT 1) INSERT INTO t(name) SELECT 'unexpected write' FROM x RETURNING * -- limit".into();
 a.apply(Command::PromptRun); a.sync();
 assert_eq!(a.db.count("t").unwrap(),0,"read-only database accepted a write through query path");
}
#[test]
fn review_readonly_prefs_do_not_mutate_db() {
 let mut a=app("CREATE TABLE t(id INTEGER PRIMARY KEY)","t"); a.readonly=true;
 a.prompt.input="set theme amber".into(); a.apply(Command::PromptRun);
 assert!(!a.db.tables().unwrap().iter().any(|t|t.name=="_phosphor_prefs"),"read-only session created metadata tables and stored preferences");
}
#[test]
fn review_long_sidebar_keeps_selected_table_visible() {
 let mut sql=String::new(); for i in 0..40 {sql.push_str(&format!("CREATE TABLE table_{i:02}(id INTEGER PRIMARY KEY);"));}
 let mut a=App::new(Box::new(db(&sql)),None);
 a.apply(Command::SidebarMove(35));
 assert!(screen(&mut a,80,24).contains("table_35"),"selected sidebar table 35 is below viewport and list never scrolls");
}
#[test]
fn review_long_form_keeps_field_visible() {
 let cols=(0..35).map(|i|format!("field_{i:02} TEXT")).collect::<Vec<_>>().join(",");
 let mut a=app(&format!("CREATE TABLE t(id INTEGER PRIMARY KEY,{cols})"),"t");
 a.apply(Command::OpenInsert); a.apply(Command::EditMove(30));
 assert!(screen(&mut a,80,24).contains("field_29"),"selected field is below viewport and form never scrolls");
}
#[test]
fn review_pragma_query_works() {
 let d=db("CREATE TABLE t(id INTEGER PRIMARY KEY)");
 assert!(d.query("PRAGMA table_info(t)").is_ok(),"global LIMIT rewriting makes PRAGMA syntax invalid");
}
#[test]
fn review_no_sql_crm_starts_on_name() {
 let mut a=app("CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, city TEXT, balance REAL DEFAULT 0)","customers");
 a.apply(Command::OpenInsert);
 for c in "Ada".chars(){key(&mut a,KeyCode::Char(c));} key(&mut a,KeyCode::Enter);
 assert!(a.db.count("customers").unwrap()>0,"documented type Ada, Enter path types into id: {:?}",a.status);
}
#[test]
fn review_shadowed_rowid_does_not_modify_multiple_rows() {
 let d=db("CREATE TABLE t(rowid INTEGER, name TEXT); INSERT INTO t VALUES(7,'Alice'),(7,'Bob')");
 let _=d.update_row("t",7,&[("name".into(),PValue::Text("changed".into()))]);
 let q=d.query("SELECT count(*) FROM t WHERE name='changed'").unwrap();
 assert_ne!(q.rows[0][0],PValue::Int(2),"both rows were modified despite expected-one-row error");
}
#[test]
fn review_lua_cannot_write_host_files() {
 let d=db("CREATE TABLE t(id INTEGER PRIMARY KEY)");
 let path=std::env::temp_dir().join("phosphor-review-lua-sandbox.txt");
 let _=std::fs::remove_file(&path);
 let code=format!("local f = io.open({:?}, 'w'); if f then f:write('review fixture only'); f:close() end",path.to_str().unwrap());
 let result=crate::script::run(&d,&code);
 let wrote=path.exists(); let _=std::fs::remove_file(&path);
 assert!(!wrote,"sandboxed Lua wrote a host file; script result={result:?}");
}
#[test]
fn review_insert_keeps_inserted_identity() {
 let mut a=app("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT); INSERT INTO t VALUES(1,'Alice'),(100,'Bob');","t");
 a.apply(Command::OpenInsert);
 if let Overlay::Edit(ed)=&mut a.overlay {ed.inputs[0]=Some("50".into()); ed.inputs[1]=Some("Carol".into());}
 a.apply(Command::EditCommitField);a.sync();
 let Overlay::Edit(ed)=&a.overlay else {panic!("no edit")};
 assert_eq!(ed.rowid,50,"successful insert switched editable form to unrelated last row");
}
#[test]
fn review_status_error_visible_with_long_path() {
 let path=std::env::temp_dir().join("phosphor-review-status-path-which-exceeds-the-terminal-width-123456789.db");
 let (d,_)=EmbeddedDb::open(path.to_str().unwrap()).unwrap();
 let mut a=App::new(Box::new(d),None);a.status=Some(("Name is required".into(),true));
 assert!(screen(&mut a,80,24).contains("Name is required"),"long database name hides validation error");
}
#[test]
fn review_split_enter_edits_selected_child() {
 let mut a=app("CREATE TABLE customers(id INTEGER PRIMARY KEY,name TEXT); CREATE TABLE orders(id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id), product TEXT); INSERT INTO customers VALUES(1,'Alice'); INSERT INTO orders VALUES(1,1,'Item');","customers");
 a.visible_cols_width=120;a.apply(Command::ToggleSplit);a.sync();
 a.apply(Command::Focus(crate::app::Focus::Detail));key(&mut a,KeyCode::Enter);
 assert!(matches!(&a.overlay,Overlay::Edit(ed) if ed.table=="orders"),"Enter in focused order pane opened parent record");
}
#[test]
fn review_qbe_run_keeps_design() {
 let mut a=app("CREATE TABLE t(id INTEGER PRIMARY KEY,amount INTEGER); INSERT INTO t VALUES(1,200);","t");
 a.apply(Command::OpenQbe(None));
 if let Overlay::Qbe(st)=&mut a.overlay {st.spec.cols[1].filter="> 100".into();}
 key(&mut a,KeyCode::F(2));key(&mut a,KeyCode::Char('Q'));
 assert!(matches!(&a.overlay,Overlay::Qbe(st) if st.spec.cols[1].filter=="> 100"),"run discards query filters, reopening starts over");
}
fn remote() -> crate::remote::RemoteDb {
 let url=std::fs::read_to_string("/tmp/phosphor-review-hrana-url").unwrap();
 crate::remote::RemoteDb::open(url.trim()).unwrap()
}
#[test]
fn review_remote_semicolon_literal() {
 let d=remote();d.execute("CREATE TABLE remote_text(name TEXT)").unwrap();
 let result=d.execute("INSERT INTO remote_text VALUES('Ada; Grace')");
 assert!(result.is_ok(),"ordinary quoted semicolon was split as SQL: {result:?}");
}
#[test]
fn review_remote_import_atomic() {
 let d=remote();d.execute("CREATE TABLE remote_csv(id INTEGER PRIMARY KEY,name TEXT)").unwrap();
 let path=std::env::temp_dir().join("phosphor-review-remote.csv");
 std::fs::write(&path,"id,name\n1,Alice\n1,Bob\n").unwrap();
 let err=crate::csv_io::import_csv(&d,"remote_csv",path.to_str().unwrap()).unwrap_err();
 assert_eq!(d.count("remote_csv").unwrap(),0,"failed remote import partially committed: {err}");
}
#[test]
fn review_remote_failed_batch_is_atomic() {
 let d=remote();d.execute("CREATE TABLE remote_batch(id INTEGER PRIMARY KEY)").unwrap();
 let result=d.execute("BEGIN; INSERT INTO remote_batch VALUES(1); INSERT INTO remote_batch VALUES(1); COMMIT");
 assert!(result.is_err());
 assert_eq!(d.count("remote_batch").unwrap(),0,"pipeline kept executing COMMIT after failed statement");
}
