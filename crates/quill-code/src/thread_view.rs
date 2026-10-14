use std::time::{Duration, Instant};

use gpui_kit::component::text::TextView;
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Div, Entity, EventEmitter, FontWeight, IntoElement, ObjectFit, ParentElement, Render, ScrollHandle,
    ScrollWheelEvent, SharedString, Stateful, Styled, Subscription, Task, Window, div, img, point, prelude::*, px, relative,
};
use quill_agent::{FileDiff, LineKind, ToolKind};
use quill_weave::State;

use crate::icons::Icon;
use crate::palette::{Palette, palette};
use crate::project;
use crate::smooth_scroll::SmoothScroll;
use crate::thread::{FileChanges, Item, Thread, ToolItem, ToolStatus, Turn, TurnState};
use crate::ui::{MONO, icon};
use crate::weave::WeaveMark;
use crate::workspace::Workspace;

const COPIED_FOR: Duration = Duration::from_millis(1500);

pub struct OpenDiff {
    pub thread: u64,
    pub turn: usize,
}

pub struct ThreadView {
    workspace: Entity<Workspace>,
    status: Entity<WeaveMark>,
    scroll: ScrollHandle,
    smooth: SmoothScroll,
    copied: Option<SharedString>,
    copy_reset: Option<Task<()>>,
    shown: Option<u64>,
    _subscription: Subscription,
}

impl EventEmitter<OpenDiff> for ThreadView {}

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

fn verb(tool: &ToolItem) -> &'static str {
    let running = matches!(tool.status, ToolStatus::Running | ToolStatus::Waiting);
    match (tool.kind, running) {
        (ToolKind::List, true) => "Looking through",
        (ToolKind::List, false) => "Looked through",
        (ToolKind::Read, true) => "Opening",
        (ToolKind::Read, false) => "Opened",
        (ToolKind::Search, true) => "Hunting for",
        (ToolKind::Search, false) => "Hunted for",
        (ToolKind::Write, true) => "Writing",
        (ToolKind::Write, false) => {
            if tool.diff.as_ref().is_some_and(|d| d.created) {
                "Added"
            } else {
                "Rewrote"
            }
        }
        (ToolKind::Edit, true) => "Changing",
        (ToolKind::Edit, false) => "Changed",
        (ToolKind::Run, true) => "Running",
        (ToolKind::Run, false) => "Ran",
        (ToolKind::Commit, true) => "Committing",
        (ToolKind::Commit, false) => "Committed",
        (ToolKind::PullRequest, true) => "Opening a pull request",
        (ToolKind::PullRequest, false) => "Opened a pull request",
    }
}

fn kind_icon(kind: ToolKind) -> Icon {
    match kind {
        ToolKind::List => Icon::FolderTree,
        ToolKind::Read => Icon::FileText,
        ToolKind::Search => Icon::Search,
        ToolKind::Write => Icon::FilePlus,
        ToolKind::Edit => Icon::Compose,
        ToolKind::Run => Icon::Terminal,
        ToolKind::Commit => Icon::Commit,
        ToolKind::PullRequest => Icon::PullRequest,
    }
}

impl ThreadView {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&workspace, |this, workspace, cx| {
            let active = workspace.read(cx).active;
            if active != this.shown {
                this.shown = active;
                this.smooth.cancel();
                this.scroll.scroll_to_bottom();
            }
            cx.notify();
        });
        ThreadView {
            workspace,
            status: cx.new(|_| WeaveMark::new(State::Thinking)),
            scroll: ScrollHandle::new(),
            smooth: SmoothScroll::default(),
            copied: None,
            copy_reset: None,
            shown: None,
            _subscription: subscription,
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

    fn tool_row(&self, thread: u64, turn: usize, tool: &ToolItem, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let id = tool.id.clone();
        let failed = tool.status == ToolStatus::Failed;
        let waiting = tool.status == ToolStatus::Waiting;
        let running = tool.status == ToolStatus::Running;
        let target = if tool.target.is_empty() { tool.kind.name().to_string() } else { tool.target.clone() };
        let has_detail = !tool.output.is_empty() || tool.diff.is_some();
        let row = div()
            .id(SharedString::from(format!("tool-{thread}-{turn}-{id}")))
            .flex()
            .items_center()
            .gap(px(10.))
            .min_h(px(30.))
            .px(px(6.))
            .mx(px(-6.))
            .rounded(px(8.))
            .text_size(px(14.))
            .when(has_detail, |row| row.cursor_pointer().hover(move |style| style.bg(p.row_hover)))
            .child(icon(kind_icon(tool.kind), 15., if failed { p.danger } else { p.text_faint }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .overflow_hidden()
                    .child(div().flex_shrink_0().text_color(p.text_faint).child(verb(tool)))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(p.text_muted)
                            .when(tool.kind == ToolKind::Run, |t| t.font_family(MONO).text_size(px(13.)))
                            .child(target),
                    )
                    .when_some(tool.diff.as_ref(), |row, diff| {
                        row.child(div().flex_shrink_0().text_size(px(12.5)).text_color(p.add).child(format!("+{}", diff.added)))
                            .child(div().flex_shrink_0().text_size(px(12.5)).text_color(p.del).child(format!("-{}", diff.removed)))
                    }),
            )
            .when(waiting, |row| row.child(div().text_size(px(12.5)).text_color(p.warning).child("Needs your OK")))
            .when(running, |row| row.child(icon(Icon::Loader, 14., p.text_faint)))
            .when(failed, |row| row.child(icon(Icon::CircleX, 14., p.danger)))
            .when(has_detail, |row| row.child(icon(if tool.open { Icon::ChevronDown } else { Icon::ChevronRight }, 14., p.text_faint)))
            .on_click(cx.listener(move |this, _, _, cx| {
                let id = id.clone();
                this.workspace.update(cx, |w, cx| w.toggle_tool(thread, turn, &id, cx));
            }));
        let detail = tool.open.then(|| {
            let body: AnyElement = match (&tool.diff, tool.kind) {
                (Some(diff), _) => diff_lines(diff, 60, p).into_any_element(),
                (None, ToolKind::Run) => div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .font_family(MONO)
                    .text_size(px(12.5))
                    .line_height(px(19.))
                    .child(div().text_color(p.link).child(format!("> {}", tool.target)))
                    .child(div().text_color(p.text_muted).child(clip(&tool.output, 60)))
                    .into_any_element(),
                _ => div()
                    .font_family(MONO)
                    .text_size(px(12.5))
                    .line_height(px(19.))
                    .text_color(if failed { p.danger } else { p.text_muted })
                    .child(clip(&tool.output, 30))
                    .into_any_element(),
            };
            div()
                .ml(px(25.))
                .mt(px(2.))
                .mb(px(6.))
                .py(px(8.))
                .px(px(10.))
                .rounded(px(10.))
                .bg(p.fill)
                .border_1()
                .border_color(p.border_soft)
                .overflow_hidden()
                .child(body)
        });
        div().flex().flex_col().child(row).children(detail).into_any_element()
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
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.text)
                            .child(format!("{count} {} touched", if count == 1 { "file" } else { "files" })),
                    )
                    .child(div().text_size(px(12.5)).text_color(p.add).child(format!("+{added}")))
                    .child(div().text_size(px(12.5)).text_color(p.del).child(format!("-{removed}")))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id(SharedString::from(format!("open-diff-{thread}-{turn}")))
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .px(px(8.))
                            .h(px(26.))
                            .rounded(px(7.))
                            .text_size(px(12.5))
                            .text_color(p.text_muted)
                            .cursor_pointer()
                            .hover(move |style| style.bg(p.row_hover).text_color(p.text))
                            .child(icon(Icon::FileDiff, 14., p.text_muted))
                            .child("Review")
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenDiff { thread, turn }))),
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
                    .when(change.created, |row| row.child(div().text_size(px(11.5)).text_color(p.text_faint).child("new")))
                    .child(div().w(px(44.)).flex().justify_end().text_size(px(12.5)).text_color(p.add).child(format!("+{}", change.added)))
                    .child(
                        div().w(px(44.)).flex().justify_end().text_size(px(12.5)).text_color(p.del).child(format!("-{}", change.removed)),
                    )
            }))
    }

    fn turn_view(&self, thread: &Thread, index: usize, turn: &Turn, pending: bool, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let tid = thread.id;
        let latest = index + 1 == thread.turns.len();
        let working = turn.working();
        let final_text = turn.final_text();
        let show_steps = working || turn.expanded;
        let mut column = div().flex().flex_col().gap(px(12.));
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
        let prompt_group = SharedString::from(format!("prompt-{tid}-{index}"));
        let message = div()
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
        column = column.child(
            div().group(prompt_group.clone()).flex().flex_col().items_end().gap(px(2.)).child(message).child(
                self.copy_button(SharedString::from(format!("copy-prompt-{tid}-{index}")), turn.prompt.clone(), cx)
                    .invisible()
                    .group_hover(prompt_group, |style| style.visible()),
            ),
        );
        let mut agent = div().flex().flex_col().gap(px(4.));
        if working {
            agent = agent.child(
                div()
                    .pb(px(6.))
                    .mb(px(4.))
                    .border_b_1()
                    .border_color(p.border_soft)
                    .text_size(px(13.5))
                    .text_color(p.text_faint)
                    .child(format!("Agent running · {}s", turn.elapsed_secs())),
            );
        } else if turn.has_steps() {
            agent = agent.child(
                div()
                    .id(SharedString::from(format!("worked-{tid}-{index}")))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pb(px(6.))
                    .mb(px(4.))
                    .border_b_1()
                    .border_color(p.border_soft)
                    .text_size(px(13.5))
                    .text_color(if turn.expanded { p.text } else { p.text_faint })
                    .cursor_pointer()
                    .hover(move |style| style.text_color(p.text))
                    .child(format!("Finished in {}s", turn.elapsed_secs()))
                    .child(icon(if turn.expanded { Icon::ChevronDown } else { Icon::ChevronRight }, 13., p.text_faint))
                    .when(!turn.expanded, |row| row.child(div().ml(px(4.)).text_color(p.text_faint).child(format!("· {}", turn.summary()))))
                    .on_click(cx.listener(move |this, _, _, cx| this.workspace.update(cx, |w, cx| w.toggle_turn(tid, index, cx)))),
            );
        }
        if show_steps {
            for (item_index, item) in turn.items.iter().enumerate() {
                if Some(item_index) == final_text {
                    continue;
                }
                agent = agent.child(match item {
                    Item::Reasoning(text) if !text.trim().is_empty() => div()
                        .flex()
                        .gap(px(10.))
                        .py(px(4.))
                        .child(div().pt(px(3.)).child(icon(Icon::Bulb, 15., p.text_faint)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(13.5))
                                .line_height(px(21.))
                                .text_color(p.text_faint)
                                .line_clamp(4)
                                .child(text.trim().to_string()),
                        )
                        .into_any_element(),
                    Item::Text(text) if !text.trim().is_empty() => div()
                        .py(px(4.))
                        .text_size(px(14.5))
                        .line_height(px(24.))
                        .text_color(p.text)
                        .child(TextView::markdown(SharedString::from(format!("md-{tid}-{index}-{item_index}")), text.clone()))
                        .into_any_element(),
                    Item::Tool(tool) => self.tool_row(tid, index, tool, cx),
                    _ => div().into_any_element(),
                });
            }
        }
        if let Some(item_index) = final_text
            && let Item::Text(text) = &turn.items[item_index]
        {
            let reply_group = SharedString::from(format!("reply-{tid}-{index}"));
            let copy = self
                .copy_button(SharedString::from(format!("copy-reply-{tid}-{index}")), text.clone(), cx)
                .when(!latest, |button| button.invisible().group_hover(reply_group.clone(), |style| style.visible()));
            agent = agent.child(
                div()
                    .group(reply_group)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .pt(px(4.))
                            .text_size(px(14.5))
                            .line_height(px(24.))
                            .text_color(p.text)
                            .child(TextView::markdown(SharedString::from(format!("md-{tid}-{index}-{item_index}")), text.clone())),
                    )
                    .child(copy),
            );
        }
        if working {
            let label = if pending {
                "Waiting on you"
            } else {
                match turn.items.last() {
                    None => "Warming up the model",
                    Some(Item::Reasoning(_)) => "Thinking",
                    Some(Item::Text(_)) => "Writing",
                    Some(Item::Tool(tool)) if tool.status == ToolStatus::Running => "Using a tool",
                    Some(Item::Tool(_)) => "Thinking",
                }
            };
            agent = agent.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(6.))
                    .text_size(px(13.5))
                    .text_color(p.text_faint)
                    .child(div().size(px(20.)).child(self.status.clone()))
                    .child(label),
            );
        }
        match &turn.state {
            TurnState::Stopped => {
                agent = agent.child(div().pt(px(4.)).text_size(px(13.)).text_color(p.text_faint).child("You stopped this reply"))
            }
            TurnState::Failed(error) => {
                agent = agent.child(
                    div()
                        .mt(px(4.))
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .px(px(12.))
                        .py(px(10.))
                        .rounded(px(10.))
                        .bg(p.danger.opacity(0.1))
                        .text_size(px(13.5))
                        .text_color(p.danger)
                        .child(icon(Icon::CircleAlert, 15., p.danger))
                        .child(div().flex_1().child(error.clone())),
                )
            }
            _ => {}
        }
        let changes = turn.changes();
        if !working && !changes.is_empty() {
            agent = agent.child(self.changes_card(tid, index, &changes, cx));
        }
        column = column.child(agent);
        column
    }
}

fn clip(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    if all.len() <= lines {
        return text.to_string();
    }
    format!("{}\n… {} more lines", all[..lines].join("\n"), all.len() - lines)
}

impl Render for ThreadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.glide(window);
        if !self.smooth.moving() && self.near_bottom() {
            self.scroll.scroll_to_bottom();
        }
        let workspace = self.workspace.read(cx);
        let Some(thread) = workspace.active_thread().cloned() else { return div().size_full() };
        let pending = workspace.pending(thread.id).is_some();
        let state = if pending {
            State::Idle
        } else {
            match thread.turns.last().and_then(|t| t.items.last()) {
                Some(Item::Text(_)) => State::Generating,
                _ => State::Thinking,
            }
        };
        self.status.update(cx, |mark, cx| mark.set_state(state, cx));
        let last = thread.turns.len().saturating_sub(1);
        let turns: Vec<Div> = thread
            .turns
            .iter()
            .enumerate()
            .map(|(index, turn)| self.turn_view(&thread, index, turn, pending && index == last, cx))
            .collect();
        div().size_full().child(
            div()
                .id("thread-scroll")
                .size_full()
                .overflow_y_hidden()
                .track_scroll(&self.scroll)
                .on_scroll_wheel(cx.listener(Self::wheel))
                .child(div().max_w(px(820.)).mx_auto().px(px(28.)).pt(px(20.)).pb(px(40.)).flex().flex_col().gap(px(32.)).children(turns)),
        )
    }
}