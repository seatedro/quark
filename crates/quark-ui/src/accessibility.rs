//! The accessibility tree elements build each frame, published through
//! AccessKit.
//!
//! Nodes take any `accesskit::Role` ([`AccessibilityRole`]) and publish
//! name, description, disabled, selected, checked (including mixed),
//! expanded, invalid, required, read-only, modal, range values, live
//! politeness, and focus. Text fields, the editor, and selectable text
//! publish their text as `TextRun` children with the caret and selection;
//! assistive tech can select text (`SetTextSelection`), replace the
//! selection, and set a field's value. [`Announcer`] speaks text nothing
//! on screen shows.
//!
//! What Linux screen readers get depends on accesskit_unix 0.22 /
//! accesskit_atspi_common 0.19. Served: roles, names, descriptions, the
//! states above except invalid and expanded, the Text interface (text,
//! caret, selection, set selection, word and line boundaries), the Value
//! interface for range values (set too, for nodes with
//! [`NumericActions`]), focus, and `Announcement` events. Not
//! served: EditableText (so AT-SPI clients cannot replace or set text,
//! though macOS and Windows clients can), the invalid and expanded states,
//! and character extents, since runs publish no glyph positions.
//!
//! Keyboard gaps: a clickable div needs a stable id (`id`, `test_id`,
//! `accessibility_id`) or a `focus_ring` to be a Tab stop. The radio group,
//! segmented control, select, and combobox in quark-components are one Tab
//! stop each and move with the arrow keys; tab lists and menus have no
//! arrow-key navigation, so each item is its own Tab stop. Drag-only
//! interactions have no keyboard alternative. Selectable text publishes its
//! selection but ignores selection requests, since the app owns it.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

pub use accesskit::Orientation;
pub use accesskit::Role as AccessibilityRole;
pub use accesskit::SortDirection;
use accesskit::{
    Action as AxAction, Invalid, Live, Node, NodeId, Rect as AxRect, Role, TextPosition,
    TextSelection, Toggled, Tree, TreeId, TreeUpdate,
};
use quark::SemanticRole;
use unicode_segmentation::UnicodeSegmentation;

use crate::action::{Action, FocusId};
use crate::element::{Div, ScrollActionBuilder};
use quark_render::Rect;

pub const ROOT_ID: NodeId = NodeId(1);

/// How urgently assistive tech should speak a live region's change or an
/// announcement. `Polite` waits for the current speech to finish;
/// `Assertive` interrupts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Politeness {
    Polite,
    Assertive,
}

impl From<Politeness> for Live {
    fn from(politeness: Politeness) -> Self {
        match politeness {
            Politeness::Polite => Live::Polite,
            Politeness::Assertive => Live::Assertive,
        }
    }
}

/// Where a node sits in a tree or table. Virtualized collections publish
/// only their visible rows, so the full extent and each row's place in it
/// come from these rather than from counting nodes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CollectionInfo {
    /// Depth of a tree item, 1 for top-level items.
    pub level: Option<usize>,
    /// Rows and columns of a whole table, on the table node.
    pub row_count: Option<usize>,
    pub column_count: Option<usize>,
    /// 0-based row of a table row or cell, and column of a cell or header.
    pub row_index: Option<usize>,
    pub column_index: Option<usize>,
    /// Sort state of a column header.
    pub sort: Option<SortDirection>,
    /// The container lets more than one item be selected.
    pub multiselectable: bool,
}

/// A range value (slider, progress bar, spin button).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumericValue {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub step: Option<f64>,
}

/// What assistive tech can do to a [`NumericValue`]: set it (AT-SPI's
/// `Value.SetCurrentValue`, a slider's "set value"), and, when the control
/// gives them, step it up and down. The control clamps what it is given.
#[derive(Clone)]
pub struct NumericActions {
    set: Rc<dyn Fn(f64) -> Action>,
    steps: Option<(Action, Action)>,
}

impl NumericActions {
    /// `set` builds the action that sets the value to its argument.
    pub fn new(set: impl Fn(f64) -> Action + 'static) -> Self {
        Self {
            set: Rc::new(set),
            steps: None,
        }
    }

    /// Actions for one step down and one step up.
    pub fn steps(mut self, decrement: impl Into<Action>, increment: impl Into<Action>) -> Self {
        self.steps = Some((decrement.into(), increment.into()));
        self
    }

    pub fn set(&self, value: f64) -> Action {
        (self.set)(value)
    }

    pub fn increment(&self) -> Option<&Action> {
        self.steps.as_ref().map(|(_, up)| up)
    }

    pub fn decrement(&self) -> Option<&Action> {
        self.steps.as_ref().map(|(down, _)| down)
    }
}

impl std::fmt::Debug for NumericActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NumericActions")
            .field("steps", &self.steps)
            .finish_non_exhaustive()
    }
}

/// Text that assistive tech reads by character, word, and line (AT-SPI's
/// Text interface), with the caret and selection as byte offsets into it.
#[derive(Debug, Clone, PartialEq)]
pub struct AccessibleText {
    text: Arc<str>,
    /// `(anchor, focus)`; equal for a bare caret. `None` hides the caret.
    selection: Option<(usize, usize)>,
}

impl AccessibleText {
    pub fn new(text: impl Into<Arc<str>>) -> Self {
        Self {
            text: text.into(),
            selection: None,
        }
    }

    /// The selection from `anchor` to `focus` (the caret end). Offsets off
    /// a character boundary or past the end are snapped onto the text.
    pub fn selection(mut self, anchor: usize, focus: usize) -> Self {
        self.selection = Some((self.snap(anchor), self.snap(focus)));
        self
    }

    pub fn caret(self, at: usize) -> Self {
        self.selection(at, at)
    }

    fn snap(&self, at: usize) -> usize {
        let mut at = at.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }
}

/// Runs carry character indices as `u8` (`word_starts`), so a line longer
/// than this is split into runs chained with `next_on_line`.
const MAX_RUN_CHARACTERS: usize = 255;

/// One `TextRun` child of a text node.
#[derive(Debug, PartialEq)]
struct TextRunSpec {
    bytes: Range<usize>,
    /// UTF-8 length of each character (grapheme cluster).
    character_lengths: Vec<u8>,
    word_starts: Vec<u8>,
    /// Continues the previous run's line rather than starting a new one.
    continues_line: bool,
}

impl TextRunSpec {
    /// Byte offset of character `index` (`len` is the run's end).
    fn byte_at(&self, index: usize) -> usize {
        self.bytes.start
            + self
                .character_lengths
                .iter()
                .take(index)
                .map(|len| usize::from(*len))
                .sum::<usize>()
    }

    /// Character index of `byte`, which must lie in the run.
    fn index_of(&self, byte: usize) -> usize {
        let mut at = self.bytes.start;
        for (index, len) in self.character_lengths.iter().enumerate() {
            let next = at + usize::from(*len);
            if next > byte {
                return index;
            }
            at = next;
        }
        self.character_lengths.len()
    }
}

/// Lines of `text` (each keeping its `\n`) as runs of at most
/// [`MAX_RUN_CHARACTERS`] grapheme clusters. Empty text, and text ending in
/// a newline, end with an empty run so the caret has somewhere to sit.
fn text_runs(text: &str) -> Vec<TextRunSpec> {
    let word_starts: HashSet<usize> = text
        .split_word_bound_indices()
        .filter(|(_, word)| word.chars().next().is_some_and(char::is_alphanumeric))
        .map(|(at, _)| at)
        .collect();
    let mut runs = Vec::new();
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let mut run = TextRunSpec {
            bytes: line_start..line_start,
            character_lengths: Vec::new(),
            word_starts: Vec::new(),
            continues_line: false,
        };
        for (at, grapheme) in line.grapheme_indices(true) {
            let at = line_start + at;
            // A cluster too long for a u8 length is split into its chars.
            let pieces: Vec<(usize, usize)> = if grapheme.len() <= usize::from(u8::MAX) {
                vec![(at, grapheme.len())]
            } else {
                grapheme
                    .char_indices()
                    .map(|(i, c)| (at + i, c.len_utf8()))
                    .collect()
            };
            for (at, len) in pieces {
                if run.character_lengths.len() == MAX_RUN_CHARACTERS {
                    let next = TextRunSpec {
                        bytes: at..at,
                        character_lengths: Vec::new(),
                        word_starts: Vec::new(),
                        continues_line: true,
                    };
                    runs.push(std::mem::replace(&mut run, next));
                }
                if word_starts.contains(&at) {
                    run.word_starts.push(run.character_lengths.len() as u8);
                }
                run.character_lengths.push(len as u8);
                run.bytes.end = at + len;
            }
        }
        runs.push(run);
        line_start += line.len();
    }
    if text.is_empty() || text.ends_with('\n') {
        runs.push(TextRunSpec {
            bytes: text.len()..text.len(),
            character_lengths: Vec::new(),
            word_starts: Vec::new(),
            continues_line: false,
        });
    }
    runs
}

fn text_run_id(owner: &str, index: usize) -> NodeId {
    stable_node_id(&format!("{owner}/run{index}"))
}

/// The run and character index of byte offset `at`: the run that contains
/// it, or the end of the last run.
fn text_position(owner: &str, runs: &[TextRunSpec], at: usize) -> TextPosition {
    let index = runs
        .iter()
        .position(|run| run.bytes.contains(&at))
        .unwrap_or(runs.len().saturating_sub(1));
    TextPosition {
        node: text_run_id(owner, index),
        character_index: runs.get(index).map_or(0, |run| run.index_of(at)),
    }
}

/// The exact platform role for a toolkit role. `None` for roles that only
/// group or scroll, which publish no node of their own.
pub fn accessibility_role_for(role: SemanticRole) -> Option<Role> {
    use SemanticRole as S;
    Some(match role {
        S::Button => Role::Button,
        S::Link => Role::Link,
        S::Dialog => Role::Dialog,
        S::Alert => Role::Alert,
        S::Status => Role::Status,
        S::CheckBox => Role::CheckBox,
        S::Switch => Role::Switch,
        S::RadioButton => Role::RadioButton,
        S::RadioGroup => Role::RadioGroup,
        S::Tab => Role::Tab,
        S::TabList => Role::TabList,
        S::TabPanel => Role::TabPanel,
        S::Tree => Role::Tree,
        S::TreeItem => Role::TreeItem,
        S::List => Role::List,
        S::ListItem => Role::ListItem,
        S::ListBox => Role::ListBox,
        S::ListBoxOption => Role::ListBoxOption,
        S::Menu => Role::Menu,
        S::MenuBar => Role::MenuBar,
        S::MenuItem => Role::MenuItem,
        S::ComboBox => Role::ComboBox,
        S::TextInput => Role::TextInput,
        S::Slider => Role::Slider,
        S::SpinButton => Role::SpinButton,
        S::ProgressIndicator => Role::ProgressIndicator,
        S::Heading => Role::Heading,
        S::Image => Role::Image,
        S::Toolbar => Role::Toolbar,
        S::Tooltip => Role::Tooltip,
        S::Separator => Role::Splitter,
        S::Table => Role::Table,
        S::Grid => Role::Grid,
        S::Row => Role::Row,
        S::Cell => Role::Cell,
        S::Document => Role::Document,
        S::Label => Role::Label,
        S::Navigation => Role::Navigation,
        S::Complementary => Role::Complementary,
        S::Log => Role::Log,
        S::AlertDialog => Role::AlertDialog,
        S::Window => Role::Window,
        S::ScrollArea | S::Group => return None,
        _ => return None,
    })
}

/// The toolkit role for a platform role; related roles share one (every
/// text field is a `TextInput`). `None` for roles the toolkit does not
/// distinguish.
pub fn semantic_role_for(role: Role) -> Option<SemanticRole> {
    use SemanticRole as S;
    Some(match role {
        Role::Button | Role::DefaultButton | Role::DisclosureTriangle => S::Button,
        Role::Link => S::Link,
        Role::Dialog => S::Dialog,
        Role::AlertDialog => S::AlertDialog,
        Role::Alert => S::Alert,
        Role::Status | Role::Marquee | Role::Timer => S::Status,
        Role::Log => S::Log,
        Role::CheckBox | Role::MenuItemCheckBox => S::CheckBox,
        Role::Switch => S::Switch,
        Role::RadioButton | Role::MenuItemRadio => S::RadioButton,
        Role::RadioGroup => S::RadioGroup,
        Role::Tab => S::Tab,
        Role::TabList => S::TabList,
        Role::TabPanel => S::TabPanel,
        Role::Tree | Role::TreeGrid => S::Tree,
        Role::TreeItem => S::TreeItem,
        Role::List => S::List,
        Role::ListItem => S::ListItem,
        Role::ListBox => S::ListBox,
        Role::ListBoxOption | Role::MenuListOption => S::ListBoxOption,
        Role::Menu | Role::MenuListPopup => S::Menu,
        Role::MenuBar => S::MenuBar,
        Role::MenuItem => S::MenuItem,
        Role::ComboBox | Role::EditableComboBox => S::ComboBox,
        Role::TextInput
        | Role::MultilineTextInput
        | Role::SearchInput
        | Role::PasswordInput
        | Role::EmailInput
        | Role::NumberInput
        | Role::PhoneNumberInput
        | Role::UrlInput
        | Role::DateInput
        | Role::DateTimeInput
        | Role::TimeInput
        | Role::WeekInput
        | Role::MonthInput => S::TextInput,
        Role::Slider => S::Slider,
        Role::SpinButton => S::SpinButton,
        Role::ProgressIndicator | Role::Meter => S::ProgressIndicator,
        Role::Heading => S::Heading,
        Role::Image | Role::Canvas | Role::Figure => S::Image,
        Role::Toolbar => S::Toolbar,
        Role::Tooltip => S::Tooltip,
        Role::Splitter => S::Separator,
        Role::Table => S::Table,
        Role::Grid => S::Grid,
        Role::Row => S::Row,
        Role::Cell | Role::GridCell | Role::ColumnHeader | Role::RowHeader => S::Cell,
        Role::Document | Role::Terminal | Role::Article => S::Document,
        Role::Label => S::Label,
        Role::Navigation => S::Navigation,
        Role::Complementary => S::Complementary,
        Role::Window => S::Window,
        Role::ScrollView => S::ScrollArea,
        Role::Group | Role::Section | Role::Pane => S::Group,
        _ => return None,
    })
}

#[derive(Debug, Clone)]
pub enum AccessibilityAction {
    Click(Action),
    Focus(FocusId),
    TextValue(FocusId),
    Scroll(ScrollActionBuilder),
    EditorViewport {
        focus: FocusId,
        scroll: ScrollActionBuilder,
    },
    /// Set or step a range value.
    Numeric(NumericActions),
}

#[derive(Debug, Clone)]
pub struct AccessibilityNode {
    id: NodeId,
    role: Role,
    /// In layout coordinates; published through `transform`.
    bounds: Rect,
    /// Layout coordinates to window coordinates, set when the node is
    /// pushed under a transformed element.
    transform: quark::Transform2D,
    label: Option<Arc<str>>,
    value: Option<Arc<str>>,
    description: Option<Arc<str>>,
    disabled: bool,
    selected: Option<bool>,
    toggled: Option<Toggled>,
    expanded: Option<bool>,
    invalid: bool,
    required: bool,
    read_only: bool,
    modal: bool,
    live: Option<Politeness>,
    numeric: Option<NumericValue>,
    orientation: Option<Orientation>,
    text: Option<AccessibleText>,
    /// Keyboard focus target the node represents, when its action does
    /// not name one (a focusable button).
    focus: Option<FocusId>,
    /// 1-based position among siblings and the set size, for list items
    /// whose set is larger than the materialized rows.
    position_in_set: Option<(usize, usize)>,
    /// Item count of a container (list), for adapters that read the set
    /// size from the container rather than the items (AT-SPI).
    set_size: Option<usize>,
    collection: CollectionInfo,
    action: Option<AccessibilityAction>,
    author_id: Arc<str>,
    parent: Option<NodeId>,
}

impl AccessibilityNode {
    /// A node keyed by `key`. Pass an `Arc<str>` the element keeps between
    /// frames (see [`Self::shared`]) to build the node without allocating.
    pub fn new(key: impl AsRef<str>, role: Role, bounds: Rect) -> Self {
        Self::shared(Arc::from(key.as_ref()), role, bounds)
    }

    /// Like [`Self::new`], taking the key as a shared string. Every string a
    /// node holds is shared, so a node cloned out of an element cache costs
    /// no allocation.
    pub fn shared(key: Arc<str>, role: Role, bounds: Rect) -> Self {
        Self {
            id: stable_node_id(&key),
            role,
            bounds,
            transform: quark::Transform2D::IDENTITY,
            label: None,
            value: None,
            description: None,
            disabled: false,
            selected: None,
            toggled: None,
            expanded: None,
            invalid: false,
            required: false,
            read_only: false,
            modal: false,
            live: None,
            numeric: None,
            orientation: None,
            text: None,
            focus: None,
            position_in_set: None,
            set_size: None,
            collection: CollectionInfo::default(),
            action: None,
            author_id: key,
            parent: None,
        }
    }

    pub fn button(key: impl AsRef<str>, label: impl Into<Arc<str>>, bounds: Rect) -> Self {
        Self::new(key, Role::Button, bounds).label(label)
    }

    pub fn label(mut self, label: impl Into<Arc<str>>) -> Self {
        let label = label.into();
        if !label.is_empty() {
            self.label = Some(label);
        }
        self
    }

    pub fn value(mut self, value: impl Into<Arc<str>>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn description(mut self, description: impl Into<Arc<str>>) -> Self {
        let description = description.into();
        if !description.is_empty() {
            self.description = Some(description);
        }
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = Some(selected);
        self
    }

    pub fn toggled(mut self, toggled: bool) -> Self {
        self.toggled = Some(Toggled::from(toggled));
        self
    }

    /// Checked state of a checkbox or switch that some of its children
    /// are checked in.
    pub fn mixed(mut self) -> Self {
        self.toggled = Some(Toggled::Mixed);
        self
    }

    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// A dialog that blocks the rest of the window.
    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    /// Make the node a live region: assistive tech speaks its name when
    /// it appears and whenever the name changes.
    pub fn live(mut self, politeness: Politeness) -> Self {
        self.live = Some(politeness);
        self
    }

    pub fn numeric(mut self, value: NumericValue) -> Self {
        self.numeric = Some(value);
        self
    }

    /// Which way the control lies: a splitter between side by side panes
    /// is vertical, though it moves horizontally.
    pub fn orientation(mut self, orientation: Orientation) -> Self {
        self.orientation = Some(orientation);
        self
    }

    /// Publish `text` as character ranges with its caret and selection,
    /// so screen readers can read and move through it.
    pub fn text(mut self, text: AccessibleText) -> Self {
        self.text = Some(text);
        self
    }

    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self
    }

    /// `position` is 1-based; `size` is the whole set, including items that
    /// are not in the tree (virtualized rows).
    pub fn position_in_set(mut self, position: usize, size: usize) -> Self {
        self.position_in_set = Some((position, size));
        self
    }

    /// On a container: how many items its set has, including items that
    /// are not in the tree.
    pub fn set_size(mut self, size: usize) -> Self {
        self.set_size = Some(size);
        self
    }

    /// Tree level, table extent and indices, sort state, and whether the
    /// container is multiselectable.
    pub fn collection(mut self, info: CollectionInfo) -> Self {
        self.collection = info;
        self
    }

    /// The keyboard focus target this node stands for, so assistive tech
    /// sees focus land on it and can move focus to it.
    pub fn focus(mut self, focus: FocusId) -> Self {
        self.focus = Some(focus);
        self
    }

    pub fn action(mut self, action: AccessibilityAction) -> Self {
        self.action = Some(action);
        self
    }

    pub(crate) fn id(&self) -> NodeId {
        self.id
    }

    pub(crate) fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    /// Move the node's bounds; element caches replay nodes at a new origin.
    pub(crate) fn offset(&mut self, dx: f32, dy: f32) {
        self.bounds = self.bounds.offset(dx, dy);
    }

    /// Publish the node through `transform` (layout coordinates to window
    /// coordinates); element caches replay nodes under the transform of
    /// their ancestors now.
    pub(crate) fn set_transform(&mut self, transform: quark::Transform2D) {
        self.transform = transform;
    }

    /// Window bounds: the four transformed corners' bounds. Absolute, so
    /// no ancestor's transform applies on top of them.
    pub fn window_bounds(&self) -> Rect {
        crate::element::window_rect(self.transform, self.bounds)
    }

    fn to_accesskit_node(&self) -> Node {
        let mut node = Node::new(self.role);
        node.set_bounds(ax_rect(self.window_bounds()));
        node.set_author_id(&*self.author_id);
        if let Some(label) = &self.label {
            node.set_label(&**label);
        }
        // accesskit reads a Label node's name from its value, so static text
        // with only a label would reach screen readers unnamed.
        match (&self.value, &self.label) {
            (Some(value), _) => node.set_value(&**value),
            (None, Some(label)) if self.role == Role::Label => node.set_value(&**label),
            (None, _) => {}
        }
        if let Some(description) = &self.description {
            node.set_description(&**description);
        }
        if self.disabled {
            node.set_disabled();
        }
        if let Some(selected) = self.selected {
            node.set_selected(selected);
        }
        if let Some(toggled) = self.toggled {
            node.set_toggled(toggled);
        }
        if self.invalid {
            node.set_invalid(Invalid::True);
        }
        if self.required {
            node.set_required();
        }
        if self.read_only {
            node.set_read_only();
        }
        if self.modal {
            node.set_modal();
        }
        if let Some(politeness) = self.live {
            node.set_live(politeness.into());
        }
        if let Some(numeric) = self.numeric {
            node.set_numeric_value(numeric.value);
            node.set_min_numeric_value(numeric.min);
            node.set_max_numeric_value(numeric.max);
            if let Some(step) = numeric.step {
                node.set_numeric_value_step(step);
            }
        }
        if let Some(expanded) = self.expanded {
            node.set_expanded(expanded);
        }
        if let Some(orientation) = self.orientation {
            node.set_orientation(orientation);
        }
        if let Some((position, size)) = self.position_in_set {
            // AccessKit stores a 0-based index; its adapters add 1.
            node.set_position_in_set(position.saturating_sub(1));
            node.set_size_of_set(size);
        }
        if let Some(size) = self.set_size {
            node.set_size_of_set(size);
        }
        let info = &self.collection;
        if let Some(level) = info.level {
            node.set_level(level);
        }
        if let Some(rows) = info.row_count {
            node.set_row_count(rows);
        }
        if let Some(columns) = info.column_count {
            node.set_column_count(columns);
        }
        if let Some(row) = info.row_index {
            node.set_row_index(row);
        }
        if let Some(column) = info.column_index {
            node.set_column_index(column);
        }
        if let Some(sort) = info.sort {
            node.set_sort_direction(sort);
        }
        if info.multiselectable {
            node.set_multiselectable();
        }
        match &self.action {
            Some(AccessibilityAction::Click(_)) => node.add_action(AxAction::Click),
            Some(AccessibilityAction::Focus(_)) => node.add_action(AxAction::Focus),
            // Text fields take Click as well as Focus: AT-SPI exposes only
            // Click as an action, so tools that drive controls through it
            // (cua) can reach the field. Clicking focuses it.
            Some(AccessibilityAction::TextValue(_)) => {
                node.add_action(AxAction::Click);
                node.add_action(AxAction::Focus);
                node.add_action(AxAction::SetValue);
                node.add_action(AxAction::ReplaceSelectedText);
                node.add_action(AxAction::SetTextSelection);
            }
            Some(AccessibilityAction::Scroll(_)) => {
                node.add_action(AxAction::ScrollUp);
                node.add_action(AxAction::ScrollDown);
            }
            Some(AccessibilityAction::EditorViewport { .. }) => {
                node.add_action(AxAction::Click);
                node.add_action(AxAction::Focus);
                node.add_action(AxAction::ScrollUp);
                node.add_action(AxAction::ScrollDown);
                node.add_action(AxAction::ReplaceSelectedText);
                node.add_action(AxAction::SetTextSelection);
            }
            Some(AccessibilityAction::Numeric(actions)) => {
                node.add_action(AxAction::SetValue);
                if actions.steps.is_some() {
                    node.add_action(AxAction::Increment);
                    node.add_action(AxAction::Decrement);
                }
            }
            None => {}
        }
        if self.focus.is_some() && !node.supports_action(AxAction::Focus) {
            node.add_action(AxAction::Focus);
        }
        node
    }

    /// The node's `TextRun` children, with its selection set on `node`.
    fn text_run_nodes(&self, node: &mut Node) -> Vec<(NodeId, Node)> {
        let Some(text) = &self.text else {
            return Vec::new();
        };
        let runs = text_runs(&text.text);
        let owner = &*self.author_id;
        if let Some((anchor, focus)) = text.selection {
            node.set_text_selection(TextSelection {
                anchor: text_position(owner, &runs, anchor),
                focus: text_position(owner, &runs, focus),
            });
        }
        let bounds = ax_rect(self.window_bounds());
        runs.iter()
            .enumerate()
            .map(|(index, run)| {
                let mut run_node = Node::new(Role::TextRun);
                run_node.set_bounds(bounds);
                run_node.set_value(text.text.get(run.bytes.clone()).unwrap_or_default());
                run_node.set_character_lengths(run.character_lengths.clone());
                run_node.set_word_starts(run.word_starts.clone());
                if run.continues_line {
                    run_node.set_previous_on_line(text_run_id(owner, index - 1));
                }
                if runs.get(index + 1).is_some_and(|next| next.continues_line) {
                    run_node.set_next_on_line(text_run_id(owner, index + 1));
                }
                (text_run_id(owner, index), run_node)
            })
            .collect()
    }

    /// Byte offsets `(anchor, focus)` of a selection assistive tech asked
    /// for in this node's text. `None` when it names runs the node does not
    /// have (the text changed since the tree was published).
    fn selection_offsets(&self, selection: &TextSelection) -> Option<(usize, usize)> {
        let text = self.text.as_ref()?;
        let runs = text_runs(&text.text);
        let offset = |position: &TextPosition| {
            let index = (0..runs.len())
                .find(|&index| text_run_id(&self.author_id, index) == position.node)?;
            let run = &runs[index];
            (position.character_index <= run.character_lengths.len())
                .then(|| run.byte_at(position.character_index))
        };
        Some((offset(&selection.anchor)?, offset(&selection.focus)?))
    }

    fn focus_target(&self) -> Option<FocusId> {
        if self.focus.is_some() {
            return self.focus;
        }
        match self.action {
            Some(
                AccessibilityAction::Focus(focus)
                | AccessibilityAction::TextValue(focus)
                | AccessibilityAction::EditorViewport { focus, .. },
            ) => Some(focus),
            _ => None,
        }
    }
}

/// Accessibility state a [`Div`] carries beyond its role, label, and the
/// common flags it stores itself.
#[derive(Debug, Clone, Default)]
pub(crate) struct AccessibilityExtra {
    mixed: bool,
    invalid: bool,
    required: bool,
    read_only: bool,
    live: Option<Politeness>,
    numeric: Option<NumericValue>,
    numeric_actions: Option<NumericActions>,
    orientation: Option<Orientation>,
    position_in_set: Option<(usize, usize)>,
    collection: CollectionInfo,
}

impl AccessibilityExtra {
    pub(crate) fn invalid(&self) -> bool {
        self.invalid
    }

    pub(crate) fn required(&self) -> bool {
        self.required
    }

    pub(crate) fn apply(&self, mut node: AccessibilityNode) -> AccessibilityNode {
        if self.mixed {
            node = node.mixed();
        }
        node = node
            .invalid(self.invalid)
            .required(self.required)
            .read_only(self.read_only);
        if let Some(politeness) = self.live {
            node = node.live(politeness);
        }
        if let Some(numeric) = self.numeric {
            node = node.numeric(numeric);
        }
        // A click or scroll action the div sets afterwards replaces this.
        if let Some(actions) = &self.numeric_actions {
            node = node.action(AccessibilityAction::Numeric(actions.clone()));
        }
        if let Some(orientation) = self.orientation {
            node = node.orientation(orientation);
        }
        if let Some((position, size)) = self.position_in_set {
            node = node.position_in_set(position, size);
        }
        node.collection(self.collection)
    }
}

impl Div {
    /// Checked state "mixed": a checkbox whose children are partly
    /// checked. Overrides [`Div::accessibility_toggled`] for assistive tech.
    pub fn accessibility_mixed(mut self) -> Self {
        self.accessibility_extra.mixed = true;
        self
    }

    /// The value fails validation; screen readers say "invalid entry".
    pub fn accessibility_invalid(mut self, invalid: bool) -> Self {
        self.accessibility_extra.invalid = invalid;
        self
    }

    pub fn accessibility_required(mut self, required: bool) -> Self {
        self.accessibility_extra.required = required;
        self
    }

    pub fn accessibility_read_only(mut self, read_only: bool) -> Self {
        self.accessibility_extra.read_only = read_only;
        self
    }

    /// Make the div a live region: assistive tech speaks its accessibility
    /// label when it appears and each time the label changes. Give it an
    /// [`accessibility_role`](Div::accessibility_role) (`Status` for
    /// toasts) so it publishes a node.
    pub fn live(mut self, politeness: Politeness) -> Self {
        self.accessibility_extra.live = Some(politeness);
        self
    }

    /// The range value of a slider, progress bar, or spin button.
    pub fn accessibility_numeric(mut self, value: NumericValue) -> Self {
        self.accessibility_extra.numeric = Some(value);
        self
    }

    /// Let assistive tech set and step the
    /// [`accessibility_numeric`](Div::accessibility_numeric) value. A
    /// click action on the div takes precedence.
    pub fn accessibility_numeric_actions(mut self, actions: NumericActions) -> Self {
        self.accessibility_extra.numeric_actions = Some(actions);
        self
    }

    /// Which way the control lies; see [`AccessibilityNode::orientation`].
    pub fn accessibility_orientation(mut self, orientation: Orientation) -> Self {
        self.accessibility_extra.orientation = Some(orientation);
        self
    }

    /// 1-based `position` of an item in a set of `size` items, counting
    /// items that are not painted (virtualized rows).
    pub fn accessibility_position_in_set(mut self, position: usize, size: usize) -> Self {
        self.accessibility_extra.position_in_set = Some((position, size));
        self
    }

    /// Depth of a tree item, 1 for top-level items.
    pub fn accessibility_level(mut self, level: usize) -> Self {
        self.accessibility_extra.collection.level = Some(level);
        self
    }

    /// Rows and columns of a whole table, including ones not painted.
    pub fn accessibility_table_size(mut self, rows: usize, columns: usize) -> Self {
        self.accessibility_extra.collection.row_count = Some(rows);
        self.accessibility_extra.collection.column_count = Some(columns);
        self
    }

    /// 0-based row of a table row or cell.
    pub fn accessibility_row_index(mut self, row: usize) -> Self {
        self.accessibility_extra.collection.row_index = Some(row);
        self
    }

    /// 0-based column of a table cell or column header.
    pub fn accessibility_column_index(mut self, column: usize) -> Self {
        self.accessibility_extra.collection.column_index = Some(column);
        self
    }

    /// Sort state of a column header.
    pub fn accessibility_sort(mut self, sort: SortDirection) -> Self {
        self.accessibility_extra.collection.sort = Some(sort);
        self
    }

    /// The tree, list, or table lets more than one item be selected.
    pub fn accessibility_multiselectable(mut self, multiselectable: bool) -> Self {
        self.accessibility_extra.collection.multiselectable = multiselectable;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct AccessibilityFrame {
    nodes: Vec<AccessibilityNode>,
    node_ids: HashSet<NodeId>,
    actions: HashMap<NodeId, AccessibilityAction>,
    /// Semantic node index (in the frame's `SemanticFrame`) to the
    /// accessibility node that represents it.
    semantic_owners: HashMap<usize, NodeId>,
    focused: Option<NodeId>,
    root_bounds: Rect,
}

impl AccessibilityFrame {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            root_bounds: Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            ..Self::default()
        }
    }

    /// Empty the frame for a `width` x `height` window, keeping its
    /// buffers, so a window that reuses one frame builds its tree without
    /// allocating once the buffers have grown.
    pub fn reset(&mut self, width: f32, height: f32) {
        self.nodes.clear();
        self.node_ids.clear();
        self.actions.clear();
        self.semantic_owners.clear();
        self.focused = None;
        self.root_bounds = Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        };
    }

    /// Push a direct child of the window. Returns the node's final id, which
    /// differs from the key hash when the key collided.
    pub fn push(&mut self, node: AccessibilityNode) -> NodeId {
        self.push_child(node, None)
    }

    /// Push a node under `parent`, an id previously returned by this frame,
    /// or under the window when `None`. Parents must be pushed first.
    pub fn push_child(&mut self, mut node: AccessibilityNode, parent: Option<NodeId>) -> NodeId {
        self.ensure_unique_id(&mut node);
        node.parent = parent.filter(|id| self.node_ids.contains(id) && *id != node.id);
        let id = node.id;
        if let Some(action) = node.action.clone() {
            self.actions.insert(node.id, action);
        }
        if matches!(
            node.action,
            Some(
                AccessibilityAction::Focus(_)
                    | AccessibilityAction::TextValue(_)
                    | AccessibilityAction::EditorViewport { .. }
            )
        ) {
            self.focused.get_or_insert(node.id);
        }
        self.nodes.push(node);
        id
    }

    /// Record that `id` represents semantic node `semantic_index`, so nodes
    /// emitted under that semantic node nest beneath it.
    pub fn bind_semantic(&mut self, semantic_index: usize, id: NodeId) {
        self.semantic_owners.insert(semantic_index, id);
    }

    pub fn semantic_owner(&self, semantic_index: usize) -> Option<NodeId> {
        self.semantic_owners.get(&semantic_index).copied()
    }

    fn ensure_unique_id(&mut self, node: &mut AccessibilityNode) {
        if self.node_ids.insert(node.id) {
            return;
        }

        let base_author_id = node.author_id.clone();
        let mut suffix = 2usize;
        loop {
            let author_id = format!("{base_author_id}#{suffix}");
            let id = stable_node_id(&author_id);
            if self.node_ids.insert(id) {
                node.id = id;
                node.author_id = author_id.into();
                return;
            }
            suffix += 1;
        }
    }

    /// Nodes pushed from index `start` on, in push order.
    pub(crate) fn nodes_from(&self, start: usize) -> &[AccessibilityNode] {
        &self.nodes[start..]
    }

    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn action_for(&self, id: NodeId) -> Option<&AccessibilityAction> {
        self.actions.get(&id)
    }

    /// The keyboard focus target node `id` stands for.
    pub fn focus_target_of(&self, id: NodeId) -> Option<FocusId> {
        self.nodes.iter().find(|node| node.id == id)?.focus_target()
    }

    /// The focus target of node `id` and the byte offsets `(anchor, focus)`
    /// of `selection` in its text, for a `SetTextSelection` request.
    pub fn text_selection_for(
        &self,
        id: NodeId,
        selection: &TextSelection,
    ) -> Option<(FocusId, usize, usize)> {
        let node = self.nodes.iter().find(|node| node.id == id)?;
        let (anchor, focus) = node.selection_offsets(selection)?;
        Some((node.focus_target()?, anchor, focus))
    }

    /// `app_name` labels the root window node.
    pub fn tree_update(&self, app_name: &str, focus: Option<FocusId>) -> TreeUpdate {
        let mut children: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for node in &self.nodes {
            children
                .entry(node.parent.unwrap_or(ROOT_ID))
                .or_default()
                .push(node.id);
        }

        let mut root = Node::new(Role::Window);
        root.set_bounds(ax_rect(self.root_bounds));
        root.set_label(app_name);
        root.set_children(children.remove(&ROOT_ID).unwrap_or_default());

        let mut nodes = Vec::with_capacity(self.nodes.len() + 1);
        nodes.push((ROOT_ID, root));

        let mut focused = ROOT_ID;
        // Nodes inside a live region. Platforms announce every named node
        // that inherits a region's politeness, so a toast would be spoken
        // once for itself and again for each label and button in it; the
        // region's own name is the whole announcement.
        let mut in_live_region: HashSet<NodeId> = HashSet::new();
        for node in &self.nodes {
            if focus.is_some() && node.focus_target() == focus {
                focused = node.id;
            }
            let mut ax_node = node.to_accesskit_node();
            let parent_live = node
                .parent
                .is_some_and(|parent| in_live_region.contains(&parent));
            if parent_live && node.live.is_none() {
                ax_node.set_live(Live::Off);
            }
            if parent_live || node.live.is_some() {
                in_live_region.insert(node.id);
            }
            let runs = node.text_run_nodes(&mut ax_node);
            let mut kids = children.remove(&node.id).unwrap_or_default();
            // Runs first: they are the node's own text, before any
            // descendants pushed under it.
            kids.splice(0..0, runs.iter().map(|(id, _)| *id));
            if !kids.is_empty() {
                ax_node.set_children(kids);
            }
            nodes.push((node.id, ax_node));
            nodes.extend(runs);
        }

        TreeUpdate {
            nodes,
            tree: Some(Tree {
                root: ROOT_ID,
                toolkit_name: Some("Quark".to_owned()),
                toolkit_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
            tree_id: TreeId::ROOT,
            focus: focused,
        }
    }
}

pub fn empty_tree_update(app_name: &str) -> TreeUpdate {
    AccessibilityFrame::new(1.0, 1.0).tree_update(app_name, None)
}

/// Messages for assistive tech to speak that no node on screen shows:
/// "Response complete", "3 results". Each announcement is published as a
/// new live region under the window, so the platform speaks it exactly
/// once, even when the same text is announced twice in a row.
#[derive(Debug, Default, Clone)]
pub struct Announcer {
    serial: u64,
    current: Option<(String, Politeness)>,
}

impl Announcer {
    pub fn announce(&mut self, text: impl Into<String>, politeness: Politeness) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        self.serial += 1;
        self.current = Some((text, politeness));
    }

    /// Add the latest announcement to `update`, a tree from
    /// [`AccessibilityFrame::tree_update`]. It stays until the next one
    /// replaces it; republishing it unchanged speaks nothing.
    pub fn publish(&self, update: &mut TreeUpdate) {
        let Some((text, politeness)) = &self.current else {
            return;
        };
        let id = stable_node_id(&format!("quark.announcement#{}", self.serial));
        let mut node = Node::new(Role::Status);
        node.set_label(text.clone());
        node.set_live((*politeness).into());
        node.set_bounds(AxRect::new(0.0, 0.0, 0.0, 0.0));
        if let Some((_, root)) = update.nodes.iter_mut().find(|(node, _)| *node == ROOT_ID) {
            root.push_child(id);
            update.nodes.push((id, node));
        }
    }
}

/// One stable line per node, in tree order:
/// `author_id | role | label | value | x,y,w,h` (bounds rounded to ints, `-` for None).
/// Used by the devtools harness to snapshot the accessibility "DOM".
pub fn dump_accessibility(frame: &AccessibilityFrame) -> String {
    let mut out = String::new();
    for node in &frame.nodes {
        let label = node.label.as_deref().unwrap_or("-");
        let value = node.value.as_deref().unwrap_or("-");
        let b = node.window_bounds();
        out.push_str(&format!(
            "{} | {:?} | {} | {} | {},{},{},{}\n",
            node.author_id,
            node.role,
            label,
            value,
            b.x.round() as i64,
            b.y.round() as i64,
            b.width.round() as i64,
            b.height.round() as i64,
        ));
    }
    out
}

/// The tree shape: one line per node, `  ` per depth level, as
/// `author_id | role | label`. Children follow their parent in push order.
pub fn dump_accessibility_tree(frame: &AccessibilityFrame) -> String {
    let mut children: HashMap<Option<NodeId>, Vec<usize>> = HashMap::new();
    for (index, node) in frame.nodes.iter().enumerate() {
        children.entry(node.parent).or_default().push(index);
    }
    let mut out = String::new();
    // Depth-first; parents always precede children, so this terminates.
    let mut stack: Vec<(usize, usize)> = children
        .get(&None)
        .map(|roots| roots.iter().rev().map(|&index| (index, 0)).collect())
        .unwrap_or_default();
    while let Some((index, depth)) = stack.pop() {
        let node = &frame.nodes[index];
        out.push_str(&format!(
            "{}{} | {:?} | {}\n",
            "  ".repeat(depth),
            node.author_id,
            node.role,
            node.label.as_deref().unwrap_or("-"),
        ));
        if let Some(kids) = children.get(&Some(node.id)) {
            stack.extend(kids.iter().rev().map(|&kid| (kid, depth + 1)));
        }
    }
    out
}

/// The published tree as assistive tech sees each node's state, one line
/// per node in `update` order, skipping the window and text runs:
/// `author_id | role | name | state...`. Text nodes show their text and,
/// like AT-SPI, the caret and selection as character offsets
/// (`text="hello" caret=2 sel=0..2`).
pub fn dump_accessibility_states(update: &TreeUpdate) -> String {
    let nodes: HashMap<NodeId, &Node> = update.nodes.iter().map(|(id, n)| (*id, n)).collect();
    let mut out = String::new();
    for (id, node) in &update.nodes {
        if *id == ROOT_ID || node.role() == Role::TextRun {
            continue;
        }
        let name = node
            .label()
            .or_else(|| (node.role() == Role::Label).then(|| node.value()).flatten())
            .unwrap_or("-");
        let mut line = format!(
            "{} | {:?} | {name}",
            node.author_id().unwrap_or("-"),
            node.role()
        );
        let mut state = |s: String| {
            line.push_str(" | ");
            line.push_str(&s);
        };
        if let Some(description) = node.description() {
            state(format!("desc={description:?}"));
        }
        if node.is_disabled() {
            state("disabled".into());
        }
        match node.is_selected() {
            Some(true) => state("selected".into()),
            Some(false) => state("unselected".into()),
            None => {}
        }
        match node.toggled() {
            Some(Toggled::True) => state("checked".into()),
            Some(Toggled::False) => state("unchecked".into()),
            Some(Toggled::Mixed) => state("mixed".into()),
            None => {}
        }
        match node.is_expanded() {
            Some(true) => state("expanded".into()),
            Some(false) => state("collapsed".into()),
            None => {}
        }
        if node.invalid().is_some() {
            state("invalid".into());
        }
        if node.is_required() {
            state("required".into());
        }
        if node.is_read_only() {
            state("readonly".into());
        }
        if node.is_modal() {
            state("modal".into());
        }
        if let Some(live) = node.live().filter(|live| *live != Live::Off) {
            state(format!("live={live:?}").to_lowercase());
        }
        if let Some(value) = node.numeric_value() {
            let min = node.min_numeric_value().unwrap_or(f64::NAN);
            let max = node.max_numeric_value().unwrap_or(f64::NAN);
            state(format!("range={value}/{min}..{max}"));
        }
        let runs: Vec<(NodeId, &Node)> = node
            .children()
            .iter()
            .filter_map(|kid| nodes.get(kid).map(|n| (*kid, *n)))
            .filter(|(_, n)| n.role() == Role::TextRun)
            .collect();
        if runs.is_empty() {
            if let Some(value) = node.value().filter(|_| node.role() != Role::Label) {
                state(format!("value={value:?}"));
            }
        } else {
            let text: String = runs.iter().filter_map(|(_, n)| n.value()).collect();
            state(format!("text={text:?}"));
            // AT-SPI counts offsets in Unicode scalar values.
            let offset = |position: TextPosition| {
                let mut before = 0;
                for (run_id, run) in &runs {
                    let value = run.value().unwrap_or_default();
                    if *run_id == position.node {
                        let bytes: usize = run
                            .character_lengths()
                            .get(..position.character_index)?
                            .iter()
                            .map(|len| usize::from(*len))
                            .sum();
                        return Some(before + value.get(..bytes)?.chars().count());
                    }
                    before += value.chars().count();
                }
                None
            };
            if let Some(selection) = node.text_selection() {
                let (anchor, focus) = (offset(selection.anchor), offset(selection.focus));
                let fmt = |at: Option<usize>| at.map_or("?".to_owned(), |at| at.to_string());
                state(format!("caret={}", fmt(focus)));
                if anchor != focus {
                    state(format!("sel={}..{}", fmt(anchor), fmt(focus)));
                }
            }
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn ax_rect(rect: Rect) -> AxRect {
    AxRect::new(
        f64::from(rect.x),
        f64::from(rect.y),
        f64::from(rect.x + rect.width),
        f64::from(rect.y + rect.height),
    )
}

/// Ids 0 and 1 are reserved for the runner's placeholder root and [`ROOT_ID`].
fn stable_node_id(key: &str) -> NodeId {
    NodeId(quark::stable_hash(key).max(2))
}

#[cfg(test)]
mod tests {
    use accesskit::Role;

    use super::*;

    fn rect() -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        }
    }

    #[test]
    fn tree_update_publishes_children_under_their_parent() {
        let mut frame = AccessibilityFrame::new(100.0, 100.0);
        let dialog = frame.push(AccessibilityNode::new("dialog", Role::Dialog, rect()));
        let ok = frame.push_child(AccessibilityNode::button("ok", "OK", rect()), Some(dialog));
        let update = frame.tree_update("Test", None);
        let children = |id: NodeId| {
            update
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .map(|(_, node)| node.children().to_vec())
                .expect("node in update")
        };
        assert_eq!(children(ROOT_ID), vec![dialog]);
        assert_eq!(children(dialog), vec![ok]);
    }

    #[test]
    fn label_node_publishes_its_text_as_value() {
        let mut frame = AccessibilityFrame::new(100.0, 100.0);
        let id = frame.push(AccessibilityNode::new("text", Role::Label, rect()).label("Hello"));
        let update = frame.tree_update("Test", None);
        let node = update.nodes.iter().find(|(node_id, _)| *node_id == id);
        assert_eq!(node.and_then(|(_, node)| node.value()), Some("Hello"));
    }

    #[test]
    fn duplicate_node_keys_are_disambiguated() {
        let mut frame = AccessibilityFrame::new(100.0, 100.0);
        frame.push(AccessibilityNode::new("same", Role::Label, rect()).label("One"));
        frame.push(AccessibilityNode::new("same", Role::Label, rect()).label("Two"));

        let update = frame.tree_update("Test", None);
        let root = update
            .nodes
            .iter()
            .find(|(id, _)| *id == ROOT_ID)
            .map(|(_, node)| node)
            .expect("root node");
        let children = root.children();
        assert_eq!(children.len(), 2);
        assert_ne!(children[0], children[1]);
    }

    /// `start..end text chars=N words=[..]`, with `+line` on a run that
    /// continues the previous run's line.
    fn dump_runs(text: &str) -> String {
        text_runs(text)
            .iter()
            .map(|run| {
                format!(
                    "{}..{} {:?} chars={} words={:?}{}\n",
                    run.bytes.start,
                    run.bytes.end,
                    &text[run.bytes.clone()],
                    run.character_lengths.len(),
                    run.word_starts,
                    if run.continues_line { " +line" } else { "" },
                )
            })
            .collect()
    }

    #[test]
    fn text_splits_into_runs_by_line_and_length() {
        let long = "a".repeat(300);
        let cases = [
            ("", "0..0 \"\" chars=0 words=[]\n"),
            (
                "ab cd\nx",
                "0..6 \"ab cd\\n\" chars=6 words=[0, 3]\n6..7 \"x\" chars=1 words=[0]\n",
            ),
            // The caret after a final newline sits on an empty last line.
            (
                "a\n",
                "0..2 \"a\\n\" chars=2 words=[0]\n2..2 \"\" chars=0 words=[]\n",
            ),
            // A combining accent is one character with its base.
            ("e\u{301}x", "0..4 \"e\\u{301}x\" chars=2 words=[0]\n"),
            (
                long.as_str(),
                &format!(
                    "0..255 {:?} chars=255 words=[0]\n255..300 {:?} chars=45 words=[] +line\n",
                    &long[..255],
                    &long[255..]
                ),
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(dump_runs(text), expected, "{text:?}");
        }
    }

    const FIELD: FocusId = FocusId::from_key("field");

    /// A frame with one text field showing `text` with `selection`.
    fn field_frame(text: &str, anchor: usize, focus: usize) -> (AccessibilityFrame, NodeId) {
        let mut frame = AccessibilityFrame::new(100.0, 100.0);
        let id = frame.push(
            AccessibilityNode::new("field", Role::TextInput, rect())
                .label("Message")
                .text(AccessibleText::new(text).selection(anchor, focus))
                .action(AccessibilityAction::TextValue(FIELD)),
        );
        (frame, id)
    }

    /// The accessibility frame of `root` painted into a 400x300 window.
    fn painted(root: impl crate::element::IntoAnyElement) -> AccessibilityFrame {
        use crate::element::{ElementContext, render_element};
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let mut layouts = quark_text::LayoutCache::default();
        let theme = crate::theme::Theme::default_dark();
        let signals = quark::reactive::SignalStore::new();
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        cx.accessibility = AccessibilityFrame::new(400.0, 300.0);
        let mut root = root.into_any();
        render_element(
            &mut root,
            &mut quark_render::Scene::default(),
            &mut cx,
            400.0,
            300.0,
        );
        std::mem::take(&mut cx.accessibility)
    }

    // The div builder's roles and states reach the published tree as the
    // platform properties screen readers read.
    #[test]
    fn div_roles_and_states_are_published() {
        use crate::element::div;
        use crate::style::Styled;
        let control = |id: &str, role: Role, label: &str| {
            div()
                .w(40.0)
                .h(20.0)
                .accessibility_id(id)
                .accessibility_role(role)
                .accessibility_label(label)
        };
        let root = div()
            .w(400.0)
            .h(300.0)
            .flex_col()
            .child(control("wifi", Role::Switch, "Wi-Fi").accessibility_toggled(true))
            .child(control("all", Role::CheckBox, "Select all").accessibility_mixed())
            .child(
                control("email", Role::TextInput, "Email")
                    .accessibility_invalid(true)
                    .accessibility_required(true)
                    .accessibility_description("Not an address"),
            )
            .child(control("more", Role::Button, "More").accessibility_expanded(false))
            .child(
                control("volume", Role::Slider, "Volume")
                    .accessibility_numeric(NumericValue {
                        value: 30.0,
                        min: 0.0,
                        max: 100.0,
                        step: Some(10.0),
                    })
                    .accessibility_disabled(true),
            )
            .child(
                control("confirm", Role::Dialog, "Confirm")
                    .focus_scope("confirm")
                    .trap_focus(true),
            );
        let update = painted(root).tree_update("Test", None);
        assert_eq!(
            dump_accessibility_states(&update),
            "wifi | Switch | Wi-Fi | checked\n\
             all | CheckBox | Select all | mixed\n\
             email | TextInput | Email | desc=\"Not an address\" | invalid | required\n\
             more | Button | More | collapsed\n\
             volume | Slider | Volume | disabled | range=30/0..100\n\
             confirm | Dialog | Confirm | modal\n"
        );
    }

    // Regression: keyboard focus on a clickable div (Tab to a button) was
    // never published, so screen readers stayed on the window.
    #[test]
    fn focused_button_is_the_published_focus() {
        use crate::element::div;
        use crate::style::Styled;
        #[derive(Clone, Debug, PartialEq)]
        struct Save;
        let root = div().w(400.0).h(300.0).child(
            div()
                .w(80.0)
                .h(30.0)
                .accessibility_id("save")
                .accessibility_role(Role::Button)
                .accessibility_label("Save")
                .on_click(Action::new(Save)),
        );
        let frame = painted(root);
        let update = frame.tree_update("Test", Some(FocusId::from_key("save")));
        let focused = update.nodes.iter().find(|(id, _)| *id == update.focus);
        assert_eq!(
            focused.and_then(|(_, node)| node.label()),
            Some("Save"),
            "focus is on {:?}",
            focused.map(|(_, node)| node.role())
        );
    }

    proptest::proptest! {
        // A selection assistive tech sends back in the published positions
        // lands on the byte offsets the field published.
        #[test]
        fn selection_round_trips_through_published_positions(
            text in "[a-z \u{e9}\u{301}\u{1F600}\n]{0,40}",
            pick in proptest::prelude::any::<proptest::sample::Index>(),
        ) {
            let mut boundaries: Vec<usize> =
                text.grapheme_indices(true).map(|(at, _)| at).collect();
            boundaries.push(text.len());
            let at = *pick.get(&boundaries);
            let (frame, id) = field_frame(&text, 0, at);
            let update = frame.tree_update("Test", None);
            let published = update
                .nodes
                .iter()
                .find(|(node, _)| *node == id)
                .and_then(|(_, node)| node.text_selection())
                .expect("published selection");
            proptest::prop_assert_eq!(
                frame.text_selection_for(id, published),
                Some((FIELD, 0, at))
            );
        }
    }

    #[cfg(target_os = "linux")]
    mod atspi {
        use std::sync::{Arc, Mutex, RwLock};

        use accesskit::ActionRequest;
        use accesskit_atspi_common::{
            Adapter, AdapterCallback, AppContext, Event, InterfaceSet, NodeId as AtspiNodeId,
            ObjectEvent, WindowBounds,
        };

        use super::*;

        #[derive(Default)]
        struct Seen {
            nodes: Vec<AtspiNodeId>,
            announcements: Vec<String>,
        }

        struct Recorder(Arc<Mutex<Seen>>);

        impl AdapterCallback for Recorder {
            fn register_interfaces(&self, _: &Adapter, id: AtspiNodeId, _: InterfaceSet) {
                self.0.lock().unwrap().nodes.push(id);
            }

            fn unregister_interfaces(&self, _: &Adapter, _: AtspiNodeId, _: InterfaceSet) {}

            fn emit_event(&self, _: &Adapter, event: Event) {
                if let Event::Object {
                    event: ObjectEvent::Announcement(text, politeness),
                    ..
                } = event
                {
                    let line = format!("{text} ({politeness:?})");
                    self.0.lock().unwrap().announcements.push(line);
                }
            }
        }

        struct NoActions;

        impl accesskit::ActionHandler for NoActions {
            fn do_action(&mut self, _: ActionRequest) {}
        }

        /// AccessKit's AT-SPI layer, minus D-Bus: what a Linux screen
        /// reader would be told about the trees fed to it.
        pub(super) struct AtspiProbe {
            adapter: Adapter,
            seen: Arc<Mutex<Seen>>,
        }

        impl AtspiProbe {
            pub(super) fn new(initial: TreeUpdate) -> Self {
                let seen = Arc::new(Mutex::new(Seen::default()));
                let context: Arc<RwLock<AppContext>> = AppContext::new(Some("Test".into()));
                let adapter = Adapter::new(
                    &context,
                    Recorder(Arc::clone(&seen)),
                    initial,
                    true,
                    WindowBounds::default(),
                    NoActions,
                );
                Self { adapter, seen }
            }

            /// Apply `update`; returns the announcements it caused.
            pub(super) fn update(&mut self, update: TreeUpdate) -> Vec<String> {
                self.adapter.update(update);
                std::mem::take(&mut self.seen.lock().unwrap().announcements)
            }

            /// Every node with AT-SPI's Text interface, as
            /// `name: text | caret=N | sel=A..B` (offsets in scalar values).
            pub(super) fn texts(&self) -> String {
                let ids = self.seen.lock().unwrap().nodes.clone();
                let mut out = String::new();
                for id in ids {
                    let node = self.adapter.platform_node(id);
                    let Ok(count) = node.character_count() else {
                        continue;
                    };
                    let (start, end) = node.selection(0).unwrap();
                    out.push_str(&format!(
                        "{}: {:?} | caret={} | sel={start}..{end}\n",
                        node.name().unwrap(),
                        node.text(0, count).unwrap(),
                        node.caret_offset().unwrap(),
                    ));
                }
                out
            }
        }

        // A field's text, caret, and selection reach AT-SPI's Text
        // interface, with offsets in characters even across multibyte text.
        #[test]
        fn text_field_caret_and_selection_reach_atspi() {
            let text = "h\u{e9}llo \u{1F600} w\u{f6}rld";
            let start = text.find('l').unwrap();
            let end = text.find(" w").unwrap();
            let (frame, _) = field_frame(text, start, end);
            let probe = AtspiProbe::new(frame.tree_update("Test", None));
            assert_eq!(
                probe.texts(),
                "Message: \"h\u{e9}llo \u{1F600} w\u{f6}rld\" | caret=7 | sel=2..7\n"
            );
        }

        // A live div is spoken when it appears and when its label changes,
        // once (not again for its contents), and not on frames that leave
        // it alone.
        #[test]
        fn live_region_speaks_on_appearance_and_change() {
            use crate::element::div;
            use crate::style::Styled;
            let view = |status: Option<&str>| {
                div().w(400.0).h(300.0).optional_child(status.map(|label| {
                    div()
                        .w(200.0)
                        .h(40.0)
                        .accessibility_id("status")
                        .accessibility_role(Role::Status)
                        .accessibility_label(label)
                        .live(Politeness::Polite)
                        // Named content inside the region is not spoken
                        // on its own.
                        .child(crate::element::text(label))
                        .child(
                            div()
                                .w(20.0)
                                .h(20.0)
                                .accessibility_id("status.close")
                                .accessibility_role(Role::Button)
                                .accessibility_label("Dismiss"),
                        )
                }))
            };
            let tree = |status| painted(view(status)).tree_update("Test", None);
            let mut probe = AtspiProbe::new(tree(None));

            assert_eq!(probe.update(tree(Some("Copied"))), ["Copied (Polite)"]);
            assert_eq!(probe.update(tree(Some("Copied"))), Vec::<String>::new());
            assert_eq!(
                probe.update(tree(Some("Copied 2 files"))),
                ["Copied 2 files (Polite)"]
            );
        }

        // Each announcement is spoken once: republishing it is silent, and
        // announcing the same text again speaks it again.
        #[test]
        fn each_announcement_is_spoken_once() {
            let frame = AccessibilityFrame::new(100.0, 100.0);
            let mut announcer = Announcer::default();
            let tree = |announcer: &Announcer| {
                let mut update = frame.tree_update("Test", None);
                announcer.publish(&mut update);
                update
            };
            let mut probe = AtspiProbe::new(tree(&announcer));

            announcer.announce("Saved", Politeness::Polite);
            assert_eq!(probe.update(tree(&announcer)), ["Saved (Polite)"]);
            assert_eq!(probe.update(tree(&announcer)), Vec::<String>::new());
            announcer.announce("Saved", Politeness::Polite);
            assert_eq!(probe.update(tree(&announcer)), ["Saved (Polite)"]);
            announcer.announce("Failed", Politeness::Assertive);
            assert_eq!(probe.update(tree(&announcer)), ["Failed (Assertive)"]);
        }
    }
}
