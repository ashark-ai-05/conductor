//! Terminal palette for the agent workbench; meaning never relies on colour alone.

use conductor_model::{Source, Verdict};
use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub background: Color,
    pub surface: Color,
    pub fail_bg: Color,
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
        background: Color::Rgb(0x17, 0x1b, 0x20),
        surface: Color::Rgb(0x1d, 0x23, 0x2a),
        fail_bg: Color::Rgb(0x36, 0x24, 0x29),
        text: Color::Rgb(0xd8, 0xdf, 0xe8),
        dim: Color::Rgb(0x8a, 0x97, 0xa7),
        faint: Color::Rgb(0x5b, 0x66, 0x74),
        line: Color::Rgb(0x29, 0x31, 0x3b),
        accent: Color::Rgb(0x8f, 0xba, 0xfa),
        sel: Color::Rgb(0x1c, 0x24, 0x36),
        pass: Color::Rgb(0x82, 0xd9, 0xc2),
        fail: Color::Rgb(0xf0, 0x7d, 0x7d),
        warn: Color::Rgb(0xe3, 0xb5, 0x60),
        run: Color::Rgb(0x8e, 0xa5, 0xff),
        blocked: Color::Rgb(0xf0, 0x8b, 0xbd),
        blocked_bg: Color::Rgb(0x2c, 0x18, 0x23),
        pass_bg: Color::Rgb(0x15, 0x29, 0x1e),
    };

    pub const LIGHT: Theme = Theme {
        background: Color::Rgb(0xfa, 0xfb, 0xfc),
        surface: Color::Rgb(0xf0, 0xf3, 0xf5),
        fail_bg: Color::Rgb(0xfa, 0xe9, 0xe9),
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

    /// The work-in-progress colour, breathing towards dim and back over sixteen frames.
    pub fn pulse(&self, frame: usize) -> Style {
        let step = (frame % 16) as u16;
        let k = if step < 8 { step } else { 16 - step };
        self.fg(match (self.run, self.dim) {
            (Color::Rgb(r, g, b), Color::Rgb(r2, g2, b2)) => {
                let mix = |a: u8, b: u8| ((a as u16 * (16 - k) + b as u16 * k) / 16) as u8;
                Color::Rgb(mix(r, r2), mix(g, g2), mix(b, b2))
            }
            (run, _) => run,
        })
    }

    pub fn verdict(&self, v: Verdict) -> Style {
        self.fg(match v {
            Verdict::Passed => self.pass,
            Verdict::Failed => self.fail,
            Verdict::Flaky | Verdict::Overridden | Verdict::Unwitnessed => self.warn,
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
