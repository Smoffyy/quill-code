use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::text::{Line, accent, bold, dim, good, pad, plain, truncate};

pub struct Choice<T> {
    pub label: String,
    pub detail: String,
    pub value: T,
    pub current: bool,
}

impl<T> Choice<T> {
    pub fn new(label: impl Into<String>, detail: impl Into<String>, value: T) -> Choice<T> {
        Choice { label: label.into(), detail: detail.into(), value, current: false }
    }

    pub fn current(mut self, current: bool) -> Choice<T> {
        self.current = current;
        self
    }
}

pub struct Picker<T> {
    pub title: String,
    pub hint: String,
    pub choices: Vec<Choice<T>>,
    pub query: String,
    pub note: Option<Line>,
    filter: bool,
    selected: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Picked {
    Nothing,
    Chosen(usize),
    Closed,
}

impl<T> Picker<T> {
    pub fn new(title: impl Into<String>, choices: Vec<Choice<T>>) -> Picker<T> {
        let selected = choices.iter().position(|choice| choice.current).unwrap_or(0);
        let filter = choices.len() > 9;
        let hint = if filter {
            "type to filter · ↑↓ move · enter choose · esc close"
        } else {
            "↑↓ or a number · enter choose · esc close"
        };
        Picker { title: title.into(), hint: hint.into(), choices, query: String::new(), note: None, filter, selected }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Picker<T> {
        self.hint = hint.into();
        self
    }

    pub fn filtered(mut self, filter: bool) -> Picker<T> {
        self.filter = filter;
        self
    }

    pub fn visible(&self) -> Vec<usize> {
        let query = self.query.to_lowercase();
        self.choices
            .iter()
            .enumerate()
            .filter(|(_, choice)| {
                query.is_empty() || choice.label.to_lowercase().contains(&query) || choice.detail.to_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect()
    }

    pub fn highlighted(&self) -> Option<usize> {
        self.visible().get(self.selected).copied()
    }

    pub fn paste(&mut self, text: &str) {
        if self.filter {
            self.query.push_str(text.lines().next().unwrap_or_default().trim());
            self.selected = 0;
        }
    }

    pub fn select(&mut self, index: usize) {
        if let Some(position) = self.visible().iter().position(|visible| *visible == index) {
            self.selected = position;
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Picked {
        let count = self.visible().len();
        let control = key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => return Picked::Closed,
            KeyCode::Enter => return self.highlighted().map_or(Picked::Nothing, Picked::Chosen),
            KeyCode::Up if count > 0 => self.selected = self.selected.checked_sub(1).unwrap_or(count - 1),
            KeyCode::Down | KeyCode::Tab if count > 0 => self.selected = (self.selected + 1) % count,
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(8),
            KeyCode::PageDown if count > 0 => self.selected = (self.selected + 8).min(count - 1),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = count.saturating_sub(1),
            KeyCode::Backspace if self.filter => {
                self.query.pop();
                self.selected = 0;
            }
            KeyCode::Char(c) if self.filter && !control => {
                self.query.push(c);
                self.selected = 0;
            }
            KeyCode::Char(c) if !self.filter && !control => {
                let index = c.to_digit(10).and_then(|n| (n as usize).checked_sub(1)).and_then(|n| self.visible().get(n).copied());
                return index.map_or(Picked::Nothing, Picked::Chosen);
            }
            _ => {}
        }
        Picked::Nothing
    }

    pub fn render(&self, width: usize, rows: usize) -> Vec<Line> {
        let visible = self.visible();
        let mut lines = vec![truncate(&Line::styled(&self.title, bold()), width)];
        if self.filter {
            let query = if self.query.is_empty() {
                Line::styled("  type to filter", dim())
            } else {
                Line::styled("  › ", accent()).with(&self.query, plain())
            };
            lines.push(query);
        }
        let rows = rows.max(3);
        let start = self.selected.saturating_sub(rows / 2).min(visible.len().saturating_sub(rows));
        let label_width = visible.iter().map(|index| self.choices[*index].label.chars().count()).max().unwrap_or(0).min(width / 2);
        for (position, index) in visible.iter().enumerate().skip(start).take(rows) {
            let choice = &self.choices[*index];
            let on = position == self.selected;
            let mut line = if on { Line::styled("▸ ", accent()) } else { Line::plain("  ") };
            if !self.filter {
                line.push(format!("{}. ", position + 1), dim());
            }
            line.push(if choice.current { "✓ " } else { "  " }, good());
            line.push(pad(&choice.label, label_width), if on { bold() } else { plain() });
            if !choice.detail.is_empty() {
                line.push(format!("  {}", choice.detail), dim());
            }
            lines.push(truncate(&line, width));
        }
        if visible.is_empty() {
            lines.push(Line::styled("  Nothing matches", dim()));
        } else if visible.len() > rows {
            lines.push(Line::styled(format!("  {} of {}", self.selected + 1, visible.len()), dim()));
        }
        if let Some(note) = &self.note {
            lines.push(truncate(note, width));
        }
        lines.push(truncate(&Line::styled(&self.hint, dim()), width));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker(count: usize) -> Picker<usize> {
        Picker::new("Pick", (0..count).map(|n| Choice::new(format!("item {n}"), "", n).current(n == 1)).collect())
    }

    fn press(picker: &mut Picker<usize>, code: KeyCode) -> Picked {
        picker.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn the_current_choice_starts_highlighted() {
        let mut short = picker(3);
        assert_eq!(press(&mut short, KeyCode::Enter), Picked::Chosen(1));
        press(&mut short, KeyCode::Down);
        press(&mut short, KeyCode::Down);
        assert_eq!(press(&mut short, KeyCode::Enter), Picked::Chosen(0));
        assert_eq!(press(&mut short, KeyCode::Char('3')), Picked::Chosen(2));
        assert_eq!(press(&mut short, KeyCode::Esc), Picked::Closed);
    }

    #[test]
    fn long_lists_filter_as_you_type() {
        let mut long = picker(12);
        for c in "item 1".chars() {
            press(&mut long, KeyCode::Char(c));
        }
        assert_eq!(long.visible(), vec![1, 10, 11]);
        press(&mut long, KeyCode::Down);
        assert_eq!(press(&mut long, KeyCode::Enter), Picked::Chosen(10));
        let lines: Vec<String> = long.render(40, 5).iter().map(Line::text).collect();
        assert_eq!(lines[1], "  › item 1");
        assert!(lines.iter().any(|line| line.starts_with("▸ ") && line.contains("item 10")));
    }
}