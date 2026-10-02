//! Mermaid flowcharts and sequence diagrams drawn in terminal cells. Ratatui has no diagram
//! widget, so this lays the diagram out on a character grid. A diagram this cannot read or
//! fit returns `None`, and the caller shows its source instead.
use crate::theme::Theme;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, PartialEq)]
enum Ink {
    None,
    Frame,
    Label,
    Edge,
    Note,
}

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

struct Grid {
    cells: Vec<Vec<(char, Ink)>>,
    /// Which sides each edge cell joins, so crossing and merging edges pick the right glyph.
    joins: Vec<Vec<u8>>,
}
impl Grid {
    fn new(width: usize, height: usize) -> Self {
        Self {
            cells: vec![vec![(' ', Ink::None); width]; height],
            joins: vec![vec![0; width]; height],
        }
    }
    fn put(&mut self, x: usize, y: usize, c: char, ink: Ink) {
        if let Some(cell) = self.cells.get_mut(y).and_then(|row| row.get_mut(x)) {
            *cell = (c, ink);
        }
    }
    fn text(&mut self, x: usize, y: usize, text: &str, ink: Ink) {
        for (i, c) in text.chars().enumerate() {
            self.put(x + i, y, c, ink);
        }
    }
    fn join(&mut self, x: usize, y: usize, sides: u8) {
        let Some(mask) = self.joins.get_mut(y).and_then(|row| row.get_mut(x)) else {
            return;
        };
        *mask |= sides;
        let glyph = match *mask {
            m if m == UP | DOWN => '│',
            m if m == LEFT | RIGHT => '─',
            m if m == DOWN | RIGHT => '┌',
            m if m == DOWN | LEFT => '┐',
            m if m == UP | RIGHT => '└',
            m if m == UP | LEFT => '┘',
            m if m == UP | DOWN | RIGHT => '├',
            m if m == UP | DOWN | LEFT => '┤',
            m if m == LEFT | RIGHT | DOWN => '┬',
            m if m == LEFT | RIGHT | UP => '┴',
            m if m == UP | DOWN | LEFT | RIGHT => '┼',
            m if m & (UP | DOWN) != 0 => '│',
            _ => '─',
        };
        self.put(x, y, glyph, Ink::Edge);
    }
    fn frame(&mut self, x: usize, y: usize, width: usize, corners: [char; 6]) {
        let [tl, tr, bl, br, flat, side] = corners;
        for i in 1..width - 1 {
            self.put(x + i, y, flat, Ink::Frame);
            self.put(x + i, y + 2, flat, Ink::Frame);
        }
        self.put(x, y, tl, Ink::Frame);
        self.put(x + width - 1, y, tr, Ink::Frame);
        self.put(x, y + 1, side, Ink::Frame);
        self.put(x + width - 1, y + 1, side, Ink::Frame);
        self.put(x, y + 2, bl, Ink::Frame);
        self.put(x + width - 1, y + 2, br, Ink::Frame);
    }
    fn lines(mut self, t: &Theme) -> Vec<Line<'static>> {
        while self
            .cells
            .last()
            .is_some_and(|row| row.iter().all(|(c, _)| *c == ' '))
        {
            self.cells.pop();
        }
        self.cells
            .into_iter()
            .map(|row| {
                let end = row
                    .iter()
                    .rposition(|(c, _)| *c != ' ')
                    .map_or(0, |i| i + 1);
                let mut spans: Vec<Span<'static>> = vec![];
                let mut last = Ink::None;
                for (c, ink) in row.into_iter().take(end) {
                    if ink != last || spans.is_empty() {
                        spans.push(Span::styled(
                            String::new(),
                            match ink {
                                Ink::None => t.text(),
                                Ink::Frame => t.fg(t.run),
                                Ink::Label => t.bold(),
                                Ink::Edge => t.fg(t.accent),
                                Ink::Note => t.dim(),
                            },
                        ));
                        last = ink;
                    }
                    spans.last_mut().unwrap().content.to_mut().push(c);
                }
                Line::from(spans)
            })
            .collect()
    }
}

/// Labels are laid out one character per cell; anything wider would break the drawing.
fn label(raw: &str) -> Option<String> {
    let text = raw
        .trim()
        .trim_matches('"')
        .replace("<br/>", " ")
        .replace("<br>", " ")
        .trim()
        .to_owned();
    (text.chars().count() == text.width() && text.chars().count() <= 40).then_some(text)
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Box,
    Round,
    Choice,
    /// A bend in an edge that passes a layer without stopping there.
    Pass,
}
struct Node {
    id: String,
    label: String,
    shape: Shape,
}
struct Edge {
    from: usize,
    to: usize,
    label: String,
    arrow: bool,
}

struct Flow {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}
impl Flow {
    fn node(&mut self, id: &str, shown: Option<(String, Shape)>) -> usize {
        let index = self
            .nodes
            .iter()
            .position(|n| n.id == id)
            .unwrap_or_else(|| {
                self.nodes.push(Node {
                    id: id.to_owned(),
                    label: id.to_owned(),
                    shape: Shape::Box,
                });
                self.nodes.len() - 1
            });
        if let Some((label, shape)) = shown {
            self.nodes[index].label = label;
            self.nodes[index].shape = shape;
        }
        index
    }
    /// `A[Label]`, `B(Label)`, `C{Label}` or a bare id, returning the rest of the line.
    fn read_node<'a>(&mut self, text: &'a str) -> Option<(usize, &'a str)> {
        let text = text.trim_start();
        let end = text
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(text.len());
        if end == 0 {
            return None;
        }
        let (id, rest) = text.split_at(end);
        let open = rest
            .find(|c: char| !"[({>/\\".contains(c))
            .unwrap_or(rest.len());
        if open == 0 {
            return Some((self.node(id, None), rest));
        }
        let shape = match rest.chars().next() {
            Some('(') => Shape::Round,
            Some('{') => Shape::Choice,
            _ => Shape::Box,
        };
        let body = &rest[open..];
        let quoted = body.starts_with('"');
        let close = if quoted {
            body[1..].find('"').map(|i| i + 2)?
        } else {
            body.find(|c: char| "])}".contains(c))?
        };
        let shown = label(body[..close].trim_end_matches(['/', '\\']))?;
        let after = body[close..].trim_start_matches(|c: char| "])}/\\".contains(c));
        Some((self.node(id, Some((shown, shape))), after))
    }
    /// `-->`, `---`, `-.->`, `==>`, `-->|text|` or `-- text -->`, returning the rest.
    fn read_link(text: &str) -> Option<(String, bool, &str)> {
        let text = text.trim_start();
        let link = |c: char| "<-=.>ox".contains(c);
        if !text.starts_with(['-', '=', '<']) {
            return None;
        }
        let end = text.find(|c: char| !link(c)).unwrap_or(text.len());
        let (mut arrow, mut rest) = (&text[..end], &text[end..]);
        let mut shown = String::new();
        if !arrow.ends_with(['>', 'o', 'x']) && !matches!(arrow, "---" | "===" | "-.-") {
            // `-- text -->`: the words sit between two halves of the link.
            let close = ["--", "==", "-."]
                .iter()
                .filter_map(|mark| rest.find(mark))
                .min()?;
            shown = label(&rest[..close])?;
            let tail = &rest[close..];
            let end = tail.find(|c: char| !link(c)).unwrap_or(tail.len());
            arrow = &tail[..end];
            rest = &tail[end..];
        }
        let rest = rest.trim_start();
        let rest = if let Some(body) = rest.strip_prefix('|') {
            let close = body.find('|')?;
            shown = label(&body[..close])?;
            &body[close + 1..]
        } else {
            rest
        };
        Some((shown, arrow.ends_with('>'), rest))
    }
    fn parse(source: &str) -> Option<Self> {
        let mut flow = Flow {
            nodes: vec![],
            edges: vec![],
        };
        for line in source.lines().skip(1) {
            let line = line.trim().trim_end_matches(';');
            let skip = [
                "%%",
                "subgraph",
                "end",
                "classDef",
                "class ",
                "style ",
                "linkStyle",
                "click ",
                "direction",
            ];
            if line.is_empty() || skip.iter().any(|word| line.starts_with(word)) {
                continue;
            }
            let (first, mut rest) = flow.read_node(line)?;
            let mut sources = vec![first];
            loop {
                while let Some(more) = rest.trim_start().strip_prefix('&') {
                    let (node, after) = flow.read_node(more)?;
                    sources.push(node);
                    rest = after;
                }
                let Some((shown, arrow, after)) = Self::read_link(rest) else {
                    break;
                };
                let (first, mut after) = flow.read_node(after)?;
                let mut targets = vec![first];
                while let Some(more) = after.trim_start().strip_prefix('&') {
                    let (node, next) = flow.read_node(more)?;
                    targets.push(node);
                    after = next;
                }
                for from in &sources {
                    for to in &targets {
                        flow.edges.push(Edge {
                            from: *from,
                            to: *to,
                            label: shown.clone(),
                            arrow,
                        });
                    }
                }
                sources = targets;
                rest = after;
            }
            if !rest.trim().is_empty() {
                return None;
            }
        }
        (!flow.nodes.is_empty() && flow.nodes.len() <= 40).then_some(flow)
    }

    /// Top-down layers: every edge points to a lower layer, and an edge that would point
    /// back up (a loop) is drawn upwards with its arrowhead at the top.
    fn draw(mut self, width: usize, t: &Theme) -> Option<Vec<Line<'static>>> {
        let count = self.nodes.len();
        // Depth-first order finds the edges that close a loop.
        let mut state = vec![0u8; count];
        let mut back = vec![false; self.edges.len()];
        fn visit(n: usize, edges: &[Edge], state: &mut [u8], back: &mut [bool]) {
            state[n] = 1;
            for (i, e) in edges.iter().enumerate() {
                if e.from != n || e.from == e.to {
                    continue;
                }
                match state[e.to] {
                    0 => visit(e.to, edges, state, back),
                    1 => back[i] = true,
                    _ => {}
                }
            }
            state[n] = 2;
        }
        for n in 0..count {
            if state[n] == 0 {
                visit(n, &self.edges, &mut state, &mut back);
            }
        }
        // (upper, lower, label, arrow at the lower end, arrow at the upper end)
        let mut links: Vec<(usize, usize, String, bool, bool)> = self
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.from != e.to)
            .map(|(i, e)| {
                if back[i] {
                    (e.to, e.from, e.label.clone(), false, e.arrow)
                } else {
                    (e.from, e.to, e.label.clone(), e.arrow, false)
                }
            })
            .collect();
        let mut layer = vec![0usize; count];
        for _ in 0..count {
            for (upper, lower, ..) in &links {
                layer[*lower] = layer[*lower].max(layer[*upper] + 1);
            }
        }
        // An edge that skips layers gets a pass-through point in each one it crosses.
        let mut hops = vec![];
        for (upper, lower, shown, down, up) in std::mem::take(&mut links) {
            let mut from = upper;
            for depth in layer[upper] + 1..layer[lower] {
                self.nodes.push(Node {
                    id: String::new(),
                    label: String::new(),
                    shape: Shape::Pass,
                });
                layer.push(depth);
                let pass = self.nodes.len() - 1;
                hops.push((from, pass, String::new(), false, up && from == upper));
                from = pass;
            }
            hops.push((from, lower, shown, down, up && from == upper));
        }
        let depth = layer.iter().max().map_or(0, |d| d + 1);
        let mut rows: Vec<Vec<usize>> = vec![vec![]; depth];
        for (n, d) in layer.iter().enumerate() {
            rows[*d].push(n);
        }
        let size = |n: &Node| match n.shape {
            Shape::Pass => 1,
            _ => n.label.chars().count() + 4,
        };
        let mut centre = vec![0usize; self.nodes.len()];
        let mut left = vec![0usize; self.nodes.len()];
        let row_width = |row: &[usize], nodes: &[Node]| {
            row.iter().map(|n| size(&nodes[*n])).sum::<usize>() + row.len().saturating_sub(1) * 3
        };
        let total = rows
            .iter()
            .map(|row| row_width(row, &self.nodes))
            .max()
            .unwrap_or(0);
        if total > width || total == 0 {
            return None;
        }
        // Each layer is placed using the centres already set for the layer above.
        #[allow(clippy::needless_range_loop)]
        for d in 0..depth {
            // Keep each node under the nodes that lead to it, so edges cross less.
            if d > 0 {
                let above = |n: usize| {
                    let from: Vec<_> = hops
                        .iter()
                        .filter(|h| h.1 == n)
                        .map(|h| centre[h.0])
                        .collect();
                    if from.is_empty() {
                        usize::MAX
                    } else {
                        from.iter().sum::<usize>() / from.len()
                    }
                };
                rows[d].sort_by_key(|n| above(*n));
            }
            let mut x = (total - row_width(&rows[d], &self.nodes)) / 2;
            for n in &rows[d] {
                left[*n] = x;
                centre[*n] = x + size(&self.nodes[*n]) / 2;
                x += size(&self.nodes[*n]) + 3;
            }
        }
        let mut grid = Grid::new(total + 24, depth * 6);
        for (n, node) in self.nodes.iter().enumerate() {
            let y = layer[n] * 6;
            match node.shape {
                Shape::Pass => {
                    for dy in 0..3 {
                        grid.join(centre[n], y + dy, UP | DOWN);
                    }
                }
                shape => {
                    grid.frame(
                        left[n],
                        y,
                        size(node),
                        match shape {
                            Shape::Round => ['╭', '╮', '╰', '╯', '─', '│'],
                            Shape::Choice => ['╔', '╗', '╚', '╝', '═', '║'],
                            _ => ['┌', '┐', '└', '┘', '─', '│'],
                        },
                    );
                    grid.text(left[n] + 2, y + 1, &node.label, Ink::Label);
                }
            }
        }
        for (upper, lower, shown, down, up) in &hops {
            let (sx, tx) = (centre[*upper], centre[*lower]);
            // A one-cell jog between boxes of odd and even width is noise: go straight down.
            let tx = if sx.abs_diff(tx) <= 1 { sx } else { tx };
            let y = layer[*upper] * 6 + 3;
            grid.join(sx, y, UP | DOWN);
            if sx == tx {
                grid.join(sx, y + 1, UP | DOWN);
            } else {
                let (low, high) = (sx.min(tx), sx.max(tx));
                grid.join(sx, y + 1, UP | if tx > sx { RIGHT } else { LEFT });
                grid.join(tx, y + 1, DOWN | if tx > sx { LEFT } else { RIGHT });
                for x in low + 1..high {
                    grid.join(x, y + 1, LEFT | RIGHT);
                }
            }
            grid.join(tx, y + 2, UP | DOWN);
            if *up {
                grid.put(sx, y, '▲', Ink::Edge);
            }
            if *down {
                grid.put(tx, y + 2, '▼', Ink::Edge);
            }
            if !shown.is_empty() {
                grid.text(tx + 2, y + 2, shown, Ink::Note);
            }
        }
        Some(grid.lines(t))
    }
}

/// `A->>B: text`, split into the two names, whether the line is dashed, and the text.
fn message(line: &str) -> Option<(&str, &str, bool, &str)> {
    let (link, text) = line.split_once(':').unwrap_or((line, ""));
    for arrow in ["-->>", "->>", "-->", "->", "--x", "-x", "--)", "-)"] {
        if let Some((from, to)) = link.split_once(arrow) {
            let to = to.trim().trim_start_matches(['+', '-']);
            return Some((from.trim(), to, arrow.starts_with("--"), text.trim()));
        }
    }
    None
}

fn sequence(source: &str, width: usize, t: &Theme) -> Option<Vec<Line<'static>>> {
    enum Step {
        Message(usize, usize, bool, String),
        Note(String),
    }
    let mut names: Vec<(String, String)> = vec![];
    let mut steps = vec![];
    let find = |names: &mut Vec<(String, String)>, id: &str| -> Option<usize> {
        if let Some(i) = names.iter().position(|(key, _)| key == id) {
            return Some(i);
        }
        names.push((id.to_owned(), label(id)?));
        Some(names.len() - 1)
    };
    for line in source.lines().skip(1) {
        let line = line.trim();
        let first = line.split_whitespace().next().unwrap_or("");
        match first {
            "" | "%%" | "end" | "activate" | "deactivate" | "autonumber" => {}
            "participant" | "actor" => {
                let rest = line[first.len()..].trim();
                let (id, shown) = rest.split_once(" as ").unwrap_or((rest, rest));
                let i = find(&mut names, id.trim())?;
                names[i].1 = label(shown)?;
            }
            "Note" | "note" | "loop" | "alt" | "else" | "opt" | "par" | "and" => {
                let text = line.split_once(':').map_or(line, |(_, text)| text);
                steps.push(Step::Note(label(text)?));
            }
            _ => {
                let (from, to, dashed, text) = message(line)?;
                let from = find(&mut names, from)?;
                let to = find(&mut names, to)?;
                steps.push(Step::Message(from, to, dashed, label(text)?));
            }
        }
    }
    if names.is_empty() || names.len() > 8 || steps.is_empty() {
        return None;
    }
    // Lifelines sit far enough apart for the longest message between them.
    let mut gap = vec![0usize; names.len()];
    for i in 1..names.len() {
        gap[i] = (names[i - 1].1.chars().count() + names[i].1.chars().count()) / 2 + 6;
    }
    for step in &steps {
        if let Step::Message(from, to, _, text) = step
            && from != to
        {
            let (low, high) = (*from.min(to), *from.max(to));
            let each = (text.chars().count() + 4).div_ceil(high - low);
            for g in &mut gap[low + 1..=high] {
                *g = (*g).max(each);
            }
        }
    }
    let half = names[0].1.chars().count() / 2 + 2;
    let mut at = vec![half; names.len()];
    for i in 1..names.len() {
        at[i] = at[i - 1] + gap[i];
    }
    let last = names.last()?.1.chars().count();
    let total = at[names.len() - 1] + last / 2 + 3;
    if total > width {
        return None;
    }
    let mut grid = Grid::new(total + 42, 3 + steps.len() * 2 + 1);
    for (i, (_, shown)) in names.iter().enumerate() {
        let size = shown.chars().count() + 4;
        let x = at[i] - size / 2;
        grid.frame(x, 0, size, ['┌', '┐', '└', '┘', '─', '│']);
        grid.text(x + 2, 1, shown, Ink::Label);
    }
    for y in 3..3 + steps.len() * 2 + 1 {
        for x in &at {
            grid.put(*x, y, '│', Ink::Frame);
        }
    }
    for (n, step) in steps.iter().enumerate() {
        let y = 3 + n * 2;
        match step {
            Step::Note(text) => grid.text(at[0] + 2, y + 1, text, Ink::Note),
            Step::Message(from, to, _, text) if from == to => {
                grid.text(at[*from] + 1, y + 1, "◀╮ ", Ink::Edge);
                grid.text(at[*from] + 4, y + 1, text, Ink::Note);
            }
            Step::Message(from, to, dashed, text) => {
                let (low, high) = (at[*from.min(to)], at[*from.max(to)]);
                let room = high - low - 1;
                let shown = text.chars().count();
                grid.text(low + 1 + room.saturating_sub(shown) / 2, y, text, Ink::Note);
                for x in low + 1..high {
                    grid.put(x, y + 1, if *dashed { '╌' } else { '─' }, Ink::Edge);
                }
                if to > from {
                    grid.put(high - 1, y + 1, '▶', Ink::Edge);
                } else {
                    grid.put(low + 1, y + 1, '◀', Ink::Edge);
                }
            }
        }
    }
    Some(grid.lines(t))
}

/// Draws a Mermaid flowchart or sequence diagram, or returns `None` for a kind, a syntax or
/// a size this does not handle. Every flowchart is laid out top-down.
pub fn mermaid(source: &str, width: u16, t: &Theme) -> Option<Vec<Line<'static>>> {
    let source = source.trim();
    let kind = source.split_whitespace().next()?;
    match kind {
        "graph" | "flowchart" => Flow::parse(source)?.draw(width as usize, t),
        "sequenceDiagram" => sequence(source, width as usize, t),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_flowchart_is_drawn_as_boxes_joined_by_arrows() {
        let out = text(
            &mermaid(
                "graph TD\n  A[Bug reported] --> B{Reproduced?}\n  B -->|yes| C(Fix)\n  B -->|no| D[Ask reporter]\n  C --> E[Deploy]\n  D --> A",
                100,
                &Theme::DARK,
            )
            .unwrap(),
        );
        for word in [
            "Bug reported",
            "Reproduced?",
            "Fix",
            "Ask reporter",
            "Deploy",
            "yes",
            "no",
        ] {
            assert!(out.contains(word), "{word} missing in\n{out}");
        }
        assert!(out.contains("┌──────────────┐") && out.contains("╔═════════════╗"));
        assert!(out.contains('▼') && out.contains('▲'), "{out}");
        assert!(out.lines().all(|l| l.chars().count() <= 124));
    }

    #[test]
    fn a_sequence_diagram_has_lifelines_and_labelled_messages() {
        let out = text(
            &mermaid(
                "sequenceDiagram\n  participant D as Developer\n  participant C as Conductor\n  D->>C: start run\n  C-->>D: receipt\n  Note over D,C: evidence recorded",
                100,
                &Theme::DARK,
            )
            .unwrap(),
        );
        assert!(out.contains("Developer") && out.contains("Conductor"));
        assert!(out.contains("start run") && out.contains("receipt"));
        assert!(out.contains('▶') && out.contains('◀') && out.contains('╌'));
        assert!(out.contains("evidence recorded"));
    }

    #[test]
    fn what_cannot_be_drawn_is_left_to_the_caller() {
        assert!(mermaid("pie title Pets\n \"Dogs\" : 3", 80, &Theme::DARK).is_none());
        assert!(mermaid("graph TD\n A[one] --> B[two]", 6, &Theme::DARK).is_none());
        assert!(mermaid("graph TD\n A[one] ??? B", 80, &Theme::DARK).is_none());
    }
}
