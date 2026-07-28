//! Phosphor themes. Green P1 is the default and the namesake.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color,
    fg: Color,
    dim_fg: Color,
    err_fg: Color,
}

pub const GREEN: Theme = Theme {
    name: "green",
    bg: Color::Rgb(0x03, 0x0a, 0x03),
    fg: Color::Rgb(0x33, 0xff, 0x33),
    dim_fg: Color::Rgb(0x1a, 0x99, 0x1a),
    err_fg: Color::Rgb(0xff, 0x66, 0x33),
};

pub const AMBER: Theme = Theme {
    name: "amber",
    bg: Color::Rgb(0x0a, 0x06, 0x00),
    fg: Color::Rgb(0xff, 0xb0, 0x00),
    dim_fg: Color::Rgb(0x99, 0x6a, 0x00),
    err_fg: Color::Rgb(0xff, 0x40, 0x40),
};

pub const PAPER: Theme = Theme {
    name: "paper",
    bg: Color::Rgb(0xf2, 0xef, 0xe5),
    fg: Color::Rgb(0x22, 0x22, 0x22),
    dim_fg: Color::Rgb(0x77, 0x77, 0x6a),
    err_fg: Color::Rgb(0xaa, 0x22, 0x22),
};

pub const BLUE: Theme = Theme {
    name: "blue",
    bg: Color::Rgb(0x00, 0x04, 0x10),
    fg: Color::Rgb(0x66, 0xcc, 0xff),
    dim_fg: Color::Rgb(0x2a, 0x66, 0x88),
    err_fg: Color::Rgb(0xff, 0x88, 0x44),
};

pub const ALL: [&Theme; 4] = [&GREEN, &AMBER, &PAPER, &BLUE];

impl Theme {
    pub fn by_name(name: &str) -> Option<&'static Theme> {
        ALL.into_iter().find(|t| t.name == name)
    }

    pub fn base(&self) -> Style {
        Style::default().fg(self.fg).bg(self.bg)
    }

    pub fn dim(&self) -> Style {
        Style::default().fg(self.dim_fg).bg(self.bg)
    }

    pub fn bright(&self) -> Style {
        self.base().add_modifier(Modifier::BOLD)
    }

    pub fn error(&self) -> Style {
        Style::default().fg(self.err_fg).bg(self.bg)
    }

    /// The cell/row cursor: inverse video, like God and IBM intended.
    pub fn cursor(&self) -> Style {
        Style::default().fg(self.bg).bg(self.fg)
    }

    pub fn health(&self, status: &str) -> Style {
        match status {
            "ok" => self.base(),
            "warn" => Style::default().fg(Color::Rgb(0xff, 0xb0, 0x00)).bg(self.bg),
            "attention" => self.error(),
            _ => self.dim(),
        }
    }
}
