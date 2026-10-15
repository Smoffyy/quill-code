use std::path::Path;

use serde_json::{Value, json};

use super::args::{self, BODY, BRANCH, MESSAGE, TITLE};
use super::{Ctx, Output, Prepared, Spec, Tool, Toolset};
use crate::policy::Effect;
use crate::protocol::Category;

pub struct CommitChanges;

const COMMIT: Spec = Spec {
    name: "commit_changes",
    title: "Commit",
    summary: "Stages every change and commits it, optionally on a new branch.",
    aliases: &["commit", "git_commit", "create_commit"],
    toolset: Toolset::Git,
    category: Category::Git,
    active: "Committing",
    done: "Committed",
    parallel: false,
    description: "Stage every change in the project and commit it. Give a branch to commit on a new branch, which a pull request needs when you are on the main branch or not on a branch.",
    guide: "commit_changes stages every change and commits it, optionally on a new branch.",
};

fn commit(root: &Path, message: &str, branch: Option<&str>) -> Result<String, String> {
    if let Some(branch) = branch.filter(|b| quill_git::branch(root).as_deref() != Some(*b)) {
        if !quill_git::valid_branch(branch) {
            return Err(format!("\"{branch}\" is not a branch name Git accepts."));
        }
        quill_git::create_branch(root, branch).or_else(|_| quill_git::switch_branch(root, branch))?;
    }
    let sha = quill_git::commit_all(root, message)?;
    let on = quill_git::branch(root).unwrap_or_else(|| "a detached HEAD".into());
    Ok(format!("Committed {sha} on {on}."))
}

impl Tool for CommitChanges {
    fn spec(&self) -> &'static Spec {
        &COMMIT
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": { "type": "string", "description": "Commit message: a short summary line, optionally followed by a blank line and details." },
                "branch": { "type": "string", "description": "Optional branch to create or switch to before committing, for example fix-login." }
            },
            "required": ["message"]
        })
    }

    fn target(&self, args: &Value) -> String {
        args::text(args, &MESSAGE).unwrap_or_default().lines().next().unwrap_or_default().trim().to_string()
    }

    fn prepare(&self, args: &Value, ctx: &Ctx<'_>) -> Result<Prepared, String> {
        let message = args::trimmed(args, &MESSAGE).ok_or("commit_changes needs a \"message\".")?.to_string();
        let branch = args::trimmed(args, &BRANCH).map(str::to_string);
        let subject = match &branch {
            Some(branch) => format!("{} (on {branch})", self.target(args)),
            None => self.target(args),
        };
        let root = ctx.root.to_path_buf();
        Ok(Prepared::new(
            Effect::Publish,
            subject,
            "Wants to commit",
            Box::new(move |_: &Ctx<'_>| Output::settle(commit(&root, &message, branch.as_deref()))),
        ))
    }
}

pub struct OpenPullRequest;

const PULL: Spec = Spec {
    name: "open_pull_request",
    title: "Open pull requests",
    summary: "Pushes the branch to GitHub and opens a pull request.",
    aliases: &["pull_request", "create_pull_request", "open_pr", "create_pr"],
    toolset: Toolset::Git,
    category: Category::Git,
    active: "Opening a pull request",
    done: "Opened a pull request",
    parallel: false,
    description: "Push the current branch to GitHub and open a pull request into the main branch. Commit your work with commit_changes first.",
    guide: "open_pull_request pushes the current branch to GitHub and opens a pull request into the main branch.",
};

fn open(root: &Path, title: &str, body: &str) -> Result<String, String> {
    let branch = quill_git::branch(root).ok_or("Not on a branch. Commit to a new branch with commit_changes first.")?;
    let base = quill_git::default_branch(root);
    if branch == base {
        return Err(format!("You are on {base}, the main branch. Commit the work to a new branch with commit_changes first."));
    }
    let remote =
        quill_git::remote_url(root).ok_or("This repository has no remote called origin, so there is nowhere to open a pull request.")?;
    let slug = quill_git::github_slug(&remote).ok_or("Pull requests can only be opened for repositories on GitHub.")?;
    quill_git::push(root, &branch)?;
    let url = quill_git::github::create_pull_request(&slug, &branch, &base, title, body)?;
    Ok(format!("Pushed {branch} and opened a pull request into {base}: {url}"))
}

impl Tool for OpenPullRequest {
    fn spec(&self) -> &'static Spec {
        &PULL
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "Pull request title." },
                "body": { "type": "string", "description": "What changed and why, in Markdown." }
            },
            "required": ["title"]
        })
    }

    fn target(&self, args: &Value) -> String {
        args::text(args, &TITLE).unwrap_or_default().trim().to_string()
    }

    fn prepare(&self, args: &Value, ctx: &Ctx<'_>) -> Result<Prepared, String> {
        let title = args::trimmed(args, &TITLE).ok_or("open_pull_request needs a \"title\".")?.to_string();
        let body = args::text(args, &BODY).unwrap_or_default().trim().to_string();
        let root = ctx.root.to_path_buf();
        Ok(Prepared::new(
            Effect::Publish,
            title.clone(),
            "Wants to open a pull request",
            Box::new(move |_: &Ctx<'_>| Output::settle(open(&root, &title, &body))),
        ))
    }
}