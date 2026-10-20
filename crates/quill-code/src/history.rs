use crossterm::style::{Color, Stylize};
use quill_config::Reasoning;
use quill_core::{Category, FileDiff, LineKind, PlanStep, StepStatus};

use crate::markdown;
use crate::text::{Line, accent, bad, bold, clean, dim, fill, good, indent, plain, shade, truncate, warn, wrap};
use crate::thread::{Item, ToolItem, ToolStatus, Turn, TurnState, now_ms};
use crate::usage::count;

const DIFF_LINES: usize = 12;
const OUTPUT_LINES: usize = 3;
const FAILED_LINES: usize = 8;
const LIVE_OUTPUT: usize = 5;
const LIVE_TEXT: usize = 14;
const LIVE_TOOLS: usize = 4;
const HOLD_MS: u64 = 3000;

#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub width: usize,
    pub reasoning: Reasoning,
    pub group: bool,
}

impl Look {
    fn inner(&self, used: usize) -> usize {
        self.width.saturating_sub(used).max(10)
    }
}

fn block(out: &mut Vec<Line>, lines: Vec<Line>) {
    if !lines.is_empty() {
        out.push(Line::default());
        out.extend(lines);
    }
}

fn hidden_tool(tool: &ToolItem) -> bool {
    matches!(tool.category, Category::Plan | Category::Question)
}

fn blank(text: &str) -> bool {
    text.trim().is_empty()
}

pub fn plan(turn: &Turn) -> Option<(&[PlanStep], Option<&str>)> {
    turn.items.iter().rev().find_map(|item| match item {
        Item::Plan { steps, explanation } => Some((steps.as_slice(), explanation.as_deref())),
        _ => None,
    })
}

fn explored(tools: &[&ToolItem], working: bool) -> Line {
    let mut kinds: Vec<(&str, usize)> = Vec::new();
    for tool in tools {
        let kind = match tool.tool.as_str() {
            "read_file" => "read",
            "grep" | "find_files" => "search",
            "list_dir" => "list",
            _ if tool.category == Category::Command => "run",
            _ => "lookup",
        };
        match kinds.iter_mut().find(|(name, _)| *name == kind) {
            Some((_, count)) => *count += 1,
            None => kinds.push((kind, 1)),
        }
    }
    let mut line = Line::default();
    for (index, (kind, count)) in kinds.iter().enumerate() {
        let (done, active, word) = match *kind {
            "read" => ("read", "reading", "file"),
            "search" => ("searched for", "searching for", "pattern"),
            "list" => ("listed", "listing", "folder"),
            "run" => ("ran", "running", "shell command"),
            _ => ("looked up", "looking up", "lookup"),
        };
        let mut verb = if working { active } else { done }.to_string();
        if index == 0 {
            verb[..1].make_ascii_uppercase();
        } else {
            verb.insert_str(0, ", ");
        }
        counted(&mut line, &verb, *count, word);
    }
    line
}

fn counted(line: &mut Line, lead: &str, count: usize, word: &str) {
    line.push(format!("{lead} "), dim());
    line.push(count.to_string(), bold());
    line.push(if count == 1 { format!(" {word}") } else { format!(" {word}s") }, dim());
}

fn seconds_label(seconds: f32) -> String {
    if seconds < 10.0 { format!("{seconds:.1}s") } else { duration(seconds.round() as u64) }
}

pub fn duration(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
    }
}

fn command_body(output: &str) -> &str {
    match output.split_once('\n') {
        Some((first, rest)) if first.starts_with("Exit code") || first.starts_with("Stopped") => rest,
        None if output.starts_with("Exit code") => "",
        _ => output,
    }
}

fn telling(output: &str) -> String {
    let lines: Vec<&str> = output
        .lines()
        .filter(|line| {
            let line = line.trim();
            let position = line.starts_with("At line:") || line.starts_with("+ ...") || line.trim_start_matches(['+', ' ', '~']).is_empty();
            let record = line.starts_with("+ CategoryInfo") || line.starts_with("+ FullyQualifiedErrorId");
            !line.is_empty() && !line.starts_with("at ") && !position && !record
        })
        .collect();
    lines.join("\n")
}

fn target(tool: &ToolItem) -> String {
    match tool.target.trim() {
        "" => tool.tool.replace('_', " "),
        "." | "./" => "the project".into(),
        text => clean(text).split_whitespace().collect::<Vec<_>>().join(" "),
    }
}

fn tool_name(tool: &ToolItem) -> String {
    match tool.tool.as_str() {
        "read_file" => "Read".into(),
        "grep" => "Search".into(),
        "find_files" => "Find".into(),
        "list_dir" => "List".into(),
        "write_file" => "Write".into(),
        "edit_file" | "apply_patch" => "Update".into(),
        "shell" => "Shell".into(),
        "commit_changes" => "Commit".into(),
        "open_pull_request" => "Pull request".into(),
        other => {
            let mut name = other.replace('_', " ");
            if let Some(first) = name.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            name
        }
    }
}

fn bubble(text: &str, look: Look) -> Vec<Line> {
    let lines = wrap(&Line::styled(clean(text.trim()), shade(plain())), look.inner(4));
    let lines = indent(lines, Line::styled(" › ", shade(accent())), Line::styled("   ", shade(plain())));
    lines.into_iter().map(|line| fill(line, look.width, shade(plain()))).collect()
}

fn answer(lines: Vec<Line>, first: bool) -> Vec<Line> {
    let mark = if first { Line::plain("● ") } else { Line::plain("  ") };
    indent(lines, mark, Line::plain("  "))
}

pub fn prompt_lines(turn: &Turn, look: Look) -> Vec<Line> {
    let mut lines = bubble(&turn.prompt, look);
    let images = turn.images.iter().map(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default());
    let names: Vec<String> = if turn.attachments.is_empty() { images.collect() } else { turn.attachments.clone() };
    for name in names {
        lines.push(fill(Line::styled(format!("   + {name}"), shade(dim())), look.width, shade(plain())));
    }
    lines
}

pub fn tool_rows(tool: &ToolItem, look: Look, spinner: &str) -> Vec<Line> {
    let mark = match tool.status {
        ToolStatus::Running => Line::styled(format!("{spinner} "), accent()),
        ToolStatus::Waiting => Line::styled("! ", warn()),
        ToolStatus::Failed => Line::styled("● ", bad()),
        ToolStatus::Done if tool.exit_code().is_some_and(|code| code != 0) => Line::styled("● ", bad()),
        ToolStatus::Done => Line::styled("● ", good()),
    };
    let mut name = Line::styled(tool_name(tool), bold());
    name.push(format!("({})", target(tool)), plain());
    let mut tail = Line::default();
    if tool.category == Category::Command
        && let Some(code) = tool.exit_code().filter(|code| *code != 0)
    {
        tail.push(format!(" · exit {code}"), bad());
    }
    match tool.status {
        ToolStatus::Running => {
            let seconds = now_ms().saturating_sub(tool.started) / 1000;
            if seconds > 0 {
                tail.push(format!(" · {}", duration(seconds)), dim());
            }
        }
        ToolStatus::Waiting => tail.push(" · needs your OK", warn()),
        _ => {
            if let Some(seconds) = tool.seconds().filter(|seconds| *seconds >= 1.0) {
                tail.push(format!(" · {}", seconds_label(seconds)), dim());
            }
        }
    }
    let mut lines = indent(wrap(&name, look.inner(2)), mark, Line::plain("  "));
    if let Some(last) = lines.last_mut() {
        last.append(tail);
    }
    lines.into_iter().map(|line| truncate(&line, look.width)).collect()
}

fn connect(lines: Vec<Line>, look: Look) -> Vec<Line> {
    lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let mut row = if index == 0 { Line::styled("  └ ", dim()) } else { Line::plain("    ") };
            row.append(line);
            truncate(&row, look.width)
        })
        .collect()
}

fn output_lines(text: &str, limit: usize, from_end: bool, style: crossterm::style::ContentStyle) -> Vec<Line> {
    let all: Vec<&str> = text.trim_end().lines().collect();
    if all.iter().all(|line| blank(line)) {
        return Vec::new();
    }
    let hidden = all.len().saturating_sub(limit);
    let mut lines = Vec::new();
    if from_end && hidden > 0 {
        lines.push(Line::styled(format!("… {hidden} earlier lines"), dim()));
    }
    let shown = if from_end { &all[hidden..] } else { &all[..all.len().min(limit)] };
    lines.extend(shown.iter().map(|line| Line::styled(*line, style)));
    if !from_end && hidden > 0 {
        lines.push(Line::styled(format!("… {hidden} more lines"), dim()));
    }
    lines
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 { format!("{count} {word}") } else { format!("{count} {word}s") }
}

fn changed(diffs: &[FileDiff]) -> Line {
    let added: usize = diffs.iter().map(|diff| diff.added).sum();
    let removed: usize = diffs.iter().map(|diff| diff.removed).sum();
    let mut line = Line::default();
    if let [diff] = diffs
        && diff.created
    {
        counted(&mut line, "Wrote", added, "line");
        line.push(format!(" to {}", diff.path), dim());
        return line;
    }
    if added > 0 {
        counted(&mut line, "Added", added, "line");
    }
    if removed > 0 {
        counted(&mut line, if added > 0 { ", removed" } else { "Removed" }, removed, "line");
    }
    if line.width() == 0 {
        line.push("No lines changed", dim());
    }
    if diffs.len() > 1 {
        line.push(format!(" in {} files", diffs.len()), dim());
    }
    line
}

pub fn diff_lines(diff: &FileDiff, limit: usize, look: Look) -> Vec<Line> {
    let mut lines = Vec::new();
    let room = look.width.saturating_sub(5);
    for line in diff.lines.iter().take(limit) {
        let (number, sign, style) = match line.kind {
            LineKind::Added => (line.new, "+", Some(Color::AnsiValue(22))),
            LineKind::Removed => (line.old, "-", Some(Color::AnsiValue(52))),
            LineKind::Context => (line.new, " ", None),
            LineKind::Gap => {
                lines.push(Line::styled("     ⋯", dim()));
                continue;
            }
            LineKind::Note => {
                lines.push(Line::styled(format!("          {}", line.text), dim()));
                continue;
            }
        };
        let number = number.map(|n| n.to_string()).unwrap_or_default();
        let row = match style {
            Some(color) => {
                let shaded = plain().with(Color::White).on(color);
                let row = Line::styled(format!("{number:>4} "), shaded.dim()).with(format!("{sign} {}", clean(&line.text)), shaded);
                fill(truncate(&row, room), room, shaded)
            }
            None => truncate(&Line::styled(format!("{number:>4} "), dim()).with(format!("  {}", clean(&line.text)), dim()), room),
        };
        lines.push(Line::plain("     ").with_line(row));
    }
    if diff.lines.len() > limit {
        lines.push(Line::styled(format!("     … {} more lines, /diff shows them all", diff.lines.len() - limit), dim()));
    }
    lines
}

fn explored_output(tool: &ToolItem, output: &str) -> String {
    let lines: Vec<&str> = output.trim_end().lines().collect();
    let more = lines.last().and_then(|line| line.strip_prefix("... and ")?.split(' ').next()?.parse::<usize>().ok());
    let found = lines.iter().filter(|line| **line != "--").count() - usize::from(more.is_some()) + more.unwrap_or(0);
    match tool.tool.as_str() {
        "read_file" if lines.first().is_some_and(|line| line.ends_with(" is empty.")) => "Empty folder".into(),
        "read_file" if lines.first().is_some_and(|line| line.ends_with(" contains:")) => {
            format!("Listed {}", plural(found - 1, "item"))
        }
        "read_file" => format!(
            "Read {}",
            plural(lines.iter().filter(|line| line.trim_start().starts_with(|c: char| c.is_ascii_digit())).count(), "line")
        ),
        "find_files" if output.starts_with("No files match") => "No files found".into(),
        "find_files" => format!("Found {}", plural(found, "file")),
        _ if output.starts_with("No matches") => "No matches".into(),
        _ => format!("Found {}", plural(found, "line")),
    }
}

pub fn tool_body(tool: &ToolItem, look: Look) -> Vec<Line> {
    if !tool.diffs.is_empty() {
        let mut lines = connect(vec![changed(&tool.diffs)], look);
        for diff in &tool.diffs {
            if tool.diffs.len() > 1 {
                lines.push(Line::styled(format!("     {}", diff.path), dim()));
            }
            lines.extend(diff_lines(diff, DIFF_LINES, look));
        }
        return lines;
    }
    let output = clean(&tool.output);
    let failed = tool.status == ToolStatus::Failed;
    match tool.category {
        Category::Command => {
            let failed = failed || tool.exit_code().is_some_and(|code| code != 0);
            let body = command_body(&output);
            let lines = if failed {
                output_lines(&telling(body), FAILED_LINES, true, plain())
            } else {
                output_lines(body, OUTPUT_LINES, true, dim())
            };
            connect(lines, look)
        }
        _ if failed => connect(output_lines(&output, 3, false, bad()), look),
        Category::Explore => connect(vec![Line::styled(explored_output(tool, &output), dim())], look),
        Category::Edit => Vec::new(),
        _ => connect(output_lines(&output, 2, false, dim()), look),
    }
}

pub fn running_command(tool: &ToolItem, look: Look) -> Vec<Line> {
    let output = clean(&tool.output);
    let mut lines = output_lines(command_body(&output), LIVE_OUTPUT, true, dim());
    if lines.is_empty() {
        lines.push(Line::styled("Running…", dim()));
    }
    connect(lines, look)
}

fn grouped(tool: &ToolItem) -> bool {
    match tool.category {
        Category::Explore => tool.status != ToolStatus::Failed,
        Category::Command => tool.status != ToolStatus::Failed && tool.exit_code().is_none_or(|code| code == 0),
        _ => false,
    }
}

fn group_span(items: &[Item], start: usize, look: Look) -> (usize, usize) {
    let mut end = start;
    for (index, item) in items.iter().enumerate().skip(start) {
        match item {
            Item::Tool(tool) if hidden_tool(tool) => {}
            Item::Tool(tool) if grouped(tool) => end = index + 1,
            Item::Reasoning(_) if look.reasoning == Reasoning::Hidden => {}
            Item::Text(text) | Item::Reasoning(text) if blank(text) => {}
            _ => return (end, index),
        }
    }
    (end, items.len())
}

fn group_tools(items: &[Item]) -> Vec<&ToolItem> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Tool(tool) if grouped(tool) => Some(tool),
            _ => None,
        })
        .collect()
}

fn held(tools: &[&ToolItem]) -> bool {
    tools.iter().filter_map(|tool| tool.ended).max().is_some_and(|ended| now_ms() < ended + HOLD_MS)
}

fn group_head(tools: &[&ToolItem], look: Look, spinner: &str) -> Line {
    let working = tools.iter().any(|tool| tool.working());
    let mut head = if working { Line::styled(format!("{spinner} "), accent()) } else { Line::plain("  ") };
    head.append(explored(tools, working));
    truncate(&head, look.width)
}

pub fn group_rows(tools: &[&ToolItem], look: Look, spinner: &str) -> Vec<Line> {
    let mut lines = vec![group_head(tools, look, spinner)];
    for tool in &tools[tools.len().saturating_sub(LIVE_TOOLS)..] {
        lines.extend(tool_rows(tool, look, spinner));
        if tool.status == ToolStatus::Running && tool.category == Category::Command {
            lines.extend(running_command(tool, look));
        }
    }
    lines
}

fn unmarked(line: &str) -> String {
    let line = line.replace(['*', '`', '#'], "");
    let line = line.trim().trim_start_matches(['-', '•']).trim_start();
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let line = match line[digits..].strip_prefix(['.', ')']) {
        Some(rest) if digits > 0 => rest,
        _ => line,
    };
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sentences(text: &str) -> (Vec<String>, String) {
    let text = clean(text);
    let mut done = Vec::new();
    let mut rest = String::new();
    let lines: Vec<&str> = text.split('\n').collect();
    for (index, line) in lines.iter().enumerate() {
        let line = unmarked(line);
        let mut start = 0;
        for (at, c) in line.char_indices() {
            if matches!(c, '.' | '!' | '?' | ':') && line[at + 1..].starts_with(' ') {
                done.push(line[start..=at].trim().to_string());
                start = at + 1;
            }
        }
        let tail = line[start..].trim().to_string();
        if index + 1 < lines.len() {
            done.extend(Some(tail).filter(|tail| !tail.is_empty()));
        } else {
            rest = tail;
        }
    }
    (done, rest)
}

pub fn summary(text: &str) -> Option<String> {
    sentences(text).0.pop()
}

fn last_sentence(text: &str) -> String {
    let (mut done, rest) = sentences(text);
    if !rest.is_empty() {
        done.push(rest);
    }
    done.pop().unwrap_or_default()
}

fn reasoning_lines(text: &str, look: Look) -> Vec<Line> {
    let text = clean(text.trim());
    if blank(&text) {
        return Vec::new();
    }
    match look.reasoning {
        Reasoning::Hidden => Vec::new(),
        Reasoning::Collapsed => {
            vec![truncate(&Line::styled("◇ Thought ", dim()).with(last_sentence(&text), dim().italic()), look.width)]
        }
        Reasoning::Expanded => {
            let mut lines = vec![Line::styled("◇ Thought", dim())];
            lines.extend(indent(wrap(&Line::styled(text, dim().italic()), look.inner(2)), Line::plain("  "), Line::plain("  ")));
            lines
        }
    }
}

fn thinking_lines(text: &str, look: Look, thought: Option<&str>) -> Vec<Line> {
    let mut lines = Vec::new();
    match look.reasoning {
        Reasoning::Hidden => {}
        Reasoning::Collapsed => {
            lines.push(Line::styled("◇ Thinking", dim()));
            if let Some(thought) = thought {
                let wrapped = wrap(&Line::styled(thought, dim().italic()), look.inner(2));
                lines.extend(indent(wrapped.into_iter().take(2).collect(), Line::plain("  "), Line::plain("  ")));
            }
        }
        Reasoning::Expanded => {
            let text = clean(text.trim());
            let tail: Vec<&str> = text.lines().filter(|line| !blank(line)).collect();
            lines.push(Line::styled("◇ Thinking", dim()));
            for line in &tail[tail.len().saturating_sub(6)..] {
                lines.push(truncate(&Line::styled(format!("  {}", line.trim()), dim().italic()), look.width));
            }
        }
    }
    lines
}

pub fn plan_lines(steps: &[PlanStep], explanation: Option<&str>, look: Look) -> Vec<Line> {
    let done = steps.iter().filter(|step| step.status == StepStatus::Done).count();
    let mut lines = vec![Line::styled("Plan", bold()).with(format!(" · {done} of {} done", steps.len()), dim())];
    for step in steps {
        let (mark, mark_style, style) = match step.status {
            StepStatus::Done => ("✓ ", good(), dim()),
            StepStatus::Active => ("› ", accent(), bold()),
            StepStatus::Pending => ("○ ", dim(), plain()),
        };
        let text = wrap(&Line::styled(clean(&step.text), style), look.inner(4));
        lines.extend(indent(text, Line::plain("  ").with(mark, mark_style), Line::plain("    ")));
    }
    if let Some(explanation) = explanation.filter(|text| !blank(text)) {
        lines.extend(indent(wrap(&Line::styled(clean(explanation), dim()), look.inner(2)), Line::plain("  "), Line::plain("  ")));
    }
    lines
}

fn question_lines(question: &str, answer: Option<&str>, look: Look) -> Vec<Line> {
    let mut lines = indent(wrap(&Line::styled(clean(question), bold()), look.inner(2)), Line::styled("? ", accent()), Line::plain("  "));
    match answer {
        Some(answer) => lines.extend(bubble(answer, look)),
        None => lines.push(Line::styled("  Not answered", dim())),
    }
    lines
}

fn steer_lines(text: &str, look: Look) -> Vec<Line> {
    let mut lines = bubble(text, look);
    lines.push(Line::styled("   added while working", dim()));
    lines
}

fn note_lines(text: &str, look: Look) -> Vec<Line> {
    indent(wrap(&Line::styled(clean(text), dim()), look.inner(2)), Line::styled("· ", dim()), Line::plain("  "))
}

fn footer(turn: &Turn) -> Line {
    let millis = turn.worked_ms.unwrap_or_else(|| now_ms().saturating_sub(turn.started));
    let mut parts = vec![format!("Worked for {}", seconds_label(millis as f32 / 1000.0))];
    parts.extend(turn.model.clone());
    parts.extend(generated(turn));
    Line::styled(format!("  {}", parts.join(" · ")), dim())
}

pub fn generated(turn: &Turn) -> Option<String> {
    (turn.usage.completion > 0).then(|| format!("↓ {} tokens", count(turn.usage.completion)))
}

pub fn ending_lines(turn: &Turn, look: Look) -> Vec<Line> {
    let mut lines = Vec::new();
    match &turn.state {
        TurnState::Stopped => lines.push(Line::styled("■ You stopped this reply", dim())),
        TurnState::Failed(error) => {
            lines.extend(indent(wrap(&Line::styled(clean(error), bad()), look.inner(2)), Line::styled("✗ ", bad()), Line::plain("  ")))
        }
        _ => {}
    }
    let changes = turn.changes();
    if !changes.is_empty() {
        let added: usize = changes.iter().map(|change| change.added).sum();
        let removed: usize = changes.iter().map(|change| change.removed).sum();
        let files = if changes.len() == 1 { "file" } else { "files" };
        lines.push(
            Line::styled(format!("{} {files} changed", changes.len()), bold())
                .with(format!(" +{added}"), good())
                .with(format!(" -{removed}"), bad()),
        );
        for change in &changes {
            let mut line = Line::plain(format!("  {}", change.path))
                .with(format!(" +{}", change.added), good())
                .with(format!(" -{}", change.removed), bad());
            if change.created {
                line.push(" new", dim());
            }
            if change.deleted {
                line.push(" deleted", dim());
            }
            lines.push(truncate(&line, look.width));
        }
    }
    lines.push(truncate(&footer(turn), look.width));
    lines
}

pub fn activity(turn: &Turn, waiting: bool, asked: bool) -> String {
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
        Some(Item::Tool(tool)) if tool.working() && !hidden_tool(tool) => "Working".into(),
        _ => "Processing".into(),
    }
}

pub fn writing(turn: &Turn) -> bool {
    matches!(activity(turn, false, false).as_str(), "Writing" | "Working")
}

pub fn plan_progress(turn: &Turn, look: Look) -> Option<Line> {
    let (steps, _) = plan(turn)?;
    let done = steps.iter().filter(|step| step.status == StepStatus::Done).count();
    let next = steps
        .iter()
        .find(|step| step.status == StepStatus::Active)
        .or_else(|| steps.iter().find(|step| step.status == StepStatus::Pending));
    let mut line = Line::styled(format!("Plan {done}/{}", steps.len()), dim());
    if let Some(step) = next {
        line.push(format!(" · {}", clean(&step.text)), plain());
    }
    Some(truncate(&line, look.width))
}

fn settled(text: &str, from: usize) -> usize {
    let mut fence = false;
    let mut cut = from;
    let mut position = from;
    for line in text[from..].split_inclusive('\n') {
        position += line.len();
        if !line.ends_with('\n') {
            break;
        }
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = !fence;
        } else if trimmed.is_empty() && !fence {
            cut = position;
        }
    }
    cut
}

pub fn turn_lines(turn: &Turn, look: Look) -> Vec<Line> {
    let mut lines = vec![Line::default()];
    lines.extend(prompt_lines(turn, look));
    lines.extend(Progress::new().advance(turn, look));
    lines
}

pub struct Progress {
    item: usize,
    offset: usize,
    plan: Option<Vec<PlanStep>>,
    done: bool,
}

impl Progress {
    pub fn new() -> Progress {
        Progress { item: 0, offset: 0, plan: None, done: false }
    }

    pub fn done(&self) -> bool {
        self.done
    }

    pub fn advance(&mut self, turn: &Turn, look: Look) -> Vec<Line> {
        let mut out = Vec::new();
        if self.done {
            return out;
        }
        let working = turn.working();
        while self.item < turn.items.len() {
            let streaming = working && self.item + 1 == turn.items.len();
            match &turn.items[self.item] {
                Item::Text(text) => {
                    let end = if streaming { settled(text, self.offset) } else { text.len() };
                    if end > self.offset {
                        let chunk = &text[self.offset..end];
                        if !blank(chunk) {
                            let first = blank(&text[..self.offset]);
                            block(&mut out, answer(markdown::render(chunk, look.inner(2)), first));
                        }
                        self.offset = end;
                    }
                    if streaming {
                        break;
                    }
                }
                Item::Reasoning(text) => {
                    if streaming {
                        break;
                    }
                    block(&mut out, reasoning_lines(text, look));
                }
                Item::Tool(tool) if hidden_tool(tool) => {}
                Item::Tool(tool) if look.group && grouped(tool) => {
                    let (end, scanned) = group_span(&turn.items, self.item, look);
                    let tools = group_tools(&turn.items[self.item..end]);
                    let closed = scanned < turn.items.len() || !working;
                    if !closed || tools.iter().any(|tool| tool.working()) || held(&tools) {
                        break;
                    }
                    block(&mut out, vec![group_head(&tools, look, "•")]);
                    self.item = end;
                    self.offset = 0;
                    continue;
                }
                Item::Tool(tool) => {
                    if tool.working() {
                        break;
                    }
                    let mut lines = tool_rows(tool, look, "•");
                    lines.extend(tool_body(tool, look));
                    block(&mut out, lines);
                }
                Item::Plan { steps, explanation } => {
                    if self.plan.as_ref() != Some(steps) {
                        block(&mut out, plan_lines(steps, explanation.as_deref(), look));
                        self.plan = Some(steps.clone());
                    }
                }
                Item::Question { question, answer, .. } => {
                    if answer.is_none() && working {
                        break;
                    }
                    block(&mut out, question_lines(question, answer.as_deref(), look));
                }
                Item::Steer(text) => block(&mut out, steer_lines(text, look)),
                Item::Note(text) => block(&mut out, note_lines(text, look)),
            }
            self.item += 1;
            self.offset = 0;
        }
        if !working && self.item >= turn.items.len() {
            block(&mut out, ending_lines(turn, look));
            self.done = true;
        }
        out
    }

    pub fn live(&self, turn: &Turn, look: Look, spinner: &str, thought: Option<&str>) -> Vec<Line> {
        let mut lines = Vec::new();
        let mut index = self.item;
        while index < turn.items.len() {
            match &turn.items[index] {
                Item::Text(text) => {
                    let start = if index == self.item { self.offset.min(text.len()) } else { 0 };
                    let rest = &text[start..];
                    if !blank(rest) {
                        let rendered = answer(markdown::render(rest, look.inner(2)), blank(&text[..start]));
                        let skip = rendered.len().saturating_sub(LIVE_TEXT);
                        block(&mut lines, rendered.into_iter().skip(skip).collect());
                    }
                }
                Item::Reasoning(text) if index + 1 < turn.items.len() => block(&mut lines, reasoning_lines(text, look)),
                Item::Reasoning(text) => block(&mut lines, thinking_lines(text, look, thought)),
                Item::Tool(tool) if hidden_tool(tool) => {}
                Item::Tool(tool) if look.group && grouped(tool) => {
                    let (end, _) = group_span(&turn.items, index, look);
                    block(&mut lines, group_rows(&group_tools(&turn.items[index..end]), look, spinner));
                    index = end;
                    continue;
                }
                Item::Tool(tool) => {
                    let mut rows = tool_rows(tool, look, spinner);
                    if tool.status == ToolStatus::Running && tool.category == Category::Command {
                        rows.extend(running_command(tool, look));
                    } else if !tool.working() {
                        rows.extend(tool_body(tool, look));
                    }
                    block(&mut lines, rows);
                }
                Item::Steer(text) => block(&mut lines, steer_lines(text, look)),
                Item::Note(text) => block(&mut lines, note_lines(text, look)),
                Item::Plan { .. } | Item::Question { .. } => {}
            }
            index += 1;
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_core::diff_text;
    use quill_core::tools::legacy;

    fn look() -> Look {
        Look { width: 60, reasoning: Reasoning::Collapsed, group: true }
    }

    fn tool(id: &str, name: &str, target: &str, status: ToolStatus) -> Item {
        let mut item = ToolItem::new(legacy(name));
        item.id = id.into();
        item.target = target.into();
        item.status = status;
        Item::Tool(item)
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    #[test]
    fn streamed_text_commits_whole_paragraphs() {
        let mut turn = Turn::new("p".into(), Vec::new());
        let mut progress = Progress::new();
        turn.push_text("First paragraph.\n\nSecond is still");
        assert_eq!(texts(&progress.advance(&turn, look())), vec!["", "● First paragraph."]);
        assert_eq!(texts(&progress.live(&turn, look(), "*", None)), vec!["", "  Second is still"]);
        turn.push_text(" coming.");
        assert!(progress.advance(&turn, look()).is_empty());
        turn.finish(TurnState::Done);
        let rest = texts(&progress.advance(&turn, look()));
        assert_eq!(rest[..2], ["".to_string(), "  Second is still coming.".to_string()]);
        assert!(rest.last().unwrap().starts_with("  Worked for"));
        assert!(progress.done());
    }

    #[test]
    fn code_fences_hold_back_until_closed() {
        let mut turn = Turn::new("p".into(), Vec::new());
        let mut progress = Progress::new();
        turn.push_text("```\nfn main() {}\n\nmore\n");
        assert!(progress.advance(&turn, look()).is_empty());
        turn.push_text("```\n\nAfter.");
        assert_eq!(texts(&progress.advance(&turn, look())), vec!["", "● │ fn main() {}", "  │ ", "  │ more"]);
    }

    #[test]
    fn exploration_groups_wait_for_the_run_to_end() {
        let mut turn = Turn::new("p".into(), Vec::new());
        let mut progress = Progress::new();
        turn.items.push(tool("1", "read_file", "src/a.rs (lines 260-294)", ToolStatus::Done));
        turn.items.push(tool("2", "grep", "TODO", ToolStatus::Running));
        assert!(progress.advance(&turn, look()).is_empty());
        assert_eq!(
            texts(&progress.live(&turn, look(), "*", None)),
            vec!["", "* Reading 1 file, searching for 1 pattern", "● Read(src/a.rs (lines 260-294))", "* Search(TODO)"]
        );
        if let Item::Tool(grep) = &mut turn.items[1] {
            grep.status = ToolStatus::Done;
        }
        turn.items.push(tool("3", "shell", "cargo test", ToolStatus::Running));
        assert!(progress.advance(&turn, look()).is_empty());
        let wide = Look { width: 100, ..look() };
        assert_eq!(
            texts(&progress.live(&turn, wide, "*", None)),
            vec![
                "",
                "* Reading 1 file, searching for 1 pattern, running 1 shell command",
                "● Read(src/a.rs (lines 260-294))",
                "● Search(TODO)",
                "* Shell(cargo test)",
                "  └ Running…"
            ]
        );
        if let Item::Tool(shell) = &mut turn.items[2] {
            shell.status = ToolStatus::Done;
            shell.output = "Exit code 0".into();
        }
        turn.finish(TurnState::Done);
        let rest = texts(&progress.advance(&turn, look()));
        assert_eq!(rest[..2], ["".to_string(), "  Read 1 file, searched for 1 pattern, ran 1 shell command".to_string()]);
    }

    #[test]
    fn finished_searches_stay_open_for_a_moment() {
        let mut turn = Turn::new("p".into(), Vec::new());
        let mut progress = Progress::new();
        let mut item = ToolItem::new(legacy("grep"));
        item.id = "1".into();
        item.target = "send".into();
        item.status = ToolStatus::Done;
        item.ended = Some(now_ms());
        turn.items.push(Item::Tool(item));
        turn.push_text("Next.");
        assert!(progress.advance(&turn, look()).is_empty());
        assert_eq!(texts(&progress.live(&turn, look(), "*", None))[..3], ["", "  Searched for 1 pattern", "● Search(send)"]);
        if let Item::Tool(grep) = &mut turn.items[0] {
            grep.ended = Some(now_ms() - HOLD_MS - 1);
        }
        assert_eq!(texts(&progress.advance(&turn, look()))[..2], ["".to_string(), "  Searched for 1 pattern".to_string()]);
    }

    #[test]
    fn hidden_thoughts_do_not_split_a_group() {
        let mut turn = Turn::new("p".into(), Vec::new());
        turn.items.push(tool("1", "read_file", "a.rs", ToolStatus::Done));
        turn.push_reasoning("Now search.");
        turn.items.push(tool("2", "grep", "TODO", ToolStatus::Done));
        turn.finish(TurnState::Done);
        let lines = texts(&Progress::new().advance(&turn, Look { reasoning: Reasoning::Hidden, ..look() }));
        assert_eq!(lines[1], "  Read 1 file, searched for 1 pattern");
    }

    #[test]
    fn edits_show_their_diff() {
        let mut turn = Turn::new("p".into(), Vec::new());
        let mut item = ToolItem::new(legacy("write_file"));
        item.id = "1".into();
        item.target = "a.py".into();
        item.status = ToolStatus::Done;
        item.diffs = vec![diff_text("a.py", Some("x\n"), "y\n")];
        turn.items.push(Item::Tool(item));
        turn.finish(TurnState::Done);
        let lines = texts(&Progress::new().advance(&turn, look()));
        assert_eq!(lines[1], "● Write(a.py)");
        assert_eq!(lines[2], "  └ Added 1 line, removed 1 line");
        assert_eq!(lines[3].trim(), "1 - x");
        assert_eq!(lines[4].trim(), "1 + y");
        assert!(lines.iter().any(|line| line == "1 file changed +1 -1"));
    }

    #[test]
    fn reads_get_their_own_row_when_grouping_is_off() {
        let mut turn = Turn::new("p".into(), Vec::new());
        let mut item = ToolItem::new(legacy("read_file"));
        item.id = "1".into();
        item.target = "src/a.rs".into();
        item.status = ToolStatus::Done;
        item.output = "    1→one\n    2\n    3→three\n(showing lines 1-3 of 9; pass start_line to read more)".into();
        turn.items.push(Item::Tool(item));
        turn.finish(TurnState::Done);
        let lines = texts(&Progress::new().advance(&turn, Look { group: false, ..look() }));
        assert_eq!(lines[1..3], ["● Read(src/a.rs)".to_string(), "  └ Read 3 lines".to_string()]);
    }

    #[test]
    fn searches_say_what_they_found() {
        let searched = |name: &str, output: &str| {
            let mut item = ToolItem::new(legacy(name));
            item.status = ToolStatus::Done;
            item.output = output.into();
            texts(&tool_body(&item, look()))[0].clone()
        };
        assert_eq!(searched("grep", "No matches for \"sick\"."), "  └ No matches");
        assert_eq!(searched("grep", "a.hx:1: sick\n--\na.hx:9: sick\n... and 4 more matches. Narrow it."), "  └ Found 6 lines");
        assert_eq!(searched("find_files", "No files match \"*.hx\". Use grep."), "  └ No files found");
        assert_eq!(searched("find_files", "a.hx\nb.hx"), "  └ Found 2 files");
        assert_eq!(searched("read_file", "src/ contains:\nlib/\na.js\n... and 3 more."), "  └ Listed 5 items");
        assert_eq!(searched("read_file", "src/ is empty."), "  └ Empty folder");
    }

    #[test]
    fn failed_commands_show_their_tail() {
        let mut turn = Turn::new("p".into(), Vec::new());
        let mut item = ToolItem::new(legacy("shell"));
        item.id = "1".into();
        item.target = "cargo test".into();
        item.status = ToolStatus::Done;
        item.output = "Exit code 101\nerror: one\nerror: two".into();
        turn.items.push(Item::Tool(item));
        turn.finish(TurnState::Done);
        let lines = texts(&Progress::new().advance(&turn, look()));
        assert_eq!(lines[1], "● Shell(cargo test) · exit 101");
        assert_eq!(lines[2..4], ["  └ error: one".to_string(), "    error: two".to_string()]);
    }

    #[test]
    fn failed_commands_skip_stack_frames() {
        let mut item = ToolItem::new(legacy("shell"));
        item.status = ToolStatus::Done;
        item.output = "Exit code 1\nAssertionError: 26 !== 27\n    at check (a.js:1:1)\n\n    at run (b.js:2:2)\nReferenceError: before is not defined".into();
        let lines = texts(&tool_body(&item, look()));
        assert_eq!(lines, ["  └ AssertionError: 26 !== 27", "    ReferenceError: before is not defined"]);
        item.output = "Exit code 1\nGet-ChildItem : no parameter named 'la'.\nAt line:1 char:93\n+ ... $ProgressPreference='SilentlyContinue'; ls -la\n+                                               ~~~\n    + CategoryInfo          : InvalidArgument\n    + FullyQualifiedErrorId : NamedParameterNotFound".into();
        let lines = texts(&tool_body(&item, look()));
        assert_eq!(lines, ["  └ Get-ChildItem : no parameter named 'la'."]);
    }

    #[test]
    fn stopped_and_failed_turns_say_so() {
        let mut stopped = Turn::new("p".into(), Vec::new());
        stopped.finish(TurnState::Stopped);
        assert!(texts(&ending_lines(&stopped, look())).contains(&"■ You stopped this reply".to_string()));
        let mut failed = Turn::new("p".into(), Vec::new());
        failed.finish(TurnState::Failed("Nothing is answering".into()));
        assert_eq!(texts(&ending_lines(&failed, look()))[0], "✗ Nothing is answering");
    }

    #[test]
    fn plans_print_when_they_first_appear_and_when_they_change() {
        let mut turn = Turn::new("p".into(), Vec::new());
        let mut progress = Progress::new();
        let step = |status| vec![PlanStep { text: "Write it".into(), status }];
        turn.set_plan(step(StepStatus::Active), None);
        turn.push_text("working");
        let first = texts(&progress.advance(&turn, look()));
        assert_eq!(first[1..], ["Plan · 0 of 1 done".to_string(), "  › Write it".to_string()]);
        turn.set_plan(step(StepStatus::Done), None);
        turn.finish(TurnState::Done);
        let last = texts(&progress.advance(&turn, look()));
        assert!(last.contains(&"Plan · 1 of 1 done".to_string()));
        assert_eq!(texts(&progress.advance(&turn, look())), Vec::<String>::new());
    }

    #[test]
    fn reasoning_follows_the_setting() {
        let mut turn = Turn::new("p".into(), Vec::new());
        turn.push_reasoning("Let me look.\nChecking the tests.");
        turn.push_text("Done.");
        turn.finish(TurnState::Done);
        let collapsed = texts(&Progress::new().advance(&turn, look()));
        assert_eq!(collapsed[1], "◇ Thought Checking the tests.");
        let hidden = texts(&Progress::new().advance(&turn, Look { reasoning: Reasoning::Hidden, ..look() }));
        assert_eq!(hidden[1], "● Done.");
    }

    #[test]
    fn thoughts_are_summarised_by_whole_sentences() {
        assert_eq!(summary("Let me look at **main.rs**. Then I will"), Some("Let me look at main.rs.".into()));
        assert_eq!(summary("Still going"), None);
        assert_eq!(last_sentence("First one. Second one"), "Second one");
        assert_eq!(summary("Facts:\n1. Foxes rest at 110 bpm\n2. They"), Some("Foxes rest at 110 bpm".into()));
    }

    #[test]
    fn activity_names_what_is_happening() {
        let mut turn = Turn::new("p".into(), Vec::new());
        assert_eq!(activity(&turn, false, false), "Warming up the model");
        turn.push_reasoning("hm");
        assert_eq!(activity(&turn, false, false), "Thinking");
        turn.items.push(tool("1", "shell", "ls", ToolStatus::Running));
        assert_eq!(activity(&turn, false, false), "Working");
        assert_eq!(activity(&turn, true, false), "Waiting for your OK");
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(42), "42s");
        assert_eq!(duration(125), "2m 5s");
        assert_eq!(duration(3720), "1h 2m");
        assert_eq!(seconds_label(2.34), "2.3s");
        assert_eq!(command_body("Exit code 0\nhello"), "hello");
    }
}