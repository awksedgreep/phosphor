//! phosphor — the 1988 green-screen database desktop, reborn.
//!
//!   phosphor <file.db>     open a SQLite/libSQL database file
//!   phosphor               open an in-memory scratch database
//!
//! PHOSPHOR_EXT=/path/to/libtimeless_ext.so loads the timeless extension
//! into embedded connections (enables dbhealth + telemetry vtabs).

mod app;
mod db;
mod qbe;
mod remote;
mod report;
mod store;
mod theme;
mod ui;

use ratatui::crossterm::event::{self, Event, KeyEventKind};

use crate::app::App;
use crate::db::{DbLink, EmbeddedDb};
use crate::remote::RemoteDb;

fn main() -> std::io::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| ":memory:".into());
    // `phosphor file.db` = embedded; `phosphor http://host:8880` = sqld
    // over Hrana HTTP (PHOSPHOR_TOKEN for authenticated servers).
    let (link, warning): (Box<dyn DbLink>, Option<String>) =
        if path.starts_with("http://") || path.starts_with("https://") {
            match RemoteDb::open(&path) {
                Ok(db) => (Box::new(db), None),
                Err(e) => {
                    eprintln!("phosphor: cannot reach {path}: {e}");
                    std::process::exit(1);
                }
            }
        } else {
            match EmbeddedDb::open(&path) {
                Ok((db, warn)) => (Box::new(db), warn),
                Err(e) => {
                    eprintln!("phosphor: cannot open {path}: {e}");
                    std::process::exit(1);
                }
            }
        };
    let mut app = App::new(link, warning);

    let mut terminal = ratatui::init();
    let result = loop {
        if let Err(e) = terminal.draw(|f| ui::draw(f, &mut app)) {
            break Err(e);
        }
        match event::read() {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                if let Some(cmd) = app.map_key(key) {
                    // The command bus: every action goes through apply()
                    // (see DESIGN.md, "Building scripting-ready").
                    app.apply(cmd);
                }
                if app.quit {
                    break Ok(());
                }
            }
            Ok(_) => {}
            Err(e) => break Err(e),
        }
    };
    ratatui::restore();
    result
}

#[cfg(test)]
mod smoke {
    use super::*;

    #[test]
    fn quit_command_quits() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        let mut app = App::new(Box::new(db), None);
        app.apply(crate::app::Command::Quit);
        assert!(app.quit);
    }
}
