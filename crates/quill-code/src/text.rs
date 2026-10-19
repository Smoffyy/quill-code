use std::fmt::Write;

use crossterm::style::{Color, ContentStyle, StyledContent, Stylize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub style: ContentStyle,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub spans: Vec<Span>,
}

impl Line {
    pub fn plain(text: impl Into<String>) -> Line {
        Line::styled(text, ContentStyle::new())
    }

    pub fn styled(text: impl Into<String>, style: ContentStyle) -> Line {
        let mut line = Line::default();
        line.push(text, style);
        line
    }

    pub fn push(&mut self, text: impl Into<String>, style: ContentStyle) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        match self.spans.last_mut() {
            Some(last) if last.style == style => last.text.push_str(&text),
            _ => self.spans.push(Span { text, style }),
        }
    }

    pub fn with(mut self, text: impl Into<String>, style: ContentStyle) -> Line {
        self.push(text, style);
        self
    }

    pub fn append(&mut self, other: Line) {
        for span in other.spans {
            self.push(span.text, span.style);
        }
    }

    pub fn with_line(mut self, other: Line) -> Line {
        self.append(other);
        self
    }

    pub fn width(&self) -> usize {
        self.spans.iter().map(|span| span.text.width()).sum()
    }

    pub fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    pub fn ansi(&self) -> String {
        let mut out = String::new();
        for span in &self.spans {
            if span.style == ContentStyle::new() {
                out.push_str(&span.text);
            } else {
                let _ = write!(out, "{}", StyledContent::new(span.style, &span.text));
            }
        }
        out
    }

    fn cells(&self) -> impl Iterator<Item = (char, ContentStyle)> + '_ {
        self.spans.iter().flat_map(|span| span.text.chars().map(move |c| (c, span.style)))
    }
}

pub fn plain() -> ContentStyle {
    ContentStyle::new()
}

pub fn dim() -> ContentStyle {
    ContentStyle::new().dim()
}

pub fn bold() -> ContentStyle {
    ContentStyle::new().bold()
}

pub fn accent() -> ContentStyle {
    ContentStyle::new().with(Color::DarkCyan)
}

pub fn good() -> ContentStyle {
    ContentStyle::new().with(Color::DarkGreen)
}

pub fn bad() -> ContentStyle {
    ContentStyle::new().with(Color::DarkRed)
}

pub fn warn() -> ContentStyle {
    ContentStyle::new().with(Color::DarkYellow)
}

pub fn blue() -> ContentStyle {
    ContentStyle::new().with(Color::Blue)
}

pub fn caret(line: &Line, column: usize) -> Line {
    let mut out = Line::default();
    let mut used = 0;
    let mut placed = false;
    for (c, style) in line.cells() {
        if !placed && used >= column {
            out.push(c, style.reverse());
            placed = true;
        } else {
            out.push(c, style);
        }
        used += char_width(c);
    }
    if !placed {
        out.push(" ".repeat(column.saturating_sub(used)), plain());
        out.push(" ", plain().reverse());
    }
    out
}

pub fn shade(style: ContentStyle) -> ContentStyle {
    let style = if style.foreground_color.is_none() { style.with(Color::White) } else { style };
    style.on(Color::AnsiValue(237))
}

pub fn fill(line: Line, width: usize, style: ContentStyle) -> Line {
    let gap = width.saturating_sub(line.width());
    line.with(" ".repeat(gap), style)
}

pub fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

pub fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' || (next == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => out.push_str("    "),
            '\n' | '\r' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    if !out.contains('\r') {
        return out;
    }
    out.split('\n').map(|line| line.rsplit('\r').find(|part| !part.is_empty()).unwrap_or_default()).collect::<Vec<_>>().join("\n")
}

struct Wrapper {
    width: usize,
    lines: Vec<Line>,
    line: Line,
    used: usize,
    wrapped: bool,
}

impl Wrapper {
    fn new(width: usize) -> Wrapper {
        Wrapper { width: width.max(1), lines: Vec::new(), line: Line::default(), used: 0, wrapped: false }
    }

    fn push(&mut self, cells: &[(char, ContentStyle)]) {
        for &(c, style) in cells {
            self.line.push(c, style);
            self.used += char_width(c);
        }
    }

    fn newline(&mut self) {
        self.lines.push(std::mem::take(&mut self.line));
        self.used = 0;
        self.wrapped = true;
    }

    fn place(&mut self, gap: &[(char, ContentStyle)], word: &[(char, ContentStyle)]) {
        let gap_width: usize = gap.iter().map(|(c, _)| char_width(*c)).sum();
        let word_width: usize = word.iter().map(|(c, _)| char_width(*c)).sum();
        let keep_gap = !(self.wrapped && self.used == 0);
        if self.used + gap_width + word_width <= self.width {
            if keep_gap {
                self.push(gap);
            }
            self.push(word);
            return;
        }
        if word_width <= self.width && self.used > 0 {
            self.newline();
            self.push(word);
            return;
        }
        if keep_gap && self.used + gap_width < self.width {
            self.push(gap);
        }
        for &cell in word {
            if self.used + char_width(cell.0) > self.width && self.used > 0 {
                self.newline();
            }
            self.push(&[cell]);
        }
    }

    fn finish(mut self) -> Vec<Line> {
        self.lines.push(self.line);
        self.lines
    }
}

pub fn split_lines(line: &Line) -> Vec<Line> {
    let mut lines = vec![Line::default()];
    for span in &line.spans {
        for (index, part) in span.text.split('\n').enumerate() {
            if index > 0 {
                lines.push(Line::default());
            }
            if let Some(last) = lines.last_mut() {
                last.push(part, span.style);
            }
        }
    }
    lines
}

pub fn wrap(line: &Line, width: usize) -> Vec<Line> {
    let mut out = Vec::new();
    for logical in split_lines(line) {
        let mut wrapper = Wrapper::new(width);
        let mut gap = Vec::new();
        let mut word = Vec::new();
        for (c, style) in logical.cells() {
            if c == ' ' {
                if !word.is_empty() {
                    wrapper.place(&gap, &word);
                    gap.clear();
                    word.clear();
                }
                gap.push((c, style));
            } else {
                word.push((c, style));
            }
        }
        if !word.is_empty() || !gap.is_empty() {
            wrapper.place(&gap, &word);
        }
        out.extend(wrapper.finish());
    }
    out
}

pub fn wrap_chars(line: &Line, width: usize) -> Vec<Line> {
    let mut out = Vec::new();
    for logical in split_lines(line) {
        let mut wrapper = Wrapper::new(width);
        for cell in logical.cells() {
            if wrapper.used + char_width(cell.0) > wrapper.width && wrapper.used > 0 {
                wrapper.newline();
            }
            wrapper.push(&[cell]);
        }
        out.extend(wrapper.finish());
    }
    out
}

pub fn truncate(line: &Line, width: usize) -> Line {
    if line.width() <= width {
        return line.clone();
    }
    let mut out = Line::default();
    let mut used = 0;
    let mut style = ContentStyle::new();
    for (c, cell_style) in line.cells() {
        let w = char_width(c);
        if used + w + 1 > width {
            break;
        }
        out.push(c, cell_style);
        used += w;
        style = cell_style;
    }
    out.with("…", style)
}

pub fn indent(lines: Vec<Line>, first: Line, rest: Line) -> Vec<Line> {
    lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let mut prefixed = if index == 0 { first.clone() } else { rest.clone() };
            prefixed.append(line);
            prefixed
        })
        .collect()
}

pub fn pad(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(text.width())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    #[test]
    fn words_wrap_at_the_width() {
        let line = Line::plain("the quick brown fox jumps");
        assert_eq!(texts(&wrap(&line, 10)), vec!["the quick", "brown fox", "jumps"]);
        assert_eq!(texts(&wrap(&line, 40)), vec!["the quick brown fox jumps"]);
    }

    #[test]
    fn long_words_break_and_indentation_stays() {
        assert_eq!(texts(&wrap(&Line::plain("abcdefghij"), 4)), vec!["abcd", "efgh", "ij"]);
        assert_eq!(texts(&wrap(&Line::plain("  indented text"), 40)), vec!["  indented text"]);
        assert_eq!(texts(&wrap(&Line::plain("one\ntwo"), 40)), vec!["one", "two"]);
        assert_eq!(texts(&wrap(&Line::default(), 10)), vec![""]);
    }

    #[test]
    fn styles_survive_wrapping() {
        let line = Line::plain("plain ").with("bold words here", bold());
        let wrapped = wrap(&line, 11);
        assert_eq!(texts(&wrapped), vec!["plain bold", "words here"]);
        assert_eq!(wrapped[0].spans[1], Span { text: "bold".into(), style: bold() });
        assert_eq!(wrapped[1].spans, vec![Span { text: "words here".into(), style: bold() }]);
    }

    #[test]
    fn wide_characters_count_twice() {
        assert_eq!(Line::plain("日本語").width(), 6);
        assert_eq!(texts(&wrap_chars(&Line::plain("日本語"), 4)), vec!["日本", "語"]);
        assert_eq!(texts(&wrap_chars(&Line::plain("abcdef"), 4)), vec!["abcd", "ef"]);
    }

    #[test]
    fn long_lines_end_with_an_ellipsis() {
        assert_eq!(truncate(&Line::plain("hello world"), 8).text(), "hello w…");
        assert_eq!(truncate(&Line::plain("short"), 8).text(), "short");
    }

    #[test]
    fn terminal_noise_is_cleaned() {
        assert_eq!(clean("\u{1b}[32mok\u{1b}[0m\r\n"), "ok\n");
        assert_eq!(clean("a\tb"), "a    b");
        assert_eq!(clean("10%\r50%\r100%\ndone"), "100%\ndone");
        assert_eq!(clean("\u{1b}]8;;http://x\u{1b}\\link\u{1b}]8;;\u{1b}\\ ok\u{7}"), "link ok");
    }

    #[test]
    fn the_caret_marks_one_cell() {
        let marked = caret(&Line::plain("abc"), 1);
        assert_eq!(marked.text(), "abc");
        assert_eq!(marked.spans[1], Span { text: "b".into(), style: plain().reverse() });
        let end = caret(&Line::plain("ab"), 2);
        assert_eq!(end.text(), "ab ");
        assert_eq!(end.spans.last().map(|span| span.style), Some(plain().reverse()));
    }

    #[test]
    fn prefixes_mark_the_first_line() {
        let lines = indent(vec![Line::plain("a"), Line::plain("b")], Line::plain("- "), Line::plain("  "));
        assert_eq!(texts(&lines), vec!["- a", "  b"]);
    }
}