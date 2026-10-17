use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use quill_git::github::{self, Visibility};
use quill_git::{self as git, FileStatus};

use crate::Msg;
use crate::backend::Writer;
use crate::composer::Composer;
use crate::picker::{Choice, Picked, Picker};
use crate::project;
use crate::text::{Line, accent, bad, bold, dim, good, pad, plain, truncate, warn};

const COMMIT_SYSTEM: &str = "You write git commit messages. Reply with the message only: one summary line in the imperative mood under 72 characters, then, only if it helps, a blank line and a few short bullet points.";

pub enum Job {
    Committed(Result<(String, String), String>),
    Pushed(Result<(), String>),
    Published(Result<String, String>),
}

#[derive(Clone, Copy)]
enum Target {
    Here,
    NewBranch,
}

#[derive(Clone, Copy)]
enum After {
    Push,
    Review,
    Publish(Visibility),
    Done,
}

enum Step {
    Files(usize),
    Target(Picker<Target>),
    Branch(Composer),
    Message(Composer),
    Working(&'static str),
    After(Picker<After>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Update {
    Stay,
    Close,
}

pub struct Flow {
    root: PathBuf,
    branch: Option<String>,
    default_branch: String,
    remote: Option<String>,
    has_commits: bool,
    files: Vec<FileStatus>,
    chosen: Vec<bool>,
    new_branch: Option<String>,
    message: String,
    step: Step,
    error: Option<String>,
    writer: Result<Writer, String>,
    tx: Sender<Msg>,
}

fn write_message(root: &Path, writer: Result<Writer, String>) -> Result<String, String> {
    let text = writer?.write(COMMIT_SYSTEM, &git::staged_diff(root, 12_000))?;
    let text = text.trim().trim_matches('`').trim().to_string();
    if text.is_empty() { Err("The model returned an empty message. Type one yourself.".into()) } else { Ok(text) }
}

impl Flow {
    pub fn new(root: PathBuf, writer: Result<Writer, String>, tx: Sender<Msg>) -> Result<Flow, String> {
        let files = git::status(&root)?;
        let mut flow = Flow {
            branch: git::branch(&root),
            default_branch: git::default_branch(&root),
            remote: git::remote_url(&root),
            has_commits: git::head(&root).is_some(),
            chosen: vec![true; files.len()],
            files,
            new_branch: None,
            message: String::new(),
            step: Step::Files(0),
            error: None,
            writer,
            tx,
            root,
        };
        if flow.files.is_empty() {
            if !flow.can_publish() {
                return Err("Nothing to commit. The working tree is clean.".into());
            }
            flow.step = Step::After(flow.after_choices());
        }
        Ok(flow)
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

    fn after_choices(&self) -> Picker<After> {
        let mut choices = Vec::new();
        if self.can_publish() {
            let name = project::clean_name(&git::repo_name(&self.root));
            choices.push(Choice::new(
                "Publish as private",
                format!("Create {name} on GitHub and push"),
                After::Publish(Visibility::Private),
            ));
            choices.push(Choice::new("Publish as public", format!("Create {name} on GitHub and push"), After::Publish(Visibility::Public)));
        } else if self.remote.is_some() {
            choices.push(Choice::new("Push", "Send the branch to origin", After::Push));
            if self.branch.as_deref().and_then(|branch| self.review_url(branch)).is_some() {
                choices.push(Choice::new("Push and open a pull request", "Opens the review page in your browser", After::Review));
            }
        }
        choices.push(Choice::new("Done", "", After::Done));
        Picker::new("What next?", choices)
    }

    fn start(&mut self, label: &'static str, work: impl FnOnce() -> Job + Send + 'static) {
        self.error = None;
        self.step = Step::Working(label);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Job(work()));
        });
    }

    fn commit(&mut self, message: String) {
        let files: Vec<String> = self.files.iter().zip(&self.chosen).filter(|(_, on)| **on).map(|(file, _)| file.path.clone()).collect();
        let (root, writer, branch) = (self.root.clone(), self.writer.clone(), self.new_branch.clone());
        self.message = message.clone();
        let label = if message.is_empty() { "Writing a commit message" } else { "Committing" };
        self.start(label, move || {
            Job::Committed((|| {
                if let Some(name) = &branch {
                    git::create_branch(&root, name)?;
                }
                git::stage(&root, &files)?;
                let message = if message.is_empty() { write_message(&root, writer)? } else { message };
                let sha = git::commit(&root, &files, &message)?;
                Ok((sha, git::branch(&root).unwrap_or_default()))
            })())
        });
    }

    fn files_key(&mut self, key: KeyEvent, highlighted: usize) -> Update {
        let count = self.files.len();
        match key.code {
            KeyCode::Esc => return Update::Close,
            KeyCode::Up if count > 0 => self.step = Step::Files(highlighted.checked_sub(1).unwrap_or(count - 1)),
            KeyCode::Down if count > 0 => self.step = Step::Files((highlighted + 1) % count),
            KeyCode::Char(' ') if count > 0 => self.chosen[highlighted] = !self.chosen[highlighted],
            KeyCode::Char('a') => {
                let all = self.chosen.iter().all(|on| *on);
                self.chosen.iter_mut().for_each(|on| *on = !all);
            }
            KeyCode::Enter => {
                if !self.chosen.iter().any(|on| *on) {
                    self.error = Some("Pick at least one file to commit.".into());
                    return Update::Stay;
                }
                self.error = None;
                self.step = match &self.branch {
                    Some(branch) => Step::Target(Picker::new(
                        "Where should the commit go?",
                        vec![
                            Choice::new(format!("Commit to {branch}"), "", Target::Here),
                            Choice::new("Commit to a new branch", "Keeps your work apart, ready for a pull request", Target::NewBranch),
                        ],
                    )),
                    None => Step::Branch(Composer::default()),
                };
            }
            _ => {}
        }
        Update::Stay
    }

    pub fn key(&mut self, key: KeyEvent, out: &mut Vec<Line>) -> Update {
        let plain_enter = key.code == KeyCode::Enter && !key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
        match &mut self.step {
            Step::Files(highlighted) => {
                let highlighted = *highlighted;
                return self.files_key(key, highlighted);
            }
            Step::Target(picker) => match picker.key(key) {
                Picked::Closed => return Update::Close,
                Picked::Chosen(index) => {
                    self.step = match picker.choices[index].value {
                        Target::Here => Step::Message(Composer::default()),
                        Target::NewBranch => Step::Branch(Composer::default()),
                    };
                }
                Picked::Nothing => {}
            },
            Step::Branch(input) => {
                if key.code == KeyCode::Esc {
                    return Update::Close;
                }
                if plain_enter {
                    let name = input.text.trim().to_string();
                    if !git::valid_branch(&name) {
                        self.error = Some("Give the new branch a name Git accepts, such as fix-login.".into());
                        return Update::Stay;
                    }
                    self.error = None;
                    self.new_branch = Some(name);
                    self.step = Step::Message(Composer::default());
                } else {
                    input.key(key);
                }
            }
            Step::Message(input) => {
                if key.code == KeyCode::Esc {
                    return Update::Close;
                }
                if plain_enter {
                    let message = input.text.trim().to_string();
                    self.commit(message);
                } else {
                    input.key(key);
                }
            }
            Step::Working(_) => {}
            Step::After(picker) => match picker.key(key) {
                Picked::Closed => return Update::Close,
                Picked::Chosen(index) => {
                    let choice = picker.choices[index].value;
                    return self.after(choice, out);
                }
                Picked::Nothing => {}
            },
        }
        Update::Stay
    }

    fn after(&mut self, choice: After, out: &mut Vec<Line>) -> Update {
        let Some(branch) = self.branch.clone() else { return Update::Close };
        let root = self.root.clone();
        match choice {
            After::Done => return Update::Close,
            After::Push | After::Review => {
                let review = (matches!(choice, After::Review)).then(|| self.review_url(&branch)).flatten();
                if let Some(url) = &review {
                    out.push(Line::styled("  Open the pull request page: ", dim()).with(url, accent()));
                }
                self.start("Pushing", move || {
                    let pushed = git::push(&root, &branch);
                    if pushed.is_ok()
                        && let Some(url) = review
                    {
                        project::open_url(&url);
                    }
                    Job::Pushed(pushed)
                });
            }
            After::Publish(visibility) => {
                self.start("Publishing to GitHub", move || {
                    Job::Published((|| {
                        let repo = github::create_repo(&project::clean_name(&git::repo_name(&root)), visibility)?;
                        git::add_remote(&root, &repo.clone_url)?;
                        git::push(&root, &branch)?;
                        Ok(repo.html_url)
                    })())
                });
            }
        }
        Update::Stay
    }

    pub fn job(&mut self, job: Job, out: &mut Vec<Line>) -> Update {
        match job {
            Job::Committed(Ok((sha, branch))) => {
                out.push(Line::styled("• ", good()).with("Committed ", dim()).with(format!("{sha} to {branch}"), plain()));
                self.has_commits = true;
                self.branch = Some(branch);
                let next = self.after_choices();
                if next.choices.len() == 1 {
                    return Update::Close;
                }
                self.step = Step::After(next);
            }
            Job::Committed(Err(error)) => {
                self.error = Some(error);
                let mut input = Composer::default();
                input.set(&self.message);
                self.step = Step::Message(input);
            }
            Job::Pushed(Ok(())) => {
                out.push(
                    Line::styled("• ", good())
                        .with("Pushed ", dim())
                        .with(format!("{} to origin", self.branch.clone().unwrap_or_default()), plain()),
                );
                return Update::Close;
            }
            Job::Published(Ok(url)) => {
                out.push(Line::styled("• ", good()).with("Published to ", dim()).with(url, accent()));
                return Update::Close;
            }
            Job::Pushed(Err(error)) | Job::Published(Err(error)) => {
                self.error = Some(error);
                self.remote = git::remote_url(&self.root);
                self.step = Step::After(self.after_choices());
            }
        }
        Update::Stay
    }

    pub fn paste(&mut self, text: &str) {
        if let Step::Branch(input) | Step::Message(input) = &mut self.step {
            input.insert(&text.replace("\r\n", "\n"));
        }
    }

    pub fn working(&self) -> bool {
        matches!(self.step, Step::Working(_))
    }

    pub fn render(&self, width: usize, rows: usize, spinner: &str) -> (Vec<Line>, Option<(usize, usize)>) {
        let mut lines = Vec::new();
        let place = match &self.branch {
            Some(branch) => format!("on {branch}"),
            None => "not on a branch yet".into(),
        };
        lines.push(Line::styled("Commit your work", bold()).with(format!(" · {place}"), dim()));
        let on_default = self.remote.is_some() && self.branch.as_deref() == Some(self.default_branch.as_str());
        let mut cursor = None;
        match &self.step {
            Step::Files(highlighted) => {
                if on_default {
                    lines.push(Line::styled("  This is your main branch. A new branch keeps it clean for a pull request.", warn()));
                }
                let count = self.chosen.iter().filter(|on| **on).count();
                lines.push(Line::styled(format!("  {count} of {} files", self.files.len()), dim()));
                let rows = rows.saturating_sub(5).max(3);
                let start = highlighted.saturating_sub(rows / 2).min(self.files.len().saturating_sub(rows));
                let widest = self.files.iter().map(|file| file.path.chars().count()).max().unwrap_or(0).min(width / 2);
                for (index, file) in self.files.iter().enumerate().skip(start).take(rows) {
                    let mark = if index == *highlighted { Line::styled("▸ ", accent()) } else { Line::plain("  ") };
                    let check = if self.chosen[index] { Line::styled("[x] ", good()) } else { Line::styled("[ ] ", dim()) };
                    let mut line = mark;
                    line.append(check);
                    line.push(pad(&file.path, widest), if self.chosen[index] { plain() } else { dim() });
                    line.push(format!("  {}", file.label()), dim());
                    line.push(format!(" +{}", file.added), good());
                    line.push(format!(" -{}", file.removed), bad());
                    lines.push(truncate(&line, width));
                }
                lines.push(Line::styled("  space toggles · a toggles all · enter continues · esc cancels", dim()));
            }
            Step::Target(picker) => lines.extend(picker.render(width, rows)),
            Step::After(picker) => lines.extend(picker.render(width, rows)),
            Step::Branch(input) => {
                lines.push(Line::plain("  New branch name"));
                let (text, at) = input.render(width, "for example fix-login");
                cursor = Some((lines.len() + at.0, at.1));
                lines.extend(text);
                lines.push(Line::styled("  enter continues · esc cancels", dim()));
            }
            Step::Message(input) => {
                lines.push(Line::plain("  Message"));
                let (text, at) = input.render(width, "leave empty and the model writes it");
                cursor = Some((lines.len() + at.0, at.1));
                lines.extend(text);
                lines.push(Line::styled("  enter commits · shift+enter adds a line · esc cancels", dim()));
            }
            Step::Working(label) => lines.push(Line::styled(format!("{spinner} {label}"), accent())),
        }
        if let Some(error) = &self.error {
            lines.push(truncate(&Line::styled(format!("  {error}"), bad()), width));
        }
        (lines, cursor)
    }
}