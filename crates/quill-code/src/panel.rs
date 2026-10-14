use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled, Subscription, Window, div,
    prelude::*, px, uniform_list,
};
use quill_agent::FileDiff;

use crate::icons::Icon;
use crate::palette::{Palette, palette};
use crate::thread_view::{diff_lines, file_icon};
use crate::ui::{MONO, icon};

const SKIP: [&str; 6] = [".git", "node_modules", "target", ".venv", "__pycache__", ".next"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Diff,
    Files,
}

pub struct ClosePanel;

struct Entry {
    name: String,
    path: PathBuf,
    dir: bool,
}

struct OpenFile {
    path: PathBuf,
    lines: Vec<SharedString>,
    note: Option<String>,
}

struct DiffFile {
    path: String,
    added: usize,
    removed: usize,
    created: bool,
    blocks: Vec<FileDiff>,
}

pub struct Panel {
    pub tab: Tab,
    root: Option<PathBuf>,
    title: String,
    files: Vec<DiffFile>,
    collapsed: HashSet<String>,
    expanded: HashSet<PathBuf>,
    listing: HashMap<PathBuf, Vec<Entry>>,
    open: Option<OpenFile>,
    search: Entity<InputState>,
    query: String,
    matches: Vec<PathBuf>,
    _subscription: Subscription,
}

impl EventEmitter<ClosePanel> for Panel {}

fn read_dir(dir: &Path) -> Vec<Entry> {
    let Ok(read) = fs::read_dir(dir) else { return Vec::new() };
    let mut entries: Vec<Entry> = read
        .filter_map(Result::ok)
        .map(|e| Entry { name: e.file_name().to_string_lossy().into_owned(), path: e.path(), dir: e.file_type().is_ok_and(|t| t.is_dir()) })
        .filter(|e| !(e.dir && SKIP.contains(&e.name.as_str())))
        .collect();
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    entries.truncate(1000);
    entries
}

fn collect(dir: &Path, needle: &str, out: &mut Vec<PathBuf>) {
    for entry in read_dir(dir) {
        if out.len() >= 300 {
            return;
        }
        if entry.dir {
            collect(&entry.path, needle, out);
        } else if entry.name.to_lowercase().contains(needle) {
            out.push(entry.path);
        }
    }
}

impl Panel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search files"));
        let subscription = cx.subscribe(&search, |this, search, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.query = search.read(cx).value().trim().to_lowercase();
                this.matches.clear();
                if let (Some(root), false) = (this.root.clone(), this.query.is_empty()) {
                    collect(&root, &this.query.clone(), &mut this.matches);
                }
                cx.notify();
            }
        });
        Panel {
            tab: Tab::Diff,
            root: None,
            title: String::new(),
            files: Vec::new(),
            collapsed: HashSet::new(),
            expanded: HashSet::new(),
            listing: HashMap::new(),
            open: None,
            search,
            query: String::new(),
            matches: Vec::new(),
            _subscription: subscription,
        }
    }

    pub fn set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.root != root {
            self.root = root;
            self.listing.clear();
            self.expanded.clear();
            self.open = None;
        }
        cx.notify();
    }

    pub fn show_diff(&mut self, title: String, diffs: Vec<FileDiff>, cx: &mut Context<Self>) {
        let mut files: Vec<DiffFile> = Vec::new();
        for diff in diffs {
            match files.iter_mut().find(|f| f.path == diff.path) {
                Some(file) => {
                    file.added += diff.added;
                    file.removed += diff.removed;
                    file.blocks.push(diff);
                }
                None => files.push(DiffFile {
                    path: diff.path.clone(),
                    added: diff.added,
                    removed: diff.removed,
                    created: diff.created,
                    blocks: vec![diff],
                }),
            }
        }
        self.files = files;
        self.title = title;
        self.collapsed.clear();
        self.tab = Tab::Diff;
        cx.notify();
    }

    pub fn show_files(&mut self, cx: &mut Context<Self>) {
        self.tab = Tab::Files;
        self.listing.clear();
        cx.notify();
    }

    fn select(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    fn toggle_dir(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        cx.notify();
    }

    fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let (lines, note) = match fs::read(&path) {
            Ok(bytes) if bytes.iter().take(8192).any(|b| *b == 0) => (Vec::new(), Some("This is a binary file.".to_string())),
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                let lines: Vec<SharedString> = text.lines().take(20_000).map(|l| SharedString::from(l.replace('\t', "    "))).collect();
                let note = (text.lines().count() > 20_000).then(|| "Showing the first 20,000 lines.".to_string());
                (lines, note)
            }
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        self.open = Some(OpenFile { path, lines, note });
        cx.notify();
    }

    fn entries(&mut self, dir: &Path) -> &Vec<Entry> {
        self.listing.entry(dir.to_path_buf()).or_insert_with(|| read_dir(dir))
    }

    fn tree_rows(&mut self, dir: PathBuf, depth: usize, rows: &mut Vec<(PathBuf, String, bool, usize)>) {
        let children: Vec<(PathBuf, String, bool)> = self.entries(&dir).iter().map(|e| (e.path.clone(), e.name.clone(), e.dir)).collect();
        for (path, name, is_dir) in children {
            if rows.len() > 2000 {
                return;
            }
            rows.push((path.clone(), name, is_dir, depth));
            if is_dir && self.expanded.contains(&path) {
                self.tree_rows(path, depth + 1, rows);
            }
        }
    }

    fn tab_button(&self, tab: Tab, name: Icon, label: &'static str, p: Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let on = self.tab == tab;
        div()
            .id(label)
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(10.))
            .rounded(px(8.))
            .text_size(px(13.))
            .text_color(if on { p.text } else { p.text_faint })
            .when(on, |b| b.bg(p.row_active))
            .cursor_pointer()
            .hover(move |style| style.text_color(p.text))
            .child(icon(name, 14., if on { p.text } else { p.text_faint }))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| this.select(tab, cx)))
    }

    fn diff_tab(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let added: usize = self.files.iter().map(|f| f.added).sum();
        let removed: usize = self.files.iter().map(|f| f.removed).sum();
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(14.))
            .py(px(10.))
            .border_b_1()
            .border_color(p.border_soft)
            .text_size(px(13.))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(self.title.clone()))
            .child(div().flex_1())
            .child(div().text_color(p.add).child(format!("+{added}")))
            .child(div().text_color(p.del).child(format!("-{removed}")));
        let body = if self.files.is_empty() {
            div()
                .p(px(24.))
                .text_size(px(13.5))
                .text_color(p.text_faint)
                .child("Nothing to review yet. Every file the agent touches shows up here.")
        } else {
            div().flex().flex_col().children(self.files.iter().map(|file| {
                let path = file.path.clone();
                let collapsed = self.collapsed.contains(&file.path);
                div()
                    .flex()
                    .flex_col()
                    .border_b_1()
                    .border_color(p.border_soft)
                    .child(
                        div()
                            .id(SharedString::from(format!("diff-file-{}", file.path)))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(12.))
                            .py(px(8.))
                            .cursor_pointer()
                            .hover(move |style| style.bg(p.row_hover))
                            .child(icon(if collapsed { Icon::ChevronRight } else { Icon::ChevronDown }, 14., p.text_faint))
                            .child(icon(
                                if file.created { Icon::FilePlus } else { Icon::FileDiff },
                                15.,
                                if file.created { p.add } else { p.text_muted },
                            ))
                            .child(
                                div().flex_1().min_w_0().font_family(MONO).text_size(px(13.)).text_color(p.text).child(file.path.clone()),
                            )
                            .child(div().text_size(px(12.5)).text_color(p.add).child(format!("+{}", file.added)))
                            .child(div().text_size(px(12.5)).text_color(p.del).child(format!("-{}", file.removed)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.collapsed.remove(&path) {
                                    this.collapsed.insert(path.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .when(!collapsed, |col| {
                        col.children(file.blocks.iter().enumerate().map(|(i, block)| {
                            div().when(i > 0, |d| d.border_t_1().border_color(p.border_soft)).pb(px(6.)).child(diff_lines(block, 3000, p))
                        }))
                    })
            }))
        };
        div().size_full().flex().flex_col().child(header).child(div().id("diff-scroll").flex_1().min_h_0().overflow_y_scroll().child(body))
    }

    fn files_tab(&mut self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let Some(root) = self.root.clone() else {
            return div()
                .p(px(24.))
                .text_size(px(13.5))
                .text_color(p.text_faint)
                .child("Pick a project to browse its files.")
                .into_any_element();
        };
        if let Some(open) = &self.open {
            let rel = open.path.strip_prefix(&root).map_or_else(|_| open.path.display().to_string(), |r| r.display().to_string());
            let count = open.lines.len();
            let width = count.to_string().len().max(2) as f32 * 8.5 + 16.;
            let lines = open.lines.clone();
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(10.))
                        .py(px(8.))
                        .border_b_1()
                        .border_color(p.border_soft)
                        .text_size(px(13.))
                        .child(
                            div()
                                .id("file-back")
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .px(px(6.))
                                .h(px(24.))
                                .rounded(px(6.))
                                .text_color(p.text_faint)
                                .cursor_pointer()
                                .hover(move |style| style.bg(p.row_hover).text_color(p.text))
                                .child(icon(Icon::ArrowLeft, 13., p.text_faint))
                                .child(crate::project::name(&root))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open = None;
                                    cx.notify();
                                })),
                        )
                        .child(icon(Icon::ChevronRight, 12., p.text_faint))
                        .child(div().font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(rel.replace('\\', "/"))),
                )
                .when_some(open.note.clone(), |col, note| {
                    col.child(div().px(px(14.)).py(px(8.)).text_size(px(12.5)).text_color(p.text_faint).child(note))
                })
                .child(
                    uniform_list("file-lines", count, move |range, _, _| {
                        range
                            .map(|n| {
                                div()
                                    .flex()
                                    .h(px(20.))
                                    .font_family(MONO)
                                    .text_size(px(12.5))
                                    .child(
                                        div()
                                            .w(px(width))
                                            .flex_shrink_0()
                                            .pr(px(12.))
                                            .flex()
                                            .justify_end()
                                            .text_color(p.text_faint)
                                            .child((n + 1).to_string()),
                                    )
                                    .child(div().whitespace_nowrap().text_color(p.text).child(lines[n].clone()))
                            })
                            .collect()
                    })
                    .flex_1()
                    .py(px(6.)),
                )
                .into_any_element();
        }
        let mut rows = Vec::new();
        if self.query.is_empty() {
            self.tree_rows(root.clone(), 0, &mut rows);
        } else {
            rows = self
                .matches
                .iter()
                .map(|path| {
                    (
                        path.clone(),
                        path.strip_prefix(&root)
                            .map_or_else(|_| path.display().to_string(), |r| r.display().to_string().replace('\\', "/")),
                        false,
                        0,
                    )
                })
                .collect();
        }
        let expanded = self.expanded.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(p.border_soft)
                    .child(div().flex_1().child(Input::new(&self.search).appearance(false).prefix(icon(Icon::Search, 14., p.text_faint))))
                    .child(
                        div()
                            .id("refresh-files")
                            .size(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(move |style| style.bg(p.row_hover))
                            .child(icon(Icon::Refresh, 13., p.text_faint))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.listing.clear();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("tree-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py(px(4.))
                    .when(rows.is_empty(), |list| {
                        list.child(div().p(px(16.)).text_size(px(13.)).text_color(p.text_faint).child(if self.query.is_empty() {
                            "Nothing in this folder yet."
                        } else {
                            "No file names match."
                        }))
                    })
                    .children(rows.into_iter().enumerate().map(|(i, (path, name, is_dir, depth))| {
                        let open_dir = expanded.contains(&path);
                        let icon_name = if is_dir { if open_dir { Icon::FolderOpen } else { Icon::Folder } } else { file_icon(&name) };
                        div()
                            .id(("tree-row", i))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(28.))
                            .pl(px(10. + depth as f32 * 14.))
                            .pr(px(10.))
                            .mx(px(4.))
                            .rounded(px(6.))
                            .text_size(px(13.))
                            .text_color(p.text_muted)
                            .cursor_pointer()
                            .hover(move |style| style.bg(p.row_hover).text_color(p.text))
                            .child(div().w(px(12.)).when(is_dir, |d| {
                                d.child(icon(if open_dir { Icon::ChevronDown } else { Icon::ChevronRight }, 12., p.text_faint))
                            }))
                            .child(icon(icon_name, 15., p.text_faint))
                            .child(div().min_w_0().whitespace_nowrap().overflow_hidden().text_ellipsis().child(name))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if is_dir {
                                    this.toggle_dir(path.clone(), cx);
                                } else {
                                    this.open_file(path.clone(), cx);
                                }
                            }))
                    })),
            )
            .into_any_element()
    }
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let body = match self.tab {
            Tab::Diff => self.diff_tab(p, cx).into_any_element(),
            Tab::Files => self.files_tab(p, cx).into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(px(44.))
                    .flex_shrink_0()
                    .px(px(8.))
                    .border_b_1()
                    .border_color(p.border_soft)
                    .child(self.tab_button(Tab::Diff, Icon::FileDiff, "Review", p, cx))
                    .child(self.tab_button(Tab::Files, Icon::Files, "Files", p, cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("close-panel")
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(7.))
                            .cursor_pointer()
                            .hover(move |style| style.bg(p.row_hover))
                            .child(icon(Icon::X, 15., p.text_faint))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ClosePanel))),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}