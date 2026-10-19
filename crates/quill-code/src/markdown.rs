use crossterm::style::{ContentStyle, Stylize};
use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::text::{Line, accent, clean, dim, split_lines, truncate, wrap, wrap_chars};

pub fn render(text: &str, width: usize) -> Vec<Line> {
    let mut writer = Writer::new(width);
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(text, options) {
        writer.event(event);
    }
    writer.flush();
    writer.lines
}

struct Container {
    first: Line,
    rest: Line,
    used: bool,
}

#[derive(Default)]
struct Table {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<Line>>,
    row: Vec<Line>,
    cell: Line,
    head: usize,
}

struct Writer {
    width: usize,
    lines: Vec<Line>,
    inline: Line,
    containers: Vec<Container>,
    lists: Vec<Option<u64>>,
    bold: usize,
    italic: usize,
    strike: usize,
    heading: Option<HeadingLevel>,
    link: Option<String>,
    code: Option<String>,
    table: Option<Table>,
    gap: bool,
}

impl Writer {
    fn new(width: usize) -> Writer {
        Writer {
            width: width.max(20),
            lines: Vec::new(),
            inline: Line::default(),
            containers: Vec::new(),
            lists: Vec::new(),
            bold: 0,
            italic: 0,
            strike: 0,
            heading: None,
            link: None,
            code: None,
            table: None,
            gap: false,
        }
    }

    fn style(&self) -> ContentStyle {
        let mut style = ContentStyle::new();
        if self.bold > 0 || self.heading.is_some() {
            style = style.bold();
        }
        if self.heading == Some(HeadingLevel::H1) || self.link.is_some() {
            style = style.underlined();
        }
        if self.italic > 0 {
            style = style.italic();
        }
        if self.strike > 0 {
            style = style.crossed_out();
        }
        style
    }

    fn write(&mut self, text: &str, style: ContentStyle) {
        let text = clean(text);
        match self.table.as_mut() {
            Some(table) => table.cell.push(text.replace('\n', " "), style),
            None => self.inline.push(text, style),
        }
    }

    fn prefix(&mut self) -> Line {
        let mut line = Line::default();
        for container in &mut self.containers {
            line.append(if container.used { container.rest.clone() } else { container.first.clone() });
            container.used = true;
        }
        line
    }

    fn room(&self) -> usize {
        let used: usize = self.containers.iter().map(|container| container.rest.width()).sum();
        self.width.saturating_sub(used).max(10)
    }

    fn start_block(&mut self) {
        self.flush();
        if self.gap && !self.lines.is_empty() {
            self.lines.push(Line::default());
        }
        self.gap = false;
    }

    fn flush(&mut self) {
        if self.inline.spans.is_empty() {
            return;
        }
        let inline = std::mem::take(&mut self.inline);
        let room = self.room();
        for logical in split_lines(&inline) {
            for wrapped in wrap(&logical, room) {
                let mut line = self.prefix();
                line.append(wrapped);
                self.lines.push(line);
            }
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match self.code.as_mut() {
                Some(code) => code.push_str(&text),
                None => self.write(&text, self.style()),
            },
            Event::Code(text) => self.write(&text, accent()),
            Event::InlineMath(text) | Event::DisplayMath(text) => self.write(&text, self.style()),
            Event::Html(text) | Event::InlineHtml(text) => self.write(&text, dim()),
            Event::FootnoteReference(name) => self.write(&format!("[{name}]"), dim()),
            Event::SoftBreak | Event::HardBreak => self.write("\n", self.style()),
            Event::Rule => {
                self.start_block();
                let mut line = self.prefix();
                line.push("─".repeat(self.room().min(48)), dim());
                self.lines.push(line);
                self.gap = true;
            }
            Event::TaskListMarker(done) => self.write(if done { "[x] " } else { "[ ] " }, dim()),
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph | Tag::HtmlBlock => self.start_block(),
            Tag::Heading { level, .. } => {
                self.start_block();
                self.heading = Some(level);
            }
            Tag::BlockQuote(_) => {
                self.start_block();
                let bar = Line::styled("▎ ", dim());
                self.containers.push(Container { first: bar.clone(), rest: bar, used: false });
            }
            Tag::CodeBlock(_) => {
                self.start_block();
                self.code = Some(String::new());
            }
            Tag::List(start) => {
                if self.lists.is_empty() {
                    self.start_block();
                } else {
                    self.flush();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let depth = self.lists.len();
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = Line::plain(format!("{number}. "));
                        *number += 1;
                        marker
                    }
                    _ => Line::styled(
                        match depth {
                            0 | 1 => "• ",
                            2 => "◦ ",
                            _ => "▪ ",
                        },
                        dim(),
                    ),
                };
                let rest = Line::plain(" ".repeat(marker.width()));
                self.containers.push(Container { first: marker, rest, used: false });
            }
            Tag::Table(alignments) => {
                self.start_block();
                self.table = Some(Table { alignments, ..Table::default() });
            }
            Tag::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.row.clear();
                }
            }
            Tag::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.cell = Line::default();
                }
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.link = Some(dest_url.to_string()),
            Tag::Image { .. } => self.write("[image: ", dim()),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => {
                self.flush();
                self.gap = true;
            }
            TagEnd::Heading(_) => {
                self.flush();
                self.heading = None;
                self.gap = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.containers.pop();
                self.gap = true;
            }
            TagEnd::CodeBlock => {
                let code = self.code.take().unwrap_or_default();
                let room = self.room().saturating_sub(2).max(10);
                for raw in clean(code.trim_end_matches('\n')).split('\n') {
                    for wrapped in wrap_chars(&Line::plain(raw), room) {
                        let mut line = self.prefix();
                        line.push("│ ", dim());
                        line.append(wrapped);
                        self.lines.push(line);
                    }
                }
                self.gap = true;
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.gap = true;
                }
            }
            TagEnd::Item => {
                self.flush();
                self.containers.pop();
            }
            TagEnd::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    let cell = std::mem::take(&mut table.cell);
                    table.row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                    table.head = table.rows.len();
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.render_table(table);
                }
                self.gap = true;
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                if let Some(url) = self.link.take() {
                    let shown = self.inline.text();
                    if !url.is_empty() && !url.starts_with('#') && !shown.ends_with(url.as_str()) {
                        self.write(&format!(" ({url})"), dim());
                    }
                }
            }
            TagEnd::Image => self.write("]", dim()),
            _ => {}
        }
    }

    fn render_table(&mut self, table: Table) {
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let mut widths = vec![0; columns];
        for row in &table.rows {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(cell.width());
            }
        }
        let room = self.room().saturating_sub(3 * (columns - 1));
        while widths.iter().sum::<usize>() > room {
            let Some(widest) = widths.iter().copied().enumerate().max_by_key(|(_, width)| *width).filter(|(_, width)| *width > 3) else {
                break;
            };
            widths[widest.0] -= 1;
        }
        for (index, row) in table.rows.iter().enumerate() {
            let mut line = self.prefix();
            for (column, width) in widths.iter().enumerate() {
                if column > 0 {
                    line.push(" │ ", dim());
                }
                let cell = truncate(row.get(column).unwrap_or(&Line::default()), *width);
                let space = width.saturating_sub(cell.width());
                let (before, after) = match table.alignments.get(column) {
                    Some(Alignment::Right) => (space, 0),
                    Some(Alignment::Center) => (space / 2, space - space / 2),
                    _ => (0, space),
                };
                line.push(" ".repeat(before), ContentStyle::new());
                if index < table.head {
                    for span in cell.spans {
                        line.push(span.text, span.style.bold());
                    }
                } else {
                    line.append(cell);
                }
                if column + 1 < columns {
                    line.push(" ".repeat(after), ContentStyle::new());
                }
            }
            self.lines.push(line);
            if index + 1 == table.head {
                let mut rule = self.prefix();
                let parts: Vec<String> = widths.iter().map(|width| "─".repeat(*width)).collect();
                rule.push(parts.join("─┼─"), dim());
                self.lines.push(rule);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(markdown: &str, width: usize) -> Vec<String> {
        render(markdown, width).iter().map(Line::text).collect()
    }

    #[test]
    fn paragraphs_and_headings_are_spaced() {
        assert_eq!(texts("# Title\n\nSome text\nthat breaks.\n\nNext.", 40), vec!["Title", "", "Some text", "that breaks.", "", "Next."]);
    }

    #[test]
    fn lists_nest_with_markers() {
        let lines = texts("- one\n- two\n  - inner\n\n1. first\n2. second", 40);
        assert_eq!(lines, vec!["• one", "• two", "  ◦ inner", "", "1. first", "2. second"]);
    }

    #[test]
    fn list_items_wrap_under_their_text() {
        assert_eq!(texts("- a long item that wraps around", 20), vec!["• a long item that", "  wraps around"]);
    }

    #[test]
    fn code_blocks_keep_their_lines() {
        let lines = texts("Run:\n\n```sh\ncargo test\n  --workspace\n```\nDone.", 40);
        assert_eq!(lines, vec!["Run:", "", "│ cargo test", "│   --workspace", "", "Done."]);
    }

    #[test]
    fn inline_styles_and_links_render() {
        let lines = render("Use `cargo` and **bold** [docs](https://x.dev).", 80);
        assert_eq!(lines[0].text(), "Use cargo and bold docs (https://x.dev).");
        assert!(lines[0].spans.iter().any(|span| span.text == "cargo" && span.style == accent()));
        assert!(lines[0].spans.iter().any(|span| span.text == "bold" && span.style == ContentStyle::new().bold()));
        assert_eq!(texts("<https://x.dev>", 80), vec!["https://x.dev"]);
    }

    #[test]
    fn tables_line_up() {
        let lines = texts("| Name | Size |\n|------|-----:|\n| a.rs | 12 |\n| bb.rs | 3 |", 40);
        assert_eq!(lines, vec!["Name  │ Size", "──────┼─────", "a.rs  │   12", "bb.rs │    3"]);
    }

    #[test]
    fn quotes_and_rules_have_marks() {
        assert_eq!(texts("> quoted\n\n---\n\nafter", 40), vec!["▎ quoted", "", &"─".repeat(40), "", "after"]);
    }

    #[test]
    fn task_lists_show_their_state() {
        assert_eq!(texts("- [x] done\n- [ ] todo", 40), vec!["• [x] done", "• [ ] todo"]);
    }
}