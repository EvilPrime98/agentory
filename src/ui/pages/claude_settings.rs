use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Subscription, Window, div, point, prelude::*, px,
};
use serde_json::{Map, Value};

use super::skills::icon_button;
use crate::backend::claude_settings::{self as store, Catalog, Kind, Setting, Shape};
use crate::i18n::{t, t_args};
use crate::ui::components::button::small_button;
use crate::ui::components::checkbox::checkbox;
use crate::ui::components::dropdown::dropdown;
use crate::ui::components::scrollbar::Scrollbar;
use crate::ui::components::segmented::segmented;
use crate::ui::components::text_area::{TextArea, TextAreaEvent};
use crate::ui::components::text_input::{TextInput, TextInputEvent};
use crate::ui::confirm::{self, ConfirmOptions};
use crate::ui::icons::{Icon, icon};
use crate::ui::theme;
use crate::ui::toast;

const OTHER_TOPIC: &str = "other";
const SEARCH_CAP: usize = 60;
const LIST_PREVIEW: usize = 20;
const ENV_MATCHES: usize = 30;
const AREA_MAX_ROWS: usize = 12;
const RAW_CHARS: usize = 400;
const POLL: Duration = Duration::from_secs(1);
const MASK: &str = "••••••••";

enum Input {
    Text,
    Number { integer: bool },
    Json(Vec<Shape>),
    NewItem,
}

#[derive(Debug, PartialEq)]
enum Invalid {
    Number,
    Integer,
    Json(String),
    Shape,
}

impl Invalid {
    fn message(&self) -> String {
        match self {
            Self::Number => t("claude_settings.error.number"),
            Self::Integer => t("claude_settings.error.integer"),
            Self::Json(e) => t_args("claude_settings.error.json", &[("error", e)]),
            Self::Shape => t("claude_settings.error.shape"),
        }
    }
}

impl Input {
    fn show(&self, value: Option<&Value>) -> String {
        match (self, value) {
            (Self::NewItem, _) | (_, None) => String::new(),
            (Self::Text, Some(Value::String(s))) => s.clone(),
            (Self::Json(_), Some(v)) => pretty(v),
            (_, Some(v)) => v.to_string(),
        }
    }

    fn read(&self, text: &str) -> Result<Value, Invalid> {
        match self {
            Self::Text => Ok(text.into()),
            Self::NewItem => Ok(text.trim().into()),
            Self::Number { integer } => match serde_json::from_str::<Value>(text.trim()) {
                Ok(v) if v.is_number() && *integer && v.is_f64() => Err(Invalid::Integer),
                Ok(v) if v.is_number() => Ok(v),
                _ => Err(Invalid::Number),
            },
            Self::Json(shapes) => {
                let value = serde_json::from_str(text).map_err(|e| Invalid::Json(e.to_string()))?;
                if store::fits(shapes, &value) {
                    Ok(value)
                } else {
                    Err(Invalid::Shape)
                }
            }
        }
    }
}

enum Editor {
    Line(Entity<TextInput>),
    Area(Entity<TextArea>),
}

impl Editor {
    fn value(&self, cx: &gpui::App) -> String {
        match self {
            Self::Line(input) => input.read(cx).value().to_string(),
            Self::Area(area) => area.read(cx).value(),
        }
    }

    fn set(&self, text: &str, cx: &mut gpui::App) {
        match self {
            Self::Line(input) => input.update(cx, |input, cx| input.set_value(text, cx)),
            Self::Area(area) => area.update(cx, |area, cx| area.set_value(text, cx)),
        }
    }
}

struct Field {
    path: Vec<String>,
    input: Input,
    editor: Editor,
    file_text: String,
    edited: bool,
    error: Option<Invalid>,
    _subs: Vec<Subscription>,
}

enum Row {
    Known(&'static Setting),
    Other(Vec<String>),
}

fn rows(
    catalog: &'static Catalog,
    values: &Map<String, Value>,
    topic: &str,
    query: &str,
    only_set: bool,
) -> Vec<(&'static str, Row)> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let everywhere = !terms.is_empty() || only_set;
    let matches = |text: String| {
        let text = text.to_lowercase();
        terms.iter().all(|term| text.contains(term))
    };
    let mut found = Vec::new();
    for id in &catalog.topics {
        if !everywhere && id != topic {
            continue;
        }
        for setting in catalog.settings.iter().filter(|s| s.topic == *id) {
            let set = store::get(values, &setting.path()).is_some();
            let listed = set || !(setting.deprecated || only_set);
            if listed && matches(format!("{} {}", setting.key, setting.description)) {
                found.push((id.as_str(), Row::Known(setting)));
            }
        }
    }
    if everywhere || topic == OTHER_TOPIC {
        for path in catalog.unknown_keys(values) {
            if matches(path.join(".")) {
                found.push((OTHER_TOPIC, Row::Other(path)));
            }
        }
    }
    found
}

fn field_id(key: &str) -> String {
    format!("key:{key}")
}

fn add_id(key: &str) -> String {
    format!("add:{key}")
}

fn env_id(name: &str) -> String {
    format!("env:{name}")
}

fn other_id(path: &[String]) -> String {
    format!("other:{}", path.join("\u{1}"))
}

fn env_path(name: &str) -> Vec<String> {
    vec!["env".to_string(), name.to_string()]
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

fn raw(value: &Value) -> String {
    let text = value.to_string();
    match text.char_indices().nth(RAW_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text,
    }
}

fn first_topic() -> &'static str {
    store::catalog()
        .topics
        .first()
        .map_or(OTHER_TOPIC, String::as_str)
}

fn topic_label(id: &str) -> String {
    t(&format!("claude_settings.topic.{id}"))
}

fn note(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.0))
        .text_color(theme::text_muted())
        .child(text.into())
}

fn warning(text: impl Into<SharedString>) -> Div {
    note(text).text_color(theme::warning())
}

fn error_line(text: impl Into<SharedString>) -> Div {
    note(text).text_color(theme::error())
}

fn badge(text: String, color: gpui::Rgba) -> Div {
    div()
        .flex_none()
        .px(px(6.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(color)
        .text_size(px(10.5))
        .text_color(color)
        .child(text)
}

fn box_frame() -> Div {
    div()
        .flex()
        .px(px(10.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(theme::border_subtle())
        .bg(theme::bg_app())
}

fn code_block(text: String) -> Div {
    box_frame()
        .py(px(8.0))
        .overflow_hidden()
        .text_size(px(12.0))
        .text_color(theme::text_secondary())
        .child(div().flex_1().min_w_0().child(text))
}

fn editor_box(field: &Field, min_rows: usize, cx: &gpui::App) -> Div {
    match &field.editor {
        Editor::Line(input) => box_frame().h(px(32.0)).child(input.clone()),
        Editor::Area(area) => {
            let rows = area.read(cx).rows().clamp(min_rows, AREA_MAX_ROWS);
            box_frame()
                .h(px(TextArea::height_for_rows(rows) + 2.0))
                .child(area.clone())
        }
    }
}

fn compact(button: impl IntoElement) -> Div {
    div().flex().child(button)
}

pub struct ClaudeSettingsPage {
    file: Option<PathBuf>,
    text: Result<Option<String>, String>,
    values: Result<Map<String, Value>, String>,
    topic: &'static str,
    only_set: bool,
    search: Entity<TextInput>,
    env_name: Entity<TextInput>,
    env_value: Entity<TextInput>,
    env_error_key: Option<&'static str>,
    fields: HashMap<String, Field>,
    open: HashSet<String>,
    menu: Option<&'static str>,
    topics_bar: Scrollbar,
    rows_bar: Scrollbar,
    _subs: Vec<Subscription>,
}

impl ClaudeSettingsPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new("", cx));
        let env_name = cx.new(|cx| TextInput::new("", cx));
        let env_value = cx.new(|cx| TextInput::new("", cx));
        let mut subs = vec![
            cx.subscribe(&search, |this, search, ev: &TextInputEvent, cx| {
                if matches!(ev, TextInputEvent::Cancel) {
                    search.update(cx, |input, cx| input.set_value("", cx));
                }
                this.show_top();
                cx.notify();
            }),
        ];
        for input in [&env_name, &env_value] {
            subs.push(cx.subscribe(input, |this, _, ev: &TextInputEvent, cx| {
                match ev {
                    TextInputEvent::Submit => this.add_env(cx),
                    TextInputEvent::Changed | TextInputEvent::Cancel => this.env_error_key = None,
                }
                cx.notify();
            }));
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut this = Self {
            file: store::path(),
            text: Ok(None),
            values: Ok(Map::new()),
            topic: first_topic(),
            only_set: false,
            search,
            env_name,
            env_value,
            env_error_key: None,
            fields: HashMap::new(),
            open: HashSet::new(),
            menu: None,
            topics_bar: Scrollbar::new(),
            rows_bar: Scrollbar::new(),
            _subs: subs,
        };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let text = match &self.file {
            Some(file) => store::read_text(file),
            None => Err("no home directory".to_string()),
        };
        if text == self.text {
            return;
        }
        self.values = text
            .clone()
            .and_then(|text| store::parse(text.as_deref().unwrap_or_default()));
        self.text = text;
        self.sync_fields(cx);
        cx.notify();
    }

    fn sync_fields(&mut self, cx: &mut Context<Self>) {
        let Ok(values) = &self.values else { return };
        for field in self.fields.values_mut() {
            if matches!(field.input, Input::NewItem) {
                continue;
            }
            field.file_text = field.input.show(store::get(values, &field.path));
            if !field.edited && field.editor.value(cx) != field.file_text {
                field.editor.set(&field.file_text, cx);
            }
        }
        self.fields.retain(|_, f| {
            f.edited || !matches!(f.editor, Editor::Line(_)) || !f.file_text.contains('\n')
        });
    }

    fn value(&self, path: &[String]) -> Option<&Value> {
        store::get(self.values.as_ref().ok()?, path)
    }

    fn ensure_field(
        &mut self,
        id: String,
        path: Vec<String>,
        input: Input,
        multiline: bool,
        cx: &mut Context<Self>,
    ) {
        if self.fields.contains_key(&id) {
            return;
        }
        let file_text = input.show(self.value(&path));
        let json = matches!(input, Input::Json(_));
        let key = id.clone();
        let (editor, subs) = if multiline || json || file_text.contains('\n') {
            let area = cx.new(|cx| {
                let area = TextArea::new(cx).with_value(&file_text);
                if json { area.with_line_numbers() } else { area }
            });
            let subs = vec![
                cx.observe(&area, |_, _, cx| cx.notify()),
                cx.subscribe(&area, move |this, _, ev: &TextAreaEvent, cx| match ev {
                    TextAreaEvent::Changed => this.mark_edited(&key, cx),
                    TextAreaEvent::Save | TextAreaEvent::Submit => this.save(&key, cx),
                }),
            ];
            (Editor::Area(area), subs)
        } else {
            let line = cx.new(|cx| TextInput::new("", cx).with_value(file_text.clone()));
            let subs =
                vec![
                    cx.subscribe(&line, move |this, _, ev: &TextInputEvent, cx| match ev {
                        TextInputEvent::Changed => this.mark_edited(&key, cx),
                        TextInputEvent::Submit => this.save(&key, cx),
                        TextInputEvent::Cancel => {}
                    }),
                ];
            (Editor::Line(line), subs)
        };
        self.fields.insert(
            id,
            Field {
                path,
                input,
                editor,
                file_text,
                edited: false,
                error: None,
                _subs: subs,
            },
        );
    }

    fn prepare(&mut self, row: &Row, cx: &mut Context<Self>) {
        let setting = match row {
            Row::Known(setting) => *setting,
            Row::Other(path) => {
                let input = Input::Json(Vec::new());
                return self.ensure_field(other_id(path), path.clone(), input, true, cx);
            }
        };
        let (key, path) = (setting.key.as_str(), setting.path());
        match setting.kind {
            Kind::Bool | Kind::Enum => {}
            Kind::String => self.ensure_field(field_id(key), path, Input::Text, false, cx),
            Kind::Multiline => self.ensure_field(field_id(key), path, Input::Text, true, cx),
            Kind::Number => {
                let input = Input::Number {
                    integer: setting.integer,
                };
                self.ensure_field(field_id(key), path, input, false, cx)
            }
            Kind::List => self.ensure_field(add_id(key), path, Input::NewItem, false, cx),
            Kind::Json | Kind::Object => {
                if self.shows_json(setting) {
                    let input = Input::Json(setting.shapes.clone());
                    self.ensure_field(field_id(key), path, input, true, cx)
                }
            }
            Kind::Env => {
                let names: Vec<String> = self
                    .value(&path)
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                    .filter(|(_, v)| v.is_string())
                    .map(|(name, _)| name.clone())
                    .collect();
                for name in names {
                    let path = env_path(&name);
                    self.ensure_field(env_id(&name), path, Input::Text, false, cx);
                }
            }
        }
    }

    fn shows_json(&self, setting: &Setting) -> bool {
        setting.kind == Kind::Json
            || self.open.contains(&format!("json:{}", setting.key))
            || self
                .value(&setting.path())
                .is_some_and(|v| !setting.accepts(v))
    }

    fn mark_edited(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(field) = self.fields.get_mut(id) {
            field.edited = true;
            field.error = None;
            cx.notify();
        }
    }

    fn write(
        &mut self,
        path: &[String],
        change: impl FnOnce(Option<&Value>) -> Result<Option<Value>, String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(file) = &self.file else { return false };
        let result = store::update(file, path, change);
        if let Err(e) = &result {
            toast::error(cx, t_args("claude_settings.error.write", &[("error", e)]));
        }
        self.reload(cx);
        cx.notify();
        result.is_ok()
    }

    fn put(&mut self, path: &[String], value: Option<Value>, cx: &mut Context<Self>) {
        if self.write(path, |_| Ok(value), cx) {
            for field in self.fields.values_mut() {
                if field.path.starts_with(path) && !matches!(field.input, Input::NewItem) {
                    field.edited = false;
                    field.error = None;
                }
            }
            self.sync_fields(cx);
        }
    }

    fn save(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(field) = self.fields.get(id) else {
            return;
        };
        if matches!(field.input, Input::NewItem) {
            return self.add_item(id, cx);
        }
        if !self.dirty(field, cx) {
            return;
        }
        let path = field.path.clone();
        match field.input.read(&field.editor.value(cx)) {
            Ok(value) => self.put(&path, Some(value), cx),
            Err(e) => {
                if let Some(field) = self.fields.get_mut(id) {
                    field.error = Some(e);
                }
                cx.notify();
            }
        }
    }

    fn dirty(&self, field: &Field, cx: &gpui::App) -> bool {
        field.edited
            && (self.value(&field.path).is_none() || field.editor.value(cx) != field.file_text)
    }

    fn add_item(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(field) = self.fields.get(id) else {
            return;
        };
        let item = field.editor.value(cx).trim().to_string();
        if item.is_empty() {
            return;
        }
        let path = field.path.clone();
        let added = self.write(
            &path,
            |current| {
                let mut items = match current {
                    None => Vec::new(),
                    Some(Value::Array(items)) => items.clone(),
                    Some(_) => return Err(t("claude_settings.warning.type")),
                };
                items.push(item.into());
                Ok(Some(items.into()))
            },
            cx,
        );
        if added && let Some(field) = self.fields.get(id) {
            field.editor.set("", cx);
        }
    }

    fn remove_item(&mut self, path: &[String], at: usize, item: Value, cx: &mut Context<Self>) {
        self.write(
            path,
            |current| {
                let Some(Value::Array(items)) = current else {
                    return Ok(current.cloned());
                };
                let mut items = items.clone();
                let found = if items.get(at) == Some(&item) {
                    Some(at)
                } else {
                    items.iter().position(|other| *other == item)
                };
                if let Some(found) = found {
                    items.remove(found);
                }
                Ok(Some(items.into()))
            },
            cx,
        );
    }

    fn reset(&mut self, path: Vec<String>, cx: &mut Context<Self>) {
        let many = match self.value(&path) {
            Some(Value::Array(items)) => !items.is_empty(),
            Some(Value::Object(map)) => !map.is_empty(),
            _ => false,
        };
        if !many {
            return self.put(&path, None, cx);
        }
        let key = path.join(".");
        let options = ConfirmOptions::new(
            t("claude_settings.reset.title"),
            t_args("claude_settings.reset.body", &[("key", &key)]),
        )
        .labels(t("claude_settings.reset"), t("common.cancel"));
        let page = cx.entity();
        confirm::ask(cx, options, move |answer, _, cx| {
            if answer == Some(true) {
                page.update(cx, |this, cx| this.put(&path, None, cx));
            }
        });
    }

    fn add_env(&mut self, cx: &mut Context<Self>) {
        let name = self.env_name.read(cx).value().trim().to_string();
        let value = self.env_value.read(cx).value().to_string();
        let path = env_path(&name);
        if name.is_empty() || name.contains(|c: char| c.is_whitespace() || c == '=') {
            self.env_error_key = Some("claude_settings.env.error_name");
        } else if self.value(&path).is_some() {
            self.env_error_key = Some("claude_settings.env.error_exists");
        } else {
            let added = self.write(
                &path,
                |current| match current {
                    Some(_) => Err(t("claude_settings.env.error_exists")),
                    None => Ok(Some(value.into())),
                },
                cx,
            );
            if added {
                for input in [&self.env_name, &self.env_value] {
                    input.update(cx, |input, cx| input.set_value("", cx));
                }
            }
        }
        cx.notify();
    }

    fn show_top(&mut self) {
        self.menu = None;
        self.rows_bar.handle().set_offset(point(px(0.0), px(0.0)));
    }

    fn toggle(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.open.remove(&id) {
            self.open.insert(id);
        }
        cx.notify();
    }

    fn toggle_button(&self, id: String, closed: &str, opened: &str, cx: &mut Context<Self>) -> Div {
        let label = if self.open.contains(&id) {
            t(opened)
        } else {
            t(closed)
        };
        compact(small_button(
            SharedString::from(id.clone()),
            label,
            cx.listener(move |this, _, _, cx| this.toggle(id.clone(), cx)),
        ))
    }

    fn save_button(&self, id: &str, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let field = self.fields.get(id)?;
        let id = id.to_string();
        self.dirty(field, cx).then(|| {
            small_button(
                SharedString::from(format!("save-{id}")),
                t("common.save"),
                cx.listener(move |this, _, _, cx| this.save(&id, cx)),
            )
        })
    }

    fn field_error(&self, id: &str) -> Option<Div> {
        let error = self.fields.get(id)?.error.as_ref()?;
        Some(error_line(error.message()))
    }

    fn bool_control(&self, setting: &'static Setting, cx: &mut Context<Self>) -> AnyElement {
        let path = setting.path();
        let selected = match self.value(&path).and_then(Value::as_bool) {
            Some(false) => 0,
            Some(true) => 1,
            None => usize::MAX,
        };
        let page = cx.entity();
        segmented(
            format!("bool-{}", setting.key),
            vec![
                t("claude_settings.off").into(),
                t("claude_settings.on").into(),
            ],
            selected,
            move |i, _, cx| page.update(cx, |this, cx| this.put(&path, Some((i == 1).into()), cx)),
        )
        .w(px(130.0))
        .into_any_element()
    }

    fn enum_control(&self, setting: &'static Setting, cx: &mut Context<Self>) -> AnyElement {
        let path = setting.path();
        let mut labels: Vec<SharedString> = vec![t("claude_settings.not_set").into()];
        labels.extend(
            setting
                .values
                .iter()
                .map(|v| SharedString::from(v.as_str())),
        );
        let selected = match self.value(&path) {
            None => 0,
            Some(value) => match setting
                .values
                .iter()
                .position(|v| value.as_str() == Some(v))
            {
                Some(i) => i + 1,
                None => {
                    labels.push(raw(value).into());
                    labels.len() - 1
                }
            },
        };
        let key = setting.key.as_str();
        let (toggle, select) = (cx.entity(), cx.entity());
        dropdown(
            format!("enum-{key}"),
            labels,
            selected,
            self.menu == Some(key),
            move |open, _, cx| {
                toggle.update(cx, |this, cx| {
                    this.menu = open.then_some(key);
                    cx.notify();
                })
            },
            move |i, _, cx| {
                select.update(cx, |this, cx| {
                    this.menu = None;
                    match i.checked_sub(1) {
                        None => this.put(&path, None, cx),
                        Some(i) => {
                            if let Some(value) = setting.values.get(i) {
                                this.put(&path, Some(value.as_str().into()), cx);
                            }
                        }
                    }
                    cx.notify();
                })
            },
        )
        .w(px(210.0))
        .into_any_element()
    }

    fn text_body(&self, setting: &'static Setting, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let id = field_id(&setting.key);
        let Some(field) = self.fields.get(&id) else {
            return Vec::new();
        };
        if let Editor::Line(input) = &field.editor {
            input.update(cx, |input, _| {
                input.set_placeholder(t("claude_settings.not_set"))
            });
        }
        let multiline = matches!(field.editor, Editor::Area(_));
        let input = editor_box(field, 2, cx);
        let save = self.save_button(&id, cx);
        let mut body = vec![if multiline {
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(input)
                .children(save.map(compact))
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(input.flex_1().min_w_0())
                .children(save)
                .into_any_element()
        }];
        body.extend(self.field_error(&id).map(IntoElement::into_any_element));
        if !setting.suggestions.is_empty() {
            let current = self.value(&setting.path()).and_then(Value::as_str);
            let chips = setting.suggestions.iter().map(|suggestion| {
                let path = setting.path();
                small_button(
                    SharedString::from(format!("suggest-{}-{suggestion}", setting.key)),
                    suggestion.clone(),
                    cx.listener(move |this, _, _, cx| {
                        this.put(&path, Some(suggestion.as_str().into()), cx)
                    }),
                )
                .when(current == Some(suggestion), |s| {
                    s.border_color(theme::accent()).text_color(theme::accent())
                })
            });
            body.push(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.0))
                    .children(chips)
                    .into_any_element(),
            );
        }
        body
    }

    fn list_body(&self, setting: &'static Setting, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let path = setting.path();
        let items = match self.value(&path) {
            None => &[][..],
            Some(value) if setting.accepts(value) => {
                value.as_array().map_or(&[][..], Vec::as_slice)
            }
            Some(_) => return Vec::new(),
        };
        let all_id = format!("all:{}", setting.key);
        let shown = if self.open.contains(&all_id) {
            items.len()
        } else {
            LIST_PREVIEW
        };
        let mut body: Vec<AnyElement> = items
            .iter()
            .take(shown)
            .enumerate()
            .map(|(i, item)| {
                let (path, item) = (path.clone(), item.clone());
                let text = item.as_str().unwrap_or_default().to_string();
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(px(12.5))
                            .child(text),
                    )
                    .child(icon_button(
                        SharedString::from(format!("remove-{}-{i}", setting.key)),
                        Icon::Close,
                        cx.listener(move |this, _, _, cx| {
                            this.remove_item(&path, i, item.clone(), cx)
                        }),
                    ))
                    .into_any_element()
            })
            .collect();
        if items.len() > LIST_PREVIEW {
            let button = self.toggle_button(
                all_id,
                "claude_settings.list.show_all",
                "claude_settings.list.show_less",
                cx,
            );
            body.push(
                button
                    .items_center()
                    .gap(px(8.0))
                    .child(note(items.len().to_string()))
                    .into_any_element(),
            );
        }
        let id = add_id(&setting.key);
        if let Some(field) = self.fields.get(&id) {
            if let Editor::Line(input) = &field.editor {
                input.update(cx, |input, _| {
                    input.set_placeholder(t("claude_settings.list.placeholder"))
                });
            }
            body.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(editor_box(field, 1, cx).flex_1().min_w_0())
                    .child(small_button(
                        SharedString::from(id.clone()),
                        t("claude_settings.add"),
                        cx.listener(move |this, _, _, cx| this.add_item(&id, cx)),
                    ))
                    .into_any_element(),
            );
        }
        body
    }

    fn json_body(
        &self,
        id: &str,
        example: Option<&Value>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(field) = self.fields.get(id) else {
            return Vec::new();
        };
        let mut body = vec![editor_box(field, 3, cx).into_any_element()];
        body.extend(self.field_error(id).map(IntoElement::into_any_element));
        let save = self.save_button(id, cx);
        let example_id = format!("example:{id}");
        let toggle = example.map(|_| {
            self.toggle_button(
                example_id.clone(),
                "claude_settings.show_example",
                "claude_settings.hide_example",
                cx,
            )
        });
        if save.is_some() || toggle.is_some() {
            body.push(
                div()
                    .flex()
                    .gap(px(8.0))
                    .children(save)
                    .children(toggle)
                    .into_any_element(),
            );
        }
        if let Some(example) = example.filter(|_| self.open.contains(&example_id)) {
            body.push(code_block(pretty(example)).into_any_element());
        }
        body
    }

    fn env_body(&self, setting: &'static Setting, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let set = match self.value(&setting.path()) {
            None => None,
            Some(Value::Object(map)) => Some(map),
            Some(_) => return Vec::new(),
        };
        let mut body: Vec<AnyElement> = set
            .into_iter()
            .flatten()
            .map(|(name, value)| self.env_row(name, value, cx))
            .collect();
        body.push(self.env_add(set, cx));
        body
    }

    fn env_row(&self, name: &str, value: &Value, cx: &mut Context<Self>) -> AnyElement {
        let id = env_id(name);
        let secret = store::is_secret(name);
        let reveal_id = format!("reveal:{name}");
        let hidden = secret && !self.open.contains(&reveal_id);
        let reveal = secret.then(|| {
            self.toggle_button(
                reveal_id,
                "claude_settings.env.reveal",
                "claude_settings.env.hide",
                cx,
            )
        });
        let path = env_path(name);
        let editor = if !value.is_string() {
            let shown = if hidden { MASK.to_string() } else { raw(value) };
            Some(warning(t_args(
                "claude_settings.warning.value",
                &[("value", &shown)],
            )))
        } else if hidden {
            Some(
                box_frame()
                    .h(px(32.0))
                    .items_center()
                    .text_color(theme::text_muted())
                    .child(MASK),
            )
        } else {
            self.fields.get(&id).map(|field| editor_box(field, 1, cx))
        };
        let description = store::catalog()
            .env_var(name)
            .map(|v| note(v.description.clone()));
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(name.to_string()),
                    )
                    .children(self.save_button(&id, cx))
                    .children(reveal)
                    .child(icon_button(
                        SharedString::from(format!("remove-{id}")),
                        Icon::Close,
                        cx.listener(move |this, _, _, cx| this.put(&path, None, cx)),
                    )),
            )
            .children(description)
            .children(editor)
            .children(self.field_error(&id))
            .into_any_element()
    }

    fn env_add(&self, set: Option<&Map<String, Value>>, cx: &mut Context<Self>) -> AnyElement {
        let typed = self.env_name.read(cx).value().trim().to_string();
        self.env_name.update(cx, |input, _| {
            input.set_placeholder(t("claude_settings.env.name"))
        });
        self.env_value.update(cx, |input, _| {
            input.set_placeholder(t("claude_settings.env.value"))
        });
        let catalog = store::catalog();
        let exact = catalog.env_var(&typed);
        let needle = typed.to_ascii_uppercase();
        let matches: Vec<_> = catalog
            .env
            .iter()
            .filter(|_| exact.is_none() && !needle.is_empty())
            .filter(|v| v.name.to_ascii_uppercase().contains(&needle))
            .filter(|v| set.is_none_or(|set| !set.contains_key(&v.name)))
            .take(ENV_MATCHES)
            .collect();
        let suggestions = (!matches.is_empty()).then(|| {
            div()
                .id("env-matches")
                .flex()
                .flex_col()
                .max_h(px(220.0))
                .overflow_y_scroll()
                .p(px(4.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(theme::border_subtle())
                .bg(theme::bg_modal())
                .children(matches.into_iter().map(|variable| {
                    let name = variable.name.as_str();
                    div()
                        .id(SharedString::from(format!("env-match-{name}")))
                        .flex()
                        .flex_col()
                        .flex_none()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::hover()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.env_name
                                .update(cx, |input, cx| input.set_value(name, cx));
                            this.env_error_key = None;
                            cx.notify();
                        }))
                        .child(div().text_size(px(12.5)).child(name))
                        .child(note(variable.description.as_str()))
                }))
        });
        let field = |input: &Entity<TextInput>| box_frame().h(px(32.0)).child(input.clone());
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .pt(px(8.0))
            .border_t_1()
            .border_color(theme::border_subtle())
            .child(note(t("claude_settings.env.add_title")))
            .child(field(&self.env_name))
            .children(suggestions)
            .children(exact.map(|v| note(v.description.clone())))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(field(&self.env_value).flex_1().min_w_0())
                    .child(small_button(
                        "env-add",
                        t("claude_settings.add"),
                        cx.listener(|this, _, _, cx| this.add_env(cx)),
                    )),
            )
            .children(self.env_error_key.map(|key| error_line(t(key))))
            .into_any_element()
    }

    fn row_head(
        &self,
        path: Vec<String>,
        description: String,
        meta: String,
        deprecated: bool,
        control: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> Div {
        let key = path.join(".");
        let set = self.value(&path).is_some();
        let state = if set {
            badge(t("claude_settings.set"), theme::accent())
        } else {
            badge(t("claude_settings.not_set"), theme::text_faint())
        };
        let reset = set.then(|| {
            small_button(
                SharedString::from(format!("reset-{key}")),
                t("claude_settings.reset"),
                cx.listener(move |this, _, _, cx| this.reset(path.clone(), cx)),
            )
        });
        div()
            .flex()
            .items_start()
            .justify_between()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.0))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(key),
                            )
                            .child(state)
                            .when(deprecated, |s| {
                                s.child(badge(t("claude_settings.deprecated"), theme::warning()))
                            }),
                    )
                    .child(note(description))
                    .when(!meta.is_empty(), |s| {
                        s.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme::text_faint())
                                .child(meta),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.0))
                    .children(control)
                    .children(reset),
            )
    }

    fn row(&self, row: &Row, cx: &mut Context<Self>) -> Div {
        let card = div().flex().flex_col().gap(px(8.0));
        let setting = match row {
            Row::Known(setting) => *setting,
            Row::Other(path) => {
                let description = t("claude_settings.unknown");
                let head = self.row_head(path.clone(), description, String::new(), false, None, cx);
                return card
                    .child(head)
                    .children(self.json_body(&other_id(path), None, cx));
            }
        };
        let control = match setting.kind {
            Kind::Bool => Some(self.bool_control(setting, cx)),
            Kind::Enum => Some(self.enum_control(setting, cx)),
            _ => None,
        };
        let meta = [
            ("claude_settings.type", &setting.type_hint),
            ("claude_settings.default", &setting.default),
        ]
        .into_iter()
        .skip(usize::from(matches!(setting.kind, Kind::Bool | Kind::Enum)))
        .filter(|(_, text)| !text.is_empty())
        .map(|(label, text)| format!("{}: {text}", t(label)))
        .collect::<Vec<_>>()
        .join(" · ");
        let head = self.row_head(
            setting.path(),
            setting.description.clone(),
            meta,
            setting.deprecated,
            control,
            cx,
        );
        let wrong = self
            .value(&setting.path())
            .filter(|v| !setting.accepts(v))
            .map(|v| match setting.kind {
                Kind::Bool | Kind::List | Kind::Env => warning(t_args(
                    "claude_settings.warning.value",
                    &[("value", &raw(v))],
                )),
                _ => warning(t("claude_settings.warning.type")),
            });
        let id = field_id(&setting.key);
        let body = match setting.kind {
            Kind::Bool | Kind::Enum => Vec::new(),
            Kind::String | Kind::Multiline | Kind::Number => self.text_body(setting, cx),
            Kind::List => self.list_body(setting, cx),
            Kind::Json => self.json_body(&id, setting.example.as_ref(), cx),
            Kind::Object => {
                let toggle = wrong.is_none().then(|| {
                    self.toggle_button(
                        format!("json:{}", setting.key),
                        "claude_settings.edit_json",
                        "claude_settings.hide_json",
                        cx,
                    )
                });
                let mut body: Vec<_> = toggle
                    .map(IntoElement::into_any_element)
                    .into_iter()
                    .collect();
                if self.shows_json(setting) {
                    body.extend(self.json_body(&id, setting.example.as_ref(), cx));
                }
                body
            }
            Kind::Env => self.env_body(setting, cx),
        };
        card.child(head).children(wrong).children(body)
    }

    fn topic_list(&self, unknown: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let catalog = store::catalog();
        let searching = self.only_set || !self.search.read(cx).value().trim().is_empty();
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for setting in &catalog.settings {
            if self.value(&setting.path()).is_some() {
                *counts.entry(setting.topic.as_str()).or_default() += 1;
            }
        }
        counts.insert(OTHER_TOPIC, unknown);
        let topics = catalog
            .topics
            .iter()
            .map(String::as_str)
            .chain((unknown > 0).then_some(OTHER_TOPIC));
        let items: Vec<_> = topics
            .map(|id| {
                let count = counts.get(id).copied().unwrap_or(0);
                div()
                    .id(SharedString::from(format!("topic-{id}")))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(6.0))
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::hover()))
                    .when(!searching && self.topic == id, |s| {
                        s.bg(theme::accent_soft())
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.topic = id;
                        this.only_set = false;
                        this.search.update(cx, |input, cx| input.set_value("", cx));
                        this.show_top();
                        cx.notify();
                    }))
                    .child(div().flex_1().min_w_0().child(topic_label(id)))
                    .when(count > 0, |s| {
                        s.child(
                            div()
                                .flex_none()
                                .text_size(px(11.0))
                                .text_color(theme::accent())
                                .child(count.to_string()),
                        )
                    })
            })
            .collect();
        div()
            .id("claude-settings-topics")
            .flex()
            .flex_col()
            .gap(px(2.0))
            .p(px(8.0))
            .overflow_y_scroll()
            .children(items)
    }

    fn content(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let query = self.search.read(cx).value().to_string();
        let Ok(values) = &self.values else {
            return Vec::new();
        };
        let mut found = rows(store::catalog(), values, self.topic, &query, self.only_set);
        if found.is_empty() {
            return vec![note(t("claude_settings.no_results")).into_any_element()];
        }
        let searching = self.only_set || !query.trim().is_empty();
        let more = if searching {
            found.len().saturating_sub(SEARCH_CAP)
        } else {
            0
        };
        found.truncate(found.len() - more);
        for (_, row) in &found {
            self.prepare(row, cx);
        }
        let mut content = Vec::new();
        for group in found.chunk_by(|a, b| a.0 == b.0) {
            content.push(
                div()
                    .text_size(px(14.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(topic_label(group[0].0))
                    .into_any_element(),
            );
            let cards: Vec<_> = group
                .iter()
                .enumerate()
                .map(|(i, (_, row))| {
                    self.row(row, cx)
                        .px(px(16.0))
                        .py(px(12.0))
                        .when(i > 0, |d| {
                            d.border_t_1().border_color(theme::border_subtle())
                        })
                })
                .collect();
            content.push(
                div()
                    .flex()
                    .flex_col()
                    .rounded(px(10.0))
                    .bg(theme::bg_panel())
                    .border_1()
                    .border_color(theme::border_subtle())
                    .children(cards)
                    .into_any_element(),
            );
        }
        if more > 0 {
            let n = more.to_string();
            content.push(note(t_args("claude_settings.more", &[("n", &n)])).into_any_element());
        }
        content
    }

    fn open_button(&self) -> Option<impl IntoElement + use<>> {
        let file = self.file.clone().filter(|f| f.exists())?;
        Some(small_button(
            "claude-settings-open",
            t("claude_settings.open_file"),
            move |_, _, _| {
                let _ = open::that(&file);
            },
        ))
    }
}

impl Render for ClaudeSettingsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let unknown = self
            .values
            .as_ref()
            .map_or(0, |values| store::catalog().unknown_keys(values).len());
        if self.topic == OTHER_TOPIC && unknown == 0 {
            self.topic = first_topic();
        }
        let path = self
            .file
            .as_ref()
            .map(|f| f.display().to_string())
            .unwrap_or_default();
        let header = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(4.0))
            .px(px(24.0))
            .pt(px(20.0))
            .pb(px(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(t("claude_settings.title")),
                    )
                    .children(self.open_button()),
            )
            .child(note(path).min_w_0().overflow_hidden());
        let page = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(header);

        if let Err(e) = &self.values {
            return page.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .mx(px(24.0))
                    .p(px(16.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(theme::error())
                    .child(error_line(t("claude_settings.invalid")))
                    .child(note(e.clone())),
            );
        }

        self.search.update(cx, |input, _| {
            input.set_placeholder(t("claude_settings.search"))
        });
        let this = cx.entity();
        let toolbar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(16.0))
            .px(px(24.0))
            .pb(px(12.0))
            .child(
                box_frame()
                    .flex_none()
                    .items_center()
                    .gap(px(8.0))
                    .w(px(360.0))
                    .h(px(32.0))
                    .child(icon(Icon::Search, px(13.0)).text_color(theme::text_faint()))
                    .child(self.search.clone()),
            )
            .child(checkbox(
                "claude-settings-only-set",
                t("claude_settings.only_set"),
                self.only_set,
                move |checked, _, cx| {
                    this.update(cx, |this, cx| {
                        this.only_set = checked;
                        this.show_top();
                        cx.notify();
                    })
                },
            ));
        let content = self.content(cx);
        let body = div()
            .flex()
            .flex_1()
            .min_h_0()
            .border_t_1()
            .border_color(theme::border_subtle())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .w(px(220.0))
                    .border_r_1()
                    .border_color(theme::border_subtle())
                    .child(self.topics_bar.overlay(self.topic_list(unknown, cx))),
            )
            .child(
                div().flex().flex_col().flex_1().min_w_0().min_h_0().child(
                    self.rows_bar.overlay(
                        div().id("claude-settings-rows").overflow_y_scroll().child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(12.0))
                                .w_full()
                                .max_w(px(860.0))
                                .p(px(24.0))
                                .children(content),
                        ),
                    ),
                ),
            );
        page.child(toolbar).child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn keys(topic: &str, query: &str, only_set: bool, file: &str) -> Vec<String> {
        rows(
            store::catalog(),
            &store::parse(file).unwrap(),
            topic,
            query,
            only_set,
        )
        .into_iter()
        .map(|(_, row)| match row {
            Row::Known(s) => s.key.clone(),
            Row::Other(path) => path.join("."),
        })
        .collect()
    }

    #[test]
    fn deprecated_keys_show_only_when_set() {
        let key = "includeCoAuthoredBy".to_string();
        assert!(!keys("git", "", false, "{}").contains(&key));
        assert!(!keys("git", "coauthored", false, "{}").contains(&key));
        assert!(keys("git", "", false, r#"{"includeCoAuthoredBy":true}"#).contains(&key));
    }

    #[test]
    fn only_set_and_search_look_in_every_topic() {
        let file = r#"{"model":"x","permissions":{"allow":[]},"mine":1}"#;
        assert_eq!(
            keys("git", "", true, file),
            ["model", "permissions", "permissions.allow", "mine"]
        );
        assert_eq!(
            keys("git", "ALLOWEDDOMAINS network", false, "{}"),
            ["sandbox.network.allowedDomains"]
        );
        assert_eq!(keys(OTHER_TOPIC, "", false, file), ["mine"]);
    }

    #[test]
    fn fields_read_their_type() {
        let integer = Input::Number { integer: true };
        assert_eq!(integer.read(" 30 "), Ok(json!(30)));
        assert_eq!(integer.read("1.5"), Err(Invalid::Integer));
        assert_eq!(integer.read(""), Err(Invalid::Number));
        assert_eq!(integer.read("\"3\""), Err(Invalid::Number));
        let object = Input::Json(vec![Shape::Object]);
        assert_eq!(object.read("{}"), Ok(json!({})));
        assert_eq!(object.read("[]"), Err(Invalid::Shape));
        assert!(matches!(object.read("{"), Err(Invalid::Json(_))));
    }

    #[test]
    fn fields_show_wrong_types_as_json() {
        assert_eq!(Input::Text.show(Some(&json!("a"))), "a");
        assert_eq!(Input::Text.show(Some(&json!([1]))), "[1]");
        assert_eq!(Input::Text.show(None), "");
        assert_eq!(
            Input::Number { integer: true }.show(Some(&json!("5"))),
            "\"5\""
        );
    }
}
