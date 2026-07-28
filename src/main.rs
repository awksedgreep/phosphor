//! phosphor — the 1988 green-screen database desktop, reborn.
//!
//!   phosphor <file.db>     open a SQLite/libSQL database file
//!   phosphor               open an in-memory scratch database
//!
//! PHOSPHOR_EXT=/path/to/libtimeless_ext.so loads the timeless extension
//! into embedded connections (enables dbhealth + telemetry vtabs).

mod app;
mod db;
mod theme;
mod ui;

use ratatui::crossterm::event::{self, Event, KeyEventKind};

use crate::app::App;
use crate::db::EmbeddedDb;

fn main() -> std::io::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| ":memory:".into());
    let (link, warning) = match EmbeddedDb::open(&path) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("phosphor: cannot open {path}: {e}");
            std::process::exit(1);
        }
    };
    let mut app = App::new(Box::new(link), warning);

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
