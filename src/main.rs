//! phosphor phase 0: the name renders in green. (DESIGN.md is the map.)

use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// P1 phosphor green — the color this project is named after.
const P1: Color = Color::Rgb(0x33, 0xff, 0x33);
const P1_DIM: Color = Color::Rgb(0x1a, 0x99, 0x1a);

fn main() -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let result = loop {
        if let Err(e) = terminal.draw(draw) {
            break Err(e);
        }
        match event::read() {
            Ok(Event::Key(key)) => {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
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

fn draw(f: &mut Frame) {
    let base = Style::default().fg(P1);
    let dim = Style::default().fg(P1_DIM);
    let bright = Style::default().fg(P1).add_modifier(Modifier::BOLD);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(dim)
        .title(Span::styled(" PHOSPHOR ", bright))
        .title_alignment(Alignment::Left);
    let inner = outer.inner(f.area());
    f.render_widget(outer, f.area());

    let [_, banner, panels, _, prompt, status] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(7),
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    let logo = [
        r"       _                     _                ",
        r" _ __ | |__   ___  ___ _ __ | |__   ___  _ __ ",
        r"| '_ \| '_ \ / _ \/ __| '_ \| '_ \ / _ \| '__|",
        r"| |_) | | | | (_) \__ \ |_) | | | | (_) | |   ",
        r"| .__/|_| |_|\___/|___/ .__/|_| |_|\___/|_|   ",
        r"|_|  the moon in a terminal  |_|  est. 1988/2026",
    ];
    f.render_widget(
        Paragraph::new(logo.iter().map(|l| Line::styled(*l, base)).collect::<Vec<_>>())
            .alignment(Alignment::Center),
        banner,
    );

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Data", bright),
            Span::styled("    Queries    Forms    Reports    Apps    Admin", dim),
        ]))
        .alignment(Alignment::Center),
        panels,
    );

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  . ", bright),
            Span::styled("_", Style::default().fg(P1).add_modifier(Modifier::SLOW_BLINK)),
        ])),
        prompt,
    );

    f.render_widget(
        Paragraph::new(Line::styled(
            " F1 Help   q/Esc Quit                        phase 0: DESIGN.md is the map ",
            dim,
        ))
        .alignment(Alignment::Right),
        status,
    );
}
