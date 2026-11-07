//! A plugin's panel: a pane of simple widgets (text, checkboxes, sliders, buttons, trees and
//! forms) bound to its commands, declared in `plugin.json` (`panel`).
//!
//! A plugin keeps no state between calls (each gets a fresh instance), so the host keeps the
//! panel's state: the widgets' values and what the plugin last said about them. Every command
//! request of a plugin with a panel carries the values (`panel` in the request), and any command
//! may answer with the panel's new state (`panel` in its answer), which is checked like any
//! other hostile input before it is kept. Nothing here knows about the user interface: the
//! engine keeps a [`PanelState`] per plugin in its session, and the front end draws it.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The most widgets a panel may declare.
pub const MAX_ITEMS: usize = 32;
/// The longest panel title, in characters.
pub const MAX_TITLE: usize = 48;
/// A slider's ends stay within this, either way.
pub const SLIDER_LIMIT: i64 = 1_000_000_000;
/// The most labels a slider may have (one per position), and the longest one, in characters.
pub const MAX_SLIDER_LABELS: usize = 2_000;
pub const MAX_SLIDER_LABEL: usize = 80;
/// The longest text or label a plugin may set, in characters.
pub const MAX_STATE_TEXT: usize = 4_000;
pub const MAX_STATE_LABEL: usize = 200;
/// The longest declared label, text or help, in characters.
pub const MAX_DECLARED: usize = 2_000;
/// The most nodes a tree holds, how deep they go, and the longest node id, in characters.
pub const MAX_TREE_NODES: usize = 5_000;
pub const MAX_TREE_DEPTH: usize = 8;
pub const MAX_TREE_ID: usize = 200;
/// The most fields a form holds, and a field's id and choices.
pub const MAX_FORM_FIELDS: usize = 64;
pub const MAX_FORM_FIELD_ID: usize = 64;
pub const MAX_FORM_CHOICES: usize = 256;
/// The widget kinds.
pub const KINDS: &[&str] = &["text", "checkbox", "slider", "button", "tree", "form"];

/// The panel as `plugin.json` declares it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelDecl {
    /// The pane's title (its tab), and the label of the ribbon button that opens it.
    pub title: String,
    /// One of RustGIS's own icons for that button (`plugin` when unset or unknown).
    #[serde(default)]
    pub icon: String,
    /// Shown on hover over that button.
    #[serde(default)]
    pub help: String,
    /// A command of the plugin's run each time the panel opens, to answer the panel's state
    /// (empty for none).
    #[serde(default)]
    pub state: String,
    pub items: Vec<PanelItemDecl>,
}

/// A widget. `kind` is `text`, `checkbox`, `slider`, `button`, `tree` or `form`; the other fields
/// apply to some.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelItemDecl {
    pub kind: String,
    /// How the plugin names it in values and states. A button's defaults to its command.
    #[serde(default)]
    pub id: String,
    /// A caption (text, slider), the box's label (checkbox) or the button's label (its
    /// command's label when unset).
    #[serde(default)]
    pub label: String,
    /// Shown on hover.
    #[serde(default)]
    pub help: String,
    /// A text's first contents, until the plugin sets them.
    #[serde(default)]
    pub text: String,
    /// The command it runs: a button's when clicked; a checkbox's when ticked or cleared; a
    /// slider's when let go (or stepped); a tree's when a node is chosen; a form's when one of
    /// its fields changes. Optional for checkboxes, sliders, trees and forms, whose values then
    /// just go with the plugin's next request.
    #[serde(default)]
    pub command: String,
    /// A checkbox's first value (a bool), or a slider's (a whole number; its `min` if unset).
    #[serde(default)]
    pub default: Option<Value>,
    /// A slider's ends (whole numbers), until the plugin sets them.
    #[serde(default)]
    pub min: i64,
    #[serde(default)]
    pub max: i64,
    /// A slider with ‹ and › buttons that step it by one.
    #[serde(default)]
    pub steps: bool,
    /// A tree with a search box above it, which keeps the nodes whose label holds what's typed.
    #[serde(default)]
    pub search: bool,
}

impl PanelItemDecl {
    /// Its id: `id`, or a button's command.
    pub fn key(&self) -> &str {
        if self.id.is_empty() && self.kind == "button" { &self.command } else { &self.id }
    }

    /// Whether it has a value (a checkbox, a slider, a tree or a form).
    pub fn has_value(&self) -> bool {
        matches!(self.kind.as_str(), "checkbox" | "slider" | "tree" | "form")
    }
}

/// One field of a form, as the plugin gave it. Its value is kept with the form's, by field id.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormField {
    /// How the plugin names it in the form's value. Unique in the form.
    pub id: String,
    /// `text`, `number`, `checkbox`, `select` (one of `choices`) or `multi` (several of them).
    pub kind: String,
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub help: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(skip_serializing_if = "not_true")]
    pub required: bool,
    /// A number that is a whole number.
    #[serde(skip_serializing_if = "not_true")]
    pub integer: bool,
}

fn not_true(b: &bool) -> bool {
    !*b
}

impl FormField {
    /// The value `v` takes, made to this field's shape, or `None` when it doesn't.
    fn value_of(&self, v: &Value) -> Option<Value> {
        match self.kind.as_str() {
            "text" => v.as_str().map(|s| Value::String(clip(s, MAX_STATE_TEXT))),
            "number" => number(v, self.integer),
            "checkbox" => v.as_bool().map(Value::Bool),
            "select" => v.as_str().filter(|s| self.choices.iter().any(|c| c == s)).map(|s| json!(clip(s, MAX_STATE_LABEL))),
            "multi" => {
                let parts: Vec<&str> = match v {
                    Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                    Value::String(s) => s.split(',').map(str::trim).collect(),
                    _ => return None,
                };
                let mut out: Vec<Value> = Vec::new();
                for s in parts {
                    if !self.choices.iter().any(|c| c == s) {
                        continue;
                    }
                    let s = clip(s, MAX_STATE_LABEL);
                    if !out.iter().any(|v| v.as_str() == Some(s.as_str())) {
                        out.push(Value::String(s));
                    }
                }
                Some(Value::Array(out))
            }
            _ => None,
        }
    }

    /// The value it has when the plugin gave none: its default, or an empty one of its kind.
    fn initial(&self) -> Value {
        self.default.as_ref().and_then(|d| self.value_of(d)).unwrap_or_else(|| match self.kind.as_str() {
            "number" => json!(0),
            "checkbox" => Value::Bool(false),
            "select" => json!(self.choices.first().cloned().unwrap_or_default()),
            "multi" => Value::Array(Vec::new()),
            _ => json!(""),
        })
    }
}

/// A whole or fractional number from JSON, as this field takes it.
fn number(v: &Value, integer: bool) -> Option<Value> {
    let f = v.as_f64().filter(|x| x.is_finite())?;
    if integer { (f.fract() == 0.0 && f.abs() <= 9_007_199_254_740_992.0).then(|| json!(f as i64)) } else { Some(json!(f)) }
}

impl PanelDecl {
    /// The widget with id `id`.
    pub fn item(&self, id: &str) -> Option<&PanelItemDecl> {
        self.items.iter().find(|i| i.key() == id)
    }

    /// Checks the panel; `command` says whether the plugin has a command by that id.
    pub fn validate(&self, command: &dyn Fn(&str) -> bool) -> Result<(), String> {
        let title = self.title.trim();
        if title.is_empty() || title.chars().count() > MAX_TITLE || title.contains('\n') {
            return Err(format!("plugin.json: the panel's title must be one line of 1 to {MAX_TITLE} characters"));
        }
        check("panel help", &self.help)?;
        check("panel icon", &self.icon)?;
        if !self.state.is_empty() && !command(&self.state) {
            return Err(format!("plugin.json: the panel's state command '{}' isn't one of the plugin's commands", clip(&self.state, 80)));
        }
        if self.items.is_empty() || self.items.len() > MAX_ITEMS {
            return Err(format!("plugin.json: the panel needs 1 to {MAX_ITEMS} items"));
        }
        let mut ids = HashSet::new();
        for item in &self.items {
            let kind = item.kind.as_str();
            if !KINDS.contains(&kind) {
                return Err(format!(
                    "plugin.json: panel item kind '{}' isn't text, checkbox, slider, button, tree or form",
                    clip(kind, 40)
                ));
            }
            let id = item.key();
            if !crate::manifest::valid_id(id, false) || !ids.insert(id) {
                return Err(format!(
                    "plugin.json: panel item id '{}' is invalid or repeated (lower-case letters, digits and hyphens; a button's is its command's unless given)",
                    clip(id, 80)
                ));
            }
            check("panel item label", &item.label)?;
            check("panel item help", &item.help)?;
            check("panel item text", &item.text)?;
            let what = |s: &str| format!("plugin.json: panel item '{id}': {s}");
            match kind {
                "text" if !item.command.is_empty() => return Err(what("a text runs no command")),
                "button" if item.command.is_empty() => return Err(what("a button needs a command")),
                "checkbox" if item.label.trim().is_empty() => return Err(what("a checkbox needs a label")),
                _ => {}
            }
            if !item.command.is_empty() && !command(&item.command) {
                return Err(what(&format!("'{}' isn't one of the plugin's commands", clip(&item.command, 80))));
            }
            if kind != "slider" && (item.min != 0 || item.max != 0 || item.steps) {
                return Err(what("min, max and steps are for sliders"));
            }
            if kind != "text" && !item.text.is_empty() {
                return Err(what("text is for texts"));
            }
            if kind != "tree" && item.search {
                return Err(what("search is for trees"));
            }
            match (kind, &item.default) {
                (_, None) | ("checkbox", Some(Value::Bool(_))) => {}
                ("slider", Some(v)) if whole(v).is_some_and(|v| (item.min..=item.max).contains(&v)) => {}
                ("slider", Some(_)) => return Err(what("a slider's default is a whole number from min to max")),
                ("checkbox", Some(_)) => return Err(what("a checkbox's default is true or false")),
                _ => return Err(what("only checkboxes and sliders have a default")),
            }
            if kind == "slider" && (item.min > item.max || item.min < -SLIDER_LIMIT || item.max > SLIDER_LIMIT) {
                return Err(what(&format!("a slider's min must not be above its max, both within ±{SLIDER_LIMIT}")));
            }
        }
        Ok(())
    }
}

fn check(what: &str, s: &str) -> Result<(), String> {
    if s.chars().count() > MAX_DECLARED { Err(format!("plugin.json: {what} is too long")) } else { Ok(()) }
}

/// At most `n` characters of `s`.
fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// A whole number from an integer or a float with no fraction (JSON from other languages).
fn whole(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().filter(|x| x.is_finite() && x.fract() == 0.0 && x.abs() <= SLIDER_LIMIT as f64).map(|x| x as i64))
}

/// What the plugin last said about one widget.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemState {
    /// Replaces the declared label (the plugin words it in the request's `lang`).
    pub label: Option<String>,
    /// A text's contents.
    pub text: String,
    pub enabled: bool,
    pub visible: bool,
    /// A slider's ends.
    pub min: i64,
    pub max: i64,
    /// A slider's label for each position from `min` (empty: the numbers).
    pub labels: Vec<String>,
    /// A tree's nodes (shared: the panel's front end copies the state every frame it shows).
    #[serde(skip_serializing_if = "no_nodes", serialize_with = "shared_nodes")]
    pub nodes: Arc<Vec<TreeNode>>,
    /// A form's fields, in the order they show (shared as the nodes are).
    #[serde(skip_serializing_if = "no_fields", serialize_with = "shared_fields")]
    pub fields: Arc<Vec<FormField>>,
}

fn no_fields(f: &Arc<Vec<FormField>>) -> bool {
    f.is_empty()
}

fn shared_fields<S: serde::Serializer>(f: &Arc<Vec<FormField>>, s: S) -> Result<S::Ok, S::Error> {
    f.as_slice().serialize(s)
}

fn no_nodes(n: &Arc<Vec<TreeNode>>) -> bool {
    n.is_empty()
}

fn shared_nodes<S: serde::Serializer>(n: &Arc<Vec<TreeNode>>, s: S) -> Result<S::Ok, S::Error> {
    n.as_slice().serialize(s)
}

/// A node of a tree, as the plugin gave it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TreeNode {
    /// How the plugin names it: the tree's value when it is chosen. Unique in the tree.
    pub id: String,
    pub label: String,
    /// A number shown beside it (how many datasets a theme holds, say).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TreeNode>,
}

/// The nodes in `v` (`[{"id", "label"?, "count"?, "children"?}]`), at most `left` of them
/// (counted down) and [`MAX_TREE_DEPTH`] deep below `depth`. A node without an id, or with one
/// taken already or too long, is left out with its children; a label defaults to the id (when
/// missing or blank), and is kept to one line (a row of the tree), control characters as spaces.
fn tree_nodes(v: &[Value], depth: usize, left: &mut usize, seen: &mut HashSet<String>) -> Vec<TreeNode> {
    let mut nodes = Vec::new();
    for n in v {
        if *left == 0 {
            break;
        }
        let Some(id) = n.get("id").and_then(Value::as_str).filter(|i| !i.is_empty() && i.chars().count() <= MAX_TREE_ID) else {
            continue;
        };
        if !seen.insert(id.to_string()) {
            continue;
        }
        *left -= 1;
        let label: String = n
            .get("label")
            .and_then(Value::as_str)
            .filter(|l| !l.trim().is_empty())
            .map_or_else(|| id.to_string(), |l| clip(l, MAX_STATE_LABEL))
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let children = match n.get("children").and_then(Value::as_array) {
            Some(c) if depth + 1 < MAX_TREE_DEPTH => tree_nodes(c, depth + 1, left, seen),
            _ => Vec::new(),
        };
        nodes.push(TreeNode { id: id.to_string(), label, count: n.get("count").and_then(Value::as_u64), children });
    }
    nodes
}

/// Whether `nodes` hold one with id `id`, at any depth.
fn holds(nodes: &[TreeNode], id: &str) -> bool {
    nodes.iter().any(|n| n.id == id || holds(&n.children, id))
}

/// The fields in `given` (`[{"id", "kind", "label"?, "help"?, "choices"?, "default"?,
/// "required"?, "integer"?}]`), at most [`MAX_FORM_FIELDS`] of them. A field with an id taken
/// already, an unknown kind, or no choices when it needs them is left out.
fn form_fields(given: &[Value]) -> Vec<FormField> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for f in given.iter().take(MAX_FORM_FIELDS) {
        let Some(id) = f.get("id").and_then(Value::as_str).filter(|i| i.chars().count() <= MAX_FORM_FIELD_ID) else {
            continue;
        };
        if !crate::manifest::valid_id(id, true) || !seen.insert(id.to_string()) {
            continue;
        }
        let Some(kind) =
            f.get("kind").and_then(Value::as_str).filter(|k| matches!(*k, "text" | "number" | "checkbox" | "select" | "multi"))
        else {
            continue;
        };
        let mut choices: Vec<String> = Vec::new();
        if matches!(kind, "select" | "multi") {
            let Some(list) = f.get("choices").and_then(Value::as_array) else { continue };
            for c in list.iter().take(MAX_FORM_CHOICES) {
                let Some(c) = c.as_str() else { continue };
                let c = clip(c, MAX_STATE_LABEL);
                if !choices.contains(&c) {
                    choices.push(c);
                }
            }
            if choices.is_empty() {
                continue;
            }
        }
        let mut field = FormField {
            id: id.to_string(),
            kind: kind.to_string(),
            label: f
                .get("label")
                .and_then(Value::as_str)
                .filter(|l| !l.trim().is_empty())
                .map_or_else(|| id.to_string(), |l| clip(l, MAX_STATE_LABEL)),
            help: f.get("help").and_then(Value::as_str).map(|h| clip(h, MAX_DECLARED)).unwrap_or_default(),
            choices,
            default: None,
            required: f.get("required").and_then(Value::as_bool).unwrap_or(false),
            integer: f.get("integer").and_then(Value::as_bool).unwrap_or(false),
        };
        field.default = f.get("default").and_then(|d| field.value_of(d));
        out.push(field);
    }
    out
}

/// A form's value with exactly `fields` in it, in order: what it had for a field that is still
/// there, or the field's first value.
fn normalized_form(fields: &[FormField], value: &Value) -> Value {
    let current = value.as_object();
    let mut out = Map::new();
    for f in fields {
        let v = current.and_then(|m| m.get(&f.id)).and_then(|raw| f.value_of(raw)).unwrap_or_else(|| f.initial());
        out.insert(f.id.clone(), v);
    }
    Value::Object(out)
}

/// A panel's state, kept by the host between the plugin's calls.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PanelState {
    /// Checkbox and slider values, by widget id: `true`/`false`, or a whole number.
    pub values: BTreeMap<String, Value>,
    /// Every widget's state, by id.
    pub items: BTreeMap<String, ItemState>,
    /// The declaration this state was made from, to tell when the plugin was updated (shared:
    /// the panel's front end copies the state every frame it shows).
    #[serde(skip)]
    origin: Arc<PanelDecl>,
}

impl PanelState {
    /// Whether this state was made from `decl`. A plugin updated with another panel (other
    /// widgets, a slider's range or a default) doesn't match. A plain comparison: it runs on every
    /// command of a panel plugin and every frame the panel shows.
    pub fn matches(&self, decl: &PanelDecl) -> bool {
        *self.origin == *decl
    }

    /// The state a panel starts in: what `plugin.json` declares.
    pub fn new(decl: &PanelDecl) -> PanelState {
        let mut values = BTreeMap::new();
        let mut items = BTreeMap::new();
        for d in &decl.items {
            let id = d.key().to_string();
            match d.kind.as_str() {
                "checkbox" => {
                    values.insert(id.clone(), Value::Bool(d.default.as_ref().and_then(Value::as_bool).unwrap_or(false)));
                }
                "slider" => {
                    let v = d.default.as_ref().and_then(whole).unwrap_or(d.min).clamp(d.min, d.max.max(d.min));
                    values.insert(id.clone(), json!(v));
                }
                "tree" => {
                    values.insert(id.clone(), Value::Null);
                }
                "form" => {
                    values.insert(id.clone(), Value::Object(Map::new()));
                }
                _ => {}
            }
            items.insert(
                id,
                ItemState {
                    label: None,
                    text: d.text.clone(),
                    enabled: true,
                    visible: true,
                    min: d.min,
                    max: d.max.max(d.min),
                    labels: Vec::new(),
                    nodes: Arc::default(),
                    fields: Arc::default(),
                },
            );
        }
        PanelState { values, items, origin: Arc::new(decl.clone()) }
    }

    /// A checkbox's value.
    pub fn checked(&self, id: &str) -> bool {
        self.values.get(id).and_then(Value::as_bool).unwrap_or(false)
    }

    /// A slider's value.
    pub fn slider(&self, id: &str) -> i64 {
        self.values.get(id).and_then(Value::as_i64).unwrap_or(0)
    }

    /// A tree's chosen node, if any.
    pub fn chosen(&self, id: &str) -> Option<&str> {
        self.values.get(id).and_then(Value::as_str)
    }

    /// A form's values, by field id.
    pub fn form(&self, id: &str) -> Option<&Map<String, Value>> {
        self.values.get(id).and_then(Value::as_object)
    }

    /// One field's value in a form.
    pub fn form_value(&self, id: &str, field: &str) -> Option<&Value> {
        self.form(id).and_then(|f| f.get(field))
    }

    /// A slider's label for value `v`: its own label there, or the number.
    pub fn slider_label(&self, id: &str, v: i64) -> String {
        let Some(st) = self.items.get(id) else { return v.to_string() };
        usize::try_from(v.saturating_sub(st.min))
            .ok()
            .and_then(|i| st.labels.get(i))
            .filter(|l| !l.is_empty())
            .cloned()
            .unwrap_or_else(|| v.to_string())
    }

    /// Sets widget values from `values` (`{"id": value}`): a checkbox takes a bool, a slider a
    /// whole number (kept within its ends), a tree one of its nodes' ids or `null` for none, and
    /// a form an object of field values (merged into the ones it had). Anything else is ignored.
    /// Returns the ids set.
    pub fn set_values(&mut self, decl: &PanelDecl, values: &Value) -> Vec<String> {
        let Some(given) = values.as_object() else { return Vec::new() };
        let mut set = Vec::new();
        for d in decl.items.iter().filter(|d| d.has_value()) {
            let id = d.key();
            let Some(v) = given.get(id) else { continue };
            let new = match d.kind.as_str() {
                "checkbox" => v.as_bool().map(Value::Bool),
                "tree" => match v {
                    Value::Null => Some(Value::Null),
                    Value::String(n) if self.items.get(id).is_some_and(|s| holds(&s.nodes, n)) => Some(v.clone()),
                    _ => None,
                },
                "form" => {
                    // A form takes the fields it holds, merged into what it had: a caller may
                    // set one without repeating the rest.
                    let Some(given) = v.as_object() else { continue };
                    let fields = self.items.get(id).map_or(&[][..], |s| s.fields.as_slice());
                    let mut merged = self.values.get(id).and_then(Value::as_object).cloned().unwrap_or_default();
                    let mut any = false;
                    for f in fields {
                        if let Some(raw) = given.get(&f.id)
                            && let Some(val) = f.value_of(raw)
                        {
                            merged.insert(f.id.clone(), val);
                            any = true;
                        }
                    }
                    any.then_some(Value::Object(merged))
                }
                _ => whole(v).map(|n| {
                    let (min, max) = self.items.get(id).map_or((d.min, d.max), |s| (s.min, s.max));
                    json!(n.clamp(min, max.max(min)))
                }),
            };
            if let Some(new) = new {
                self.values.insert(id.to_string(), new);
                set.push(id.to_string());
            }
        }
        set
    }

    /// Takes in a plugin's answer about its panel: `{"values": {...}, "items": {"id": {...}}}`.
    /// An item may set `label` (`null` for the declared one), `text`, `enabled`, `visible`, for a
    /// slider `min`, `max` and `labels`, for a tree `nodes`, and for a form `fields`. Unknown ids
    /// and fields, and values of the wrong type, are ignored; texts are clipped, sliders' values
    /// kept within their ends, a tree's chosen node among its nodes, and a form's value to its
    /// fields (a field the plugin no longer gives is dropped; a new one starts at its default).
    pub fn apply(&mut self, decl: &PanelDecl, update: &Value) {
        let empty = Map::new();
        let items = update.get("items").and_then(Value::as_object).unwrap_or(&empty);
        for d in &decl.items {
            let id = d.key();
            let (Some(u), Some(st)) = (items.get(id), self.items.get_mut(id)) else { continue };
            match u.get("label") {
                Some(Value::String(l)) => st.label = Some(clip(l, MAX_STATE_LABEL)),
                Some(Value::Null) => st.label = None,
                _ => {}
            }
            if d.kind == "text"
                && let Some(t) = u.get("text").and_then(Value::as_str)
            {
                st.text = clip(t, MAX_STATE_TEXT);
            }
            if let Some(b) = u.get("enabled").and_then(Value::as_bool) {
                st.enabled = b;
            }
            if let Some(b) = u.get("visible").and_then(Value::as_bool) {
                st.visible = b;
            }
            if d.kind == "slider" {
                let min = u.get("min").and_then(whole).unwrap_or(st.min);
                let max = u.get("max").and_then(whole).unwrap_or(st.max);
                if min <= max && min >= -SLIDER_LIMIT && max <= SLIDER_LIMIT {
                    (st.min, st.max) = (min, max);
                }
                if let Some(ls) = u.get("labels").and_then(Value::as_array) {
                    st.labels = ls
                        .iter()
                        .take(MAX_SLIDER_LABELS)
                        .map(|l| l.as_str().map(|s| clip(s, MAX_SLIDER_LABEL)).unwrap_or_default())
                        .collect();
                }
            }
            if d.kind == "tree"
                && let Some(nodes) = u.get("nodes").and_then(Value::as_array)
            {
                let mut left = MAX_TREE_NODES;
                st.nodes = Arc::new(tree_nodes(nodes, 0, &mut left, &mut Default::default()));
            }
            if d.kind == "form"
                && let Some(fields) = u.get("fields").and_then(Value::as_array)
            {
                st.fields = Arc::new(form_fields(fields));
            }
        }
        if let Some(values) = update.get("values") {
            self.set_values(decl, values);
        }
        // New ends keep the values within them, and new nodes the chosen one among them.
        for d in &decl.items {
            let id = d.key();
            let (Some(st), Some(v)) = (self.items.get(id), self.values.get_mut(id)) else { continue };
            match d.kind.as_str() {
                "slider" => *v = json!(v.as_i64().unwrap_or(st.min).clamp(st.min, st.max)),
                "tree" if v.as_str().is_some_and(|n| !holds(&st.nodes, n)) => *v = Value::Null,
                "form" => *v = normalized_form(&st.fields, v),
                _ => {}
            }
        }
    }

    /// What goes to the plugin in each request: `{"values": {...}, "changed": [ids]}`, `changed`
    /// naming the widgets this call set (the one the user just moved, say).
    pub fn request(&self, changed: &[String]) -> Value {
        json!({"values": self.values, "changed": changed})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form_decl() -> PanelDecl {
        PanelDecl {
            title: "Test".into(),
            icon: String::new(),
            help: String::new(),
            state: String::new(),
            items: vec![PanelItemDecl {
                kind: "form".into(),
                id: "form".into(),
                label: "Form".into(),
                help: String::new(),
                text: String::new(),
                command: String::new(),
                default: None,
                min: 0,
                max: 0,
                steps: false,
                search: false,
            }],
        }
    }

    fn fields() -> Value {
        json!({
            "items": {"form": {"fields": [
                {"id": "method", "kind": "select", "label": "Method",
                 "choices": ["fast", "careful"], "default": "careful"},
                {"id": "count", "kind": "number", "integer": true, "default": 3.0},
                {"id": "label", "kind": "text", "default": "untitled"},
                {"id": "on", "kind": "checkbox", "default": true},
                {"id": "vars", "kind": "multi", "choices": ["a", "b"], "default": "a,b"}
            ]}}
        })
    }

    #[test]
    fn a_form_takes_the_fields_its_plugin_gives_and_starts_at_their_defaults() {
        let d = form_decl();
        let mut s = PanelState::new(&d);
        assert_eq!(s.form("form"), Some(&Map::new()));
        s.apply(&d, &fields());
        let f = s.form("form").unwrap();
        assert_eq!(f["method"], json!("careful"));
        assert_eq!(f["count"], json!(3));
        assert_eq!(f["label"], json!("untitled"));
        assert_eq!(f["on"], json!(true));
        assert_eq!(f["vars"], json!(["a", "b"]));
        // A value set for one field keeps the others.
        s.set_values(&d, &json!({"form": {"label": "new", "count": 7.0}}));
        let f = s.form("form").unwrap();
        assert_eq!(f["label"], json!("new"));
        assert_eq!(f["count"], json!(7));
        assert_eq!(f["method"], json!("careful"));
        // A form whose fields changed keeps what still fits, drops the rest and starts new ones.
        s.apply(
            &d,
            &json!({"items": {"form": {"fields": [
                {"id": "label", "kind": "text", "default": "other"},
                {"id": "level", "kind": "select", "choices": ["1", "2"], "default": "2"}
            ]}}}),
        );
        let f = s.form("form").unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f["label"], json!("new"), "a field still there keeps its value");
        assert_eq!(f["level"], json!("2"), "a new field starts at its default");
    }

    #[test]
    fn a_form_ignores_fields_and_values_it_cannot_take() {
        let d = form_decl();
        let mut s = PanelState::new(&d);
        s.apply(
            &d,
            &json!({"items": {"form": {"fields": [
                {"id": "no choices", "kind": "select"},
                {"id": "kind", "kind": "unknown"},
                {"id": "pick", "kind": "select", "choices": ["a", "a", "b"]},
                {"id": "pick", "kind": "text"},
                {"id": "num", "kind": "number", "integer": true, "default": 1.5},
                {"id": "text", "kind": "text"}
            ]}}}),
        );
        let st = s.items.get("form").unwrap();
        assert_eq!(st.fields.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["pick", "num", "text"]);
        assert_eq!(st.fields[0].choices, ["a", "b"]);
        assert_eq!(st.fields[1].default, None, "1.5 is no whole number");
        assert_eq!(s.form("form").unwrap()["num"], json!(0));
        // Values of the wrong type or not among the choices are ignored; text is clipped.
        s.set_values(&d, &json!({"form": {"pick": "zzz", "num": "many", "text": 7, "unknown": "x"}}));
        let f = s.form("form").unwrap();
        assert_eq!(f["pick"], json!("a"));
        assert_eq!(f["num"], json!(0));
        assert_eq!(f["text"], json!(""));
        assert!(!f.contains_key("unknown"));
        let long = "x".repeat(MAX_STATE_TEXT + 10);
        s.set_values(&d, &json!({"form": {"text": long}}));
        assert_eq!(s.form("form").unwrap()["text"].as_str().unwrap().chars().count(), MAX_STATE_TEXT);
    }
}
