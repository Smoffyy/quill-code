use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui_kit::component::text::TextView;
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Div, Entity, EventEmitter, FontWeight, IntoElement, ObjectFit, ParentElement, Render, ScrollHandle,
    ScrollWheelEvent, SharedString, Stateful, Styled, Subscription, Task, Window, div, img, point, prelude::*, px, relative,
};
use quill_config::Reasoning;
use quill_core::{Category, FileDiff, LineKind, PlanStep, StepStatus};
use quill_weave::State;

use crate::config::Config;
use crate::icons::Icon;
use crate::motion;
use crate::palette::{Palette, palette};
use crate::project;
use crate::smooth_scroll::SmoothScroll;
use crate::thread::{FileChanges, Item, Thread, ToolItem, ToolStatus, Turn, TurnState};
use crate::ui::{MONO, icon};
use crate::weave::WeaveMark;
use crate::workspace::Workspace;

const COPIED_FOR: Duration = Duration::from_millis(1500);
const LIVE_LINES: usize = 14;

pub enum ThreadEvent {
    OpenDiff { thread: u64, turn: usize },
    Retry { thread: u64, turn: usize },
}

pub struct ThreadView {
    workspace: Entity<Workspace>,
    status: Entity<WeaveMark>,
    scroll: ScrollHandle,
    smooth: SmoothScroll,
    copied: Option<SharedString>,
    copy_reset: Option<Task<()>>,
    shown: Option<u64>,
    open: HashSet<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ThreadEvent> for ThreadView {}

enum Block<'a> {
    One(usize, &'a Item),
    Explore(Vec<&'a ToolItem>),
}

pub fn file_icon(path: &str) -> Icon {
    let ext = path.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
    match ext.as_str() {
        "md" | "txt" | "rst" => Icon::FileText,
        "json" | "toml" | "yaml" | "yml" | "ini" | "cfg" => Icon::File,
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => Icon::Image,
        "" => Icon::File,
        _ => Icon::FileCode,
    }
}

pub fn diff_lines(diff: &FileDiff, limit: usize, p: Palette) -> Div {
    let shown = diff.lines.len().min(limit);
    div()
        .flex()
        .flex_col()
        .font_family(MONO)
        .text_size(px(12.5))
        .line_height(px(20.))
        .children(diff.lines.iter().take(shown).map(|line| {
            let (bg, sign, color) = match line.kind {
                LineKind::Added => (Some(p.add.opacity(0.12)), "+", p.text),
                LineKind::Removed => (Some(p.del.opacity(0.12)), "-", p.text),
                LineKind::Context => (None, " ", p.text_muted),
                LineKind::Gap => (Some(p.fill), "", p.text_faint),
            };
            let number = |n: Option<usize>| n.map(|n| n.to_string()).unwrap_or_default();
            div()
                .flex()
                .when_some(bg, |row, bg| row.bg(bg))
                .when(line.kind == LineKind::Added, |row| row.border_l_2().border_color(p.add))
                .when(line.kind == LineKind::Removed, |row| row.border_l_2().border_color(p.del))
                .child(div().w(px(40.)).flex_shrink_0().pr(px(8.)).flex().justify_end().text_color(p.text_faint).child(number(line.old)))
                .child(div().w(px(40.)).flex_shrink_0().pr(px(8.)).flex().justify_end().text_color(p.text_faint).child(number(line.new)))
                .child(div().w(px(14.)).flex_shrink_0().text_color(p.text_faint).child(sign))
                .child(
                    div().flex_1().min_w_0().whitespace_nowrap().overflow_hidden().text_color(color).child(if line.kind == LineKind::Gap {
                        "⋯".to_string()
                    } else {
                        line.text.clone()
                    }),
                )
        }))
        .when(diff.lines.len() > shown, |col| {
            col.child(
                div()
                    .px(px(12.))
                    .py(px(4.))
                    .text_color(p.text_faint)
                    .child(format!("{} more lines in the review panel", diff.lines.len() - shown)),
            )
        })
}

pub fn tool_icon(tool: &ToolItem) -> Icon {
    match tool.tool.as_str() {
        "list_dir" => Icon::FolderTree,
        "read_file" => Icon::FileText,
        "write_file" => Icon::FilePlus,
        "commit_changes" => Icon::Commit,
        "open_pull_request" => Icon::PullRequest,
        _ => match tool.category {
            Category::Explore => Icon::Search,
            Category::Edit => Icon::Compose,
            Category::Command => Icon::Terminal,
            Category::Plan => Icon::Check,
            Category::Question => Icon::Chat,
            Category::Git => Icon::Branch,
            Category::Web => Icon::Globe,
            Category::Desktop => Icon::Cube,
            Category::Agent => Icon::Bolt,
            Category::Other => Icon::Sliders,
        },
    }
}

fn hidden_tool(tool: &ToolItem) -> bool {
    matches!(tool.category, Category::Plan | Category::Question)
}

fn flush<'a>(items: &'a [Item], run: &mut Vec<usize>, out: &mut Vec<Block<'a>>) {
    match run.as_slice() {
        [] => {}
        [one] => out.push(Block::One(*one, &items[*one])),
        many => out.push(Block::Explore(
            many.iter()
                .filter_map(|index| match &items[*index] {
                    Item::Tool(tool) => Some(tool),
                    _ => None,
                })
                .collect(),
        )),
    }
    run.clear();
}

fn blocks(items: &[Item], skip: Option<usize>, group: bool) -> Vec<Block<'_>> {
    let mut out = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if Some(index) == skip || matches!(item, Item::Plan { .. }) {
            continue;
        }
        match item {
            Item::Tool(tool) if hidden_tool(tool) => continue,
            Item::Tool(tool) if group && tool.category == Category::Explore => run.push(index),
            Item::Text(text) | Item::Reasoning(text) if text.trim().is_empty() => continue,
            _ => {
                flush(items, &mut run, &mut out);
                out.push(Block::One(index, item));
            }
        }
    }
    flush(items, &mut run, &mut out);
    out
}

fn explored(tools: &[&ToolItem]) -> String {
    let count = |names: &[&str]| tools.iter().filter(|t| names.contains(&t.tool.as_str())).count();
    let (reads, searches, lists) = (count(&["read_file"]), count(&["grep", "find_files"]), count(&["list_dir"]));
    let other = tools.len() - reads - searches - lists;
    let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut parts = Vec::new();
    if reads > 0 {
        parts.push(plural(reads, "file", "files"));
    }
    if searches > 0 {
        parts.push(plural(searches, "search", "searches"));
    }
    if lists > 0 {
        parts.push(plural(lists, "folder", "folders"));
    }
    if other > 0 {
        parts.push(plural(other, "lookup", "lookups"));
    }
    parts.join(", ")
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if c != '\r' {
            out.push(c);
        }
    }
    out
}

fn head(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    if all.len() <= lines {
        return text.to_string();
    }
    format!("{}\n… {} more lines", all[..lines].join("\n"), all.len() - lines)
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    if all.len() <= lines {
        return text.to_string();
    }
    format!("… {} earlier lines\n{}", all.len() - lines, all[all.len() - lines..].join("\n"))
}

fn seconds_label(seconds: f32) -> String {
    if seconds < 10.0 { format!("{seconds:.1}s") } else { format!("{}s", seconds.round() as u64) }
}

fn command_body(output: &str) -> &str {
    match output.split_once('\n') {
        Some((first, rest)) if first.starts_with("Exit code") || first.starts_with("Stopped") => rest,
        None if output.starts_with("Exit code") => "",
        _ => output,
    }
}

impl ThreadView {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&workspace, |this, workspace, cx| {
                let active = workspace.read(cx).active;
                if active != this.shown {
                    this.shown = active;
                    this.smooth.cancel();
                    this.scroll.scroll_to_bottom();
                }
                cx.notify();
            }),
            cx.observe_global::<Config>(|_, cx| cx.notify()),
        ];
        ThreadView {
            workspace,
            status: cx.new(|_| WeaveMark::new(State::Thinking)),
            scroll: ScrollHandle::new(),
            smooth: SmoothScroll::default(),
            copied: None,
            copy_reset: None,
            shown: None,
            open: HashSet::new(),
            _subscriptions: subscriptions,
        }
    }

    fn near_bottom(&self) -> bool {
        let offset = -self.scroll.offset().y;
        self.scroll.max_offset().y - offset < px(2.)
    }

    fn wheel(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(window.line_height());
        self.smooth.nudge(self.scroll.offset().y, delta.y, self.scroll.max_offset().y);
        cx.notify();
    }

    fn glide(&mut self, window: &mut Window) {
        let offset = self.scroll.offset();
        if let Some(y) = self.smooth.step(offset.y, Instant::now()) {
            self.scroll.set_offset(point(offset.x, y));
        }
        if self.smooth.moving() {
            window.request_animation_frame();
        }
    }

    fn toggle(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.open.remove(&key) {
            self.open.insert(key);
        }
        cx.notify();
    }

    fn copy(&mut self, id: SharedString, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(id);
        self.copy_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            let _ = this.update(cx, |this, cx| {
                this.copied = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn copy_button(&self, id: SharedString, text: String, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = palette(cx);
        let done = self.copied.as_ref() == Some(&id);
        let click = id.clone();
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(26.))
            .rounded(px(7.))
            .cursor_pointer()
            .hover(move |style| style.bg(p.row_hover))
            .child(icon(if done { Icon::Check } else { Icon::Copy }, 14., if done { p.success } else { p.text_muted }))
            .on_click(cx.listener(move |this, _, _, cx| this.copy(click.clone(), text.clone(), cx)))
    }

    fn row_shell(&self, key: &str, open: bool, has_detail: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = palette(cx);
        let toggle = key.to_string();
        div()
            .id(SharedString::from(key.to_string()))
            .flex()
            .items_center()
            .gap(px(10.))
            .min_h(px(30.))
            .px(px(8.))
            .mx(px(-8.))
            .rounded(px(8.))
            .text_size(px(14.))
            .when(open, |row| row.bg(p.fill))
            .when(has_detail, |row| {
                row.cursor_pointer()
                    .hover(move |style| style.bg(p.row_hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle.clone(), cx)))
            })
    }

    fn status_mark(&self, key: &str, tool: &ToolItem, p: Palette) -> AnyElement {
        match tool.status {
            ToolStatus::Running => {
                motion::spin(SharedString::from(format!("{key}-spin")), icon(Icon::Loader, 14., p.text_faint)).into_any_element()
            }
            ToolStatus::Waiting => div()
                .px(px(7.))
                .h(px(20.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .bg(p.warning.opacity(0.14))
                .text_size(px(12.))
                .text_color(p.warning)
                .child("Needs your OK")
                .into_any_element(),
            ToolStatus::Failed => icon(Icon::CircleX, 14., p.danger).into_any_element(),
            ToolStatus::Done => div().into_any_element(),
        }
    }

    fn tool_body(&self, key: &str, tool: &ToolItem, command: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let failed = tool.status == ToolStatus::Failed;
        let content: AnyElement = if !tool.diffs.is_empty() {
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .children(tool.diffs.iter().map(|diff| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .when(tool.diffs.len() > 1, |col| {
                            col.child(div().font_family(MONO).text_size(px(12.)).text_color(p.text_muted).child(diff.path.clone()))
                        })
                        .child(diff_lines(diff, 80, p))
                }))
                .into_any_element()
        } else if command {
            let output = strip_ansi(&tool.output);
            let body = command_body(&output);
            let shown = if tool.working() { tail(body, LIVE_LINES) } else { head(body, 120) };
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .font_family(MONO)
                .text_size(px(12.5))
                .line_height(px(19.))
                .child(div().text_color(p.link).child(format!("$ {}", tool.target)))
                .when(!shown.trim().is_empty(), |col| col.child(div().text_color(p.text_muted).child(shown)))
                .into_any_element()
        } else {
            div()
                .font_family(MONO)
                .text_size(px(12.5))
                .line_height(px(19.))
                .text_color(if failed { p.danger } else { p.text_muted })
                .child(head(&tool.output, 40))
                .into_any_element()
        };
        motion::rise(
            SharedString::from(format!("{key}-body")),
            div()
                .ml(px(25.))
                .mt(px(2.))
                .mb(px(6.))
                .py(px(8.))
                .px(px(10.))
                .rounded(px(10.))
                .bg(if command { p.sidebar } else { p.fill })
                .border_1()
                .border_color(p.border_soft)
                .overflow_hidden()
                .child(content),
            -4.,
            cx,
        )
        .into_any_element()
    }

    fn tool_row(&self, thread: u64, turn: usize, tool: &ToolItem, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let key = format!("tool-{thread}-{turn}-{}", tool.id);
        let open = self.open.contains(&key);
        let failed = tool.status == ToolStatus::Failed;
        let command = tool.category == Category::Command;
        let target =
            if tool.target.is_empty() { tool.tool.replace('_', " ") } else { tool.target.lines().next().unwrap_or_default().to_string() };
        let added: usize = tool.diffs.iter().map(|d| d.added).sum();
        let removed: usize = tool.diffs.iter().map(|d| d.removed).sum();
        let has_detail = !tool.output.trim().is_empty() || !tool.diffs.is_empty();
        let live = command && tool.status == ToolStatus::Running && !tool.output.trim().is_empty();
        let exit = if command { tool.exit_code().filter(|code| *code != 0) } else { None };
        let row = self
            .row_shell(&key, open, has_detail, cx)
            .child(icon(tool_icon(tool), 15., if failed { p.danger } else { p.text_faint }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .overflow_hidden()
                    .child(div().flex_shrink_0().text_color(p.text_faint).child(tool.verb().to_string()))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(p.text_muted)
                            .when(command, |t| t.font_family(MONO).text_size(px(13.)))
                            .child(target),
                    )
                    .when(!tool.diffs.is_empty(), |row| {
                        row.child(div().flex_shrink_0().text_size(px(12.5)).text_color(p.add).child(format!("+{added}")))
                            .child(div().flex_shrink_0().text_size(px(12.5)).text_color(p.del).child(format!("−{removed}")))
                    }),
            )
            .when_some(exit, |row, code| {
                row.child(
                    div()
                        .px(px(6.))
                        .rounded(px(5.))
                        .bg(p.danger.opacity(0.12))
                        .text_size(px(11.5))
                        .text_color(p.danger)
                        .child(format!("exit {code}")),
                )
            })
            .when_some(tool.seconds().filter(|s| *s >= 1.0), |row, seconds| {
                row.child(div().text_size(px(12.)).text_color(p.text_faint).child(seconds_label(seconds)))
            })
            .child(self.status_mark(&key, tool, p))
            .when(has_detail, |row| row.child(icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }, 13., p.text_faint)));
        let body = (open || live).then(|| self.tool_body(&key, tool, command, cx));
        div().flex().flex_col().child(row).children(body).into_any_element()
    }

    fn explore_group(&self, thread: u64, turn: usize, tools: &[&ToolItem], cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let key = format!("explore-{thread}-{turn}-{}", tools[0].id);
        let open = self.open.contains(&key);
        let working = tools.iter().any(|t| t.working());
        let failed = tools.iter().filter(|t| t.status == ToolStatus::Failed).count();
        let row = self
            .row_shell(&key, open, true, cx)
            .child(icon(Icon::Search, 15., p.text_faint))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().text_color(p.text_faint).child(if working { "Exploring" } else { "Explored" }))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(p.text_muted)
                            .child(explored(tools)),
                    ),
            )
            .when(failed > 0, |row| row.child(div().text_size(px(12.)).text_color(p.danger).child(format!("{failed} failed"))))
            .when(working, |row| row.child(motion::spin(SharedString::from(format!("{key}-spin")), icon(Icon::Loader, 14., p.text_faint))))
            .child(icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }, 13., p.text_faint));
        let inner = open.then(|| {
            let rows: Vec<AnyElement> = tools.iter().map(|tool| self.tool_row(thread, turn, tool, cx)).collect();
            motion::rise(
                SharedString::from(format!("{key}-inner")),
                div().ml(px(14.)).pl(px(10.)).border_l_1().border_color(p.border_soft).flex().flex_col().children(rows),
                -4.,
                cx,
            )
        });
        div().flex().flex_col().child(row).children(inner).into_any_element()
    }

    fn reasoning(&self, thread: u64, turn: usize, index: usize, text: &str, streaming: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = palette(cx);
        let mode = Config::get(cx).appearance.reasoning;
        if mode == Reasoning::Hidden && !streaming {
            return None;
        }
        let key = format!("think-{thread}-{turn}-{index}");
        let open = (mode == Reasoning::Expanded) != self.open.contains(&key);
        let text = text.trim().to_string();
        let preview = text.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or_default().trim().to_string();
        let label = div().flex_shrink_0().text_color(p.text_faint).child(if streaming { "Thinking" } else { "Thought" });
        let label: AnyElement = if streaming {
            motion::shimmer(SharedString::from(format!("{key}-shimmer")), label).into_any_element()
        } else {
            label.into_any_element()
        };
        let row = self
            .row_shell(&key, open, true, cx)
            .child(icon(Icon::Bulb, 15., p.text_faint))
            .child(div().flex_1().min_w_0().flex().items_center().gap(px(6.)).child(label).when(!open, |row| {
                row.child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(13.))
                        .text_color(p.text_faint)
                        .child(preview),
                )
            }))
            .child(icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }, 13., p.text_faint));
        let body = open.then(|| {
            div()
                .ml(px(25.))
                .mb(px(4.))
                .pl(px(12.))
                .border_l_2()
                .border_color(p.border_soft)
                .text_size(px(13.5))
                .line_height(px(21.))
                .text_color(p.text_faint)
                .child(text.clone())
        });
        Some(div().flex().flex_col().child(row).children(body).into_any_element())
    }

    fn changes_card(&self, thread: u64, turn: usize, changes: &[FileChanges], cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let added: usize = changes.iter().map(|c| c.added).sum();
        let removed: usize = changes.iter().map(|c| c.removed).sum();
        let count = changes.len();
        div()
            .mt(px(6.))
            .rounded(px(12.))
            .bg(p.fill)
            .border_1()
            .border_color(p.border_soft)
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(10.))
                    .text_size(px(13.5))
                    .child(icon(Icon::FileDiff, 15., p.text_muted))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.text)
                            .child(format!("{count} {} changed", if count == 1 { "file" } else { "files" })),
                    )
                    .child(div().text_size(px(12.5)).text_color(p.add).child(format!("+{added}")))
                    .child(div().text_size(px(12.5)).text_color(p.del).child(format!("−{removed}")))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id(SharedString::from(format!("open-diff-{thread}-{turn}")))
                            .flex()
                            .items_center()
                            .px(px(10.))
                            .h(px(28.))
                            .rounded(px(7.))
                            .border_1()
                            .border_color(p.border)
                            .text_size(px(12.5))
                            .text_color(p.text)
                            .cursor_pointer()
                            .hover(move |style| style.bg(p.row_hover))
                            .child("Review")
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(ThreadEvent::OpenDiff { thread, turn }))),
                    ),
            )
            .children(changes.iter().map(|change| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(14.))
                    .py(px(7.))
                    .border_t_1()
                    .border_color(p.border_soft)
                    .child(icon(file_icon(&change.path), 15., p.text_muted))
                    .child(div().flex_1().min_w_0().font_family(MONO).text_size(px(13.)).text_color(p.text).child(change.path.clone()))
                    .when(change.created, |row| {
                        row.child(
                            div()
                                .px(px(6.))
                                .rounded(px(5.))
                                .bg(p.success.opacity(0.12))
                                .text_size(px(11.))
                                .text_color(p.success)
                                .child("new"),
                        )
                    })
                    .child(div().w(px(44.)).flex().justify_end().text_size(px(12.5)).text_color(p.add).child(format!("+{}", change.added)))
                    .child(
                        div().w(px(44.)).flex().justify_end().text_size(px(12.5)).text_color(p.del).child(format!("−{}", change.removed)),
                    )
            }))
    }

    fn activity(turn: &Turn, waiting: bool, asked: bool) -> String {
        if asked {
            return "Waiting for your answer".into();
        }
        if waiting {
            return "Waiting for your OK".into();
        }
        match turn.items.iter().rev().find(|item| !matches!(item, Item::Plan { .. } | Item::Note(_) | Item::Steer(_))) {
            None => "Warming up the model".into(),
            Some(Item::Reasoning(_)) => "Thinking".into(),
            Some(Item::Text(_)) => "Writing".into(),
            Some(Item::Tool(tool)) if tool.working() => {
                let target = tool.target.lines().next().unwrap_or_default();
                if target.is_empty() { tool.active.clone() } else { format!("{} {target}", tool.active) }
            }
            _ => "Thinking".into(),
        }
    }

    fn header(&self, thread: &Thread, index: usize, turn: &Turn, waiting: bool, asked: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = palette(cx);
        let tid = thread.id;
        if turn.working() {
            let tone = if waiting || asked { p.warning } else { p.text_faint };
            let label = div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(tone)
                .child(Self::activity(turn, waiting, asked));
            return Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pb(px(8.))
                    .mb(px(2.))
                    .border_b_1()
                    .border_color(p.border_soft)
                    .text_size(px(13.5))
                    .child(motion::shimmer(SharedString::from(format!("activity-{tid}-{index}")), label))
                    .child(div().flex_1())
                    .child(div().flex_shrink_0().text_size(px(12.5)).text_color(p.text_faint).child(format!("{}s", turn.elapsed_secs())))
                    .into_any_element(),
            );
        }
        if !turn.has_steps() {
            return None;
        }
        let expanded = turn.expanded;
        Some(
            div()
                .id(SharedString::from(format!("worked-{tid}-{index}")))
                .flex()
                .items_center()
                .gap(px(6.))
                .pb(px(8.))
                .mb(px(2.))
                .border_b_1()
                .border_color(p.border_soft)
                .text_size(px(13.5))
                .text_color(if expanded { p.text } else { p.text_faint })
                .cursor_pointer()
                .hover(move |style| style.text_color(p.text))
                .child(format!("Worked for {}s", turn.elapsed_secs()))
                .child(icon(if expanded { Icon::ChevronDown } else { Icon::ChevronRight }, 13., p.text_faint))
                .when(!expanded, |row| {
                    row.child(
                        div()
                            .ml(px(4.))
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(p.text_faint)
                            .child(turn.summary()),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| this.workspace.update(cx, |w, cx| w.toggle_turn(tid, index, cx))))
                .into_any_element(),
        )
    }

    fn prompt(&self, thread: &Thread, index: usize, turn: &Turn, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let tid = thread.id;
        let mut column = div().flex().flex_col().gap(px(8.));
        if !turn.images.is_empty() {
            column = column.child(div().flex().flex_wrap().justify_end().gap(px(8.)).children(turn.images.iter().enumerate().map(
                |(n, path)| {
                    let open = path.clone();
                    div()
                        .id(SharedString::from(format!("turn-image-{tid}-{index}-{n}")))
                        .w(px(200.))
                        .h(px(140.))
                        .rounded(px(12.))
                        .border_1()
                        .border_color(p.border_soft)
                        .bg(p.fill)
                        .cursor_pointer()
                        .child(img(path.clone()).size_full().rounded(px(11.)).object_fit(ObjectFit::Cover))
                        .on_click(move |_, _, _| project::open_folder(&open))
                },
            )));
        }
        let group = SharedString::from(format!("prompt-{tid}-{index}"));
        let bubble = div()
            .max_w(relative(0.85))
            .px(px(16.))
            .py(px(10.))
            .rounded(px(18.))
            .bg(p.fill)
            .text_size(px(14.5))
            .line_height(px(23.))
            .text_color(p.text)
            .child(turn.prompt.clone())
            .when(!turn.attachments.is_empty(), |bubble| {
                bubble.child(div().mt(px(8.)).flex().flex_wrap().gap(px(6.)).children(turn.attachments.iter().map(|name| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .px(px(8.))
                        .h(px(24.))
                        .rounded(px(7.))
                        .bg(p.fill)
                        .text_size(px(12.))
                        .text_color(p.text_muted)
                        .child(icon(file_icon(name), 12., p.text_muted))
                        .child(name.clone())
                })))
            });
        column.child(
            div().group(group.clone()).flex().flex_col().items_end().gap(px(2.)).child(bubble).child(
                self.copy_button(SharedString::from(format!("copy-prompt-{tid}-{index}")), turn.prompt.clone(), cx)
                    .invisible()
                    .group_hover(group, |style| style.visible()),
            ),
        )
    }

    fn block(&self, tid: u64, index: usize, turn: &Turn, block: &Block<'_>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = palette(cx);
        let last = turn.items.len().saturating_sub(1);
        let element = match block {
            Block::Explore(tools) => self.explore_group(tid, index, tools, cx),
            Block::One(item_index, item) => match item {
                Item::Reasoning(text) => return self.reasoning(tid, index, *item_index, text, turn.working() && *item_index == last, cx),
                Item::Text(text) => div()
                    .py(px(4.))
                    .text_size(px(14.5))
                    .line_height(px(24.))
                    .text_color(p.text)
                    .child(TextView::markdown(SharedString::from(format!("md-{tid}-{index}-{item_index}")), text.clone()))
                    .into_any_element(),
                Item::Tool(tool) => self.tool_row(tid, index, tool, cx),
                Item::Question { question, answer, .. } => div()
                    .flex()
                    .gap(px(10.))
                    .py(px(4.))
                    .child(div().pt(px(3.)).child(icon(Icon::Chat, 15., p.link)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .text_size(px(13.5))
                            .child(div().text_color(p.text).child(question.clone()))
                            .child(div().text_color(if answer.is_some() { p.text_muted } else { p.warning }).child(match answer {
                                Some(answer) => format!("You answered: {answer}"),
                                None => "Waiting for your answer".to_string(),
                            })),
                    )
                    .into_any_element(),
                Item::Steer(text) => div()
                    .flex()
                    .justify_end()
                    .py(px(2.))
                    .child(
                        div()
                            .max_w(relative(0.8))
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap(px(2.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .text_size(px(11.5))
                                    .text_color(p.text_faint)
                                    .child(icon(Icon::Steer, 12., p.text_faint))
                                    .child("Added while working"),
                            )
                            .child(
                                div()
                                    .px(px(12.))
                                    .py(px(6.))
                                    .rounded(px(14.))
                                    .border_1()
                                    .border_color(p.border_soft)
                                    .text_size(px(13.5))
                                    .text_color(p.text_muted)
                                    .child(text.clone()),
                            ),
                    )
                    .into_any_element(),
                Item::Note(text) => div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .py(px(4.))
                    .text_size(px(12.5))
                    .text_color(p.text_faint)
                    .child(icon(Icon::Info, 13., p.text_faint))
                    .child(text.clone())
                    .into_any_element(),
                Item::Plan { .. } => return None,
            },
        };
        Some(element)
    }

    fn turn_view(&self, thread: &Thread, index: usize, turn: &Turn, waiting: bool, asked: bool, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let tid = thread.id;
        let latest = index + 1 == thread.turns.len();
        let working = turn.working();
        let final_text = turn.final_text();
        let show_steps = working || turn.expanded;
        let group = Config::get(cx).appearance.group_exploration;
        let mut agent = div().flex().flex_col().gap(px(4.));
        if let Some(header) = self.header(thread, index, turn, waiting, asked, cx) {
            agent = agent.child(header);
        }
        if show_steps {
            let plan = turn.items.iter().find_map(|item| match item {
                Item::Plan { steps, explanation } => Some((steps, explanation)),
                _ => None,
            });
            if let Some((steps, explanation)) = plan {
                agent = agent.child(plan_card(steps, explanation.as_deref(), p));
            }
            for (n, block) in blocks(&turn.items, final_text, group).iter().enumerate() {
                if let Some(element) = self.block(tid, index, turn, block, cx) {
                    let id = SharedString::from(format!("block-{tid}-{index}-{n}"));
                    agent = agent.child(if working { motion::rise(id, div().child(element), 6., cx).into_any_element() } else { element });
                }
            }
        }
        if let Some(item_index) = final_text
            && let Item::Text(text) = &turn.items[item_index]
        {
            let reply_group = SharedString::from(format!("reply-{tid}-{index}"));
            let copy = self
                .copy_button(SharedString::from(format!("copy-reply-{tid}-{index}")), text.clone(), cx)
                .when(!latest, |button| button.invisible().group_hover(reply_group.clone(), |style| style.visible()));
            let mut model = turn.model.clone().unwrap_or_default();
            if let Some(usd) = turn.cost.filter(|usd| *usd > 0.0) {
                model = format!("{model} · {}", if usd < 0.01 { format!("${usd:.4}") } else { format!("${usd:.2}") });
            }
            agent = agent.child(
                div()
                    .group(reply_group)
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .pt(px(6.))
                            .text_size(px(14.5))
                            .line_height(px(24.))
                            .text_color(p.text)
                            .child(TextView::markdown(SharedString::from(format!("md-{tid}-{index}-{item_index}")), text.clone())),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(copy)
                            .when(!model.is_empty(), |row| row.child(div().text_size(px(12.)).text_color(p.text_faint).child(model))),
                    ),
            );
        }
        match &turn.state {
            TurnState::Stopped => {
                agent = agent.child(
                    div()
                        .pt(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(13.))
                        .text_color(p.text_faint)
                        .child(icon(Icon::CircleStop, 14., p.text_faint))
                        .child("You stopped this reply")
                        .when(latest, |row| row.child(self.retry_button(tid, index, cx))),
                )
            }
            TurnState::Failed(error) => {
                agent = agent.child(
                    div()
                        .mt(px(4.))
                        .flex()
                        .items_start()
                        .gap(px(10.))
                        .px(px(12.))
                        .py(px(10.))
                        .rounded(px(10.))
                        .bg(p.danger.opacity(0.1))
                        .border_1()
                        .border_color(p.danger.opacity(0.25))
                        .text_size(px(13.5))
                        .text_color(p.danger)
                        .child(div().pt(px(1.)).child(icon(Icon::CircleAlert, 15., p.danger)))
                        .child(div().flex_1().child(error.clone()))
                        .when(latest, |row| row.child(self.retry_button(tid, index, cx))),
                )
            }
            _ => {}
        }
        let changes = turn.changes();
        if !working && !changes.is_empty() {
            agent = agent.child(self.changes_card(tid, index, &changes, cx));
        }
        let gutter =
            div().w(px(26.)).flex_shrink_0().pt(px(1.)).when(latest, |gutter| gutter.child(div().size(px(24.)).child(self.status.clone())));
        let response = div().flex().gap(px(12.)).child(gutter).child(agent.flex_1().min_w_0());
        div().flex().flex_col().gap(px(14.)).child(self.prompt(thread, index, turn, cx)).child(response)
    }

    fn retry_button(&self, thread: u64, turn: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = palette(cx);
        div()
            .id(SharedString::from(format!("retry-{thread}-{turn}")))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(5.))
            .px(px(10.))
            .h(px(26.))
            .rounded(px(7.))
            .border_1()
            .border_color(p.border)
            .text_size(px(12.5))
            .text_color(p.text)
            .cursor_pointer()
            .hover(move |style| style.bg(p.row_hover))
            .child(icon(Icon::Retry, 13., p.text_muted))
            .child("Try again")
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(ThreadEvent::Retry { thread, turn })))
    }
}

fn plan_card(steps: &[PlanStep], explanation: Option<&str>, p: Palette) -> Div {
    let done = steps.iter().filter(|step| step.status == StepStatus::Done).count();
    let ratio = if steps.is_empty() { 0.0 } else { done as f32 / steps.len() as f32 };
    div()
        .my(px(4.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(12.))
        .bg(p.fill)
        .border_1()
        .border_color(p.border_soft)
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .text_size(px(12.5))
                .text_color(p.text_faint)
                .child(icon(Icon::ListChecks, 14., p.text_muted))
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(p.text_muted).child("Plan"))
                .child(format!("{done} of {} done", steps.len()))
                .child(
                    div()
                        .flex_1()
                        .h(px(4.))
                        .rounded_full()
                        .bg(p.border_soft)
                        .child(div().h_full().w(relative(ratio)).rounded_full().bg(p.success)),
                ),
        )
        .children(steps.iter().map(|step| {
            let mark = match step.status {
                StepStatus::Done => icon(Icon::CheckCircle, 15., p.success).into_any_element(),
                StepStatus::Active => icon(Icon::Loader, 15., p.link).into_any_element(),
                StepStatus::Pending => {
                    div().size(px(13.)).m(px(1.)).rounded_full().border_1().border_color(p.text_faint).into_any_element()
                }
            };
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .text_size(px(13.5))
                .text_color(match step.status {
                    StepStatus::Done => p.text_faint,
                    StepStatus::Active => p.text,
                    StepStatus::Pending => p.text_muted,
                })
                .when(step.status == StepStatus::Done, |row| row.line_through())
                .child(div().flex_shrink_0().child(mark))
                .child(step.text.clone())
        }))
        .when_some(explanation, |card, text| {
            card.child(div().pt(px(2.)).text_size(px(12.5)).text_color(p.text_faint).child(text.to_string()))
        })
}

impl Render for ThreadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.glide(window);
        if !self.smooth.moving() && self.near_bottom() {
            self.scroll.scroll_to_bottom();
        }
        let workspace = self.workspace.read(cx);
        let Some(thread) = workspace.active_thread().cloned() else { return div().size_full() };
        let waiting = workspace.pending(thread.id).is_some();
        let asked = workspace.asked(thread.id).is_some();
        let working = thread.turns.last().is_some_and(|t| t.working());
        let state = if waiting || asked || !working {
            State::Idle
        } else {
            match thread.turns.last().and_then(|t| t.items.last()) {
                Some(Item::Text(_)) => State::Generating,
                _ => State::Thinking,
            }
        };
        self.status.update(cx, |mark, cx| mark.set_state(state, cx));
        let width = f32::from(Config::get(cx).appearance.transcript_width.clamp(560, 1600));
        let last = thread.turns.len().saturating_sub(1);
        let turns: Vec<Div> = thread
            .turns
            .iter()
            .enumerate()
            .map(|(index, turn)| self.turn_view(&thread, index, turn, waiting && index == last, asked && index == last, cx))
            .collect();
        div().size_full().child(
            div()
                .id("thread-scroll")
                .size_full()
                .overflow_y_hidden()
                .track_scroll(&self.scroll)
                .on_scroll_wheel(cx.listener(Self::wheel))
                .child(div().max_w(px(width)).mx_auto().px(px(28.)).pt(px(20.)).pb(px(40.)).flex().flex_col().gap(px(36.)).children(turns)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_core::tools::legacy;

    fn tool(name: &str) -> Item {
        let mut item = ToolItem::new(legacy(name));
        item.id = name.into();
        item.status = ToolStatus::Done;
        Item::Tool(item)
    }

    #[test]
    fn exploration_runs_become_one_group() {
        let items = vec![tool("read_file"), tool("grep"), tool("list_dir"), tool("shell"), tool("read_file"), Item::Text("done".into())];
        let grouped = blocks(&items, Some(5), true);
        assert_eq!(grouped.len(), 3);
        assert!(matches!(&grouped[0], Block::Explore(tools) if tools.len() == 3));
        assert!(matches!(&grouped[1], Block::One(3, _)));
        assert!(matches!(&grouped[2], Block::One(4, _)));
        assert_eq!(blocks(&items, Some(5), false).len(), 5);
    }

    #[test]
    fn exploration_reads_like_a_count() {
        let items = [tool("read_file"), tool("read_file"), tool("grep")];
        let tools: Vec<&ToolItem> = items
            .iter()
            .filter_map(|item| match item {
                Item::Tool(tool) => Some(tool),
                _ => None,
            })
            .collect();
        assert_eq!(explored(&tools), "2 files, 1 search");
    }

    #[test]
    fn terminal_output_is_cleaned_for_display() {
        assert_eq!(strip_ansi("\u{1b}[32mok\u{1b}[0m\r\n"), "ok\n");
        assert_eq!(tail("a\nb\nc", 2), "… 1 earlier lines\nb\nc");
        assert_eq!(command_body("Exit code 0\nhello"), "hello");
        assert_eq!(command_body("Exit code 0"), "");
        assert_eq!(seconds_label(2.34), "2.3s");
        assert_eq!(seconds_label(42.6), "43s");
    }
}