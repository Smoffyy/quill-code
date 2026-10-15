use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem};
use gpui_kit::{AnyElement, App, Div, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div, prelude::*, px};
use quill_config::{KeySource, ModelRef, Price, Provider, ProviderKind, forget_key, key_source, store_key};
use quill_providers::{Effort, context_label, normalize_url};

use crate::backend::{Backend, Connection, provider_icon};
use crate::config::Config;
use crate::icons::Icon;
use crate::palette::palette;
use crate::settings::{change, ghost, secondary, show_note};
use crate::ui::{Tone, button, icon};

type Note = Entity<Option<(bool, SharedString)>>;

pub fn groups(backend: Entity<Backend>, note: Note, cx: &App) -> Vec<SettingGroup> {
    let count = Config::get(cx).models.providers.len();
    let mut providers = SettingGroup::new()
        .title("Providers")
        .description("Where models come from. Every model a provider offers shows up in the prompt box, grouped by provider.");
    for index in 0..count {
        let backend = backend.clone();
        let note = note.clone();
        providers = providers.item(
            SettingItem::render(move |_, window, cx| provider_card(index, &backend, &note, window, cx))
                .keywords(["provider", "key", "server"]),
        );
    }
    let adding = backend.clone();
    providers = providers.item(SettingItem::render(move |_, _, cx| add_row(&adding, &note, cx)).keywords(["add", "detect", "find"]));
    vec![providers, defaults(), roles(&backend, cx), this_model(&backend, cx)]
}

fn provider_card(index: usize, backend: &Entity<Backend>, note: &Note, window: &mut Window, cx: &mut App) -> AnyElement {
    let p = palette(cx);
    let Some(provider) = Config::get(cx).models.providers.get(index).cloned() else { return div().into_any_element() };
    let source = backend.read(cx).source(&provider.id).cloned();
    let connection = source.as_ref().map_or(Connection::Checking, |s| s.connection.clone());
    let status = source.as_ref().map(|s| s.status()).unwrap_or_else(|| "Connecting".into());
    let color = match connection {
        Connection::Online => p.success,
        Connection::Checking | Connection::Off => p.text_faint,
        Connection::NeedsKey => p.warning,
        Connection::Offline(_) => p.danger,
    };
    let id = provider.id.clone();
    let url_value = provider.url.clone();
    let address = window.use_keyed_state(SharedString::from(format!("provider-url-{id}")), cx, move |window, cx| {
        let placeholder =
            if provider_kind(index, cx) == ProviderKind::ClaudeCode { "Found automatically" } else { quill_config::DEFAULT_SERVER };
        InputState::new(window, cx).placeholder(placeholder).default_value(url_value)
    });
    let key_input = window.use_keyed_state(SharedString::from(format!("provider-key-{id}")), cx, |window, cx| {
        let mut state = InputState::new(window, cx).placeholder("Paste an API key");
        state.set_masked(true, window, cx);
        state
    });
    let header = div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(icon(provider_icon(provider.kind), 18., p.text))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(provider.name.clone()))
                .when(provider.name != provider.kind.label(), |col| {
                    col.child(div().text_size(px(12.)).text_color(p.text_faint).child(provider.kind.label()))
                }),
        )
        .child(ghost(("provider-refresh", index), Icon::Refresh, p).tooltip(crate::ui::tip("Check again")).on_click({
            let backend = backend.clone();
            move |_, _, cx| backend.update(cx, |b, cx| b.refresh(cx))
        }))
        .child(button(("provider-toggle", index), if provider.enabled { "Turn off" } else { "Turn on" }, Tone::Secondary, p).on_click({
            let backend = backend.clone();
            move |_, _, cx| {
                change(cx, |s| {
                    if let Some(entry) = s.models.providers.get_mut(index) {
                        entry.enabled = !entry.enabled;
                    }
                });
                backend.update(cx, |b, cx| b.refresh(cx));
            }
        }))
        .child(ghost(("provider-remove", index), Icon::Trash, p).tooltip(crate::ui::tip("Remove this provider")).on_click({
            let backend = backend.clone();
            let id = id.clone();
            move |_, _, cx| {
                let _ = forget_key(&id);
                change(cx, |s| {
                    if index < s.models.providers.len() {
                        s.models.providers.remove(index);
                    }
                });
                backend.update(cx, |b, cx| b.refresh(cx));
            }
        }));
    let status_line = div()
        .flex()
        .items_start()
        .gap(px(8.))
        .text_size(px(12.5))
        .text_color(p.text_muted)
        .child(div().mt(px(5.)).size(px(7.)).flex_shrink_0().rounded_full().bg(color))
        .child(div().flex_1().min_w_0().child(status));
    let address_label = if provider.kind == ProviderKind::ClaudeCode { "Program" } else { "Address" };
    let address_row = field_row(
        address_label,
        div().flex().gap(px(8.)).child(div().flex_1().child(Input::new(&address))).child(
            secondary(("provider-connect", index), "Connect", p).on_click({
                let backend = backend.clone();
                let address = address.clone();
                move |_, _, cx| {
                    let text = address.read(cx).value().trim().to_string();
                    change(cx, |s| {
                        if let Some(entry) = s.models.providers.get_mut(index) {
                            entry.url = if entry.kind == ProviderKind::ClaudeCode || text.is_empty() { text } else { normalize_url(&text) };
                        }
                    });
                    backend.update(cx, |b, cx| b.refresh(cx));
                }
            }),
        ),
        cx,
    );
    let mut card = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(14.))
        .rounded(px(12.))
        .border_1()
        .border_color(p.border_soft)
        .bg(p.raised)
        .child(header)
        .child(status_line)
        .child(address_row);
    if provider.kind.needs_key() {
        card = card.child(key_row(index, &provider, &key_input, backend, note, cx));
    }
    if provider.kind == ProviderKind::ClaudeCode {
        card = card.child(aliases(index, &provider, window, cx));
    }
    if provider.kind == ProviderKind::Anthropic {
        let on = provider.fallback;
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.5))
                        .text_color(p.text_muted)
                        .child("When a model declines a request, let Anthropic retry it on the model it recommends."),
                )
                .child(button(("provider-fallback", index), if on { "Retrying" } else { "Not retrying" }, Tone::Secondary, p).on_click(
                    move |_, _, cx| {
                        change(cx, |s| {
                            if let Some(entry) = s.models.providers.get_mut(index) {
                                entry.fallback = !on;
                            }
                        })
                    },
                )),
        );
    }
    card.into_any_element()
}

fn provider_kind(index: usize, cx: &App) -> ProviderKind {
    Config::get(cx).models.providers.get(index).map(|p| p.kind).unwrap_or_default()
}

fn field_row(label: &'static str, content: impl IntoElement, cx: &App) -> Div {
    let p = palette(cx);
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(div().w(px(76.)).flex_shrink_0().text_size(px(12.5)).text_color(p.text_faint).child(label))
        .child(div().flex_1().min_w_0().child(content))
}

fn key_row(index: usize, provider: &Provider, input: &Entity<InputState>, backend: &Entity<Backend>, note: &Note, cx: &App) -> AnyElement {
    let p = palette(cx);
    let source = match key_source(provider) {
        KeySource::Stored => "Saved in Windows Credential Manager.".to_string(),
        KeySource::Environment(name) => format!("Using {name} from your environment."),
        KeySource::Missing => "No key yet. Keys are kept in Windows Credential Manager, never in settings.json.".to_string(),
    };
    let stored = key_source(provider) == KeySource::Stored;
    let id = provider.id.clone();
    let row = div()
        .flex()
        .gap(px(8.))
        .child(div().flex_1().child(Input::new(input).mask_toggle()))
        .child(secondary(("provider-key-save", index), "Save key", p).on_click({
            let (input, backend, note, id) = (input.clone(), backend.clone(), note.clone(), id.clone());
            move |_, window, cx| {
                let key = input.read(cx).value().trim().to_string();
                if key.is_empty() {
                    return;
                }
                let result = store_key(&id, &key).map(|_| "Key saved".to_string());
                show_note(&note, result, cx);
                input.update(cx, |input, cx| input.set_value("", window, cx));
                backend.update(cx, |b, cx| b.refresh(cx));
            }
        }))
        .when(stored, |row| {
            let (backend, note, id) = (backend.clone(), note.clone(), id.clone());
            row.child(ghost(("provider-key-forget", index), Icon::Trash, p).tooltip(crate::ui::tip("Forget the saved key")).on_click(
                move |_, _, cx| {
                    show_note(&note, forget_key(&id).map(|_| "Key removed".to_string()), cx);
                    backend.update(cx, |b, cx| b.refresh(cx));
                },
            ))
        });
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(field_row("API key", row, cx))
        .child(
            div()
                .pl(px(88.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(p.text_faint)
                .child(icon(Icon::Key, 12., p.text_faint))
                .child(source),
        )
        .into_any_element()
}

fn aliases(index: usize, provider: &Provider, window: &mut Window, cx: &mut App) -> AnyElement {
    let p = palette(cx);
    let input = window.use_keyed_state(SharedString::from(format!("provider-alias-{}", provider.id)), cx, |window, cx| {
        InputState::new(window, cx).placeholder("Add a model, such as opus or claude-sonnet-5-5")
    });
    let chips = div().flex().flex_wrap().gap(px(6.)).children(provider.models.iter().enumerate().map(|(n, alias)| {
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
            .child(alias.clone())
            .child(ghost((SharedString::from(format!("alias-remove-{index}")), n), Icon::X, p).size(px(20.)).on_click(move |_, _, cx| {
                change(cx, |s| {
                    if let Some(entry) = s.models.providers.get_mut(index)
                        && n < entry.models.len()
                    {
                        entry.models.remove(n);
                    }
                })
            }))
    }));
    let add = input.clone();
    let row = div().flex().gap(px(8.)).child(div().flex_1().child(Input::new(&input))).child(
        secondary(("alias-add", index), "Add", p).on_click(move |_, window, cx| {
            let value = add.read(cx).value().trim().to_string();
            if value.is_empty() {
                return;
            }
            change(cx, |s| {
                if let Some(entry) = s.models.providers.get_mut(index)
                    && !entry.models.contains(&value)
                {
                    entry.models.push(value);
                }
            });
            add.update(cx, |input, cx| input.set_value("", window, cx));
        }),
    );
    div().flex().flex_col().gap(px(8.)).child(field_row("Models", chips, cx)).child(div().pl(px(88.)).child(row)).into_any_element()
}

fn add_row(backend: &Entity<Backend>, note: &Note, cx: &App) -> AnyElement {
    let p = palette(cx);
    let shown = note.read(cx).clone();
    let scan = {
        let (backend, note) = (backend.clone(), note.clone());
        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
            show_note(&note, Ok("Looking on this computer".into()), cx);
            let task = backend.update(cx, |b, cx| b.scan(cx));
            let note = note.clone();
            cx.spawn(async move |cx| {
                let found = task.await;
                let text = match found {
                    0 => "No other servers answered on the usual ports".to_string(),
                    1 => "Found a server and added it".to_string(),
                    n => format!("Found {n} servers and added them"),
                };
                cx.update(|cx| show_note(&note, Ok(text), cx));
            })
            .detach();
        }
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(div().text_size(px(13.)).text_color(p.text_muted).child("Add a provider"))
        .child(div().flex().flex_wrap().gap(px(8.)).children(ProviderKind::ALL.map(|kind| {
            let backend = backend.clone();
            button(SharedString::from(format!("add-provider-{}", kind.label())), kind.label(), Tone::Secondary, p)
                .tooltip(crate::ui::tip(kind.description()))
                .on_click(move |_, _, cx| {
                    change(cx, |s| {
                        let id = s.models.fresh_id(kind);
                        let taken = s.models.providers.iter().filter(|p| p.kind == kind).count();
                        let name = if taken == 0 { kind.label().to_string() } else { format!("{} {}", kind.label(), taken + 1) };
                        s.models.providers.push(Provider::new(&id, &name, kind));
                    });
                    backend.update(cx, |b, cx| b.refresh(cx));
                })
        })))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(button("scan-servers", "Find local servers", Tone::Primary, p).on_click(scan))
                .child(div().text_size(px(12.5)).text_color(p.text_faint).child(format!(
                    "Looks on this computer at ports {}.",
                    quill_config::LOCAL_PORTS.map(|port| port.to_string()).join(", ")
                )))
                .when_some(shown, |row, (ok, text)| {
                    row.child(div().text_size(px(12.5)).text_color(if ok { p.success } else { p.danger }).child(text))
                }),
        )
        .into_any_element()
}

fn defaults() -> SettingGroup {
    let options = Effort::ALL.iter().map(|e| (SharedString::from(e.wire()), SharedString::from(e.label()))).collect();
    SettingGroup::new()
        .title("Reasoning")
        .item(
            SettingItem::new(
                "Effort",
                SettingField::dropdown(
                    options,
                    |cx| Config::get(cx).models.effort.wire().into(),
                    |value, cx| {
                        if let Some(effort) = Effort::ALL.into_iter().find(|e| e.wire() == value.as_ref()) {
                            change(cx, |s| s.models.effort = effort);
                        }
                    },
                )
                .default_value(Effort::High.wire()),
            )
            .description(
                "How hard Claude, OpenAI reasoning models and Claude Code think. A model that lacks a level uses the nearest one.",
            ),
        )
        .item(
            SettingItem::new(
                "Thinking on local models",
                SettingField::switch(|cx| Config::get(cx).models.thinking, |on, cx| change(cx, |s| s.models.thinking = on))
                    .default_value(true),
            )
            .description("Let local models whose chat template supports it think before they answer."),
        )
}

fn encode(model: &ModelRef) -> SharedString {
    format!("{}\t{}", model.provider, model.model).into()
}

fn decode(value: &str) -> Option<ModelRef> {
    value.split_once('\t').map(|(provider, model)| ModelRef::new(provider, model))
}

fn role_item(
    title: &'static str,
    description: &'static str,
    options: Vec<(SharedString, SharedString)>,
    get: fn(&quill_config::Roles) -> Option<ModelRef>,
    set: fn(&mut quill_config::Roles, Option<ModelRef>),
) -> SettingItem {
    SettingItem::new(
        title,
        SettingField::dropdown(
            options,
            move |cx| get(&Config::get(cx).models.roles).map(|m| encode(&m)).unwrap_or_default(),
            move |value, cx| change(cx, |s| set(&mut s.models.roles, decode(&value))),
        )
        .default_value(""),
    )
    .description(description)
}

fn roles(backend: &Entity<Backend>, cx: &App) -> SettingGroup {
    let mut options = vec![(SharedString::default(), SharedString::from("Same as the chat"))];
    for source in backend.read(cx).sources.iter().filter(|s| s.ready()) {
        for model in &source.models {
            let target = ModelRef::new(source.config.id.clone(), model.id.clone());
            options.push((encode(&target), format!("{} · {}", source.config.name, model.label()).into()));
        }
    }
    SettingGroup::new()
        .title("Models for side jobs")
        .item(role_item(
            "Summaries",
            "Writes the summary when a long thread is compacted. A small, fast model works well.",
            options.clone(),
            |roles| roles.compaction.clone(),
            |roles, model| roles.compaction = model,
        ))
        .item(role_item(
            "Commit messages",
            "Writes commit messages when you leave the message empty.",
            options,
            |roles| roles.commit.clone(),
            |roles, model| roles.commit = model,
        ))
}

fn this_model(backend: &Entity<Backend>, cx: &App) -> SettingGroup {
    let Some(model) = Backend::selected_ref(cx) else {
        return SettingGroup::new().title("The model in use").description("Choose a model in the prompt box to adjust it here.");
    };
    let detected = backend.read(cx).find(&model).map(|(_, m)| m.clone());
    let label = detected.as_ref().map_or(model.model.clone(), |m| m.label().to_string());
    let window = detected.as_ref().and_then(|m| m.context).map_or("not reported".to_string(), context_label);
    let number = |title: &'static str,
                  description: String,
                  step: f64,
                  get: fn(&quill_config::Override) -> Option<u64>,
                  set: fn(&mut quill_config::Override, Option<u64>)| {
        let (read, write) = (model.clone(), model.clone());
        SettingItem::new(
            title,
            SettingField::number_input(
                NumberFieldOptions { min: 0., max: 10_000_000., step },
                move |cx| Config::get(cx).models.override_for(&read).and_then(get).unwrap_or(0) as f64,
                move |value, cx| change(cx, |s| set(s.models.override_mut(&write), (value >= 1.).then_some(value as u64))),
            )
            .default_value(0.),
        )
        .description(description)
    };
    let (read, write) = (model.clone(), model.clone());
    SettingGroup::new()
        .title(format!("The model in use: {label}"))
        .description("Fill in what the provider does not report. Zero keeps the detected value.")
        .item(number(
            "Context window",
            format!("Tokens the model can read at once. Detected: {window}."),
            1024.,
            |o| o.context,
            |o, v| o.context = v,
        ))
        .item(number(
            "Longest reply",
            "Tokens one reply may use. Zero uses the provider's default.".into(),
            1024.,
            |o| o.max_output,
            |o, v| o.max_output = v,
        ))
        .item(
            SettingItem::new(
                "Images",
                SettingField::dropdown(
                    vec![
                        ("detected".into(), "As detected".into()),
                        ("yes".into(), "Accepts images".into()),
                        ("no".into(), "Text only".into()),
                    ],
                    move |cx| match Config::get(cx).models.override_for(&read).and_then(|o| o.vision) {
                        Some(true) => "yes".into(),
                        Some(false) => "no".into(),
                        None => "detected".into(),
                    },
                    move |value, cx| {
                        let vision = match value.as_ref() {
                            "yes" => Some(true),
                            "no" => Some(false),
                            _ => None,
                        };
                        change(cx, |s| s.models.override_mut(&write).vision = vision);
                    },
                )
                .default_value("detected"),
            )
            .description("Whether pictures you attach are sent to this model."),
        )
        .item(price_item("Input price", &model, false))
        .item(price_item("Output price", &model, true))
}

fn price_item(title: &'static str, model: &ModelRef, output: bool) -> SettingItem {
    let (read, write) = (model.clone(), model.clone());
    SettingItem::new(
        title,
        SettingField::number_input(
            NumberFieldOptions { min: 0., max: 1000., step: 0.05 },
            move |cx| {
                let price = Config::get(cx).models.override_for(&read).and_then(|o| o.price);
                price.map_or(0., |price| if output { price.output } else { price.input })
            },
            move |value, cx| {
                change(cx, |s| {
                    let entry = s.models.override_mut(&write);
                    let current = entry.price.unwrap_or(Price::new(0., 0.));
                    let (input, out) = if output { (current.input, value) } else { (value, current.output) };
                    entry.price = (input > 0. || out > 0.).then(|| Price::new(input, out));
                })
            },
        )
        .default_value(0.),
    )
    .description(if output {
        "Dollars per million tokens the model writes."
    } else {
        "Dollars per million tokens sent, used to work out what a chat costs. Cached input counts at a tenth. Zero uses the published price for Anthropic models, and no price otherwise."
    })
}