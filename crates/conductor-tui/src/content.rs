//! Reads an answer's text and decides how to show it. Fixed rules, no model call: the same
//! text always gives the same view, and anything a rule cannot read stays as written.
use conductor_model::task::{Fact, Presentation, ResultRow, SeriesPoint};

const GREEK: &[(&str, &str)] = &[
    ("alpha", "α"),
    ("beta", "β"),
    ("gamma", "γ"),
    ("delta", "δ"),
    ("epsilon", "ε"),
    ("varepsilon", "ε"),
    ("zeta", "ζ"),
    ("eta", "η"),
    ("theta", "θ"),
    ("kappa", "κ"),
    ("lambda", "λ"),
    ("mu", "μ"),
    ("nu", "ν"),
    ("xi", "ξ"),
    ("pi", "π"),
    ("rho", "ρ"),
    ("sigma", "σ"),
    ("tau", "τ"),
    ("phi", "φ"),
    ("varphi", "φ"),
    ("chi", "χ"),
    ("psi", "ψ"),
    ("omega", "ω"),
    ("Gamma", "Γ"),
    ("Delta", "Δ"),
    ("Theta", "Θ"),
    ("Lambda", "Λ"),
    ("Pi", "Π"),
    ("Sigma", "Σ"),
    ("Phi", "Φ"),
    ("Psi", "Ψ"),
    ("Omega", "Ω"),
];
const SYMBOLS: &[(&str, &str)] = &[
    ("cdot", "·"),
    ("times", "×"),
    ("div", "÷"),
    ("pm", "±"),
    ("mp", "∓"),
    ("leq", "≤"),
    ("le", "≤"),
    ("geq", "≥"),
    ("ge", "≥"),
    ("neq", "≠"),
    ("ne", "≠"),
    ("approx", "≈"),
    ("sim", "∼"),
    ("equiv", "≡"),
    ("propto", "∝"),
    ("infty", "∞"),
    ("partial", "∂"),
    ("nabla", "∇"),
    ("sum", "∑"),
    ("prod", "∏"),
    ("int", "∫"),
    ("to", "→"),
    ("rightarrow", "→"),
    ("leftarrow", "←"),
    ("Rightarrow", "⇒"),
    ("in", "∈"),
    ("forall", "∀"),
    ("exists", "∃"),
    ("ldots", "…"),
    ("cdots", "⋯"),
    ("dots", "…"),
    ("quad", "   "),
    ("qquad", "      "),
];
/// Function names and sizing commands: the name stays, or nothing does.
const PLAIN: &[&str] = &[
    "ln", "log", "exp", "sin", "cos", "tan", "max", "min", "lim", "det", "Pr",
];
const DROPPED: &[&str] = &[
    "left",
    "right",
    "big",
    "Big",
    "bigl",
    "bigr",
    "Bigl",
    "Bigr",
    "displaystyle",
    "textstyle",
    "limits",
    "nolimits",
];
const WRAPPERS: &[&str] = &[
    "text",
    "mathrm",
    "mathbf",
    "mathit",
    "mathcal",
    "mathbb",
    "operatorname",
    "boldsymbol",
    "textbf",
    "textit",
    "bar",
    "hat",
    "tilde",
    "vec",
];

fn superscript(c: char) -> Option<char> {
    let from = "0123456789+-=()abcdefghijklmnoprstuvwxyzT";
    let to = "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻᵀ";
    from.chars()
        .position(|f| f == c)
        .and_then(|i| to.chars().nth(i))
}
fn subscript(c: char) -> Option<char> {
    let from = "0123456789+-=()aehijklmnoprstuvx";
    let to = "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓ";
    from.chars()
        .position(|f| f == c)
        .and_then(|i| to.chars().nth(i))
}

/// A single name or number, safe to put next to `/` or `√` without brackets.
fn atom(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}
fn bracket(s: &str) -> String {
    if atom(s) {
        s.to_owned()
    } else {
        format!("({s})")
    }
}

struct Tex {
    chars: Vec<char>,
    at: usize,
}
impl Tex {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }
    /// `{…}`, one command, or one character: what `^`, `_` and `\frac` apply to.
    fn argument(&mut self) -> String {
        while self.peek() == Some(' ') {
            self.at += 1;
        }
        match self.peek() {
            Some('{') => {
                self.at += 1;
                let inner = self.until(Some('}'));
                inner.trim().to_owned()
            }
            Some('\\') => self.command(),
            Some(c) => {
                self.at += 1;
                c.to_string()
            }
            None => String::new(),
        }
    }
    fn command(&mut self) -> String {
        self.at += 1;
        let Some(first) = self.peek() else {
            return "\\".into();
        };
        if !first.is_ascii_alphabetic() {
            self.at += 1;
            return match first {
                '\\' => "\n".into(),
                ',' | ';' | ':' | ' ' => " ".into(),
                '!' => String::new(),
                other => other.to_string(),
            };
        }
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.at += 1;
        }
        let name: String = self.chars[start..self.at].iter().collect();
        let lookup = |table: &[(&str, &str)]| {
            table
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        };
        match name.as_str() {
            "frac" | "tfrac" | "dfrac" => {
                let top = self.argument();
                let bottom = self.argument();
                match (top.as_str(), bottom.as_str()) {
                    ("1", "2") => "½".into(),
                    ("1", "3") => "⅓".into(),
                    ("1", "4") => "¼".into(),
                    _ => format!("{}/{}", bracket(&top), bracket(&bottom)),
                }
            }
            "sqrt" => {
                if self.peek() == Some('[') {
                    while self.peek().is_some_and(|c| c != ']') {
                        self.at += 1;
                    }
                    self.at += 1;
                }
                format!("√{}", bracket(&self.argument()))
            }
            "begin" | "end" => {
                self.argument();
                String::new()
            }
            _ if WRAPPERS.contains(&name.as_str()) => self.argument(),
            _ if DROPPED.contains(&name.as_str()) => String::new(),
            _ if PLAIN.contains(&name.as_str()) => name,
            _ => lookup(GREEK)
                .or_else(|| lookup(SYMBOLS))
                // An unknown command is shown as written rather than dropped.
                .unwrap_or_else(|| format!("\\{name}")),
        }
    }
    fn script(&mut self, raised: bool) -> String {
        self.at += 1;
        let inner = self.argument();
        let map = if raised { superscript } else { subscript };
        if !inner.is_empty()
            && let Some(mapped) = inner.chars().map(map).collect::<Option<String>>()
        {
            return mapped;
        }
        let mark = if raised { '^' } else { '_' };
        if atom(&inner) {
            format!("{mark}{inner}")
        } else {
            format!("{mark}({inner})")
        }
    }
    fn until(&mut self, close: Option<char>) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            match c {
                _ if Some(c) == close => {
                    self.at += 1;
                    return out;
                }
                '\\' => out.push_str(&self.command()),
                '^' => out.push_str(&self.script(true)),
                '_' => out.push_str(&self.script(false)),
                '{' => {
                    self.at += 1;
                    out.push_str(&self.until(Some('}')));
                }
                '&' => self.at += 1,
                '~' => {
                    self.at += 1;
                    out.push(' ');
                }
                _ => {
                    self.at += 1;
                    out.push(c);
                }
            }
        }
        out
    }
}

/// LaTeX maths as the nearest plain Unicode: `\frac{a}{b}` is `a/b`, `\sigma^2` is `σ²`.
pub fn tex(source: &str) -> String {
    let mut parser = Tex {
        chars: source.chars().collect(),
        at: 0,
    };
    let text = parser.until(None);
    text.lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// Whether `$…$` holds maths and not two prices in one sentence.
fn mathy(inner: &str) -> bool {
    !inner.is_empty()
        && !inner.starts_with(' ')
        && !inner.ends_with(' ')
        && inner.contains(['\\', '^', '_', '='])
}

/// Rewrites LaTeX maths so the Markdown renderer can show it: display maths becomes a fenced
/// `math` block, one formula per line, and inline maths becomes inline code. Text inside
/// code is left alone.
pub fn math(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut fenced = false;
    let mut line_start = true;
    // Where the last formula block's closing fence starts, to join formulas that follow on.
    let mut open_block: Option<usize> = None;
    let mut display = |out: &mut String, inner: &str| {
        let formulas: Vec<_> = inner
            .split("\\qquad")
            .flat_map(|part| tex(part).lines().map(str::to_owned).collect::<Vec<_>>())
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty())
            .collect();
        if formulas.is_empty() || formulas.iter().any(|f| f.contains("```")) {
            return false;
        }
        match open_block {
            Some(fence) if out[fence..].trim() == "```" => out.truncate(fence),
            _ => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("\n```math\n");
            }
        }
        out.push_str(&formulas.join("\n"));
        out.push('\n');
        open_block = Some(out.len());
        out.push_str("```\n\n");
        true
    };
    let inline = |out: &mut String, inner: &str| {
        let shown = tex(inner);
        if shown.is_empty() || shown.contains(['`', '\n']) {
            return false;
        }
        out.push('`');
        out.push_str(&shown);
        out.push('`');
        true
    };
    while let Some(c) = rest.chars().next() {
        if line_start && rest.trim_start_matches(' ').starts_with("```") {
            fenced = !fenced;
        }
        line_start = c == '\n';
        if fenced {
            out.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        }
        let pairs: [(&str, &str, bool); 4] = [
            ("\\[", "\\]", true),
            ("$$", "$$", true),
            ("\\(", "\\)", false),
            ("$", "$", false),
        ];
        let mut matched = false;
        if c == '`' {
            // Inline code: copy through to its closing mark untouched.
            let end = rest[1..].find('`').map(|i| i + 2).unwrap_or(rest.len());
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        for (open, close, block) in pairs {
            if !rest.starts_with(open) {
                continue;
            }
            let body = &rest[open.len()..];
            let Some(end) = body.find(close) else {
                continue;
            };
            let inner = &body[..end];
            if open == "$" && (!mathy(inner) || inner.contains('\n')) {
                continue;
            }
            let done = if block {
                display(&mut out, inner)
            } else {
                inline(&mut out, inner)
            };
            if done {
                rest = &body[end + close.len()..];
                matched = true;
                break;
            }
        }
        if !matched {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// Markdown marks that would show as punctuation inside a widget's cell.
fn bare(cell: &str) -> String {
    cell.replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .trim()
        .to_owned()
}

/// Pulls `[2]` style references out of a cell, keeping only ones this answer can resolve.
fn references(cell: &str, sources: usize) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut refs = vec![];
    let mut rest = cell;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        match after.find(']') {
            Some(close)
                if after[..close]
                    .parse::<usize>()
                    .is_ok_and(|n| n > 0 && n <= sources)
                    && !after[close + 1..].starts_with('(') =>
            {
                refs.push(after[..close].parse().unwrap());
                text.push_str(&rest[..open]);
                rest = &after[close + 1..];
            }
            _ => {
                text.push_str(&rest[..open + 1]);
                rest = after;
            }
        }
    }
    text.push_str(rest);
    (text.trim().to_owned(), refs)
}

fn cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(bare)
        .collect()
}
fn divider(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('|') && line.contains('-') && line.chars().all(|c| "|-: ".contains(c))
}
/// `12.5 °C` as the number and what follows it.
fn measure(cell: &str) -> Option<(f64, String)> {
    let cell = cell.trim();
    let end = cell
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || "+-.,".contains(*c)))
        .map(|(i, _)| i)
        .unwrap_or(cell.len());
    let value: f64 = cell[..end].replace(',', "").parse().ok()?;
    Some((value, cell[end..].trim().to_owned()))
}

/// The short text around a widget: a heading becomes its title, the rest its summary.
fn caption(prose: &[&str], fallback: &str) -> Option<(String, String)> {
    let mut title = None;
    let mut summary = vec![];
    for line in prose {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if title.is_none() && line.starts_with('#') {
            title = Some(bare(line.trim_start_matches('#')));
        } else {
            summary.push(bare(line.trim_start_matches('#')));
        }
    }
    let summary = summary.join(" ");
    // A long explanation is an answer to read, not a caption to tuck under a widget.
    if summary.chars().count() > 400 {
        return None;
    }
    let title = title.unwrap_or_else(|| fallback.to_owned());
    let summary = if summary.is_empty() {
        title.clone()
    } else {
        summary
    };
    Some((title, summary))
}

fn table(lines: &[&str], sources: usize) -> Option<Presentation> {
    let start = lines
        .windows(2)
        .position(|w| w[0].trim().starts_with('|') && divider(w[1]))?;
    let end = lines[start..]
        .iter()
        .position(|l| !l.trim().starts_with('|'))
        .map_or(lines.len(), |n| start + n);
    let prose: Vec<&str> = lines[..start]
        .iter()
        .chain(&lines[end..])
        .copied()
        .collect();
    // A second table, code or a formula means a document, not one widget.
    if prose
        .iter()
        .any(|l| l.trim().starts_with('|') || l.contains("```") || l.contains("\\["))
    {
        return None;
    }
    let columns = cells(lines[start]);
    if !(2..=8).contains(&columns.len()) || columns.iter().any(String::is_empty) {
        return None;
    }
    let mut rows = vec![];
    for line in &lines[start + 2..end] {
        let mut row = vec![];
        let mut refs = vec![];
        for cell in cells(line) {
            let (text, found) = references(&cell, sources);
            row.push(if text.is_empty() { "—".into() } else { text });
            refs.extend(found);
        }
        if row.len() != columns.len() {
            return None;
        }
        refs.sort_unstable();
        refs.dedup();
        rows.push(ResultRow {
            cells: row,
            sources: refs,
        });
    }
    if rows.is_empty() || rows.len() > 200 {
        return None;
    }
    let (title, summary) = caption(&prose, &columns.join(" · "))?;
    // One number per row over an ordered first column reads better as a chart.
    let measures: Option<Vec<_>> = rows.iter().map(|r| measure(&r.cells[1])).collect();
    if let Some(measures) = measures.filter(|_| rows.len() >= 3 && columns.len() <= 3)
        && measures.iter().all(|(_, unit)| *unit == measures[0].1)
    {
        let unit = if measures[0].1.is_empty() {
            columns[1].clone()
        } else {
            measures[0].1.clone()
        };
        return Some(Presentation::Series {
            title,
            summary,
            unit,
            points: rows
                .into_iter()
                .zip(measures)
                .map(|(row, (value, _))| SeriesPoint {
                    detail: row
                        .cells
                        .get(2)
                        .cloned()
                        .unwrap_or_else(|| format!("{}: {}", row.cells[0], row.cells[1])),
                    label: row.cells[0].clone(),
                    value,
                    sources: row.sources,
                })
                .collect(),
        });
    }
    Some(Presentation::Table {
        title,
        summary,
        columns,
        rows,
    })
}

fn facts(lines: &[&str], sources: usize) -> Option<Presentation> {
    let mut facts = vec![];
    let mut prose = vec![];
    for line in lines {
        let trimmed = line.trim();
        let item = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("• "));
        let labelled = item.or_else(|| trimmed.starts_with("**").then_some(trimmed));
        let pair = labelled.and_then(|text| {
            let (label, value) = text.split_once(':')?;
            let (label, value) = (bare(label), bare(value));
            let (value, refs) = references(&value, sources);
            (!label.is_empty()
                && label.chars().count() <= 40
                && !value.is_empty()
                && !value.starts_with("//"))
            .then_some(Fact {
                label,
                value,
                sources: refs,
            })
        });
        match pair {
            Some(fact) => facts.push(fact),
            // A list item that is not `label: value` means this is a list to read.
            None if item.is_some() => return None,
            None => prose.push(*line),
        }
    }
    if !(3..=30).contains(&facts.len())
        || prose
            .iter()
            .any(|l| l.contains("```") || l.contains("\\[") || l.trim().starts_with('|'))
    {
        return None;
    }
    let (title, summary) = caption(&prose, "Key facts")?;
    Some(Presentation::Facts {
        title,
        summary,
        facts,
    })
}

/// The widget an answer's text calls for, when the agent did not name one: a lone table
/// becomes a selectable table or a chart, a list of `label: value` lines becomes key facts.
/// Mixed or long text returns `None` and is shown as a document.
pub fn detect(text: &str, sources: usize) -> Option<Presentation> {
    let lines: Vec<&str> = text.lines().collect();
    table(&lines, sources).or_else(|| facts(&lines, sources))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formulas_become_readable_unicode_and_unknown_commands_are_kept() {
        assert_eq!(
            tex(r"d_1 = \frac{\ln(S/K) + (r_d - r_f + \tfrac{1}{2}\sigma^2)T}{\sigma\sqrt{T}}"),
            "d₁ = (ln(S/K) + (r_d - r_f + ½σ²)T)/(σ√T)"
        );
        assert_eq!(tex(r"C = S e^{-r_f T} N(d_1)"), "C = S e^(-r_f T) N(d₁)");
        assert_eq!(tex(r"x^2 + \alpha_i \leq \infty"), "x² + αᵢ ≤ ∞");
        assert_eq!(tex(r"\mystery{x}"), r"\mysteryx");
    }

    #[test]
    fn display_maths_is_one_formula_per_line_and_code_is_untouched() {
        let out = math(
            "Use:\n\\[d_1 = \\frac{a}{b}, \\qquad d_2 = d_1 - \\sigma\\sqrt{T}\\]\nwhere \\(\\sigma\\) is vol and `\\(raw\\)` stays. It costs $5 and $6.\n```\n\\[x\\]\n```",
        );
        assert!(
            out.contains("```math\nd₁ = a/b,\nd₂ = d₁ - σ√T\n```"),
            "{out}"
        );
        assert!(out.contains("where `σ` is vol"));
        assert!(out.contains("`\\(raw\\)` stays"));
        assert!(out.contains("$5 and $6"));
        assert!(out.contains("```\n\\[x\\]\n```"));
    }

    #[test]
    fn a_lone_table_is_a_table_and_measurements_in_order_are_a_chart() {
        let text = "## Retry strategies\n\nTwo common choices.\n\n| Strategy | Wait | Fits |\n|---|---|---|\n| Backoff | Doubles [1] | Shared services |\n| Fixed | Constant | Simple jobs |";
        let Some(Presentation::Table {
            title,
            summary,
            columns,
            rows,
        }) = detect(text, 1)
        else {
            panic!("expected a table");
        };
        assert_eq!(title, "Retry strategies");
        assert_eq!(summary, "Two common choices.");
        assert_eq!(columns, ["Strategy", "Wait", "Fits"]);
        assert_eq!(rows[0].cells[1], "Doubles");
        assert_eq!(rows[0].sources, [1]);
        let chart =
            "| Hour | Temp |\n|---|---|\n| 06:00 | 17 °C |\n| 12:00 | 24 °C |\n| 18:00 | 21 °C |";
        let Some(Presentation::Series { unit, points, .. }) = detect(chart, 0) else {
            panic!("expected a chart");
        };
        assert_eq!(unit, "°C");
        assert_eq!(points[1].value, 24.0);
    }

    #[test]
    fn labelled_lines_are_facts_and_prose_stays_a_document() {
        let text = "## Melbourne\n\n- **Conditions:** Light drizzle [1]\n- **Temperature:** 11°C\n- **Humidity:** 98%";
        let Some(Presentation::Facts { title, facts, .. }) = detect(text, 1) else {
            panic!("expected facts");
        };
        assert_eq!(title, "Melbourne");
        assert_eq!(facts[0].label, "Conditions");
        assert_eq!(facts[0].value, "Light drizzle");
        assert_eq!(facts[0].sources, [1]);
        assert_eq!(
            detect("Some prose.\n\n- a point\n- another: thing\n- third", 0),
            None
        );
        assert_eq!(detect("Just a sentence about retries.", 0), None);
        let long = format!("{}\n\n| A | B |\n|---|---|\n| 1 | 2 |", "word ".repeat(120));
        assert_eq!(detect(&long, 0), None);
    }
}
