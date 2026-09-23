//! Colours, taken from the design mockup (docs/mockup.html) so the terminal and the mockup
//! stay one design.

use conductor_model::{Source, Verdict};
use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub text: Color,
    pub dim: Color,
    pub faint: Color,
    pub line: Color,
    pub accent: Color,
    pub sel: Color,
    pub pass: Color,
    pub fail: Color,
    pub warn: Color,
    pub run: Color,
    pub blocked: Color,
    pub blocked_bg: Color,
    pub pass_bg: Color,
}

impl Theme {
    pub const DARK: Theme = Theme {
        text: Color::Rgb(0xd8, 0xdf, 0xe8),
        dim: Color::Rgb(0x8a, 0x97, 0xa7),
        faint: Color::Rgb(0x5b, 0x66, 0x74),
        line: Color::Rgb(0x29, 0x31, 0x3b),
        accent: Color::Rgb(0x8e, 0xa5, 0xff),
        sel: Color::Rgb(0x1c, 0x24, 0x36),
        pass: Color::Rgb(0x72, 0xc9, 0x8f),
        fail: Color::Rgb(0xf0, 0x7d, 0x7d),
        warn: Color::Rgb(0xe3, 0xb5, 0x60),
        run: Color::Rgb(0x8e, 0xa5, 0xff),
        blocked: Color::Rgb(0xf0, 0x8b, 0xbd),
        blocked_bg: Color::Rgb(0x2c, 0x18, 0x23),
        pass_bg: Color::Rgb(0x15, 0x29, 0x1e),
    };

    pub const LIGHT: Theme = Theme {
        text: Color::Rgb(0x1c, 0x24, 0x30),
        dim: Color::Rgb(0x5f, 0x6a, 0x7a),
        faint: Color::Rgb(0x8f, 0x9a, 0xa8),
        line: Color::Rgb(0xcf, 0xd6, 0xde),
        accent: Color::Rgb(0x35, 0x52, 0xcc),
        sel: Color::Rgb(0xe2, 0xe8, 0xfb),
        pass: Color::Rgb(0x1b, 0x80, 0x48),
        fail: Color::Rgb(0xc2, 0x36, 0x3c),
        warn: Color::Rgb(0x9a, 0x63, 0x00),
        run: Color::Rgb(0x35, 0x52, 0xcc),
        blocked: Color::Rgb(0xb0, 0x30, 0x6e),
        blocked_bg: Color::Rgb(0xf8, 0xe3, 0xee),
        pass_bg: Color::Rgb(0xe1, 0xf2, 0xe8),
    };

    pub fn fg(&self, c: Color) -> Style {
        Style::new().fg(c)
    }

    pub fn text(&self) -> Style {
        self.fg(self.text)
    }

    pub fn dim(&self) -> Style {
        self.fg(self.dim)
    }

    pub fn faint(&self) -> Style {
        self.fg(self.faint)
    }

    pub fn bold(&self) -> Style {
        self.text().add_modifier(Modifier::BOLD)
    }

    pub fn verdict(&self, v: Verdict) -> Style {
        self.fg(match v {
            Verdict::Passed => self.pass,
            Verdict::Failed => self.fail,
            Verdict::Flaky | Verdict::Overridden => self.warn,
            Verdict::Running => self.run,
            Verdict::Blocked => self.blocked,
            Verdict::Pending => self.faint,
        })
    }

    pub fn source(&self, s: Source) -> Style {
        match s {
            Source::Observed => self.fg(self.run),
            Source::Measured => self.fg(self.pass),
            Source::Witnessed => self.bold(),
            Source::Inferred => self.fg(self.warn),
            Source::Human => self.fg(self.blocked),
            Source::Unmanaged => self.fg(self.fail),
        }
    }
}
