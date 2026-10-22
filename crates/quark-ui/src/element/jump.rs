//! Jump mode: a short label on every element the keyboard can act on, as
//! browsers' link hints do. Typing a label clicks its element; see
//! [`JumpHints`].

use super::*;

/// Home row keys, in the order labels use them. `;` is left out: it
/// moves between keyboard layouts.
const ALPHABET: &[u8] = b"asdfghjkl";
const BASE: usize = ALPHABET.len();
/// Four keys name 6561 elements, more than a window shows.
const MAX_LABEL: usize = 4;
/// Above anything a window or a drag preview paints.
const JUMP_Z: i32 = i32::MAX;

#[derive(Debug, Clone, Copy)]
struct Target {
    node: usize,
    /// The part of the element on screen, in window points.
    rect: Rect,
    focus: Option<FocusId>,
    /// Completing the label clicks the element; otherwise it only takes
    /// focus.
    clicks: bool,
}

/// What a key did in jump mode; see [`JumpHints::key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpKey {
    /// A label key or Backspace narrowed or widened the labels shown.
    Typed,
    /// The mode ended without a choice.
    Cancelled,
    /// A label is complete and the mode ended.
    Chosen(JumpChoice),
}

/// The element a completed label names, as a node of the frame the labels
/// were last collected from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JumpChoice {
    pub node: usize,
    /// Where focus goes, when the element can take it.
    pub focus: Option<FocusId>,
    /// Click the element ([`InputRouter::click_node`]) once focused. False
    /// for text fields and when Shift was held on a label key.
    pub click: bool,
}

/// Keyboard jump mode for one window.
///
/// [`Self::start`] labels every enabled element of a frame that has a
/// click handler or is a text field, that is on screen, and that no
/// pointer-blocking layer covers; inside an open modal scope, only its
/// elements. Labels are home row keys (`asdfghjkl`), all of one length:
/// one key for up to nine elements, two for up to 81, and so on, handed
/// out in reading order (top to bottom, then left to right), so the same
/// layout always gets the same labels.
///
/// The host passes key presses to [`Self::key`] while [`Self::is_active`]:
/// label keys narrow the labels shown, Backspace takes the last one back,
/// and any other key, Escape included, ends the mode. A completed label
/// names a [`JumpChoice`]; Shift on a label key focuses the element
/// without clicking it. Each new frame goes through [`Self::refresh`]
/// before [`Self::paint`] draws the labels over it. Costs nothing while
/// inactive.
#[derive(Default)]
pub struct JumpHints {
    active: bool,
    targets: Vec<Target>,
    label_len: usize,
    /// Alphabet indices of the keys typed so far.
    typed: [u8; MAX_LABEL],
    typed_len: usize,
    focus_only: bool,
    window: Rect,
    /// Per semantic node, the union of its visible hit regions.
    visible: Vec<Option<Rect>>,
    stack: Vec<HitId>,
}

impl JumpHints {
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Label the actionable elements `frame` shows and start the mode.
    /// Returns how many got a label; with none, the mode stays off.
    pub fn start(&mut self, frame: &InputFrame) -> usize {
        self.typed_len = 0;
        self.focus_only = false;
        self.collect(frame);
        self.active = !self.targets.is_empty();
        self.targets.len()
    }

    /// Label `frame`, a newer frame of the same window, keeping what was
    /// typed. An unchanged layout keeps its labels. Ends the mode when no
    /// label is left that matches the keys typed.
    pub fn refresh(&mut self, frame: &InputFrame) {
        if !self.active {
            return;
        }
        self.collect(frame);
        if !(0..self.targets.len()).any(|i| self.matches(i)) {
            self.cancel();
        }
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.targets.clear();
        self.typed_len = 0;
        self.focus_only = false;
    }

    /// A key press while active.
    pub fn key(&mut self, pressed: &Binding) -> JumpKey {
        let m = pressed.mods;
        let plain = !(m.cmd || m.ctrl || m.alt || m.primary);
        if !self.active || !plain {
            self.cancel();
            return JumpKey::Cancelled;
        }
        if pressed.key == "backspace" {
            self.typed_len = self.typed_len.saturating_sub(1);
            return JumpKey::Typed;
        }
        let Some(digit) = ALPHABET.iter().position(|c| pressed.key.as_bytes() == [*c]) else {
            self.cancel();
            return JumpKey::Cancelled;
        };
        self.typed[self.typed_len] = digit as u8;
        self.typed_len += 1;
        self.focus_only |= m.shift;
        let Some(first) = (0..self.targets.len()).find(|&i| self.matches(i)) else {
            self.cancel();
            return JumpKey::Cancelled;
        };
        if self.typed_len < self.label_len {
            return JumpKey::Typed;
        }
        let target = self.targets[first];
        let click = target.clicks && !self.focus_only;
        self.cancel();
        JumpKey::Chosen(JumpChoice {
            node: target.node,
            focus: target.focus,
            click,
        })
    }

    /// Draw the labels still matching on top of `scene`: an accent chip at
    /// each element's top left, kept inside the window, with the keys
    /// typed so far dimmed.
    pub fn paint(&self, scene: &mut Scene, cx: &mut ElementContext) {
        if !self.active {
            return;
        }
        let metrics = cx.theme.metrics;
        let fill = cx.theme.colors.accent;
        let edge = cx.theme.colors.background;
        let ink = cx.theme.colors.on_accent;
        let style = TextStyle::new(metrics.ui_small_font_size)
            .kind(FontKind::Mono)
            .weight(FontWeight::Semibold);
        let pad = metrics.spacing_xs;
        scene.push_z_index(JUMP_Z);
        for (i, target) in self.targets.iter().enumerate() {
            if !self.matches(i) {
                continue;
            }
            let mut keys = [0u8; MAX_LABEL];
            for (position, key) in keys[..self.label_len].iter_mut().enumerate() {
                *key = ALPHABET[self.digit(i, position)];
            }
            let Ok(label) = std::str::from_utf8(&keys[..self.label_len]) else {
                continue;
            };
            let (typed, rest) = label.split_at(self.typed_len);
            let typed = (!typed.is_empty())
                .then(|| cx.layout_text_query(&TextQuery::new(typed, style)))
                .flatten();
            let Some(rest) = cx.layout_text_query(&TextQuery::new(rest, style)) else {
                continue;
            };
            let typed_width = typed.as_ref().map_or(0.0, |layout| layout.size().0);
            let (rest_width, height) = rest.size();
            let width = typed_width + rest_width + 2.0 * pad;
            let x = target
                .rect
                .x
                .min(self.window.right() - width)
                .max(self.window.x);
            let y = target
                .rect
                .y
                .min(self.window.bottom() - height)
                .max(self.window.y);
            let chip = Rect {
                x,
                y,
                width,
                height,
            };
            let radius = metrics.control_radius.min(height / 2.0);
            scene.rounded_rect(RoundedRectPrimitive::uniform(chip, radius, fill));
            scene.border(BorderPrimitive::uniform(
                chip,
                Sz::SEPARATOR_W,
                radius,
                edge,
            ));
            let text_x = x + pad;
            if let Some(typed) = typed {
                scene.text(TextPrimitive {
                    rect: Rect {
                        x: text_x,
                        y,
                        width: typed_width,
                        height,
                    },
                    layout: ShapedText::new(typed),
                    color: ink.with_alpha(Alpha::MEDIUM),
                });
            }
            scene.text(TextPrimitive {
                rect: Rect {
                    x: text_x + typed_width,
                    y,
                    width: rest_width,
                    height,
                },
                layout: ShapedText::new(rest),
                color: ink,
            });
        }
        scene.pop_z_index();
    }

    fn collect(&mut self, frame: &InputFrame) {
        let semantic = &frame.semantic;
        let hits = &frame.hits;
        let nodes = semantic.nodes();
        self.window = semantic.root_bounds();
        self.visible.clear();
        self.visible.resize(nodes.len(), None);
        for id in hits.ids() {
            let (Some(node), Some(bounds), Some(clip)) =
                (hits.node(id), hits.bounds(id), hits.clip(id))
            else {
                continue;
            };
            let Some(rect) = bounds
                .intersection(clip)
                .and_then(|rect| rect.intersection(self.window))
            else {
                continue;
            };
            if let Some(slot) = self.visible.get_mut(node) {
                *slot = Some(slot.map_or(rect, |seen| union(seen, rect)));
            }
        }
        let modal = semantic.modal_root();
        self.targets.clear();
        for (index, node) in nodes.iter().enumerate() {
            let field = node.actions.text_value;
            let clicks = frame.handlers.click(index).is_some();
            if node.state.disabled || !(clicks || field) {
                continue;
            }
            if modal.is_some_and(|modal| !semantic.is_within(index, modal)) {
                continue;
            }
            let Some(rect) = self.visible[index] else {
                continue;
            };
            // A press at its middle would land on it, so no pointer-blocking
            // layer (a popover, a modal's backdrop) covers it.
            let (x, y) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
            hits.stack_at_into(x, y, &mut self.stack);
            let reached = self.stack.iter().any(|id| {
                hits.node(*id)
                    .is_some_and(|hit| semantic.is_within(hit, index))
            });
            if !reached {
                continue;
            }
            self.targets.push(Target {
                node: index,
                rect,
                focus: semantic.focus_id(index),
                clicks: clicks && !field,
            });
        }
        self.targets.sort_by(|a, b| {
            a.rect
                .y
                .total_cmp(&b.rect.y)
                .then(a.rect.x.total_cmp(&b.rect.x))
        });
        self.targets.truncate(BASE.pow(MAX_LABEL as u32));
        self.label_len = 1;
        while BASE.pow(self.label_len as u32) < self.targets.len() {
            self.label_len += 1;
        }
    }

    /// Alphabet index of the key at `position` in target `i`'s label.
    fn digit(&self, i: usize, position: usize) -> usize {
        i / BASE.pow((self.label_len - 1 - position) as u32) % BASE
    }

    fn matches(&self, i: usize) -> bool {
        (0..self.typed_len)
            .all(|position| self.digit(i, position) == usize::from(self.typed[position]))
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    Rect {
        x,
        y,
        width: a.right().max(b.right()) - x,
        height: a.bottom().max(b.bottom()) - y,
    }
}
