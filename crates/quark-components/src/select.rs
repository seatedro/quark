//! A select: a button showing the chosen option that opens a list of
//! options in a popover.
//!
//! The app owns a [`SelectState`] and the options, renders [`select`] from
//! them, and feeds the [`SelectMsg`]s the select emits back through
//! [`SelectState::update`], which returns what changed and where focus goes.
//!
//! Keyboard: Enter, Space, or the arrow keys open the list on the option
//! already chosen. In the list, the arrows, Home, and End move over enabled
//! options; Enter or Space chooses; Escape or Tab closes and returns focus
//! to the button. Letters and digits jump to the next option starting with
//! what was typed, opening the list or not.

use std::rc::Rc;

use quark::{TabStop, view};
use quark_ui::design::Sp;
use quark_ui::element::{AnyElement, IntoAnyElement, svg_icon, text};
use quark_ui::element::{ElementContext, RenderOnce, div};
use quark_ui::icons::lucide;
use quark_ui::style::Styled;
use quark_ui::theme::{Color, Theme};
use quark_ui::{Action, FocusId};

use crate::list_nav::{self, TYPE_AHEAD_KEYS, TypeAhead};
use crate::popover::{PopoverSide, anchored, popover_panel};

/// One option of a [`select`]. Consecutive options with the same `group`
/// are listed under that group's heading.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectOption {
    pub label: String,
    pub group: Option<String>,
    pub disabled: bool,
}

impl SelectOption {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            group: None,
            disabled: false,
        }
    }

    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// What a select asks its [`SelectState`] to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectMsg {
    Toggle,
    Open,
    Close,
    /// Move the highlight by this many enabled options.
    Move(i32),
    First,
    Last,
    /// Choose the highlighted option.
    CommitHighlighted,
    /// Choose option `n` (a click on it).
    Commit(usize),
    TypeAhead(char),
}

/// What [`SelectState::update`] changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SelectOutcome {
    /// The chosen option, when it changed.
    pub changed: Option<usize>,
    /// Where keyboard focus should go (`UiContext::set_focus`).
    pub focus: Option<FocusId>,
}

/// The state of one select: the chosen option, whether the list is open,
/// the highlighted option, and type-ahead.
#[derive(Debug)]
pub struct SelectState {
    id: Rc<str>,
    focus: FocusId,
    selected: Option<usize>,
    open: bool,
    highlighted: Option<usize>,
    type_ahead: TypeAhead,
}

impl SelectState {
    /// `id` must be unique in the window; it names the select's
    /// accessibility ids and focus targets.
    pub fn new(id: &str) -> Self {
        let id: Rc<str> = Rc::from(id);
        Self {
            focus: FocusId::from_key(&id),
            id,
            selected: None,
            open: false,
            highlighted: None,
            type_ahead: TypeAhead::default(),
        }
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn set_selected(&mut self, selected: Option<usize>) {
        self.selected = selected;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn highlighted(&self) -> Option<usize> {
        self.highlighted.filter(|_| self.open)
    }

    /// Focus target of the select's button.
    pub fn focus_id(&self) -> FocusId {
        self.focus
    }

    /// Focus target of option `index` while the list is open.
    pub fn option_focus_id(&self, index: usize) -> FocusId {
        list_nav::item_focus(self.focus, index)
    }

    /// Apply `msg`, emitted by the select rendered from these `options`;
    /// `now_ms` times type-ahead.
    pub fn update(
        &mut self,
        msg: SelectMsg,
        options: &[SelectOption],
        now_ms: u64,
    ) -> SelectOutcome {
        let disabled = |i: usize| options[i].disabled;
        let enabled = |i: usize| i < options.len() && !options[i].disabled;
        let mut out = SelectOutcome::default();
        match msg {
            SelectMsg::Toggle if self.open => {
                return self.update(SelectMsg::Close, options, now_ms);
            }
            SelectMsg::Open | SelectMsg::Toggle => {
                self.open = true;
                self.highlighted = self
                    .selected
                    .filter(|&i| enabled(i))
                    .or_else(|| list_nav::step(options.len(), None, 1, false, disabled));
                out.focus = Some(self.highlight_focus());
            }
            SelectMsg::Close => {
                if self.open {
                    self.open = false;
                    out.focus = Some(self.focus);
                }
            }
            SelectMsg::Move(_) | SelectMsg::First | SelectMsg::Last if !self.open => {
                return self.update(SelectMsg::Open, options, now_ms);
            }
            SelectMsg::Move(delta) => {
                self.highlighted =
                    list_nav::step(options.len(), self.highlighted, delta, false, disabled);
                out.focus = Some(self.highlight_focus());
            }
            SelectMsg::First | SelectMsg::Last => {
                let delta = if msg == SelectMsg::First { 1 } else { -1 };
                self.highlighted = list_nav::step(options.len(), None, delta, false, disabled);
                out.focus = Some(self.highlight_focus());
            }
            SelectMsg::CommitHighlighted => match self.highlighted() {
                Some(i) => return self.update(SelectMsg::Commit(i), options, now_ms),
                None => return self.update(SelectMsg::Close, options, now_ms),
            },
            SelectMsg::Commit(i) => {
                if enabled(i) {
                    out.changed = (self.selected != Some(i)).then_some(i);
                    self.selected = Some(i);
                    self.open = false;
                    out.focus = Some(self.focus);
                }
            }
            SelectMsg::TypeAhead(ch) => {
                let current = if self.open {
                    self.highlighted
                } else {
                    self.selected
                };
                let labels = options
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| !o.disabled)
                    .map(|(i, o)| (i, o.label.as_str()));
                if let Some(found) = self.type_ahead.push(ch, now_ms, current, labels) {
                    if self.open {
                        self.highlighted = Some(found);
                        out.focus = Some(self.highlight_focus());
                    } else if self.selected != Some(found) {
                        self.selected = Some(found);
                        out.changed = Some(found);
                    }
                }
            }
        }
        out
    }

    /// The highlighted option's focus target, or the button's when nothing
    /// can be highlighted.
    fn highlight_focus(&self) -> FocusId {
        self.highlighted
            .map_or(self.focus, |i| self.option_focus_id(i))
    }
}

/// A select rendered from `state` and `options` (the slice its
/// [`SelectState::update`] gets); `on_msg` wraps what it emits in the app's
/// action.
pub fn select(
    state: &SelectState,
    options: Rc<[SelectOption]>,
    on_msg: impl Fn(SelectMsg) -> Action + 'static,
) -> Select {
    Select {
        id: state.id.clone(),
        focus: state.focus,
        selected: state.selected,
        open: state.open,
        highlighted: state.highlighted,
        options,
        on_msg: Box::new(on_msg),
        label: String::new(),
        placeholder: String::new(),
        width: None,
        viewport: (f32::INFINITY, f32::INFINITY),
        disabled: false,
    }
}

pub struct Select {
    id: Rc<str>,
    focus: FocusId,
    selected: Option<usize>,
    open: bool,
    highlighted: Option<usize>,
    options: Rc<[SelectOption]>,
    on_msg: Box<dyn Fn(SelectMsg) -> Action>,
    label: String,
    placeholder: String,
    width: Option<f32>,
    viewport: (f32, f32),
    disabled: bool,
}

impl Select {
    /// The accessible name, such as "Fruit".
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Shown while nothing is chosen.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Width of the button and the list.
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// The window size, so the list flips above the button and stays on
    /// screen near the window's edges.
    pub fn viewport(mut self, size: (f32, f32)) -> Self {
        self.viewport = size;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for Select {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let theme = cx.theme;
        let tc = &theme.colors;
        let m = &theme.metrics;
        let scale = m.ui_scale();
        let width = self.width.unwrap_or((220.0 * scale).round());
        let msg = &self.on_msg;
        let chosen = self.selected.and_then(|i| self.options.get(i));
        let shown = chosen.map_or(self.placeholder.as_str(), |o| o.label.as_str());
        let open = self.open && !self.disabled;

        let closed_keys = ["enter", "space", "arrowdown", "arrowup", "alt+arrowdown"];
        view! {
            <div class="relative flex-col" w={width}
                 @when {!self.disabled && !open} {
                     @for key in closed_keys { on_key={(key, msg(SelectMsg::Open))} }
                 }
                 @when {!self.disabled && open} {
                     on_key={("arrowdown", msg(SelectMsg::Move(1)))}
                     on_key={("arrowup", msg(SelectMsg::Move(-1)))}
                     on_key={("pagedown", msg(SelectMsg::Move(10)))}
                     on_key={("pageup", msg(SelectMsg::Move(-10)))}
                     on_key={("home", msg(SelectMsg::First))}
                     on_key={("end", msg(SelectMsg::Last))}
                     on_key={("enter", msg(SelectMsg::CommitHighlighted))}
                     on_key={("space", msg(SelectMsg::CommitHighlighted))}
                     on_key={("alt+arrowup", msg(SelectMsg::CommitHighlighted))}
                     on_key={("escape", msg(SelectMsg::Close))}
                     on_key={("tab", msg(SelectMsg::Close))}
                     on_key={("shift+tab", msg(SelectMsg::Close))}
                 }
                 @when {!self.disabled && (open || cx.focus == Some(self.focus))} {
                     @for key in TYPE_AHEAD_KEYS {
                         on_key={(key, msg(SelectMsg::TypeAhead(key.chars().next().unwrap_or_default())))}
                     }
                 }>
                <div class="flex-row items-center w-full" gap={m.spacing_sm} px={m.spacing_md}
                     py={m.spacing_xs + (Sp::XXS * scale).round()} rounded={m.control_radius}
                     bg={tc.element_background}
                     border={if open { tc.focus_border } else { tc.border_variant }}
                     accessibility_id={&*self.id} test_id="select-trigger" focus_ring={self.focus}
                     accessibility_role={accesskit::Role::ComboBox} aria-label={self.label.clone()}
                     aria-valuetext={shown.to_owned()} aria-expanded={open}
                     aria-disabled={self.disabled}
                     @when {!self.disabled} {
                         hover_bg={tc.element_hover} class="cursor-pointer"
                         on:click={msg(SelectMsg::Toggle)}
                     }>
                    <div class="flex-1">
                        <text class="text-sm truncate"
                              color={if chosen.is_some() && !self.disabled { tc.text } else { tc.text_muted }}>
                            {shown.to_owned()}
                        </text>
                    </div>
                    <icon svg={if open { lucide::CHEVRON_UP } else { lucide::CHEVRON_DOWN }}
                          size={m.ui_small_font_size} color={tc.text_muted} />
                </div>
                if open {
                    <anchored(
                        view! {
                            <popover_panel(theme) w={width} py={m.spacing_xs}
                                accessibility_id={format!("{}-listbox", self.id)}
                                test_id="select-listbox"
                                accessibility_role={accesskit::Role::ListBox}
                                aria-label={self.label.clone()}>
                                {...option_rows(
                                    &self.id,
                                    &self.options,
                                    self.selected,
                                    self.highlighted,
                                    |i| list_nav::item_focus(self.focus, i),
                                    |i| msg(SelectMsg::Commit(i)),
                                    theme,
                                )}
                            </popover_panel>
                        },
                        PopoverSide::Bottom,
                        self.viewport,
                    ) />
                }
            </div>
        }
    }
}

/// The option rows of a list popover, with a heading per group. Options
/// take focus (so screen readers follow the highlight) but are not Tab
/// stops: the arrow keys move between them.
pub(crate) fn option_rows(
    id: &str,
    options: &[SelectOption],
    selected: Option<usize>,
    highlighted: Option<usize>,
    focus: impl Fn(usize) -> FocusId,
    commit: impl Fn(usize) -> Action,
    theme: &Theme,
) -> Vec<AnyElement> {
    let mut rows = Vec::with_capacity(options.len());
    let mut i = 0;
    while i < options.len() {
        let group = options[i].group.as_deref();
        let end = (i..options.len())
            .find(|&j| options[j].group.as_deref() != group)
            .unwrap_or(options.len());
        let items = (i..end).map(|j| {
            option_row(
                id,
                j,
                &options[j],
                selected == Some(j),
                highlighted == Some(j),
                focus(j),
                commit(j),
                theme,
            )
        });
        match group {
            Some(name) => rows.push(group_box(id, i, name, items, theme)),
            None => rows.extend(items),
        }
        i = end;
    }
    rows
}

fn group_box(
    id: &str,
    first: usize,
    name: &str,
    items: impl Iterator<Item = AnyElement>,
    theme: &Theme,
) -> AnyElement {
    let m = &theme.metrics;
    view! {
        <div class="flex-col w-full" accessibility_id={format!("{id}-group-{first}")}
             accessibility_role={accesskit::Role::Group} aria-label={name.to_owned()}>
            <div px={m.spacing_md} pt={m.spacing_xs} pb={(Sp::XXS * m.ui_scale()).round()}>
                <text class="text-xs font-semibold" color={theme.colors.text_muted}>
                    {name.to_owned()}
                </text>
            </div>
            {...items}
        </div>
    }
}

#[allow(clippy::too_many_arguments)]
fn option_row(
    id: &str,
    index: usize,
    option: &SelectOption,
    selected: bool,
    highlighted: bool,
    focus: FocusId,
    commit: Action,
    theme: &Theme,
) -> AnyElement {
    let tc = &theme.colors;
    let m = &theme.metrics;
    let scale = m.ui_scale();
    view! {
        <div class="flex-row items-center w-full" gap={m.spacing_sm} px={m.spacing_md}
             py={m.spacing_xs + (Sp::XXS * scale).round()}
             accessibility_id={format!("{id}-option-{index}")} test_id="select-option"
             accessibility_role={accesskit::Role::ListBoxOption}
             aria-label={option.label.clone()} aria-selected={selected}
             aria-disabled={option.disabled}
             bg={if highlighted { tc.ghost_element_selected } else { Color::TRANSPARENT }}
             @when {!option.disabled} {
                 focus_ring={focus} tab_stop={TabStop::disabled(0)}
                 hover_bg={tc.ghost_element_hover} class="cursor-pointer" on:click={commit}
             }>
            <div class="flex-1">
                <text class="text-sm truncate"
                      color={if option.disabled {
                          tc.text_muted
                      } else if selected {
                          tc.text_strong
                      } else {
                          tc.text
                      }}>
                    {option.label.clone()}
                </text>
            </div>
            if selected {
                <icon svg={lucide::CHECK} size={m.ui_small_font_size} color={tc.accent} />
            }
        </div>
    }
}
