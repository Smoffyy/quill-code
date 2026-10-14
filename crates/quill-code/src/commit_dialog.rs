use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::{
    Context, Div, Entity, EventEmitter, FontWeight, IntoElement, MouseButton, ParentElement, Render, SharedString, Stateful, Styled,
    Window, div, prelude::*, px,
};
use quill_llama::{ChatRequest, Event, Message, Server};

use crate::git::{self, FileStatus};
use crate::github::{self, Visibility};
use crate::icons::Icon;
use crate::palette::{Palette, palette};
use crate::project;
use crate::thread_view::file_icon;
use crate::ui::{MONO, icon};

pub enum CommitEvent {
    Close,
    Updated(String),
}

#[derive(Clone, PartialEq)]
enum Stage {
    Ready,
    Working(&'static str),
    Committed { sha: String, branch: String },
    Pushed { branch: String },
    Published { url: String },
}

pub struct CommitDialog {
    root: PathBuf,
    branch: Option<String>,
    default_branch: String,
    remote: Option<String>,
    has_commits: bool,
    files: Vec<FileStatus>,
    selected: HashSet<String>,
    message: Entity<TextareaState>,
    new_branch: Option<Entity<InputState>>,
    stage: Stage,
    error: Option<String>,
    server_url: String,
    model: Option<String>,
}

impl EventEmitter<CommitEvent> for CommitDialog {}

impl CommitDialog {
    pub fn new(root: PathBuf, server_url: String, model: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Describe the change, or leave this empty and the model will write it")
                .auto_grow(3, 8)
        });
        message.update(cx, |message, cx| message.focus(window, cx));
        let (files, error) = match git::status(&root) {
            Ok(files) => (files, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        let selected = files.iter().map(|f| f.path.clone()).collect();
        CommitDialog {
            branch: git::branch(&root),
            default_branch: git::default_branch(&root),
            remote: git::remote_url(&root),
            has_commits: git::head(&root).is_some(),
            root,
            files,
            selected,
            message,
            new_branch: None,
            stage: Stage::Ready,
            error,
            server_url,
            model,
        }
    }

    fn toggle_file(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.selected.remove(&path) {
            self.selected.insert(path);
        }
        cx.notify();
    }

    fn toggle_all(&mut self, cx: &mut Context<Self>) {
        if self.selected.len() == self.files.len() {
            self.selected.clear();
        } else {
            self.selected = self.files.iter().map(|f| f.path.clone()).collect();
        }
        cx.notify();
    }

    fn ask_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Branch name, for example fix-login"));
        input.update(cx, |input, cx| input.focus(window, cx));
        self.new_branch = Some(input);
        cx.notify();
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        let files: Vec<String> = self.files.iter().filter(|f| self.selected.contains(&f.path)).map(|f| f.path.clone()).collect();
        if files.is_empty() {
            self.error = Some("Pick at least one file to commit.".into());
            cx.notify();
            return;
        }
        let new_branch = self.new_branch.as_ref().map(|input| input.read(cx).value().trim().to_string());
        if new_branch.as_ref().is_some_and(|b| !git::valid_branch(b)) {
            self.error = Some("Give the new branch a name Git accepts, such as fix-login.".into());
            cx.notify();
            return;
        }
        if new_branch.is_none() && self.branch.is_none() {
            self.error = Some("This folder is not on a branch yet. Commit to a new branch instead.".into());
            cx.notify();
            return;
        }
        let message = self.message.read(cx).value().trim().to_string();
        let (root, url, model) = (self.root.clone(), self.server_url.clone(), self.model.clone());
        self.error = None;
        self.stage = Stage::Working(if message.is_empty() { "Writing a commit message" } else { "Committing" });
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            if let Some(name) = &new_branch {
                git::create_branch(&root, name)?;
            }
            git::stage(&root, &files)?;
            let message = if message.is_empty() { write_message(&root, &url, model.as_deref())? } else { message };
            let sha = git::commit(&root, &files, &message)?;
            Ok::<_, String>((sha, git::branch(&root).unwrap_or_default()))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            _ = this.update(cx, |this, cx| {
                match result {
                    Ok((sha, branch)) => {
                        cx.emit(CommitEvent::Updated(format!("Committed {sha} to {branch}")));
                        this.has_commits = true;
                        this.branch = Some(branch.clone());
                        this.stage = Stage::Committed { sha, branch };
                    }
                    Err(error) => {
                        this.error = Some(error);
                        this.stage = Stage::Ready;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn push(&mut self, open_review: bool, cx: &mut Context<Self>) {
        let Some(branch) = self.branch.clone() else { return };
        let root = self.root.clone();
        let review = self.review_url(&branch);
        self.stage = Stage::Working("Pushing");
        self.error = None;
        cx.notify();
        let pushing = branch.clone();
        let task = cx.background_executor().spawn(async move { git::push(&root, &pushing) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        if open_review && let Some(url) = &review {
                            cx.open_url(url);
                        }
                        this.stage = Stage::Pushed { branch };
                    }
                    Err(error) => {
                        this.error = Some(error);
                        this.stage = Stage::Committed { sha: git::head(&this.root).unwrap_or_default(), branch };
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn publish(&mut self, visibility: Visibility, cx: &mut Context<Self>) {
        let Some(branch) = self.branch.clone().filter(|_| !matches!(self.stage, Stage::Working(_))) else { return };
        let root = self.root.clone();
        let fallback = self.stage.clone();
        self.stage = Stage::Working("Publishing to GitHub");
        self.error = None;
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            let repo = github::create_repo(&project::clean_name(&git::repo_name(&root)), visibility)?;
            git::add_remote(&root, &repo.clone_url)?;
            git::push(&root, &branch)?;
            Ok::<_, String>(repo.html_url)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            _ = this.update(cx, |this, cx| {
                this.remote = git::remote_url(&this.root);
                match result {
                    Ok(url) => {
                        cx.emit(CommitEvent::Updated(format!("Published to {url}")));
                        this.stage = Stage::Published { url };
                    }
                    Err(error) => {
                        this.error = Some(error);
                        this.stage = fallback;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn can_publish(&self) -> bool {
        self.remote.is_none() && self.has_commits && self.branch.is_some()
    }

    fn review_url(&self, branch: &str) -> Option<String> {
        if branch == self.default_branch {
            return None;
        }
        self.remote.as_deref().and_then(|remote| git::review_url(remote, &self.default_branch, branch))
    }

    fn file_rows(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        div().flex().flex_col().children(self.files.iter().map(|file| {
            let path = file.path.clone();
            let on = self.selected.contains(&file.path);
            div()
                .id(SharedString::from(format!("commit-file-{}", file.path)))
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(6.))
                .cursor_pointer()
                .hover(move |style| style.bg(p.row_hover))
                .child(check(on, p))
                .child(icon(file_icon(&file.path), 14., p.text_muted))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .font_family(MONO)
                        .text_size(px(12.5))
                        .text_color(if on { p.text } else { p.text_faint })
                        .child(file.path.clone()),
                )
                .child(div().text_size(px(11.5)).text_color(p.text_faint).child(file.label()))
                .child(div().w(px(40.)).flex().justify_end().text_size(px(12.)).text_color(p.add).child(format!("+{}", file.added)))
                .child(div().w(px(40.)).flex().justify_end().text_size(px(12.)).text_color(p.del).child(format!("-{}", file.removed)))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_file(path.clone(), cx)))
        }))
    }
}

fn write_message(root: &std::path::Path, url: &str, model: Option<&str>) -> Result<String, String> {
    let model = model.ok_or("Choose a model first, or type a commit message yourself.")?;
    let request = ChatRequest {
        model: model.to_string(),
        messages: vec![
            Message::system(
                "You write git commit messages. Reply with the message only: one summary line in the imperative mood under 72 characters, then, only if it helps, a blank line and a few short bullet points.",
            ),
            Message::user(git::staged_diff(root, 12_000)),
        ],
        thinking: Some(false),
        tools: Vec::new(),
    };
    let mut text = String::new();
    Server::new(url)
        .stream_chat(&request, &AtomicBool::new(false), |event| {
            if let Event::Content(chunk) = event {
                text.push_str(&chunk);
            }
        })
        .map_err(|e| e.to_string())?;
    let text = match text.rfind("</think>") {
        Some(end) => text[end + 8..].to_string(),
        None => text,
    };
    let text = text.trim().trim_matches('`').trim().to_string();
    if text.is_empty() { Err("The model returned an empty message. Type one yourself.".into()) } else { Ok(text) }
}

fn check(on: bool, p: Palette) -> Div {
    div()
        .size(px(16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(if on { p.text } else { p.border })
        .when(on, |b| b.bg(p.text).child(icon(Icon::Check, 12., p.bg)))
}

fn button(id: &'static str, label: impl Into<SharedString>, primary: bool, p: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(32.))
        .px(px(14.))
        .rounded(px(8.))
        .text_size(px(13.5))
        .cursor_pointer()
        .map(|b| {
            if primary {
                b.bg(p.text).text_color(p.bg).font_weight(FontWeight::SEMIBOLD).hover(|s| s.opacity(0.9))
            } else {
                b.border_1().border_color(p.border).text_color(p.text).hover(move |s| s.bg(p.row_hover))
            }
        })
        .child(label.into())
}

impl Render for CommitDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let viewport = window.viewport_size();
        let working = matches!(self.stage, Stage::Working(_));
        let mut body = div().flex().flex_col().gap(px(14.));
        match self.stage.clone() {
            Stage::Ready | Stage::Working(_) => {
                let on_default = self.remote.is_some() && self.branch.as_deref() == Some(self.default_branch.as_str());
                let all = !self.files.is_empty() && self.selected.len() == self.files.len();
                body = body
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(13.))
                            .text_color(p.text_muted)
                            .child(icon(Icon::Branch, 14., p.text_muted))
                            .child(match &self.branch {
                                Some(branch) => format!("On {branch}"),
                                None => "Not on a branch yet".into(),
                            })
                            .when(on_default, |row| {
                                row.child(
                                    div()
                                        .ml(px(4.))
                                        .px(px(8.))
                                        .py(px(1.))
                                        .rounded(px(6.))
                                        .bg(p.warning.opacity(0.12))
                                        .text_size(px(12.))
                                        .text_color(p.warning)
                                        .child("Your main branch. A new branch keeps it clean for a pull request."),
                                )
                            }),
                    )
                    .child(
                        div()
                            .rounded(px(12.))
                            .border_1()
                            .border_color(p.border_soft)
                            .overflow_hidden()
                            .child(
                                div()
                                    .id("toggle-all")
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .px(px(12.))
                                    .py(px(8.))
                                    .bg(p.fill)
                                    .cursor_pointer()
                                    .text_size(px(12.5))
                                    .text_color(p.text_muted)
                                    .child(check(all, p))
                                    .child(format!("{} of {} files", self.selected.len(), self.files.len()))
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle_all(cx))),
                            )
                            .child(div().id("commit-files").max_h(px(220.)).overflow_y_scroll().child(self.file_rows(p, cx)))
                            .when(self.files.is_empty(), |list| {
                                list.child(
                                    div()
                                        .p(px(16.))
                                        .text_size(px(13.))
                                        .text_color(p.text_faint)
                                        .child("Nothing to commit. The working tree is clean."),
                                )
                            }),
                    )
                    .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child("Message"))
                    .child(div().text_size(px(13.5)).child(Textarea::new(&self.message)))
                    .when_some(self.new_branch.clone(), |col, input| {
                        col.child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child("New branch"))
                            .child(div().font_family(MONO).text_size(px(13.)).child(Input::new(&input)))
                    });
            }
            Stage::Committed { sha, branch } => {
                body = body.child(done_note(format!("Committed {sha} to {branch}."), p)).when(self.remote.is_none(), |b| {
                    b.child(div().text_size(px(13.)).text_color(p.text_faint).child(format!(
                        "Not on GitHub yet. Publishing creates a repository named {} and pushes {branch} to it.",
                        project::clean_name(&git::repo_name(&self.root))
                    )))
                });
            }
            Stage::Pushed { branch } => {
                body = body.child(done_note(format!("Pushed {branch} to origin."), p));
            }
            Stage::Published { url } => {
                body = body.child(done_note(format!("Published to {url}"), p));
            }
        }
        let footer = {
            let mut row = div().flex().items_center().justify_end().gap(px(8.));
            if let Stage::Working(what) = self.stage {
                row = row.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(13.))
                        .text_color(p.text_faint)
                        .child(icon(Icon::Loader, 14., p.text_faint))
                        .child(what),
                );
            }
            match self.stage.clone() {
                Stage::Ready | Stage::Working(_) => {
                    let detached = self.branch.is_none();
                    row = row.child(
                        button("commit-cancel", "Cancel", false, p).on_click(cx.listener(|_, _, _, cx| cx.emit(CommitEvent::Close))),
                    );
                    row = if self.files.is_empty() && self.can_publish() {
                        publish_buttons(row, p, cx)
                    } else if self.new_branch.is_some() {
                        row.child(button("commit-confirm", "Create branch and commit", true, p).on_click(cx.listener(
                            move |this, _, _, cx| {
                                if !working {
                                    this.commit(cx)
                                }
                            },
                        )))
                    } else {
                        row.child(button("commit-branch", "Commit to a new branch", detached, p).on_click(cx.listener(
                            move |this, _, window, cx| {
                                if !working {
                                    this.ask_branch(window, cx)
                                }
                            },
                        )))
                        .when(!detached, |row| {
                            row.child(button("commit-confirm", "Commit", true, p).on_click(cx.listener(move |this, _, _, cx| {
                                if !working {
                                    this.commit(cx)
                                }
                            })))
                        })
                    };
                }
                Stage::Committed { branch, .. } => {
                    row =
                        row.child(button("commit-done", "Done", false, p).on_click(cx.listener(|_, _, _, cx| cx.emit(CommitEvent::Close))));
                    if self.can_publish() {
                        row = publish_buttons(row, p, cx);
                    } else if self.remote.is_some() {
                        row = row.child(
                            button("commit-push", "Push", self.review_url(&branch).is_none(), p)
                                .on_click(cx.listener(|this, _, _, cx| this.push(false, cx))),
                        );
                        if self.review_url(&branch).is_some() {
                            row = row.child(
                                button("commit-review", "Push and open a pull request", true, p)
                                    .on_click(cx.listener(|this, _, _, cx| this.push(true, cx))),
                            );
                        }
                    }
                }
                Stage::Pushed { .. } => {
                    row =
                        row.child(button("commit-done", "Done", true, p).on_click(cx.listener(|_, _, _, cx| cx.emit(CommitEvent::Close))));
                }
                Stage::Published { url } => {
                    row = row
                        .child(button("commit-done", "Done", false, p).on_click(cx.listener(|_, _, _, cx| cx.emit(CommitEvent::Close))))
                        .child(button("commit-open", "Open on GitHub", true, p).on_click(move |_, _, cx| cx.open_url(&url)));
                }
            }
            row
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .w(viewport.width)
            .h(viewport.height)
            .occlude()
            .bg(gpui_kit::black().opacity(0.55))
            .flex()
            .items_start()
            .justify_center()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(CommitEvent::Close)))
            .child(
                div()
                    .id("commit-dialog")
                    .mt(px(80.))
                    .w(px(640.))
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .p(px(22.))
                    .rounded(px(16.))
                    .bg(p.menu_bg)
                    .border_1()
                    .border_color(p.border)
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(icon(Icon::Commit, 18., p.text))
                            .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child("Commit your work")),
                    )
                    .child(body)
                    .when_some(self.error.clone(), |col, error| {
                        col.child(
                            div()
                                .px(px(12.))
                                .py(px(8.))
                                .rounded(px(8.))
                                .bg(p.danger.opacity(0.1))
                                .text_size(px(13.))
                                .text_color(p.danger)
                                .child(error),
                        )
                    })
                    .child(footer),
            )
    }
}

fn publish_buttons(row: Div, p: Palette, cx: &mut Context<CommitDialog>) -> Div {
    row.child(
        button("publish-public", "Publish as public", false, p)
            .on_click(cx.listener(|this, _, _, cx| this.publish(Visibility::Public, cx))),
    )
    .child(
        button("publish-private", "Publish as private", true, p)
            .on_click(cx.listener(|this, _, _, cx| this.publish(Visibility::Private, cx))),
    )
}

fn done_note(text: String, p: Palette) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(10.))
        .bg(p.success.opacity(0.1))
        .text_size(px(14.))
        .text_color(p.text)
        .child(icon(Icon::CheckCircle, 16., p.success))
        .child(text)
}