use std::time::Duration;

use gpui_kit::component::Icon as ComponentIcon;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::setting::{
    NumberFieldOptions, SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings as Pages,
};
use gpui_kit::{
    AnyElement, App, Context, Div, Entity, EventEmitter, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, WeakEntity, Window, div, prelude::*, px,
};
use quill_config::{Motion, Reasoning, Settings as Prefs, Theme, settings_path};
use quill_core::{Access, Rule, RuleAction, Shell};

use crate::backend::Backend;
use crate::config::{Config, personal_instructions_path, read_personal_instructions, write_personal_instructions};
use crate::icons::Icon;
use crate::palette::{Palette, palette};
use crate::project;
use crate::ui::{Tone, badge, button, icon};
use crate::version::VersionPage;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    Appearance,
    Models,
    Agent,
    Tools,
    Permissions,
    Instructions,
    Projects,
    Advanced,
    Version,
}

impl Section {
    const ALL: [Section; 10] = [
        Section::General,
        Section::Appearance,
        Section::Models,
        Section::Agent,
        Section::Tools,
        Section::Permissions,
        Section::Instructions,
        Section::Projects,
        Section::Advanced,
        Section::Version,
    ];

    fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Appearance => "Appearance",
            Section::Models => "Models",
            Section::Agent => "Agent",
            Section::Tools => "Tools",
            Section::Permissions => "Permissions",
            Section::Instructions => "Instructions",
            Section::Projects => "Projects",
            Section::Advanced => "Advanced",
            Section::Version => "Version",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Section::General => Icon::Sliders,
            Section::Appearance => Icon::Palette,
            Section::Models => Icon::Cube,
            Section::Agent => Icon::Bolt,
            Section::Tools => Icon::Wrench,
            Section::Permissions => Icon::Shield,
            Section::Instructions => Icon::FileText,
            Section::Projects => Icon::Folder,
            Section::Advanced => Icon::Gear,
            Section::Version => Icon::Info,
        }
    }
}

pub enum SettingsEvent {
    Back,
    AddProject,
}

pub struct Settings {
    section: Section,
    opened: usize,
    backend: Entity<Backend>,
    instructions: Entity<TextareaState>,
    version: Entity<VersionPage>,
    note: Entity<Option<(bool, SharedString)>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for Settings {}

impl Settings {
    pub fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let instructions = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("For example: Prefer small, focused changes. Explain risky changes before making them.")
                .auto_grow(8, 28)
                .default_value(read_personal_instructions())
        });
        let note = cx.new(|_| None);
        let subscriptions = vec![
            cx.observe(&backend, |_, _, cx| cx.notify()),
            cx.observe(&note, |_, _, cx| cx.notify()),
            cx.observe_global::<Config>(|_, cx| cx.notify()),
        ];
        Settings {
            section: Section::General,
            opened: 0,
            backend,
            instructions,
            version: cx.new(VersionPage::new),
            note,
            _subscriptions: subscriptions,
        }
    }

    pub fn open(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        self.opened += 1;
        if section == Section::Models {
            self.backend.update(cx, |b, cx| b.refresh(cx));
        }
        cx.notify();
    }

    fn pages(&self, this: WeakEntity<Settings>, cx: &App) -> Vec<SettingPage> {
        Section::ALL
            .iter()
            .map(|section| {
                let page = SettingPage::new(section.label()).icon(ComponentIcon::empty().path(section.icon().path())).resettable(false);
                let groups = match section {
                    Section::General => general(),
                    Section::Appearance => appearance(),
                    Section::Models => crate::model_settings::groups(self.backend.clone(), self.note.clone(), cx),
                    Section::Agent => agent(),
                    Section::Tools => tools(),
                    Section::Permissions => permissions(),
                    Section::Instructions => vec![instructions(self.instructions.clone(), self.note.clone())],
                    Section::Projects => vec![projects(this.clone())],
                    Section::Advanced => advanced(self.note.clone()),
                    Section::Version => {
                        let version = self.version.clone();
                        vec![SettingGroup::new().item(SettingItem::render(move |_, _, _| div().w_full().child(version.clone())))]
                    }
                };
                page.groups(groups)
            })
            .collect()
    }
}

pub(crate) fn change(cx: &mut App, edit: impl FnOnce(&mut Prefs)) {
    Config::update(cx, edit);
}

fn switch(title: &str, description: &str, get: fn(&Prefs) -> bool, set: fn(&mut Prefs, bool)) -> SettingItem {
    let default = get(&Prefs::default());
    SettingItem::new(
        title.to_string(),
        SettingField::switch(move |cx| get(Config::get(cx)), move |on, cx| change(cx, |s| set(s, on))).default_value(default),
    )
    .description(description.to_string())
}

fn number(title: &str, description: &str, range: (f64, f64, f64), get: fn(&Prefs) -> f64, set: fn(&mut Prefs, f64)) -> SettingItem {
    let (min, max, step) = range;
    let default = get(&Prefs::default());
    SettingItem::new(
        title.to_string(),
        SettingField::number_input(
            NumberFieldOptions { min, max, step },
            move |cx| get(Config::get(cx)),
            move |value, cx| change(cx, |s| set(s, value.clamp(min, max))),
        )
        .default_value(default),
    )
    .description(description.to_string())
}

fn choice(
    title: &str,
    description: &str,
    options: Vec<(&'static str, &'static str)>,
    get: fn(&Prefs) -> &'static str,
    set: fn(&mut Prefs, &str),
) -> SettingItem {
    let default = get(&Prefs::default());
    let options = options.into_iter().map(|(value, label)| (SharedString::from(value), SharedString::from(label))).collect();
    SettingItem::new(
        title.to_string(),
        SettingField::dropdown(options, move |cx| get(Config::get(cx)).into(), move |value, cx| change(cx, |s| set(s, &value)))
            .default_value(default),
    )
    .description(description.to_string())
}

fn access_key(access: Access) -> &'static str {
    match access {
        Access::Supervised => "supervised",
        Access::AutoEdits => "auto_edits",
        Access::Full => "full",
    }
}

fn shell_key(shell: Shell) -> &'static str {
    match shell {
        Shell::PowerShell => "powershell",
        Shell::Pwsh => "pwsh",
        Shell::Cmd => "cmd",
        Shell::Bash => "bash",
        Shell::Sh => "sh",
    }
}

fn general() -> Vec<SettingGroup> {
    vec![
        SettingGroup::new()
            .title("New threads")
            .item(choice(
                "Permissions",
                "What agents may do without asking. You can also change it from the prompt box.",
                Access::ALL.iter().map(|a| (access_key(*a), a.label())).collect(),
                |s| access_key(s.agent.access),
                |s, value| s.agent.access = Access::ALL.into_iter().find(|a| access_key(*a) == value).unwrap_or_default(),
            ))
            .item(switch(
                "Remember the last project",
                "New threads start in the project you used last, even after a restart.",
                |s| s.general.remember_project,
                |s, on| s.general.remember_project = on,
            )),
        SettingGroup::new()
            .title("Folders")
            .item(path_item(
                "projects-dir",
                "New projects",
                "Where new and cloned projects are created.",
                |s| s.projects_dir(),
                |s, path| s.general.projects_dir = path,
            ))
            .item(path_item(
                "worktrees-dir",
                "Isolated copies",
                "Where isolated copies of projects are made for threads that should not touch your working folder.",
                |s| s.worktrees_dir(),
                |s, path| s.general.worktrees_dir = path,
            )),
    ]
}

fn theme_key(theme: Theme) -> &'static str {
    match theme {
        Theme::System => "system",
        Theme::Dark => "dark",
        Theme::Light => "light",
    }
}

fn motion_key(motion: Motion) -> &'static str {
    match motion {
        Motion::Full => "full",
        Motion::Reduced => "reduced",
        Motion::Off => "off",
    }
}

fn reasoning_key(reasoning: Reasoning) -> &'static str {
    match reasoning {
        Reasoning::Expanded => "expanded",
        Reasoning::Collapsed => "collapsed",
        Reasoning::Hidden => "hidden",
    }
}

fn appearance() -> Vec<SettingGroup> {
    vec![
        SettingGroup::new()
            .title("Look")
            .item(choice(
                "Theme",
                "Dark, light, or follow Windows as it changes through the day.",
                vec![("system", "Match Windows"), ("dark", "Dark"), ("light", "Light")],
                |s| theme_key(s.appearance.theme),
                |s, value| {
                    s.appearance.theme =
                        [Theme::System, Theme::Dark, Theme::Light].into_iter().find(|t| theme_key(*t) == value).unwrap_or_default()
                },
            ))
            .item(number(
                "Reading width",
                "How wide replies and the prompt box may grow, in pixels.",
                (560., 1600., 20.),
                |s| f64::from(s.appearance.transcript_width),
                |s, v| s.appearance.transcript_width = v as u16,
            ))
            .item(switch(
                "Show the sidebar",
                "The thread list on the left. Ctrl B toggles it.",
                |s| s.appearance.sidebar,
                |s, on| s.appearance.sidebar = on,
            )),
        SettingGroup::new().title("Motion").item(choice(
            "Animations",
            "Menus, cards and rows ease in and out. Reduced keeps them short; off turns them off everywhere.",
            vec![("full", "Full"), ("reduced", "Reduced"), ("off", "Off")],
            |s| motion_key(s.appearance.motion),
            |s, value| {
                s.appearance.motion =
                    [Motion::Full, Motion::Reduced, Motion::Off].into_iter().find(|m| motion_key(*m) == value).unwrap_or_default()
            },
        )),
        SettingGroup::new()
            .title("Replies")
            .item(choice(
                "Reasoning",
                "How the model's thinking is shown once it has finished.",
                vec![("expanded", "Shown"), ("collapsed", "Folded"), ("hidden", "Hidden")],
                |s| reasoning_key(s.appearance.reasoning),
                |s, value| {
                    s.appearance.reasoning = [Reasoning::Expanded, Reasoning::Collapsed, Reasoning::Hidden]
                        .into_iter()
                        .find(|r| reasoning_key(*r) == value)
                        .unwrap_or_default()
                },
            ))
            .item(switch(
                "Group exploring",
                "Fold a run of reads and searches into a single row you can open.",
                |s| s.appearance.group_exploration,
                |s, on| s.appearance.group_exploration = on,
            )),
    ]
}

fn path_item(
    key: &'static str,
    title: &str,
    description: &str,
    get: fn(&Prefs) -> std::path::PathBuf,
    set: fn(&mut Prefs, Option<std::path::PathBuf>),
) -> SettingItem {
    SettingItem::new(
        title.to_string(),
        SettingField::input(
            move |cx| get(Config::get(cx)).display().to_string().into(),
            move |value, cx| {
                let value = value.trim().to_string();
                let path = (!value.is_empty()).then(|| std::path::PathBuf::from(value));
                change(cx, |s| set(s, path));
            },
        )
        .on_reset(move |cx| get(Config::get(cx)) != get(&Prefs::default()), move |_, cx| change(cx, |s| set(s, None))),
    )
    .keywords([key])
    .description(description.to_string())
}

fn agent() -> Vec<SettingGroup> {
    vec![
        SettingGroup::new()
            .title("Turns")
            .item(number(
                "Most steps per reply",
                "How many model calls one reply may make before it stops and waits for you.",
                (1., 1000., 5.),
                |s| s.agent.max_steps as f64,
                |s, v| s.agent.max_steps = v as usize,
            ))
            .item(switch(
                "Read in parallel",
                "Run several file reads and searches at the same time when the model asks for them together.",
                |s| s.agent.parallel_tools,
                |s, on| s.agent.parallel_tools = on,
            )),
        SettingGroup::new()
            .title("Context")
            .item(switch(
                "Compact automatically",
                "When the conversation nears the model's window, summarise earlier turns and trim old tool output.",
                |s| s.agent.auto_compact,
                |s, on| s.agent.auto_compact = on,
            ))
            .item(number(
                "Compact at",
                "How full the window gets, in percent, before compacting.",
                (30., 98., 1.),
                |s| f64::from(s.agent.compact_at),
                |s, v| s.agent.compact_at = v as u8,
            )),
        SettingGroup::new().title("Project instructions").item(list_item(
            "instruction-files",
            "Instruction files",
            "Files read from the project root and given to the agent, in this order.",
            "AGENTS.md",
            |s| s.agent.instruction_files.clone(),
            |s, list| s.agent.instruction_files = list,
        )),
    ]
}

fn tools() -> Vec<SettingGroup> {
    let mut available =
        SettingGroup::new().title("Available tools").description("Turn a tool off and the agent is never offered it.").item(switch(
            "Git tools",
            "Offer commit_changes and open_pull_request in projects that use Git.",
            |s| s.tools.git,
            |s, on| s.tools.git = on,
        ));
    for tool in quill_core::tools::all() {
        let spec = tool.spec();
        let name = spec.name;
        available = available.item(
            SettingItem::new(
                spec.title,
                SettingField::switch(
                    move |cx| !Config::get(cx).tools.disabled.iter().any(|t| t == name),
                    move |on, cx| {
                        change(cx, |s| {
                            s.tools.disabled.retain(|t| t != name);
                            if !on {
                                s.tools.disabled.push(name.to_string());
                            }
                        })
                    },
                )
                .default_value(true),
            )
            .description(spec.summary)
            .keywords([name]),
        );
    }
    vec![
        SettingGroup::new()
            .title("Commands")
            .item(choice(
                "Shell",
                "The shell the agent's commands run in.",
                Shell::ALL.iter().map(|shell| (shell_key(*shell), shell.label())).collect(),
                |s| shell_key(s.tools.shell),
                |s, value| s.tools.shell = Shell::ALL.into_iter().find(|shell| shell_key(*shell) == value).unwrap_or_default(),
            ))
            .item(number(
                "Command timeout",
                "Seconds a command may run when the agent does not ask for longer.",
                (5., 3600., 5.),
                |s| s.tools.command_timeout as f64,
                |s, v| s.tools.command_timeout = v as u64,
            ))
            .item(number(
                "Longest timeout",
                "The most seconds the agent may ask a single command to run.",
                (10., 7200., 10.),
                |s| s.tools.max_command_timeout as f64,
                |s, v| s.tools.max_command_timeout = v as u64,
            )),
        SettingGroup::new()
            .title("Output")
            .item(number(
                "Command output",
                "Characters of command output kept for the model; the middle is cut beyond this.",
                (1000., 500_000., 1000.),
                |s| s.tools.max_output as f64,
                |s, v| s.tools.max_output = v as usize,
            ))
            .item(number(
                "Lines per read",
                "The most lines one read_file call returns.",
                (50., 20_000., 50.),
                |s| s.tools.max_read_lines as f64,
                |s, v| s.tools.max_read_lines = v as usize,
            ))
            .item(number(
                "Search matches",
                "The most matches grep and find_files return.",
                (10., 5000., 10.),
                |s| s.tools.max_matches as f64,
                |s, v| s.tools.max_matches = v as usize,
            ))
            .item(number(
                "Folder entries",
                "The most entries list_dir shows.",
                (20., 5000., 20.),
                |s| s.tools.max_list as f64,
                |s, v| s.tools.max_list = v as usize,
            )),
        SettingGroup::new().title("Files").item(list_item(
            "ignored-folders",
            "Skipped folders",
            "Folders the file tools never look inside, on top of .gitignore.",
            "node_modules",
            |s| s.tools.ignored.clone(),
            |s, list| s.tools.ignored = list,
        )),
        available,
    ]
}

fn permissions() -> Vec<SettingGroup> {
    vec![
        SettingGroup::new()
            .title("Rules")
            .description("Rules override the permission mode. A pattern matches a command's start, or anything when empty; end it with * to match a prefix.")
            .item(SettingItem::render(|_, window, cx| rules_editor(window, cx)).keywords(["allow", "deny", "ask", "rule"])),
        SettingGroup::new().title("Read-only commands").item(list_item(
            "safe-commands",
            "Commands that only look",
            "With Trust edits, commands made only of these run without asking.",
            "git status",
            |s| s.permissions.safe_commands.clone(),
            |s, list| s.permissions.safe_commands = list,
        )),
    ]
}

fn action_label(action: RuleAction) -> &'static str {
    match action {
        RuleAction::Allow => "Allow",
        RuleAction::Ask => "Ask",
        RuleAction::Deny => "Deny",
    }
}

fn rules_editor(window: &mut Window, cx: &mut App) -> AnyElement {
    let p = palette(cx);
    let tool = window.use_keyed_state("rule-tool", cx, |window, cx| InputState::new(window, cx).placeholder("Tool, such as shell, or *"));
    let pattern =
        window.use_keyed_state("rule-pattern", cx, |window, cx| InputState::new(window, cx).placeholder("Pattern, such as npm test"));
    let rules = Config::get(cx).permissions.rules.clone();
    let add = |action: RuleAction| {
        let tool = tool.clone();
        let pattern = pattern.clone();
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
            let name = tool.read(cx).value().trim().to_string();
            if name.is_empty() {
                return;
            }
            let text = pattern.read(cx).value().trim().to_string();
            change(cx, |s| s.permissions.rules.push(Rule { tool: name, pattern: text, action }));
            tool.update(cx, |input, cx| input.set_value("", window, cx));
            pattern.update(cx, |input, cx| input.set_value("", window, cx));
        }
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(8.))
        .when(rules.is_empty(), |col| col.child(div().text_size(px(13.)).text_color(p.text_faint).child("No rules yet.")))
        .children(rules.into_iter().enumerate().map(|(index, rule)| {
            let tone = match rule.action {
                RuleAction::Allow => p.success,
                RuleAction::Ask => p.warning,
                RuleAction::Deny => p.danger,
            };
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(8.))
                .bg(p.fill)
                .child(
                    div()
                        .px(px(6.))
                        .rounded(px(5.))
                        .bg(tone.opacity(0.14))
                        .text_size(px(12.))
                        .text_color(tone)
                        .child(action_label(rule.action)),
                )
                .child(div().font_family(crate::ui::MONO).text_size(px(13.)).text_color(p.text).child(rule.tool.clone()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(crate::ui::MONO)
                        .text_size(px(13.))
                        .text_color(p.text_muted)
                        .child(if rule.pattern.is_empty() { "anything".to_string() } else { rule.pattern.clone() }),
                )
                .child(ghost(("remove-rule", index), Icon::X, p).on_click(move |_, _, cx| {
                    change(cx, |s| {
                        if index < s.permissions.rules.len() {
                            s.permissions.rules.remove(index);
                        }
                    })
                }))
        }))
        .child(
            div()
                .flex()
                .gap(px(8.))
                .child(div().w(px(180.)).child(Input::new(&tool)))
                .child(div().flex_1().child(Input::new(&pattern)))
                .child(secondary("add-allow", "Allow", p).on_click(add(RuleAction::Allow)))
                .child(secondary("add-ask", "Ask", p).on_click(add(RuleAction::Ask)))
                .child(secondary("add-deny", "Deny", p).on_click(add(RuleAction::Deny))),
        )
        .into_any_element()
}

fn list_item(
    key: &'static str,
    title: &str,
    description: &str,
    placeholder: &'static str,
    get: fn(&Prefs) -> Vec<String>,
    set: fn(&mut Prefs, Vec<String>),
) -> SettingItem {
    let title = title.to_string();
    let description = description.to_string();
    SettingItem::render(move |_, window, cx| list_editor(key, &title, &description, placeholder, get, set, window, cx))
        .keywords([key, placeholder])
        .on_reset(move |cx| get(Config::get(cx)) != get(&Prefs::default()), move |_, cx| change(cx, |s| set(s, get(&Prefs::default()))))
}

#[allow(clippy::too_many_arguments)]
fn list_editor(
    key: &'static str,
    title: &str,
    description: &str,
    placeholder: &'static str,
    get: fn(&Prefs) -> Vec<String>,
    set: fn(&mut Prefs, Vec<String>),
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let p = palette(cx);
    let input =
        window.use_keyed_state(key, cx, |window, cx| InputState::new(window, cx).placeholder(format!("Add, such as {placeholder}")));
    let items = get(Config::get(cx));
    let adding = input.clone();
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_sm().text_color(p.text).child(title.to_string()))
                .child(div().text_sm().text_color(p.text_faint).child(description.to_string())),
        )
        .child(div().flex().flex_wrap().gap(px(6.)).children(items.iter().enumerate().map(|(index, item)| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .pl(px(9.))
                .pr(px(3.))
                .h(px(26.))
                .rounded(px(7.))
                .bg(p.fill)
                .border_1()
                .border_color(p.border_soft)
                .font_family(crate::ui::MONO)
                .text_size(px(12.5))
                .text_color(p.text)
                .child(item.clone())
                .child(ghost((SharedString::from(format!("{key}-remove")), index), Icon::X, p).size(px(20.)).on_click(move |_, _, cx| {
                    change(cx, |s| {
                        let mut list = get(s);
                        if index < list.len() {
                            list.remove(index);
                        }
                        set(s, list);
                    })
                }))
        })))
        .child(div().flex().gap(px(8.)).child(div().flex_1().child(Input::new(&input))).child(
            secondary(SharedString::from(format!("{key}-add")), "Add", p).on_click(move |_, window, cx| {
                let value = adding.read(cx).value().trim().to_string();
                if value.is_empty() {
                    return;
                }
                change(cx, |s| {
                    let mut list = get(s);
                    if !list.contains(&value) {
                        list.push(value);
                    }
                    set(s, list);
                });
                adding.update(cx, |input, cx| input.set_value("", window, cx));
            }),
        ))
        .into_any_element()
}

fn instructions(textarea: Entity<TextareaState>, note: Entity<Option<(bool, SharedString)>>) -> SettingGroup {
    SettingGroup::new()
        .title("Personal instructions")
        .description(format!(
            "Added to every thread, in every project, after the project's own instruction files. Saved as {}",
            personal_instructions_path().display()
        ))
        .item(SettingItem::render(move |_, _, cx| {
            let p = palette(cx);
            let shown = note.read(cx).clone();
            let save_text = textarea.clone();
            let save_note = note.clone();
            div().w_full().flex().flex_col().gap(px(12.)).child(Textarea::new(&textarea)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(primary("save-instructions", "Save", p).on_click(move |_, _, cx| {
                        let text = save_text.read(cx).value().to_string();
                        let result = write_personal_instructions(&text);
                        show_note(&save_note, result.map(|_| "Saved".into()).map_err(|e| e.to_string()), cx);
                    }))
                    .when_some(shown, |row, (ok, text)| {
                        row.child(div().text_size(px(12.5)).text_color(if ok { p.success } else { p.danger }).child(text))
                    }),
            )
        }))
}

pub(crate) fn show_note(note: &Entity<Option<(bool, SharedString)>>, result: Result<String, String>, cx: &mut App) {
    let value = match result {
        Ok(text) => (true, SharedString::from(text)),
        Err(error) => (false, SharedString::from(error)),
    };
    note.update(cx, |note, cx| {
        *note = Some(value);
        cx.notify();
    });
    let note = note.clone();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(Duration::from_millis(1800)).await;
        note.update(cx, |note, cx| {
            *note = None;
            cx.notify();
        });
    })
    .detach();
}

fn projects(this: WeakEntity<Settings>) -> SettingGroup {
    SettingGroup::new().title("Projects").item(SettingItem::render(move |_, _, cx| {
        let p = palette(cx);
        let projects = Config::get(cx).general.projects.clone();
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(4.))
            .when(projects.is_empty(), |c| c.child(div().text_size(px(13.5)).text_color(p.text_faint).child("No projects yet.")))
            .children(projects.into_iter().enumerate().map(|(index, path)| {
                let name = project::name(&path);
                let open = path.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .py(px(8.))
                    .child(badge(&name, 22.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(14.)).text_color(p.text).child(name))
                            .child(div().text_size(px(12.5)).text_color(p.text_faint).text_ellipsis().child(path.display().to_string())),
                    )
                    .child(ghost(("open-project", index), Icon::FolderOpen, p).on_click(move |_, _, _| project::open_folder(&open)))
                    .child(ghost(("remove-project", index), Icon::Trash, p).on_click(move |_, _, cx| {
                        change(cx, |s| {
                            if index < s.general.projects.len() {
                                s.general.projects.remove(index);
                            }
                        })
                    }))
            }))
            .child(div().pt(px(6.)).child({
                let this = this.clone();
                primary("settings-add-project", "Add project", p).on_click(move |_, _, cx| {
                    let _ = this.update(cx, |_, cx| cx.emit(SettingsEvent::AddProject));
                })
            }))
    }))
}

fn advanced(note: Entity<Option<(bool, SharedString)>>) -> Vec<SettingGroup> {
    vec![SettingGroup::new().title("Settings file").item(SettingItem::render(move |_, _, cx| {
        let p = palette(cx);
        let error = Config::error(cx);
        let shown = note.read(cx).clone();
        let reload_note = note.clone();
        let reset_note = note.clone();
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(div().text_size(px(13.)).text_color(p.text_faint).child(format!(
                "Every setting lives in {}. A project can override any of them in .quill/settings.json at its root.",
                settings_path().display()
            )))
            .when_some(error, |col, error| {
                col.child(
                    div()
                        .px(px(12.))
                        .py(px(10.))
                        .rounded(px(10.))
                        .bg(p.danger.opacity(0.1))
                        .text_size(px(13.))
                        .text_color(p.danger)
                        .child(format!("{error}. Changes made here are not saved until the file is fixed or reset.")),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(secondary("open-settings-file", "Open the file", p).on_click(|_, _, _| project::open_file(&settings_path())))
                    .child(secondary("reload-settings", "Reload", p).on_click(move |_, _, cx| {
                        let changed = Config::reload(cx);
                        show_note(&reload_note, Ok(if changed { "Reloaded".into() } else { "Already up to date".into() }), cx);
                    }))
                    .child(button("reset-settings", "Reset to defaults", Tone::Danger, p).on_click(move |_, _, cx| {
                        let current = Config::get(cx).clone();
                        let mut fresh = Prefs::default();
                        fresh.general.projects = current.general.projects;
                        fresh.models.providers = current.models.providers;
                        fresh.models.selected = current.models.selected;
                        fresh.models.pinned = current.models.pinned;
                        Config::replace(cx, fresh);
                        show_note(&reset_note, Ok("Everything else is back to its default".into()), cx);
                    }))
                    .when_some(shown, |row, (ok, text)| {
                        row.child(div().text_size(px(12.5)).text_color(if ok { p.success } else { p.danger }).child(text))
                    }),
            )
    }))]
}

fn primary(id: &'static str, label: &'static str, p: Palette) -> gpui_kit::Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(32.))
        .px(px(14.))
        .rounded(px(8.))
        .bg(p.accent)
        .text_color(p.accent_text)
        .text_size(px(13.5))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|style| style.opacity(0.9))
        .child(label)
}

pub(crate) fn secondary(id: impl Into<gpui_kit::ElementId>, label: &'static str, p: Palette) -> gpui_kit::Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .h(px(32.))
        .px(px(12.))
        .rounded(px(8.))
        .border_1()
        .border_color(p.border)
        .text_color(p.text)
        .text_size(px(13.))
        .cursor_pointer()
        .hover(move |style| style.bg(p.row_hover))
        .child(label)
}

pub(crate) fn ghost(id: impl Into<gpui_kit::ElementId>, name: Icon, p: Palette) -> gpui_kit::Stateful<Div> {
    div()
        .id(id)
        .size(px(30.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .cursor_pointer()
        .hover(move |style| style.bg(p.row_hover))
        .child(icon(name, 14., p.text_muted))
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let page_ix = Section::ALL.iter().position(|s| *s == self.section).unwrap_or(0);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(14.))
                    .h(px(44.))
                    .border_b_1()
                    .border_color(p.border_soft)
                    .child(
                        div()
                            .id("settings-back")
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(30.))
                            .px(px(10.))
                            .rounded(px(8.))
                            .text_size(px(13.5))
                            .text_color(p.text_muted)
                            .cursor_pointer()
                            .hover(move |style| style.bg(p.row_hover).text_color(p.text))
                            .child(icon(Icon::ArrowLeft, 15., p.text_muted))
                            .child("Back to threads")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Back))),
                    )
                    .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child("Settings")),
            )
            .child(
                div().flex_1().min_h_0().child(
                    Pages::new(SharedString::from(format!("quill-settings-{}", self.opened)))
                        .sidebar_width(px(240.))
                        .default_selected_index(SelectIndex { page_ix, group_ix: None })
                        .pages(self.pages(cx.entity().downgrade(), cx)),
                ),
            )
    }
}