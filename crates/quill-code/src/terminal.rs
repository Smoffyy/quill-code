use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Stdio};

use futures::StreamExt;
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    Context, Entity, EventEmitter, Focusable, IntoElement, KeystrokeEvent, ParentElement, Render, ScrollStrategy, SharedString, Styled,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};

use crate::git::hidden;
use crate::icons::Icon;
use crate::palette::palette;
use crate::ui::{MONO, icon};

const DONE: &str = "__quill_done__";
const MAX_LINES: usize = 5000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Command,
    Output,
    Error,
    Note,
}

enum ShellEvent {
    Line(Kind, String),
    Done { ok: bool, cwd: String },
    Exited,
}

struct Session {
    child: Child,
    stdin: ChildStdin,
}

pub struct CloseTerminal;

pub struct Terminal {
    root: Option<PathBuf>,
    cwd: String,
    lines: Vec<(Kind, SharedString)>,
    input: Entity<InputState>,
    session: Option<Session>,
    busy: bool,
    history: Vec<String>,
    recall: Option<usize>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CloseTerminal> for Terminal {}

fn pump(reader: impl Read + Send + 'static, kind: Kind, tx: UnboundedSender<ShellEvent>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string();
                    let event = match line.strip_prefix(DONE) {
                        Some(rest) => {
                            let (ok, cwd) = rest.trim().split_once(' ').unwrap_or(("1", ""));
                            ShellEvent::Done { ok: ok == "1", cwd: cwd.to_string() }
                        }
                        None => ShellEvent::Line(kind, line),
                    };
                    if tx.unbounded_send(event).is_err() {
                        break;
                    }
                }
            }
        }
        if kind == Kind::Output {
            let _ = tx.unbounded_send(ShellEvent::Exited);
        }
    });
}

impl Terminal {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type a PowerShell command and press Enter"));
        let weak = cx.entity().downgrade();
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.submit(window, cx);
                }
            }),
            cx.intercept_keystrokes(move |event, window, cx| {
                if let Some(this) = weak.upgrade() {
                    this.update(cx, |this, cx| this.intercept(event, window, cx));
                }
            }),
        ];
        Terminal {
            root: None,
            cwd: String::new(),
            lines: Vec::new(),
            input,
            session: None,
            busy: false,
            history: Vec::new(),
            recall: None,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.root == root {
            return;
        }
        self.stop_session();
        self.root = root;
        self.cwd = self.root.as_ref().map(|r| r.display().to_string()).unwrap_or_default();
        self.lines.clear();
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn stop_session(&mut self) {
        if let Some(mut session) = self.session.take() {
            let _ = session.child.kill();
        }
        self.busy = false;
    }

    fn push(&mut self, kind: Kind, text: impl Into<SharedString>) {
        self.lines.push((kind, text.into()));
        if self.lines.len() > MAX_LINES {
            self.lines.drain(..self.lines.len() - MAX_LINES);
        }
        self.scroll.scroll_to_item(self.lines.len().saturating_sub(1), ScrollStrategy::Bottom);
    }

    fn start(&mut self, cx: &mut Context<Self>) -> bool {
        if self.session.is_some() {
            return true;
        }
        let Some(root) = self.root.clone() else {
            self.push(Kind::Note, "Open a project to use the terminal.");
            return false;
        };
        let spawned = hidden("powershell")
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", "-"])
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                self.push(Kind::Error, format!("PowerShell could not start: {error}"));
                return false;
            }
        };
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            return false;
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        pump(stdout, Kind::Output, tx.clone());
        pump(stderr, Kind::Error, tx);
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this.update(cx, |this, cx| this.apply(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        self.session = Some(Session { child, stdin });
        let mut setup = String::from(
            "[Console]::OutputEncoding=[Text.Encoding]::UTF8; $OutputEncoding=[Text.Encoding]::UTF8; $ProgressPreference='SilentlyContinue'\n",
        );
        if !self.cwd.is_empty() {
            setup.push_str(&format!("Set-Location -LiteralPath '{}'\n", self.cwd.replace('\'', "''")));
        }
        self.write(&setup);
        true
    }

    fn write(&mut self, text: &str) {
        let failed = match self.session.as_mut() {
            Some(session) => session.stdin.write_all(text.as_bytes()).and_then(|_| session.stdin.flush()).is_err(),
            None => true,
        };
        if failed {
            self.stop_session();
            self.push(Kind::Error, "The PowerShell session ended. Press Restart to open a new one.");
        }
    }

    fn apply(&mut self, event: ShellEvent, cx: &mut Context<Self>) {
        match event {
            ShellEvent::Line(kind, text) => self.push(kind, text),
            ShellEvent::Done { ok, cwd } => {
                self.busy = false;
                if !cwd.is_empty() {
                    self.cwd = cwd;
                }
                if !ok {
                    self.push(Kind::Note, "The command reported an error.");
                }
            }
            ShellEvent::Exited => {
                if self.session.take().is_some() {
                    self.push(Kind::Note, "PowerShell exited.");
                }
                self.busy = false;
            }
        }
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let command = self.input.read(cx).value().trim().to_string();
        if command.is_empty() || self.busy || !self.start(cx) {
            cx.notify();
            return;
        }
        self.input.update(cx, |input, cx| input.set_value("", window, cx));
        if self.history.last() != Some(&command) {
            self.history.push(command.clone());
        }
        self.recall = None;
        self.push(Kind::Command, command.clone());
        self.busy = true;
        self.write(&format!("{command}\n$__ok=$?; [Console]::Out.WriteLine('{DONE} ' + [int]$__ok + ' ' + (Get-Location).Path)\n"));
        cx.notify();
    }

    fn restart(&mut self, cx: &mut Context<Self>) {
        self.stop_session();
        self.push(Kind::Note, "Restarted PowerShell.");
        self.start(cx);
        cx.notify();
    }

    fn intercept(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if !matches!(key, "up" | "down") || self.history.is_empty() || !self.input.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        let last = self.history.len() - 1;
        self.recall = match (key, self.recall) {
            ("up", None) => Some(last),
            ("up", Some(i)) => Some(i.saturating_sub(1)),
            ("down", Some(i)) if i < last => Some(i + 1),
            _ => None,
        };
        let text = self.recall.map(|i| self.history[i].clone()).unwrap_or_default();
        self.input.update(cx, |input, cx| input.set_value(text, window, cx));
        cx.stop_propagation();
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.stop_session();
    }
}

impl Render for Terminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let lines = self.lines.clone();
        let cwd = self.cwd.clone();
        let tool = |id: &'static str, name: Icon| {
            div()
                .id(id)
                .size(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(move |style| style.bg(p.row_hover))
                .child(icon(name, 14., p.text_faint))
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.sidebar)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(36.))
                    .flex_shrink_0()
                    .px(px(12.))
                    .border_b_1()
                    .border_color(p.border_soft)
                    .child(icon(Icon::Terminal, 14., p.text_muted))
                    .child(div().text_size(px(13.)).text_color(p.text).child("Terminal"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_family(MONO)
                            .text_size(px(12.))
                            .text_color(p.text_faint)
                            .child(cwd.clone()),
                    )
                    .when(self.busy, |row| row.child(div().text_size(px(12.)).text_color(p.link).child("running")))
                    .child(tool("terminal-clear", Icon::Eraser).on_click(cx.listener(|this, _, _, cx| {
                        this.lines.clear();
                        cx.notify();
                    })))
                    .child(tool("terminal-restart", Icon::Retry).on_click(cx.listener(|this, _, _, cx| this.restart(cx))))
                    .child(tool("terminal-close", Icon::X).on_click(cx.listener(|_, _, _, cx| cx.emit(CloseTerminal)))),
            )
            .child(
                uniform_list("terminal-lines", lines.len(), move |range, _, _| {
                    range
                        .map(|i| {
                            let (kind, text) = &lines[i];
                            let color = match kind {
                                Kind::Command => p.text,
                                Kind::Output => p.text_muted,
                                Kind::Error => p.danger,
                                Kind::Note => p.text_faint,
                            };
                            div()
                                .h(px(19.))
                                .px(px(12.))
                                .flex()
                                .font_family(MONO)
                                .text_size(px(12.5))
                                .whitespace_nowrap()
                                .text_color(color)
                                .when(*kind == Kind::Command, |row| row.child(div().text_color(p.link).pr(px(8.)).child("PS>")))
                                .child(text.clone())
                        })
                        .collect()
                })
                .track_scroll(&self.scroll)
                .flex_1()
                .py(px(6.)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(36.))
                    .flex_shrink_0()
                    .px(px(12.))
                    .border_t_1()
                    .border_color(p.border_soft)
                    .font_family(MONO)
                    .text_size(px(12.5))
                    .child(div().text_color(p.link).child("PS>"))
                    .child(div().flex_1().child(Input::new(&self.input).appearance(false))),
            )
    }
}