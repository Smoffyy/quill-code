use std::io::{self, Stdout, Write, stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use crossterm::Command;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::queue;
use crossterm::terminal::{
    self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
};

use crate::Msg;
use crate::text::{Line, dim, truncate};

static PAUSED: AtomicBool = AtomicBool::new(false);

const WHEEL: i64 = 3;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub lines: Vec<Line>,
    pub cursor: Option<(usize, usize)>,
    pub top: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    Lines(i64),
    Pages(i64),
    Top,
    Bottom,
}

#[derive(Default)]
pub struct Layout {
    pub width: usize,
    pub height: usize,
    transcript: Vec<Line>,
    offset: usize,
    seen: usize,
    area: usize,
    painted: Vec<String>,
}

fn code(out: &mut String, command: impl Command) {
    let _ = command.write_ansi(out);
}

impl Layout {
    pub fn new(width: usize, height: usize) -> Layout {
        Layout { width, height, ..Layout::default() }
    }

    pub fn room(&self) -> usize {
        self.width.max(20) - 1
    }

    pub fn rows(&self) -> usize {
        self.height.max(4)
    }

    pub fn transcript(&self) -> &[Line] {
        &self.transcript
    }

    pub fn resized(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.painted.clear();
    }

    pub fn clear(&mut self) {
        self.transcript.clear();
        self.offset = 0;
        self.seen = 0;
        self.painted.clear();
    }

    pub fn repaint(&mut self) {
        self.painted.clear();
    }

    pub fn scroll(&mut self, scroll: Scroll) {
        let page = self.area.saturating_sub(2).max(1) as i64;
        let moved = |by: i64| (self.offset as i64 + by).max(0) as usize;
        self.offset = match scroll {
            Scroll::Lines(lines) => moved(lines),
            Scroll::Pages(pages) => moved(pages * page),
            Scroll::Top => usize::MAX,
            Scroll::Bottom => 0,
        };
    }

    pub fn frame(&mut self, history: &[Line], live: &Live) -> String {
        self.transcript.extend_from_slice(history);
        let rows = self.rows();
        let top = live.top.min(live.lines.len());
        let bottom = &live.lines[top..];
        let hidden = bottom.len().saturating_sub(rows);
        let below = &bottom[hidden..];
        let area = rows - below.len();
        let total = self.transcript.len() + top;
        if self.offset > 0 {
            self.offset += total.saturating_sub(self.seen);
        }
        self.seen = total;
        self.offset = self.offset.min(total.saturating_sub(area));
        self.area = area;
        let end = total - self.offset;
        let start = end.saturating_sub(area);
        let count = self.transcript.len();
        let mut screen: Vec<Line> = (start..end)
            .map(|index| if index < count { self.transcript[index].clone() } else { live.lines[index - count].clone() })
            .collect();
        if self.offset > 0
            && let Some(last) = screen.last_mut()
        {
            *last = Line::styled(format!("  ↓ {} more lines below · page down or scroll to follow", self.offset), dim());
        }
        screen.resize(area, Line::default());
        screen.extend(below.iter().cloned());
        let cursor = live.cursor.and_then(|(row, column)| Some((area + row.checked_sub(top + hidden)?, column)));
        let mut out = String::new();
        code(&mut out, BeginSynchronizedUpdate);
        if self.painted.len() != rows {
            self.painted = vec![String::new(); rows];
            code(&mut out, Clear(ClearType::All));
        }
        let room = self.room();
        for (row, line) in screen.iter().enumerate() {
            let text = truncate(line, room).ansi();
            if self.painted[row] != text {
                code(&mut out, MoveTo(0, row as u16));
                out.push_str(&text);
                code(&mut out, Clear(ClearType::UntilNewLine));
                self.painted[row] = text;
            }
        }
        let (row, column) = cursor.filter(|(row, _)| *row < rows).unwrap_or((rows - 1, 0));
        code(&mut out, MoveTo(column as u16, row as u16));
        code(&mut out, EndSynchronizedUpdate);
        out
    }
}

pub struct Screen {
    out: Stdout,
    pub layout: Layout,
    enhanced: bool,
    active: bool,
}

fn restore(enhanced: bool) {
    let mut out = stdout();
    if enhanced {
        let _ = queue!(out, PopKeyboardEnhancementFlags);
    }
    let _ = queue!(out, DisableMouseCapture, DisableBracketedPaste, LeaveAlternateScreen, Show);
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
}

fn enter(out: &mut Stdout, enhanced: bool) -> io::Result<()> {
    terminal::enable_raw_mode()?;
    queue!(out, EnterAlternateScreen, EnableBracketedPaste, EnableMouseCapture, Hide)?;
    if enhanced {
        queue!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
    }
    out.flush()
}

impl Screen {
    pub fn start() -> io::Result<Screen> {
        let enhanced = matches!(terminal::supports_keyboard_enhancement(), Ok(true));
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore(enhanced);
            hook(info);
        }));
        let mut out = stdout();
        queue!(out, SetTitle("Quill Code"))?;
        enter(&mut out, enhanced)?;
        let (width, height) = terminal::size()?;
        Ok(Screen { out, layout: Layout::new(width as usize, height as usize), enhanced, active: true })
    }

    pub fn resized(&mut self) {
        if let Ok((width, height)) = terminal::size() {
            self.layout.resized(width as usize, height as usize);
        }
    }

    pub fn clear(&mut self) {
        self.layout.clear();
    }

    pub fn draw(&mut self, history: &[Line], live: &Live) -> io::Result<()> {
        let frame = self.layout.frame(history, live);
        self.out.write_all(frame.as_bytes())?;
        self.out.flush()
    }

    pub fn finish(&mut self, lines: &[Line]) -> io::Result<()> {
        restore(self.enhanced);
        self.active = false;
        let mut out = stdout();
        for line in self.layout.transcript().iter().chain(lines) {
            writeln!(out, "{}", line.ansi())?;
        }
        out.flush()
    }

    pub fn suspend<R>(&mut self, lines: &[Line], run: impl FnOnce() -> R) -> io::Result<R> {
        PAUSED.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(150));
        self.layout.frame(lines, &Live::default());
        restore(self.enhanced);
        let result = run();
        enter(&mut self.out, self.enhanced)?;
        self.layout.repaint();
        PAUSED.store(false, Ordering::Relaxed);
        Ok(result)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        if self.active {
            restore(self.enhanced);
        }
    }
}

fn typed(key: &KeyEvent) -> Option<char> {
    let altgr = key.modifiers.contains(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let plain = !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    match key.code {
        KeyCode::Char(c) if plain || altgr => Some(c),
        KeyCode::Enter if plain => Some('\n'),
        KeyCode::Tab if plain => Some('\t'),
        _ => None,
    }
}

pub fn translate(events: Vec<Event>) -> Vec<Msg> {
    let presses: Vec<&KeyEvent> = events
        .iter()
        .filter_map(|event| match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => Some(key),
            _ => None,
        })
        .collect();
    let pasted =
        presses.len() > 2 && presses.iter().all(|key| typed(key).is_some()) && presses.iter().any(|key| key.code != KeyCode::Enter);
    if pasted {
        return vec![Msg::Paste(presses.iter().filter_map(|key| typed(key)).collect())];
    }
    events
        .into_iter()
        .filter_map(|event| match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => Some(Msg::Key(key)),
            Event::Paste(text) => Some(Msg::Paste(text)),
            Event::Resize(..) => Some(Msg::Resize),
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => Some(Msg::Scroll(Scroll::Lines(WHEEL))),
                MouseEventKind::ScrollDown => Some(Msg::Scroll(Scroll::Lines(-WHEEL))),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

pub fn listen(tx: Sender<Msg>) {
    std::thread::Builder::new()
        .name("quill-input".into())
        .spawn(move || {
            loop {
                if PAUSED.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                match event::poll(Duration::from_millis(100)) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(_) => return,
                }
                let Ok(first) = event::read() else { return };
                let mut batch = vec![first];
                while batch.len() < 8192 && event::poll(Duration::from_millis(4)).unwrap_or(false) {
                    match event::read() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }
                for msg in translate(batch) {
                    if tx.send(msg).is_err() {
                        return;
                    }
                }
            }
        })
        .expect("start the input thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{MouseEvent, MouseEventKind};

    struct Vt {
        width: usize,
        grid: Vec<Vec<char>>,
        row: usize,
        column: usize,
    }

    impl Vt {
        fn new(width: usize, height: usize) -> Vt {
            Vt { width, grid: vec![vec![' '; width]; height], row: 0, column: 0 }
        }

        fn feed(&mut self, text: &str) {
            let mut chars = text.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '\u{1b}' => {
                        chars.next();
                        let mut params = String::new();
                        let mut last = ' ';
                        for next in chars.by_ref() {
                            if ('@'..='~').contains(&next) {
                                last = next;
                                break;
                            }
                            params.push(next);
                        }
                        let numbers: Vec<usize> = params.trim_start_matches('?').split(';').filter_map(|p| p.parse().ok()).collect();
                        match last {
                            'H' => {
                                self.row = numbers.first().copied().unwrap_or(1).max(1) - 1;
                                self.column = numbers.get(1).copied().unwrap_or(1).max(1) - 1;
                            }
                            'K' => self.grid[self.row][self.column..].fill(' '),
                            'J' => self.grid.iter_mut().for_each(|row| row.fill(' ')),
                            _ => {}
                        }
                    }
                    c => {
                        if self.column < self.width {
                            self.grid[self.row][self.column] = c;
                        }
                        self.column += 1;
                    }
                }
            }
        }

        fn screen(&self) -> Vec<String> {
            self.grid.iter().map(|row| row.iter().collect::<String>().trim_end().to_string()).collect()
        }
    }

    fn lines(texts: &[&str]) -> Vec<Line> {
        texts.iter().map(|text| Line::plain(*text)).collect()
    }

    fn live(texts: &[&str], cursor: Option<(usize, usize)>) -> Live {
        Live { lines: lines(texts), cursor, top: 0 }
    }

    fn numbered(from: usize, to: usize) -> Vec<Line> {
        (from..=to).map(|n| Line::plain(format!("line {n}"))).collect()
    }

    #[test]
    fn the_prompt_stays_at_the_bottom_and_history_fills_from_the_top() {
        let mut vt = Vt::new(40, 6);
        let mut layout = Layout::new(40, 6);
        let banner = Live { lines: lines(&["banner", "› ", "status"]), cursor: Some((1, 2)), top: 1 };
        vt.feed(&layout.frame(&[], &banner));
        assert_eq!(vt.screen(), vec!["banner", "", "", "", "›", "status"]);
        assert_eq!((vt.row, vt.column), (4, 2));
        vt.feed(&layout.frame(&lines(&["banner", "› hi"]), &live(&["› ", "status"], Some((0, 2)))));
        assert_eq!(vt.screen(), vec!["banner", "› hi", "", "", "›", "status"]);
        vt.feed(&layout.frame(&numbered(1, 6), &live(&["› ", "status"], Some((0, 2)))));
        assert_eq!(vt.screen(), vec!["line 3", "line 4", "line 5", "line 6", "›", "status"]);
    }

    #[test]
    fn scrolling_up_holds_the_view_while_output_arrives() {
        let mut vt = Vt::new(60, 6);
        let mut layout = Layout::new(60, 6);
        let prompt = live(&["› ", "status"], Some((0, 2)));
        vt.feed(&layout.frame(&numbered(1, 10), &prompt));
        assert_eq!(vt.screen()[..4], ["line 7", "line 8", "line 9", "line 10"]);
        layout.scroll(Scroll::Lines(3));
        vt.feed(&layout.frame(&[], &prompt));
        assert_eq!(vt.screen()[..3], ["line 4", "line 5", "line 6"]);
        assert!(vt.screen()[3].contains("3 more lines below"));
        vt.feed(&layout.frame(&numbered(11, 12), &prompt));
        assert_eq!(vt.screen()[..3], ["line 4", "line 5", "line 6"]);
        assert!(vt.screen()[3].contains("5 more lines below"));
        layout.scroll(Scroll::Top);
        vt.feed(&layout.frame(&[], &prompt));
        assert_eq!(vt.screen()[0], "line 1");
        layout.scroll(Scroll::Bottom);
        vt.feed(&layout.frame(&[], &prompt));
        assert_eq!(vt.screen()[..4], ["line 9", "line 10", "line 11", "line 12"]);
    }

    #[test]
    fn a_live_area_taller_than_the_screen_keeps_its_bottom() {
        let mut vt = Vt::new(30, 4);
        let mut layout = Layout::new(30, 4);
        vt.feed(&layout.frame(&[], &live(&["a", "b", "c", "d", "› ", "status"], Some((4, 2)))));
        assert_eq!(vt.screen(), vec!["c", "d", "›", "status"]);
        assert_eq!((vt.row, vt.column), (2, 2));
    }

    #[test]
    fn only_changed_rows_are_painted() {
        let mut layout = Layout::new(30, 4);
        let prompt = live(&["› ", "status"], Some((0, 2)));
        layout.frame(&lines(&["hello"]), &prompt);
        let again = layout.frame(&[], &prompt);
        assert!(!again.contains("hello") && !again.contains("status"));
        layout.clear();
        assert!(layout.frame(&[], &prompt).contains("status"));
    }

    #[test]
    fn bursts_of_typing_become_a_paste() {
        let key = |code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
        let burst = vec![key(KeyCode::Char('a')), key(KeyCode::Enter), key(KeyCode::Char('b')), key(KeyCode::Char('c'))];
        assert!(matches!(translate(burst).as_slice(), [Msg::Paste(text)] if text == "a\nbc"));
        assert_eq!(translate(vec![key(KeyCode::Char('a')), key(KeyCode::Enter)]).len(), 2);
        assert_eq!(translate(vec![key(KeyCode::Up), key(KeyCode::Up), key(KeyCode::Up)]).len(), 3);
        let release = Event::Key(KeyEvent::new_with_kind(KeyCode::Char('x'), KeyModifiers::NONE, KeyEventKind::Release));
        assert!(translate(vec![release]).is_empty());
        let wheel = Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollUp, column: 0, row: 0, modifiers: KeyModifiers::NONE });
        assert!(matches!(translate(vec![wheel]).as_slice(), [Msg::Scroll(Scroll::Lines(WHEEL))]));
    }
}