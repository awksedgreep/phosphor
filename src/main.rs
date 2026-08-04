//! phosphor — the 1988 green-screen database desktop, reborn.
//!
//!   phosphor <file.db>     open a SQLite/libSQL database file
//!   phosphor               open an in-memory scratch database
//!
//! PHOSPHOR_EXT=/path/to/libtimeless_ext.so loads the timeless extension
//! into embedded connections (enables dbhealth + telemetry vtabs).

mod app;
mod appsgen;
mod db;
mod forms;
mod help;
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

const USAGE: &str = "\
phosphor — the green-screen database desktop of 1988, reborn

USAGE
    phosphor [OPTIONS] [DATABASE]

    DATABASE     a SQLite/libSQL file (created if missing), or an
                 http(s):// URL of a self-hosted sqld server.
                 Defaults to an in-memory scratch database.

OPTIONS
    --app [NAME]   boot into an application menu crafted with the
                   Applications Generator (A inside phosphor)
    --manual       print the full manual as markdown and exit
    -h, --help     this help
    -V, --version  version

ENVIRONMENT
    PHOSPHOR_EXT    path to libdbhealth_ext.so / libtimeless_ext.so —
                    loads dbhealth + telemetry into embedded databases
    PHOSPHOR_TOKEN  bearer token for authenticated sqld/Turso servers

Inside phosphor: F1 is the manual, Esc always backs out, . is the
dot prompt, q quits from the top level (Ctrl-Q from anywhere).
";

fn main() -> std::io::Result<()> {
    // phosphor [--app [name]] <file.db | http(s)://sqld>
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("phosphor {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--manual") {
        print!("{}", help::manual_markdown());
        return Ok(());
    }
    let mut app_mode = false;
    let mut app_name: Option<String> = None;
    if let Some(i) = args.iter().position(|a| a == "--app") {
        app_mode = true;
        args.remove(i);
        // `--app crm file.db`: a non-path next arg is the app name.
        if i < args.len() && !args[i].contains('.') && !args[i].starts_with("http") {
            app_name = Some(args.remove(i));
        }
    }
    let path = args
        .first()
        .cloned()
        .unwrap_or_else(|| ":memory:".into());
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
    if app_mode {
        // The database IS the application (DESIGN.md phase 5).
        app.app_home = app_name.clone().or_else(|| {
            crate::appsgen::list_apps(app.db.as_ref()).into_iter().next()
        });
        app.apply(app::Command::OpenAppMenu(app.app_home.clone()));
    }

    fn finish(
        _terminal: ratatui::DefaultTerminal,
        r: std::io::Result<()>,
    ) -> std::io::Result<()> {
        ratatui::restore();
        r
    }

    let mut terminal = ratatui::init();
    let result = loop {
        if let Err(e) = terminal.draw(|f| ui::draw(f, &mut app)) {
            break Err(e);
        }
        // Poll instead of block so time-based behavior (the live health
        // console) can tick between keystrokes.
        // Drain EVERY pending event before redrawing: at high key-
        // repeat rates one-event-per-frame made rendering the speed
        // limit (record paging capped at the autorepeat rate).
        let mut budget = 64;
        let mut first = true;
        loop {
            let ready = if first {
                event::poll(std::time::Duration::from_millis(250))
            } else {
                event::poll(std::time::Duration::ZERO)
            };
            first = false;
            match ready {
                Ok(true) => match event::read() {
                    Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                        if let Some(cmd) = app.map_key(key) {
                            // The command bus: every action goes through
                            // apply() (DESIGN.md, "Building scripting-ready").
                            app.apply(cmd);
                        }
                        budget -= 1;
                        if budget == 0 || app.quit {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(e) => return finish(terminal, Err(e)),
                },
                Ok(false) => break,
                Err(e) => return finish(terminal, Err(e)),
            }
        }
        app.tick();
        if app.quit {
            break Ok(());
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
