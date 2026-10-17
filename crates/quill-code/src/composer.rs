use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::attachments::Attachment;
use crate::text::{Line, accent, char_width, dim, plain};
use crate::workspace::Prompt;

#[derive(Default)]
pub struct Composer {
    pub text: String,
    pub cursor: usize,
    pub attachments: Vec<Attachment>,
    history: Vec<String>,
    recall: Option<usize>,
    draft: String,
}

fn word_start(text: &str, from: usize) -> usize {
    let before = &text[..from];
    let trimmed = before.trim_end_matches(|c: char| !c.is_alphanumeric());
    trimmed.trim_end_matches(char::is_alphanumeric).len()
}

fn word_end(text: &str, from: usize) -> usize {
    let after = &text[from..];
    let skipped = after.len() - after.trim_start_matches(|c: char| !c.is_alphanumeric()).len();
    let rest = &after[skipped..];
    from + skipped + (rest.len() - rest.trim_start_matches(char::is_alphanumeric).len())
}

fn pasted_path(text: &str) -> Option<PathBuf> {
    let trimmed = text.trim().trim_matches(['"', '\'']);
    if trimmed.is_empty() || trimmed.contains('\n') {
        return None;
    }
    let path = PathBuf::from(trimmed.strip_prefix("file://").unwrap_or(trimmed));
    path.is_file().then_some(path)
}

impl Composer {
    pub fn new(history: Vec<String>) -> Composer {
        Composer { history, ..Composer::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.attachments.is_empty()
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.attachments.clear();
        self.recall = None;
    }

    pub fn take(&mut self) -> Prompt {
        let text = self.text.trim().to_string();
        if !text.is_empty() && self.history.last() != Some(&text) {
            self.history.push(text.clone());
        }
        let attachments = std::mem::take(&mut self.attachments);
        self.clear();
        Prompt { text, attachments }
    }

    pub fn insert(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.recall = None;
    }

    pub fn paste(&mut self, text: &str) {
        if let Some(attachment) = pasted_path(text).and_then(Attachment::from_path) {
            if !self.attachments.iter().any(|a| a.path == attachment.path) {
                self.attachments.push(attachment);
            }
            return;
        }
        self.insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].find('\n').map_or(self.text.len(), |i| self.cursor + i)
    }

    fn previous(&self) -> usize {
        self.text[..self.cursor].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.cursor..].chars().next().map_or(self.cursor, |c| self.cursor + c.len_utf8())
    }

    fn column(&self) -> usize {
        self.text[self.line_start()..self.cursor].chars().count()
    }

    fn at_column(&self, start: usize, column: usize) -> usize {
        let line = &self.text[start..];
        let end = line.find('\n').unwrap_or(line.len());
        line[..end].char_indices().nth(column).map_or(start + end, |(i, _)| start + i)
    }

    fn vertical(&mut self, up: bool) -> bool {
        let column = self.column();
        let start = self.line_start();
        if up {
            if start == 0 {
                return false;
            }
            let above = self.text[..start - 1].rfind('\n').map_or(0, |i| i + 1);
            self.cursor = self.at_column(above, column);
        } else {
            let end = self.line_end();
            if end == self.text.len() {
                return false;
            }
            self.cursor = self.at_column(end + 1, column);
        }
        true
    }

    fn recall_history(&mut self, up: bool) {
        let next = match (self.recall, up) {
            (None, true) => self.history.len().checked_sub(1),
            (Some(index), true) => Some(index.saturating_sub(1)),
            (Some(index), false) => Some(index + 1),
            (None, false) => return,
        };
        match next {
            Some(index) if index < self.history.len() => {
                if self.recall.is_none() {
                    self.draft = std::mem::take(&mut self.text);
                }
                self.recall = Some(index);
                self.text = self.history[index].clone();
            }
            Some(_) => {
                self.recall = None;
                self.text = std::mem::take(&mut self.draft);
            }
            None => return,
        }
        self.cursor = self.text.len();
    }

    pub fn key(&mut self, key: KeyEvent) -> bool {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let altgr = control && alt;
        match key.code {
            KeyCode::Char(c) if !control || altgr => self.insert(&c.to_string()),
            KeyCode::Char('j') if control => self.insert("\n"),
            KeyCode::Enter if key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => self.insert("\n"),
            KeyCode::Backspace if self.cursor == 0 => {
                self.attachments.pop();
            }
            KeyCode::Backspace if control || alt => {
                let start = word_start(&self.text, self.cursor);
                self.text.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            KeyCode::Char('w') if control => {
                let start = word_start(&self.text, self.cursor);
                self.text.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            KeyCode::Backspace => {
                let start = self.previous();
                self.text.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            KeyCode::Delete => {
                let end = self.next();
                self.text.replace_range(self.cursor..end, "");
            }
            KeyCode::Left if control || alt => self.cursor = word_start(&self.text, self.cursor),
            KeyCode::Right if control || alt => self.cursor = word_end(&self.text, self.cursor),
            KeyCode::Left => self.cursor = self.previous(),
            KeyCode::Right => self.cursor = self.next(),
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::Char('a') if control => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),
            KeyCode::Char('e') if control => self.cursor = self.line_end(),
            KeyCode::Char('u') if control => {
                let start = self.line_start();
                self.text.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            KeyCode::Char('k') if control => {
                let end = self.line_end();
                self.text.replace_range(self.cursor..end, "");
            }
            KeyCode::Up => {
                if !self.vertical(true) {
                    self.recall_history(true);
                }
            }
            KeyCode::Down => {
                if !self.vertical(false) {
                    self.recall_history(false);
                }
            }
            _ => return false,
        }
        true
    }

    pub fn render(&self, width: usize, placeholder: &str) -> (Vec<Line>, (usize, usize)) {
        let mut lines = Vec::new();
        if !self.attachments.is_empty() {
            let names: Vec<String> = self.attachments.iter().map(|a| format!("+ {}", a.name)).collect();
            lines.push(Line::styled(format!("  {}  ", names.join("  ")), dim()).with("backspace removes the last", dim()));
        }
        if self.text.is_empty() {
            let cursor = (lines.len(), 2);
            lines.push(Line::styled("› ", accent()).with(placeholder, dim()));
            return (lines, cursor);
        }
        let room = width.saturating_sub(3).max(4);
        let mut cursor = (lines.len(), 2);
        let mut offset = 0;
        for logical in self.text.split('\n') {
            let mut rows: Vec<(usize, String)> = Vec::new();
            let mut start = offset;
            let mut chunk = String::new();
            let mut used = 0;
            for (index, c) in logical.char_indices() {
                let w = char_width(c);
                if used + w > room && !chunk.is_empty() {
                    rows.push((start, std::mem::take(&mut chunk)));
                    start = offset + index;
                    used = 0;
                }
                chunk.push(c);
                used += w;
            }
            rows.push((start, chunk));
            let count = rows.len();
            for (index, (start, text)) in rows.into_iter().enumerate() {
                let end = start + text.len();
                if self.cursor >= start && (self.cursor < end || (self.cursor == end && index + 1 == count)) {
                    let column = self.text[start..self.cursor].chars().map(char_width).sum::<usize>();
                    cursor = (lines.len(), 2 + column);
                }
                let prefix = if offset == 0 && index == 0 { Line::styled("› ", accent()) } else { Line::plain("  ") };
                lines.push(prefix.with(text, plain()));
            }
            offset += logical.len() + 1;
        }
        (lines, cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn press(composer: &mut Composer, code: KeyCode, modifiers: KeyModifiers) {
        composer.key(KeyEvent::new(code, modifiers));
    }

    fn typed(text: &str) -> Composer {
        let mut composer = Composer::new(Vec::new());
        composer.insert(text);
        composer
    }

    #[test]
    fn editing_moves_by_characters_and_words() {
        let mut composer = typed("hello wörld");
        press(&mut composer, KeyCode::Left, KeyModifiers::NONE);
        press(&mut composer, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(composer.text, "hello wörd");
        press(&mut composer, KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(&composer.text[composer.cursor..], "wörd");
        press(&mut composer, KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(composer.text, "wörd");
        press(&mut composer, KeyCode::End, KeyModifiers::NONE);
        press(&mut composer, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(composer.text, "");
    }

    #[test]
    fn new_lines_come_from_modified_enter_and_ctrl_j() {
        let mut composer = typed("a");
        press(&mut composer, KeyCode::Enter, KeyModifiers::SHIFT);
        press(&mut composer, KeyCode::Char('b'), KeyModifiers::NONE);
        press(&mut composer, KeyCode::Char('j'), KeyModifiers::CONTROL);
        press(&mut composer, KeyCode::Char('c'), KeyModifiers::NONE);
        assert_eq!(composer.text, "a\nb\nc");
        press(&mut composer, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(&composer.text[..composer.cursor], "a\nb");
        assert!(!composer.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    }

    #[test]
    fn history_comes_back_with_up_and_down() {
        let mut composer = Composer::new(vec!["first".into(), "second".into()]);
        composer.insert("draft");
        press(&mut composer, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(composer.text, "second");
        press(&mut composer, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(composer.text, "first");
        press(&mut composer, KeyCode::Down, KeyModifiers::NONE);
        press(&mut composer, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(composer.text, "draft");
        let prompt = composer.take();
        assert_eq!(prompt.text, "draft");
        press(&mut composer, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(composer.text, "draft");
    }

    #[test]
    fn long_text_wraps_and_the_cursor_follows() {
        let composer = typed("abcdefgh\nxy");
        let (lines, cursor) = composer.render(8, "");
        let texts: Vec<String> = lines.iter().map(Line::text).collect();
        assert_eq!(texts, vec!["› abcde", "  fgh", "  xy"]);
        assert_eq!(cursor, (2, 4));
        let mut start = typed("abcdefgh");
        start.cursor = 5;
        assert_eq!(start.render(8, "").1, (1, 2));
        assert_eq!(Composer::new(Vec::new()).render(20, "Ask").0[0].text(), "› Ask");
    }

    #[test]
    fn pasted_paths_become_attachments() {
        let dir = std::env::temp_dir().join(format!("quill-paste-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("shot.png");
        std::fs::write(&file, [1u8]).unwrap();
        let mut composer = Composer::new(Vec::new());
        composer.paste(&format!("\"{}\"", file.display()));
        assert_eq!(composer.attachments.len(), 1);
        assert!(composer.text.is_empty());
        composer.paste("line one\r\nline two");
        assert_eq!(composer.text, "line one\nline two");
        composer.cursor = 0;
        press(&mut composer, KeyCode::Backspace, KeyModifiers::NONE);
        assert!(composer.attachments.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}