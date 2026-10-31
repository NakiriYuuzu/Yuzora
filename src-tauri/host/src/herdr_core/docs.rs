//! Space / tab / split layout kept in the core's API document `yuzora` (`doc.get` / `doc.set`,
//! upstream `schema/README.md` "Documents"): opaque JSON, optimistic versions, 1 MiB each.
//!
//! The merge logic lives here and nowhere else. A caller never sends a whole document; it
//! sends an [`Op`] that names its targets by id, and [`update`] reads the latest value,
//! applies the operation to it and writes with the version it read. A `conflict` means
//! someone else wrote in between: read again and apply the operation to *that* value, never
//! retry the old value with a new version.
//!
//! Invariants kept on every write:
//! * every level (root, space, tab, split, leaf) keeps the fields it does not know;
//! * splits and leaves have stable ids;
//! * one terminal appears in at most one leaf (the first in document order keeps it, a later
//!   leaf becomes vacant and is reported as displaced).
//!
//! A document with a newer `schema`, or whose `app` is not `yuzora`, is read-only.

use std::collections::HashSet;

use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::rpc::RpcError;
use crate::herdr_limits::MAX_LAYOUT_DEPTH;

/// Document name (terminal-name rules: 1-32 of a-z 0-9 - _ . , not id-shaped).
pub const DOC_NAME: &str = "yuzora";
/// The `app` field: guards against another app writing the same name.
pub const APP: &str = "yuzora";
/// Schema this build writes. A reader that sees a larger one shows it read-only.
pub const SCHEMA: u32 = 1;
/// `doc.set` refuses values whose compact JSON exceeds this (upstream `api/doc.rs`).
pub const MAX_BYTES: usize = 1024 * 1024;
/// Read-apply-write rounds before [`DocError::GaveUp`].
pub const DEFAULT_ATTEMPTS: usize = 5;
/// Split ratios are kept inside this range so no pane collapses to nothing.
pub const MIN_RATIO: f64 = 0.05;
pub const MAX_RATIO: f64 = 0.95;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutDoc {
    /// Schema version of this document.
    pub schema: u32,
    pub app: String,
    #[serde(default)]
    pub spaces: Vec<Space>,
    /// Fields of a newer writer of the same schema, kept on rewrite.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Space {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Absolute path new terminals start in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(rename = "activeTab", default, skip_serializing_if = "Option::is_none")]
    pub active_tab: Option<String>,
    #[serde(default)]
    pub tabs: Vec<Tab>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub id: String,
    pub title: String,
    pub layout: Node,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A split tree. `Unknown` is a node of a `kind` this build does not know: kept verbatim.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Leaf(Leaf),
    Split(Split),
    Unknown(Value),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Leaf {
    pub id: String,
    /// A core terminal address (`t3`). `None` is a vacant leaf: its terminal moved to another
    /// leaf, and the UI shows it as moved out until it is closed.
    pub terminal: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Split {
    pub id: String,
    pub dir: Dir,
    /// Share of the first child, within [`MIN_RATIO`, `MAX_RATIO`].
    pub ratio: f64,
    pub a: Box<Node>,
    pub b: Box<Node>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    /// Side by side.
    H,
    /// Stacked.
    V,
}

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        fn tagged<T: Serialize, S: Serializer>(
            kind: &str,
            node: &T,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            let mut value = serde_json::to_value(node).map_err(S::Error::custom)?;
            if let Value::Object(map) = &mut value {
                map.insert("kind".into(), kind.into());
            }
            value.serialize(serializer)
        }
        match self {
            Node::Leaf(leaf) => tagged("leaf", leaf, serializer),
            Node::Split(split) => tagged("split", split, serializer),
            Node::Unknown(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = Value::deserialize(deserializer)?;
        let kind = value.get("kind").and_then(Value::as_str).map(str::to_owned);
        if !matches!(kind.as_deref(), Some("leaf" | "split")) {
            return Ok(Node::Unknown(value));
        }
        if let Value::Object(map) = &mut value {
            map.remove("kind");
        }
        if kind.as_deref() == Some("leaf") {
            serde_json::from_value(value).map(Node::Leaf)
        } else {
            serde_json::from_value(value).map(Node::Split)
        }
        .map_err(D::Error::custom)
    }
}

impl Node {
    pub fn id(&self) -> Option<&str> {
        match self {
            Node::Leaf(leaf) => Some(&leaf.id),
            Node::Split(split) => Some(&split.id),
            Node::Unknown(value) => value.get("id").and_then(Value::as_str),
        }
    }

    /// Nesting depth: a lone leaf is 1.
    pub fn depth(&self) -> usize {
        match self {
            Node::Split(split) => 1 + split.a.depth().max(split.b.depth()),
            _ => 1,
        }
    }

    fn leaves_mut(&mut self, f: &mut impl FnMut(&mut Leaf)) {
        match self {
            Node::Leaf(leaf) => f(leaf),
            Node::Split(split) => {
                split.a.leaves_mut(f);
                split.b.leaves_mut(f);
            }
            Node::Unknown(_) => {}
        }
    }

    fn leaves(&self, f: &mut impl FnMut(&Leaf)) {
        match self {
            Node::Leaf(leaf) => f(leaf),
            Node::Split(split) => {
                split.a.leaves(f);
                split.b.leaves(f);
            }
            Node::Unknown(_) => {}
        }
    }

    fn ids(&self, out: &mut HashSet<String>) {
        if let Some(id) = self.id() {
            out.insert(id.to_string());
        }
        if let Node::Split(split) = self {
            split.a.ids(out);
            split.b.ids(out);
        }
    }

    /// The node with `id` and its depth (`depth` is this node's own).
    fn find_mut<'a>(&'a mut self, id: &str, depth: usize) -> Option<(&'a mut Node, usize)> {
        if self.id() == Some(id) {
            return Some((self, depth));
        }
        match self {
            Node::Split(split) => {
                if let Some(found) = split.a.find_mut(id, depth + 1) {
                    return Some(found);
                }
                split.b.find_mut(id, depth + 1)
            }
            _ => None,
        }
    }
}

impl LayoutDoc {
    pub fn empty() -> LayoutDoc {
        LayoutDoc {
            schema: SCHEMA,
            app: APP.into(),
            spaces: Vec::new(),
            extra: Map::new(),
        }
    }

    pub fn space(&self, id: &str) -> Option<&Space> {
        self.spaces.iter().find(|s| s.id == id)
    }

    /// Every terminal address the layout references, in document order.
    pub fn terminals(&self) -> Vec<String> {
        let mut out = Vec::new();
        for tab in self.spaces.iter().flat_map(|s| &s.tabs) {
            tab.layout.leaves(&mut |leaf| {
                if let Some(t) = &leaf.terminal {
                    out.push(t.clone());
                }
            });
        }
        out
    }

    fn all_ids(&self) -> HashSet<String> {
        let mut ids = HashSet::new();
        for space in &self.spaces {
            ids.insert(space.id.clone());
            for tab in &space.tabs {
                ids.insert(tab.id.clone());
                tab.layout.ids(&mut ids);
            }
        }
        ids
    }

    /// Enforces "one terminal in at most one leaf" (the first keeps it) and repairs an
    /// `activeTab` that points at nothing. Returns the leaves that became vacant.
    pub fn normalize(&mut self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut displaced = Vec::new();
        for space in &mut self.spaces {
            for tab in &mut space.tabs {
                tab.layout.leaves_mut(&mut |leaf| {
                    if let Some(terminal) = &leaf.terminal {
                        if !seen.insert(terminal.clone()) {
                            leaf.terminal = None;
                            displaced.push(leaf.id.clone());
                        }
                    }
                });
            }
            let dangling = space
                .active_tab
                .as_ref()
                .is_some_and(|a| !space.tabs.iter().any(|t| &t.id == a));
            if dangling {
                space.active_tab = space.tabs.first().map(|t| t.id.clone());
            }
        }
        displaced
    }
}

/// A fresh id for a space, tab, split or leaf: `<prefix>_<12 hex>`. Callers choose ids before
/// building an [`Op`], so a re-applied operation always creates the same nodes.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{:012x}", rand::random::<u64>() & 0xffff_ffff_ffff)
}

// ---------------------------------------------------------------------------------- decoding

/// Why the document may be shown but not written. Tagged `reason`, not `kind`: it is nested in
/// [`LayoutState::ReadOnly`], whose own tag is `kind`, and two tags of one name would
/// serialize as a duplicate key (`{"kind":"readOnly","kind":"newerSchema",..}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "camelCase")]
pub enum ReadOnlyReason {
    /// Written by a newer Yuzora.
    NewerSchema { schema: u32 },
    /// Another app's document under the same name.
    ForeignApp { app: Option<String> },
    /// Not a layout document this build can parse.
    Invalid { message: String },
}

impl ReadOnlyReason {
    pub fn code(&self) -> &'static str {
        match self {
            ReadOnlyReason::NewerSchema { .. } => "newer_schema",
            ReadOnlyReason::ForeignApp { .. } => "foreign_app",
            ReadOnlyReason::Invalid { .. } => "invalid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LayoutState {
    /// Never written, or deleted.
    Empty,
    Doc {
        doc: LayoutDoc,
        /// Leaves the one-terminal-per-leaf rule vacated while decoding (not written yet).
        displaced: Vec<String>,
    },
    ReadOnly(ReadOnlyReason),
}

/// Parses a `doc.get` value. Never fails: what cannot be written safely is `ReadOnly`.
pub fn decode(value: &Value) -> LayoutState {
    if value.is_null() {
        return LayoutState::Empty;
    }
    if value.get("app").and_then(Value::as_str) != Some(APP) {
        return LayoutState::ReadOnly(ReadOnlyReason::ForeignApp {
            app: value.get("app").and_then(Value::as_str).map(String::from),
        });
    }
    let Some(schema) = value
        .get("schema")
        .and_then(Value::as_u64)
        .and_then(|s| u32::try_from(s).ok())
    else {
        return LayoutState::ReadOnly(ReadOnlyReason::Invalid {
            message: "no schema".into(),
        });
    };
    if schema > SCHEMA {
        return LayoutState::ReadOnly(ReadOnlyReason::NewerSchema { schema });
    }
    match serde_json::from_value::<LayoutDoc>(value.clone()) {
        Ok(mut doc) => {
            let displaced = doc.normalize();
            LayoutState::Doc { doc, displaced }
        }
        Err(e) => LayoutState::ReadOnly(ReadOnlyReason::Invalid {
            message: e.to_string(),
        }),
    }
}

/// The JSON value to send, refused locally when it would exceed the core's limit.
pub fn encode(doc: &LayoutDoc) -> Result<Value, DocError> {
    let value = serde_json::to_value(doc).map_err(|e| DocError::Invalid(e.to_string()))?;
    let bytes = serde_json::to_vec(&value)
        .map_err(|e| DocError::Invalid(e.to_string()))?
        .len();
    if bytes > MAX_BYTES {
        return Err(DocError::TooLarge(bytes));
    }
    Ok(value)
}

// ---------------------------------------------------------------------------------- operations

/// One change to the layout, targets by id. Camel-case JSON with an `op` tag, e.g.
/// `{"op":"setRatio","node":"n_1","ratio":0.3}`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Op {
    AddSpace {
        id: String,
        name: String,
        root: String,
        #[serde(default)]
        color: Option<String>,
    },
    RenameSpace {
        space: String,
        name: String,
    },
    /// Takes the space out of the layout. What becomes of its terminals is the caller's call
    /// (Q9: by default they keep running; only closing a tab or leaf ends its terminal), see
    /// [`Applied::removed_terminals`].
    RemoveSpace {
        space: String,
    },
    /// Adds a tab with one leaf and makes it the active tab.
    AddTab {
        space: String,
        tab: String,
        title: String,
        leaf: String,
        terminal: String,
    },
    RemoveTab {
        space: String,
        tab: String,
    },
    SetActiveTab {
        space: String,
        tab: String,
    },
    /// Replaces a leaf by a split (ratio 0.5) of that leaf and a new one holding `terminal`.
    SplitLeaf {
        leaf: String,
        dir: Dir,
        split: String,
        new_leaf: String,
        terminal: String,
    },
    SetRatio {
        node: String,
        ratio: f64,
    },
    /// The sibling takes the parent's place. The last leaf of a tab removes the tab.
    CloseLeaf {
        leaf: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpError {
    NoSuchSpace(String),
    NoSuchTab(String),
    NoSuchNode(String),
    NotALeaf(String),
    NotASplit(String),
    /// An id the operation introduces already exists in the document.
    DuplicateId(String),
    /// The terminal already sits in another leaf.
    TerminalInUse(String),
    BadRatio,
    TooDeep,
}

impl OpError {
    pub fn code(&self) -> &'static str {
        match self {
            OpError::NoSuchSpace(_) => "no_such_space",
            OpError::NoSuchTab(_) => "no_such_tab",
            OpError::NoSuchNode(_) => "no_such_node",
            OpError::NotALeaf(_) => "not_a_leaf",
            OpError::NotASplit(_) => "not_a_split",
            OpError::DuplicateId(_) => "duplicate_id",
            OpError::TerminalInUse(_) => "terminal_in_use",
            OpError::BadRatio => "bad_ratio",
            OpError::TooDeep => "too_deep",
        }
    }
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpError::NoSuchSpace(id) => write!(f, "no space {id}"),
            OpError::NoSuchTab(id) => write!(f, "no tab {id}"),
            OpError::NoSuchNode(id) => write!(f, "no node {id}"),
            OpError::NotALeaf(id) => write!(f, "node {id} is not a leaf"),
            OpError::NotASplit(id) => write!(f, "node {id} is not a split"),
            OpError::DuplicateId(id) => write!(f, "id {id} already exists"),
            OpError::TerminalInUse(t) => write!(f, "terminal {t} is already in a leaf"),
            OpError::BadRatio => write!(f, "ratio is not a number"),
            OpError::TooDeep => write!(f, "layout nests deeper than {MAX_LAYOUT_DEPTH}"),
        }
    }
}

impl std::error::Error for OpError {}

/// What a successfully applied operation did beyond editing the tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Terminals whose leaves the operation removed. Whether to end them is up to the caller
    /// and the UI (Q9: closing a tab or leaf ends its terminal; removing a space by default only
    /// takes its terminals out of the layout).
    pub removed_terminals: Vec<String>,
    /// Leaves left vacant by the one-terminal-per-leaf rule.
    pub displaced: Vec<String>,
}

fn require_new_id(ids: &HashSet<String>, id: &str) -> Result<(), OpError> {
    if ids.contains(id) {
        Err(OpError::DuplicateId(id.to_string()))
    } else {
        Ok(())
    }
}

fn require_free_terminal(doc: &LayoutDoc, terminal: &str) -> Result<(), OpError> {
    if doc.terminals().iter().any(|t| t == terminal) {
        Err(OpError::TerminalInUse(terminal.to_string()))
    } else {
        Ok(())
    }
}

fn space_mut<'a>(doc: &'a mut LayoutDoc, id: &str) -> Result<&'a mut Space, OpError> {
    doc.spaces
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| OpError::NoSuchSpace(id.to_string()))
}

fn terminals_of(tab: &Tab) -> Vec<String> {
    let mut out = Vec::new();
    tab.layout
        .leaves(&mut |leaf| out.extend(leaf.terminal.clone()));
    out
}

/// Removes tab `index`; an active tab that goes passes to its neighbor.
fn remove_tab_at(space: &mut Space, index: usize) -> Vec<String> {
    let tab = space.tabs.remove(index);
    if space.active_tab.as_deref() == Some(tab.id.as_str()) {
        space.active_tab = space
            .tabs
            .get(index)
            .or_else(|| space.tabs.last())
            .map(|t| t.id.clone());
    }
    terminals_of(&tab)
}

/// Removes leaf `id` under `node` (which must be a split), the sibling replacing the parent.
fn remove_leaf(node: &mut Node, id: &str) -> Option<Leaf> {
    let a_first = match node {
        Node::Split(split) => match (&*split.a, &*split.b) {
            (Node::Leaf(leaf), _) if leaf.id == id => Some(true),
            (_, Node::Leaf(leaf)) if leaf.id == id => Some(false),
            _ => None,
        },
        _ => return None,
    };
    match a_first {
        Some(a_first) => {
            let Node::Split(split) = std::mem::replace(node, Node::Unknown(Value::Null)) else {
                return None;
            };
            let (gone, kept) = if a_first {
                (split.a, split.b)
            } else {
                (split.b, split.a)
            };
            *node = *kept;
            match *gone {
                Node::Leaf(leaf) => Some(leaf),
                _ => None,
            }
        }
        None => match node {
            Node::Split(split) => {
                remove_leaf(&mut split.a, id).or_else(|| remove_leaf(&mut split.b, id))
            }
            _ => None,
        },
    }
}

/// Applies `op` to `doc`, then re-establishes the invariants.
pub fn apply(doc: &mut LayoutDoc, op: &Op) -> Result<Applied, OpError> {
    let mut applied = Applied::default();
    match op {
        Op::AddSpace {
            id,
            name,
            root,
            color,
        } => {
            require_new_id(&doc.all_ids(), id)?;
            doc.spaces.push(Space {
                id: id.clone(),
                name: name.clone(),
                color: color.clone(),
                root: Some(root.clone()),
                active_tab: None,
                tabs: Vec::new(),
                extra: Map::new(),
            });
        }
        Op::RenameSpace { space, name } => space_mut(doc, space)?.name = name.clone(),
        Op::RemoveSpace { space } => {
            let index = doc
                .spaces
                .iter()
                .position(|s| &s.id == space)
                .ok_or_else(|| OpError::NoSuchSpace(space.clone()))?;
            let removed = doc.spaces.remove(index);
            applied.removed_terminals = removed.tabs.iter().flat_map(terminals_of).collect();
        }
        Op::AddTab {
            space,
            tab,
            title,
            leaf,
            terminal,
        } => {
            let ids = doc.all_ids();
            require_new_id(&ids, tab)?;
            require_new_id(&ids, leaf)?;
            if tab == leaf {
                return Err(OpError::DuplicateId(leaf.clone()));
            }
            require_free_terminal(doc, terminal)?;
            let space = space_mut(doc, space)?;
            space.tabs.push(Tab {
                id: tab.clone(),
                title: title.clone(),
                layout: Node::Leaf(Leaf {
                    id: leaf.clone(),
                    terminal: Some(terminal.clone()),
                    extra: Map::new(),
                }),
                extra: Map::new(),
            });
            space.active_tab = Some(tab.clone());
        }
        Op::RemoveTab { space, tab } => {
            let space = space_mut(doc, space)?;
            let index = space
                .tabs
                .iter()
                .position(|t| &t.id == tab)
                .ok_or_else(|| OpError::NoSuchTab(tab.clone()))?;
            applied.removed_terminals = remove_tab_at(space, index);
        }
        Op::SetActiveTab { space, tab } => {
            let space = space_mut(doc, space)?;
            if !space.tabs.iter().any(|t| &t.id == tab) {
                return Err(OpError::NoSuchTab(tab.clone()));
            }
            space.active_tab = Some(tab.clone());
        }
        Op::SplitLeaf {
            leaf,
            dir,
            split,
            new_leaf,
            terminal,
        } => {
            let ids = doc.all_ids();
            require_new_id(&ids, split)?;
            require_new_id(&ids, new_leaf)?;
            if split == new_leaf {
                return Err(OpError::DuplicateId(split.clone()));
            }
            require_free_terminal(doc, terminal)?;
            let (node, depth) = doc
                .spaces
                .iter_mut()
                .flat_map(|s| s.tabs.iter_mut())
                .find_map(|t| t.layout.find_mut(leaf, 1))
                .ok_or_else(|| OpError::NoSuchNode(leaf.clone()))?;
            if !matches!(node, Node::Leaf(_)) {
                return Err(OpError::NotALeaf(leaf.clone()));
            }
            if depth + 1 > MAX_LAYOUT_DEPTH {
                return Err(OpError::TooDeep);
            }
            let old = std::mem::replace(node, Node::Unknown(Value::Null));
            *node = Node::Split(Split {
                id: split.clone(),
                dir: *dir,
                ratio: 0.5,
                a: Box::new(old),
                b: Box::new(Node::Leaf(Leaf {
                    id: new_leaf.clone(),
                    terminal: Some(terminal.clone()),
                    extra: Map::new(),
                })),
                extra: Map::new(),
            });
        }
        Op::SetRatio { node, ratio } => {
            if !ratio.is_finite() {
                return Err(OpError::BadRatio);
            }
            let (found, _) = doc
                .spaces
                .iter_mut()
                .flat_map(|s| s.tabs.iter_mut())
                .find_map(|t| t.layout.find_mut(node, 1))
                .ok_or_else(|| OpError::NoSuchNode(node.clone()))?;
            match found {
                Node::Split(split) => split.ratio = ratio.clamp(MIN_RATIO, MAX_RATIO),
                _ => return Err(OpError::NotASplit(node.clone())),
            }
        }
        Op::CloseLeaf { leaf } => {
            let mut closed = None;
            'spaces: for space in &mut doc.spaces {
                for index in 0..space.tabs.len() {
                    let root = &mut space.tabs[index].layout;
                    if matches!(root, Node::Leaf(l) if &l.id == leaf) {
                        closed = Some(remove_tab_at(space, index));
                        break 'spaces;
                    }
                    if let Some(gone) = remove_leaf(root, leaf) {
                        closed = Some(gone.terminal.into_iter().collect());
                        break 'spaces;
                    }
                }
            }
            match closed {
                Some(terminals) => applied.removed_terminals = terminals,
                None => {
                    let is_split = doc
                        .spaces
                        .iter_mut()
                        .flat_map(|s| s.tabs.iter_mut())
                        .find_map(|t| t.layout.find_mut(leaf, 1))
                        .is_some();
                    return Err(if is_split {
                        OpError::NotALeaf(leaf.clone())
                    } else {
                        OpError::NoSuchNode(leaf.clone())
                    });
                }
            }
        }
    }
    applied.displaced = doc.normalize();
    Ok(applied)
}

// ---------------------------------------------------------------------------------- updating

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocError {
    /// Larger than [`MAX_BYTES`] once serialized: never sent.
    TooLarge(usize),
    /// Shown read-only, never overwritten.
    ReadOnly(ReadOnlyReason),
    Op(OpError),
    Rpc(RpcError),
    /// The connection failed (`ClientError::code()` of the cause).
    Transport(String),
    /// Still conflicting after the allowed rounds: the operation is dropped, read again.
    GaveUp {
        attempts: usize,
    },
    /// The layout could not be serialized.
    Invalid(String),
}

impl DocError {
    pub fn code(&self) -> &'static str {
        match self {
            DocError::TooLarge(_) => "too_large",
            DocError::ReadOnly(_) => "read_only",
            DocError::Op(_) => "op",
            DocError::Rpc(_) => "rpc",
            DocError::Transport(_) => "transport",
            DocError::GaveUp { .. } => "gave_up",
            DocError::Invalid(_) => "invalid",
        }
    }
}

impl std::fmt::Display for DocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DocError::TooLarge(n) => write!(f, "layout of {n} bytes exceeds {MAX_BYTES}"),
            DocError::ReadOnly(reason) => write!(f, "layout is read-only ({})", reason.code()),
            DocError::Op(e) => e.fmt(f),
            DocError::Rpc(e) => e.fmt(f),
            DocError::Transport(code) => write!(f, "connection failed ({code})"),
            DocError::GaveUp { attempts } => {
                write!(f, "layout kept conflicting ({attempts} tries)")
            }
            DocError::Invalid(m) => write!(f, "layout cannot be encoded: {m}"),
        }
    }
}

impl std::error::Error for DocError {}

/// Why a document call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IoFailure {
    /// The core answered with an error (`conflict` is retried by [`update_with`]).
    Rpc(RpcError),
    /// The call never got an answer; the stable code of the cause.
    Transport(String),
}

impl IoFailure {
    fn into_error(self) -> DocError {
        match self {
            IoFailure::Rpc(e) => DocError::Rpc(e),
            IoFailure::Transport(code) => DocError::Transport(code),
        }
    }
}

/// What the update loop needs from a connection.
pub trait DocIo {
    /// `doc.get`: the value (`null` when unwritten) and its version (0 when unwritten).
    fn get(&mut self, name: &str) -> Result<(Value, u64), IoFailure>;
    /// `doc.set` with the version read; `conflict` when someone else wrote since.
    fn set(&mut self, name: &str, value: &Value, version: u64) -> Result<u64, IoFailure>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct Updated {
    /// The version just written.
    pub version: u64,
    /// Read-apply-write rounds used (1: no conflict).
    pub attempts: usize,
    pub doc: LayoutDoc,
    pub applied: Applied,
}

/// Read, change, write; on `conflict` read again and run `change` on the fresh value (never a
/// blind retry with the new version).
pub fn update_with(
    io: &mut impl DocIo,
    name: &str,
    max_attempts: usize,
    mut change: impl FnMut(&mut LayoutDoc) -> Result<Applied, OpError>,
) -> Result<Updated, DocError> {
    for attempt in 1..=max_attempts {
        let (value, version) = io.get(name).map_err(IoFailure::into_error)?;
        let (mut doc, decode_displaced) = match decode(&value) {
            LayoutState::Empty => (LayoutDoc::empty(), Vec::new()),
            LayoutState::Doc { doc, displaced } => (doc, displaced),
            LayoutState::ReadOnly(reason) => return Err(DocError::ReadOnly(reason)),
        };
        let mut applied = change(&mut doc).map_err(DocError::Op)?;
        // What decoding vacated is written now too: report it with the rest.
        applied.displaced.splice(0..0, decode_displaced);
        let next = encode(&doc)?;
        match io.set(name, &next, version) {
            Ok(version) => {
                return Ok(Updated {
                    version,
                    attempts: attempt,
                    doc,
                    applied,
                })
            }
            Err(IoFailure::Rpc(e)) if e.is_conflict() => continue,
            Err(e) => return Err(e.into_error()),
        }
    }
    Err(DocError::GaveUp {
        attempts: max_attempts,
    })
}

/// [`update_with`] for one [`Op`].
pub fn update(
    io: &mut impl DocIo,
    name: &str,
    max_attempts: usize,
    op: &Op,
) -> Result<Updated, DocError> {
    update_with(io, name, max_attempts, |doc| apply(doc, op))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use serde_json::json;

    use super::*;
    use crate::herdr_core::fixtures;

    /// Document-name rules (upstream `api/doc.rs`), kept here to pin `DOC_NAME` against them.
    fn valid_name(name: &str) -> bool {
        let allowed =
            |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.');
        let starts = name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit());
        let id_shaped = name.len() >= 2
            && name.starts_with('t')
            && name[1..].chars().next().is_some_and(|c| c.is_ascii_digit());
        starts && name.len() <= 32 && name.chars().all(allowed) && !id_shaped
    }

    type CompetingWriter = Box<dyn FnOnce(&mut MemoryIo)>;

    /// An in-memory core document store with the upstream version rules.
    #[derive(Default)]
    struct MemoryIo {
        docs: std::collections::BTreeMap<String, (Value, u64)>,
        sets: usize,
        /// Runs once before the next `set`: a competing writer.
        before_set: Option<CompetingWriter>,
        /// Conflict this many times in a row, whatever the version.
        always_conflict: bool,
    }

    impl MemoryIo {
        fn put(&mut self, name: &str, value: Value) {
            let version = self.docs.get(name).map_or(0, |d| d.1) + 1;
            self.docs.insert(name.into(), (value, version));
        }
    }

    impl DocIo for MemoryIo {
        fn get(&mut self, name: &str) -> Result<(Value, u64), IoFailure> {
            Ok(self.docs.get(name).cloned().unwrap_or((Value::Null, 0)))
        }

        fn set(&mut self, name: &str, value: &Value, version: u64) -> Result<u64, IoFailure> {
            if let Some(hook) = self.before_set.take() {
                hook(self);
            }
            self.sets += 1;
            let current = self.docs.get(name).map_or(0, |d| d.1);
            if self.always_conflict || current != version {
                return Err(IoFailure::Rpc(RpcError {
                    code: -32000,
                    message: format!("document \"{name}\" is at version {current}, not {version}"),
                    kind: "conflict".into(),
                    hint: String::new(),
                }));
            }
            self.put(name, value.clone());
            Ok(current + 1)
        }
    }

    fn add_space(id: &str, root: &str) -> Op {
        Op::AddSpace {
            id: id.into(),
            name: format!("repo {id}"),
            root: root.into(),
            color: None,
        }
    }

    fn add_tab(space: &str, tab: &str, leaf: &str, terminal: &str) -> Op {
        Op::AddTab {
            space: space.into(),
            tab: tab.into(),
            title: format!("tab {tab}"),
            leaf: leaf.into(),
            terminal: terminal.into(),
        }
    }

    fn split(leaf: &str, id: &str, new_leaf: &str, terminal: &str) -> Op {
        Op::SplitLeaf {
            leaf: leaf.into(),
            dir: Dir::H,
            split: id.into(),
            new_leaf: new_leaf.into(),
            terminal: terminal.into(),
        }
    }

    /// s1 { tab t1 { split n1 [ leaf l1(t1), split n2 [ leaf l2(t2), leaf l3(t3) ] ] } }
    fn sample() -> LayoutDoc {
        let mut doc = LayoutDoc::empty();
        for op in [
            add_space("s1", "/work/a"),
            add_tab("s1", "tab1", "l1", "t1"),
            split("l1", "n1", "l2", "t2"),
            split("l2", "n2", "l3", "t3"),
        ] {
            apply(&mut doc, &op).unwrap();
        }
        doc
    }

    fn tab_layout(doc: &LayoutDoc) -> &Node {
        &doc.spaces[0].tabs[0].layout
    }

    #[test]
    fn name_rules_match_upstream() {
        assert!(valid_name(DOC_NAME));
        assert!(valid_name("yuzora.layout"));
        for bad in ["", "Yuzora", "t7", "-x", "a/b", &"x".repeat(33)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn layout_round_trips_with_the_documented_shape() {
        let doc = sample();
        let value = encode(&doc).unwrap();
        assert_eq!(value["schema"], 1);
        assert_eq!(value["app"], "yuzora");
        let space = &value["spaces"][0];
        assert_eq!(
            (space["id"].as_str(), space["root"].as_str()),
            (Some("s1"), Some("/work/a"))
        );
        assert_eq!(space["activeTab"], "tab1");
        let layout = &space["tabs"][0]["layout"];
        assert_eq!(layout["kind"], "split");
        assert_eq!(layout["dir"], "h");
        assert_eq!(layout["ratio"], 0.5);
        assert_eq!(
            layout["a"],
            json!({"id": "l1", "kind": "leaf", "terminal": "t1"})
        );
        assert_eq!(layout["b"]["kind"], "split");
        match decode(&value) {
            LayoutState::Doc {
                doc: back,
                displaced,
            } => {
                assert_eq!(back, doc);
                assert!(displaced.is_empty());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(doc.terminals(), ["t1", "t2", "t3"]);
        assert_eq!(decode(&Value::Null), LayoutState::Empty);
    }

    /// A document as a newer writer of the same schema could have written it: an unknown field
    /// on every level, and one node of a kind this build does not know.
    fn doc_with_foreign_fields() -> Value {
        json!({
            "schema": 1, "app": "yuzora", "pinned": ["s1"],
            "spaces": [{
                "id": "s1", "name": "a", "root": "/a", "activeTab": "tab1", "icon": "bolt",
                "tabs": [{
                    "id": "tab1", "title": "t", "badge": 3,
                    "layout": {
                        "kind": "split", "id": "n1", "dir": "v", "ratio": 0.4, "minPx": 80,
                        "a": {"kind": "leaf", "id": "l1", "terminal": "t1", "zoom": true},
                        "b": {"kind": "stack", "id": "k1", "children": ["t9"]}
                    }
                }]
            }]
        })
    }

    #[test]
    fn unknown_fields_survive_at_every_level_and_through_a_rewrite() {
        let mut io = MemoryIo::default();
        io.put(DOC_NAME, doc_with_foreign_fields());
        // Rename (space), set ratio (split), split the known leaf: each rewrites the document.
        for op in [
            Op::RenameSpace {
                space: "s1".into(),
                name: "renamed".into(),
            },
            Op::SetRatio {
                node: "n1".into(),
                ratio: 0.3,
            },
            split("l1", "n2", "l2", "t2"),
        ] {
            update(&mut io, DOC_NAME, 3, &op).unwrap();
        }
        let value = io.docs[DOC_NAME].0.clone();
        assert_eq!(value["pinned"], json!(["s1"]), "root");
        assert_eq!(value["spaces"][0]["icon"], "bolt", "space");
        assert_eq!(value["spaces"][0]["name"], "renamed");
        let tab = &value["spaces"][0]["tabs"][0];
        assert_eq!(tab["badge"], 3, "tab");
        assert_eq!(tab["layout"]["minPx"], 80, "split");
        assert_eq!(tab["layout"]["ratio"], 0.3);
        // The known leaf now sits under the new split, with its own unknown field intact.
        let wrapped = &tab["layout"]["a"];
        assert_eq!(wrapped["id"], "n2");
        assert_eq!(
            wrapped["a"],
            json!({"id": "l1", "kind": "leaf", "terminal": "t1", "zoom": true})
        );
        // The node of an unknown kind is untouched.
        assert_eq!(
            tab["layout"]["b"],
            json!({"kind": "stack", "id": "k1", "children": ["t9"]})
        );
    }

    #[test]
    fn ids_are_stable_and_new_nodes_get_the_ids_the_op_names() {
        let doc = sample();
        let Node::Split(root) = tab_layout(&doc) else {
            panic!("root is a split");
        };
        assert_eq!(root.id, "n1");
        assert_eq!(root.a.id(), Some("l1"));
        let Node::Split(inner) = &*root.b else {
            panic!("inner split");
        };
        assert_eq!(
            (inner.id.as_str(), inner.a.id(), inner.b.id()),
            ("n2", Some("l2"), Some("l3"))
        );
        // Splitting l1 again keeps l1's id and wraps it.
        let mut doc = doc;
        apply(&mut doc, &split("l1", "n3", "l4", "t4")).unwrap();
        let ids = doc.all_ids();
        for id in ["s1", "tab1", "n1", "n2", "n3", "l1", "l2", "l3", "l4"] {
            assert!(ids.contains(id), "{id}");
        }
        assert_eq!(new_id("n").len(), 14);
        assert_ne!(new_id("n"), new_id("n"));
    }

    #[test]
    fn a_terminal_sits_in_one_leaf_only() {
        let mut doc = sample();
        // Ops refuse to place a terminal twice.
        assert_eq!(
            apply(&mut doc, &split("l1", "n9", "l9", "t2")),
            Err(OpError::TerminalInUse("t2".into()))
        );
        assert_eq!(
            apply(&mut doc, &add_tab("s1", "tab2", "l8", "t3")),
            Err(OpError::TerminalInUse("t3".into()))
        );
        assert_eq!(doc, sample(), "a refused op changes nothing");
        // A document two writers made inconsistent is repaired on read: the first leaf in
        // document order keeps the terminal, the later one is vacated.
        let mut broken = encode(&sample()).unwrap();
        broken["spaces"][0]["tabs"][0]["layout"]["b"]["b"]["terminal"] = json!("t1");
        let LayoutState::Doc { doc, displaced } = decode(&broken) else {
            panic!("decodes");
        };
        assert_eq!(displaced, ["l3"]);
        assert_eq!(doc.terminals(), ["t1", "t2"]);
        // ... and the repair is what the next write persists.
        let mut io = MemoryIo::default();
        io.put(DOC_NAME, broken);
        let updated = update(
            &mut io,
            DOC_NAME,
            3,
            &Op::RenameSpace {
                space: "s1".into(),
                name: "x".into(),
            },
        )
        .unwrap();
        assert_eq!(updated.applied.displaced, ["l3"]);
        assert_eq!(
            io.docs[DOC_NAME].0["spaces"][0]["tabs"][0]["layout"]["b"]["b"],
            json!({"id": "l3", "kind": "leaf", "terminal": null})
        );
        // Across spaces too.
        let mut doc = sample();
        apply(&mut doc, &add_space("s2", "/work/b")).unwrap();
        assert_eq!(
            apply(&mut doc, &add_tab("s2", "tab9", "l9", "t1")),
            Err(OpError::TerminalInUse("t1".into()))
        );
    }

    #[test]
    fn a_conflict_reads_again_and_applies_the_op_to_the_newer_value() {
        let mut io = MemoryIo::default();
        update(&mut io, DOC_NAME, 3, &add_space("s1", "/a")).unwrap();
        // While we apply `rename`, another writer adds a space and a second one a tab.
        io.before_set = Some(Box::new(|io| {
            let (value, _) = io.docs[DOC_NAME].clone();
            let LayoutState::Doc { mut doc, .. } = decode(&value) else {
                panic!()
            };
            apply(&mut doc, &add_space("s2", "/b")).unwrap();
            apply(&mut doc, &add_tab("s2", "tabX", "lX", "t50")).unwrap();
            io.put(DOC_NAME, encode(&doc).unwrap());
        }));
        let updated = update(
            &mut io,
            DOC_NAME,
            3,
            &Op::RenameSpace {
                space: "s1".into(),
                name: "mine".into(),
            },
        )
        .unwrap();
        assert_eq!(updated.attempts, 2);
        assert_eq!(io.sets, 3, "1 setup + conflicted + retried");
        // Both writers' work is in the result; a blind retry would have dropped s2.
        let ids: Vec<&str> = updated.doc.spaces.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["s1", "s2"]);
        assert_eq!(updated.doc.spaces[0].name, "mine");
        assert_eq!(updated.doc.terminals(), ["t50"]);
        assert_eq!(updated.version, 3);
    }

    #[test]
    fn a_conflict_that_invalidates_the_op_reports_the_op_error_not_gave_up() {
        let mut io = MemoryIo::default();
        update(&mut io, DOC_NAME, 3, &add_space("s1", "/a")).unwrap();
        io.before_set = Some(Box::new(|io| {
            let (value, _) = io.docs[DOC_NAME].clone();
            let LayoutState::Doc { mut doc, .. } = decode(&value) else {
                panic!()
            };
            apply(&mut doc, &Op::RemoveSpace { space: "s1".into() }).unwrap();
            io.put(DOC_NAME, encode(&doc).unwrap());
        }));
        let err = update(
            &mut io,
            DOC_NAME,
            3,
            &Op::RenameSpace {
                space: "s1".into(),
                name: "mine".into(),
            },
        )
        .unwrap_err();
        assert_eq!(err, DocError::Op(OpError::NoSuchSpace("s1".into())));
    }

    #[test]
    fn it_gives_up_after_the_allowed_rounds() {
        let mut io = MemoryIo {
            always_conflict: true,
            ..MemoryIo::default()
        };
        let err = update(&mut io, DOC_NAME, 4, &add_space("s1", "/a")).unwrap_err();
        assert_eq!(err, DocError::GaveUp { attempts: 4 });
        assert_eq!(io.sets, 4);
        assert_eq!(err.code(), "gave_up");
        // Zero rounds never touches the core.
        let mut untouched = MemoryIo::default();
        assert_eq!(
            update(&mut untouched, DOC_NAME, 0, &add_space("s1", "/a")).unwrap_err(),
            DocError::GaveUp { attempts: 0 }
        );
    }

    #[test]
    fn newer_schema_and_foreign_documents_are_never_written() {
        for (value, reason) in [
            (
                json!({"schema": 2, "app": "yuzora", "spaces": []}),
                ReadOnlyReason::NewerSchema { schema: 2 },
            ),
            (
                json!({"schema": 1, "app": "herdr-app"}),
                ReadOnlyReason::ForeignApp {
                    app: Some("herdr-app".into()),
                },
            ),
            (
                json!({"schema": 1}),
                ReadOnlyReason::ForeignApp { app: None },
            ),
            (
                json!({"schema": 1, "app": "yuzora", "spaces": "nope"}),
                ReadOnlyReason::Invalid {
                    message: "invalid type: string \"nope\", expected a sequence".into(),
                },
            ),
            (
                json!({"app": "yuzora"}),
                ReadOnlyReason::Invalid {
                    message: "no schema".into(),
                },
            ),
        ] {
            let mut io = MemoryIo::default();
            io.put(DOC_NAME, value.clone());
            let err = update(&mut io, DOC_NAME, 3, &add_space("s1", "/a")).unwrap_err();
            assert_eq!(err, DocError::ReadOnly(reason.clone()), "{value}");
            assert_eq!(io.sets, 0, "{value}: nothing was written");
            assert!(matches!(decode(&value), LayoutState::ReadOnly(r) if r == reason));
        }
    }

    #[test]
    fn a_layout_state_serializes_with_one_kind_and_a_separate_reason() {
        // `ReadOnly(reason)` nests one internally tagged enum in another: the two tags must
        // not share a name, or the text carries `kind` twice and `to_value` keeps only the last.
        let cases = [
            (
                LayoutState::ReadOnly(ReadOnlyReason::NewerSchema { schema: 2 }),
                r#"{"kind":"readOnly","reason":"newerSchema","schema":2}"#,
            ),
            (
                LayoutState::ReadOnly(ReadOnlyReason::ForeignApp {
                    app: Some("herdr-app".into()),
                }),
                r#"{"kind":"readOnly","reason":"foreignApp","app":"herdr-app"}"#,
            ),
            (
                LayoutState::ReadOnly(ReadOnlyReason::ForeignApp { app: None }),
                r#"{"kind":"readOnly","reason":"foreignApp","app":null}"#,
            ),
            (
                LayoutState::ReadOnly(ReadOnlyReason::Invalid {
                    message: "no schema".into(),
                }),
                r#"{"kind":"readOnly","reason":"invalid","message":"no schema"}"#,
            ),
            (LayoutState::Empty, r#"{"kind":"empty"}"#),
        ];
        for (state, text) in cases {
            let serialized = serde_json::to_string(&state).unwrap();
            assert_eq!(serialized, text);
            assert_eq!(
                serialized.matches("\"kind\"").count(),
                1,
                "{serialized}: one `kind` key"
            );
            // The value form agrees with the text form: nothing was overwritten on the way.
            let value = serde_json::to_value(&state).unwrap();
            assert_eq!(value, serde_json::from_str::<Value>(text).unwrap());
        }
        // The document state keeps its own tag and its own fields.
        let state = decode(&json!({"schema": 1, "app": "yuzora", "spaces": []}));
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["kind"], "doc");
        assert_eq!(value["displaced"], json!([]));
        assert_eq!(value["doc"]["app"], "yuzora");
    }

    #[test]
    fn the_size_limit_is_checked_before_sending() {
        let mut io = MemoryIo::default();
        let mut huge = LayoutDoc::empty();
        huge.extra
            .insert("pad".into(), json!("x".repeat(MAX_BYTES)));
        io.put(DOC_NAME, serde_json::to_value(&huge).unwrap());
        let err = update(&mut io, DOC_NAME, 3, &add_space("s1", "/a")).unwrap_err();
        assert!(
            matches!(err, DocError::TooLarge(n) if n > MAX_BYTES),
            "{err:?}"
        );
        assert_eq!(io.sets, 0);
        // A real layout is far from the limit.
        let one = serde_json::to_vec(&encode(&sample()).unwrap())
            .unwrap()
            .len();
        assert!(MAX_BYTES / one > 1000, "one space is {one} bytes");
    }

    #[test]
    fn closing_collapses_the_tree_and_the_last_leaf_removes_the_tab() {
        let mut doc = sample();
        // Close l2: its sibling l3 takes the inner split's place.
        let applied = apply(&mut doc, &Op::CloseLeaf { leaf: "l2".into() }).unwrap();
        assert_eq!(applied.removed_terminals, ["t2"]);
        let Node::Split(root) = tab_layout(&doc) else {
            panic!()
        };
        assert_eq!((root.a.id(), root.b.id()), (Some("l1"), Some("l3")));
        // Close l1 and then l3: the tab goes with its last leaf.
        apply(&mut doc, &Op::CloseLeaf { leaf: "l1".into() }).unwrap();
        assert_eq!(tab_layout(&doc).id(), Some("l3"));
        let applied = apply(&mut doc, &Op::CloseLeaf { leaf: "l3".into() }).unwrap();
        assert_eq!(applied.removed_terminals, ["t3"]);
        assert!(doc.spaces[0].tabs.is_empty());
        assert_eq!(doc.spaces[0].active_tab, None);
        // Targets must exist and be what the op needs.
        let mut doc = sample();
        assert_eq!(
            apply(&mut doc, &Op::CloseLeaf { leaf: "zz".into() }),
            Err(OpError::NoSuchNode("zz".into()))
        );
        assert_eq!(
            apply(&mut doc, &Op::CloseLeaf { leaf: "n1".into() }),
            Err(OpError::NotALeaf("n1".into()))
        );
        assert_eq!(
            apply(&mut doc, &split("n1", "n5", "l5", "t5")),
            Err(OpError::NotALeaf("n1".into()))
        );
    }

    #[test]
    fn tabs_spaces_and_active_tab_follow_the_ops() {
        let mut doc = LayoutDoc::empty();
        apply(&mut doc, &add_space("s1", "/a")).unwrap();
        for (tab, leaf, terminal) in [("a", "la", "t1"), ("b", "lb", "t2"), ("c", "lc", "t3")] {
            apply(&mut doc, &add_tab("s1", tab, leaf, terminal)).unwrap();
        }
        assert_eq!(
            doc.spaces[0].active_tab.as_deref(),
            Some("c"),
            "a new tab is active"
        );
        apply(
            &mut doc,
            &Op::SetActiveTab {
                space: "s1".into(),
                tab: "a".into(),
            },
        )
        .unwrap();
        assert_eq!(doc.spaces[0].active_tab.as_deref(), Some("a"));
        // Removing the active tab hands the role to its neighbor.
        let applied = apply(
            &mut doc,
            &Op::RemoveTab {
                space: "s1".into(),
                tab: "a".into(),
            },
        )
        .unwrap();
        assert_eq!(applied.removed_terminals, ["t1"]);
        assert_eq!(doc.spaces[0].active_tab.as_deref(), Some("b"));
        apply(
            &mut doc,
            &Op::RemoveTab {
                space: "s1".into(),
                tab: "c".into(),
            },
        )
        .unwrap();
        assert_eq!(doc.spaces[0].active_tab.as_deref(), Some("b"));
        assert_eq!(
            apply(
                &mut doc,
                &Op::SetActiveTab {
                    space: "s1".into(),
                    tab: "zz".into()
                }
            ),
            Err(OpError::NoSuchTab("zz".into()))
        );
        // Removing a space reports every terminal in it.
        apply(&mut doc, &add_tab("s1", "d", "ld", "t9")).unwrap();
        let applied = apply(&mut doc, &Op::RemoveSpace { space: "s1".into() }).unwrap();
        assert_eq!(applied.removed_terminals, ["t2", "t9"]);
        assert!(doc.spaces.is_empty());
        // Duplicate ids are refused, whichever level they collide on.
        let mut doc = sample();
        for op in [
            add_space("tab1", "/x"),
            add_tab("s1", "s1", "lnew", "t70"),
            add_tab("s1", "tab7", "tab7", "t70"),
            split("l1", "n1", "lnew", "t70"),
            split("l1", "lnew", "lnew", "t70"),
        ] {
            assert!(
                matches!(apply(&mut doc, &op), Err(OpError::DuplicateId(_))),
                "{op:?}"
            );
        }
        assert_eq!(doc, sample());
    }

    #[test]
    fn ratios_are_clamped_and_must_be_numbers() {
        let mut doc = sample();
        let set = |doc: &mut LayoutDoc, ratio: f64| {
            apply(
                doc,
                &Op::SetRatio {
                    node: "n1".into(),
                    ratio,
                },
            )
        };
        set(&mut doc, 0.3).unwrap();
        let ratio = |doc: &LayoutDoc| match tab_layout(doc) {
            Node::Split(s) => s.ratio,
            _ => panic!(),
        };
        assert_eq!(ratio(&doc), 0.3);
        set(&mut doc, 0.0).unwrap();
        assert_eq!(ratio(&doc), MIN_RATIO);
        set(&mut doc, 7.0).unwrap();
        assert_eq!(ratio(&doc), MAX_RATIO);
        assert_eq!(set(&mut doc, f64::NAN), Err(OpError::BadRatio));
        assert_eq!(set(&mut doc, f64::INFINITY), Err(OpError::BadRatio));
        assert_eq!(
            apply(
                &mut doc,
                &Op::SetRatio {
                    node: "l1".into(),
                    ratio: 0.5
                }
            ),
            Err(OpError::NotASplit("l1".into()))
        );
        assert_eq!(
            apply(
                &mut doc,
                &Op::SetRatio {
                    node: "nope".into(),
                    ratio: 0.5
                }
            ),
            Err(OpError::NoSuchNode("nope".into()))
        );
    }

    #[test]
    fn splitting_stops_at_the_layout_depth_limit() {
        let mut doc = LayoutDoc::empty();
        apply(&mut doc, &add_space("s1", "/a")).unwrap();
        apply(&mut doc, &add_tab("s1", "tab1", "l0", "t0")).unwrap();
        // Always split the newest leaf: depth grows by one each time.
        let mut last = "l0".to_string();
        let mut depth = 1;
        loop {
            let next = format!("l{}", depth);
            let op = split(&last, &format!("n{depth}"), &next, &format!("t{depth}"));
            match apply(&mut doc, &op) {
                Ok(_) => {
                    depth += 1;
                    last = next;
                }
                Err(e) => {
                    assert_eq!(e, OpError::TooDeep);
                    break;
                }
            }
        }
        assert_eq!(depth, MAX_LAYOUT_DEPTH);
        assert_eq!(tab_layout(&doc).depth(), MAX_LAYOUT_DEPTH);
        // The result still round-trips through the core's JSON depth limit.
        let value = encode(&doc).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&value.to_string()).unwrap(),
            value
        );
    }

    #[test]
    fn ops_parse_from_the_frontend_json() {
        let op: Op = serde_json::from_value(json!({
            "op": "splitLeaf", "leaf": "l1", "dir": "v", "split": "n1", "newLeaf": "l2", "terminal": "t2"
        }))
        .unwrap();
        assert_eq!(
            op,
            Op::SplitLeaf {
                leaf: "l1".into(),
                dir: Dir::V,
                split: "n1".into(),
                new_leaf: "l2".into(),
                terminal: "t2".into()
            }
        );
        let op: Op =
            serde_json::from_value(json!({"op": "addSpace", "id": "s", "name": "n", "root": "/r"}))
                .unwrap();
        assert!(matches!(op, Op::AddSpace { color: None, .. }));
        assert!(serde_json::from_value::<Op>(json!({"op": "dropTable"})).is_err());
    }

    /// Replays a recorded doc exchange: requests must match, responses come from the recording.
    struct Replay {
        steps: VecDeque<(Value, Value)>,
    }

    impl Replay {
        fn take(&mut self, method: &str, params: Value) -> Result<Value, IoFailure> {
            let (req, resp) = self.steps.pop_front().expect("more requests than recorded");
            assert_eq!(req["method"], method);
            for k in ["name", "version"] {
                if !params[k].is_null() {
                    assert_eq!(req["params"][k], params[k], "{method} {k}");
                }
            }
            match resp.get("error") {
                Some(e) => Err(IoFailure::Rpc(RpcError::from_json(e))),
                None => Ok(resp["result"].clone()),
            }
        }
    }

    impl DocIo for Replay {
        fn get(&mut self, name: &str) -> Result<(Value, u64), IoFailure> {
            let r = self.take("doc.get", json!({"name": name}))?;
            Ok((r["value"].clone(), r["version"].as_u64().unwrap()))
        }

        fn set(&mut self, name: &str, _value: &Value, version: u64) -> Result<u64, IoFailure> {
            Ok(
                self.take("doc.set", json!({"name": name, "version": version}))?["version"]
                    .as_u64()
                    .unwrap(),
            )
        }
    }

    #[test]
    fn recorded_conflict_is_resolved_by_reading_again() {
        // Recorded against the pinned core: writer A read v1, writer B wrote v2, A's set
        // conflicted, A read v2 and re-applied its change.
        let rec = fixtures::recorded("docs-conflict.ndjson");
        let pairs = rec.request_response_pairs_for("A");
        // Recorded under a scratch name (same rules as DOC_NAME) so reruns start from version 0.
        let name = pairs[0].0["params"]["name"].as_str().unwrap().to_string();
        assert!(valid_name(&name) && name.starts_with(DOC_NAME));
        let mut io = Replay {
            steps: pairs.into(),
        };
        // A's first get+set that created v1.
        assert_eq!(io.get(&name).unwrap(), (Value::Null, 0));
        assert_eq!(io.set(&name, &Value::Null, 0).unwrap(), 1);
        let mut seen = Vec::new();
        let updated = update_with(&mut io, &name, 3, |doc| {
            seen.push(doc.spaces.len());
            apply(doc, &add_space(&format!("s{}", seen.len()), "/a"))
        })
        .unwrap();
        assert_eq!(updated.attempts, 2, "first set conflicted");
        assert_eq!(updated.version, 3);
        // The change ran again on B's newer value (2 spaces), not on the stale one. The
        // recorded layouts predate the node `kind` tag: their nodes are kept as unknown.
        assert_eq!(seen, [1, 2]);
        assert_eq!(updated.doc.spaces.len(), 3);
        let conflict = rec
            .errors()
            .into_iter()
            .find(|e| e.is_conflict())
            .expect("conflict recorded");
        assert!(
            conflict.message.contains("at version 2"),
            "{}",
            conflict.message
        );
    }

    #[test]
    fn recorded_limits_match_the_constants() {
        let rec = fixtures::recorded("docs-limits.ndjson");
        let notes = rec.notes();
        // A value of exactly MAX_BYTES serialized was accepted; MAX_BYTES + 1 refused.
        let refused = notes
            .iter()
            .find_map(|n| n.get("refused_error").filter(|e| !e.is_null()))
            .expect("refused_error note");
        assert_eq!(refused["kind"], "invalid_params");
        assert!(
            refused["message"].as_str().unwrap().contains("1 MiB"),
            "{refused}"
        );
        assert!(
            notes.iter().any(|n| n["accepted_bytes"] == MAX_BYTES),
            "{notes:?}"
        );
        assert!(
            notes.iter().any(|n| n["refused_bytes"] == MAX_BYTES + 1),
            "{notes:?}"
        );
        // null deletes but keeps the version; a writer with version 0 cannot recreate it.
        assert!(notes.iter().any(|n| {
            n["after_delete"]["value"].is_null() && n["after_delete"]["version"].as_u64() > Some(0)
        }));
        assert!(rec.errors().iter().any(|e| e.is_conflict()));
        // The document survived a core restart (re-read after reconnect).
        assert!(notes
            .iter()
            .any(|n| n["after_restart_version"].as_u64().is_some()));
    }
}
