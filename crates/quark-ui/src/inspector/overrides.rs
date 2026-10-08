//! Session style overrides: inspector edits keyed by an element's stable
//! key, applied over its declared style every frame until cleared. Nothing
//! is persisted.

use std::collections::HashMap;

use quark::{Color, ElementStyle, UiKey};

/// The properties an override can replace. `None` keeps the declared value.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StyleEdit {
    /// Uniform padding, in points.
    pub padding: Option<f32>,
    /// Row and column gap, in points.
    pub gap: Option<f32>,
    pub background: Option<Color>,
    pub border_color: Option<Color>,
    /// Uniform corner radius, in points.
    pub radius: Option<f32>,
}

impl StyleEdit {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The edit as `view!` attributes to paste into the element's source:
    /// `p={12.0} bg={Color::rgba(235, 87, 87, 255)}`. Values are the points
    /// on screen; in a `view! { scale, .. }`, divide padding, gap, and
    /// radius by the zoom.
    pub fn patch(&self) -> String {
        let color = |c: Color| format!("Color::rgba({}, {}, {}, {})", c.r, c.g, c.b, c.a);
        let attrs = [
            self.padding.map(|v| format!("p={{{v:?}}}")),
            self.gap.map(|v| format!("gap={{{v:?}}}")),
            self.background.map(|c| format!("bg={{{}}}", color(c))),
            self.border_color
                .map(|c| format!("border={{{}}}", color(c))),
            self.radius.map(|v| format!("rounded={{{v:?}}}")),
        ];
        attrs.into_iter().flatten().collect::<Vec<_>>().join(" ")
    }

    pub fn apply(&self, style: &mut ElementStyle) {
        if let Some(padding) = self.padding {
            let length = taffy::LengthPercentage::length(padding);
            style.layout.padding = taffy::Rect {
                left: length,
                right: length,
                top: length,
                bottom: length,
            };
        }
        if let Some(gap) = self.gap {
            let length = taffy::LengthPercentage::length(gap);
            style.layout.gap = taffy::Size {
                width: length,
                height: length,
            };
        }
        if let Some(background) = self.background {
            style.background = Some(background);
        }
        if let Some(border) = self.border_color {
            style.border_color = Some(border);
            // A border color on a borderless element would never show.
            if style.border_widths == [0.0; 4] {
                style.border_widths = [1.0; 4];
            }
        }
        if let Some(radius) = self.radius {
            style.corner_radii = [radius; 4];
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StyleOverrides {
    edits: HashMap<UiKey, StyleEdit>,
    /// Bumped by every change, so cached subtrees rebuild under new edits.
    revision: u64,
}

impl StyleOverrides {
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    pub fn len(&self) -> usize {
        self.edits.len()
    }

    pub fn get(&self, key: &UiKey) -> Option<&StyleEdit> {
        self.edits.get(key)
    }

    /// Changes so far; zero while no edit was ever made.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Change the override for `key`; an edit left empty is dropped.
    pub fn edit(&mut self, key: UiKey, f: impl FnOnce(&mut StyleEdit)) {
        let edit = self.edits.entry(key.clone()).or_default();
        f(edit);
        if edit.is_empty() {
            self.edits.remove(&key);
        }
        self.revision += 1;
    }

    pub fn clear(&mut self, key: &UiKey) {
        if self.edits.remove(key).is_some() {
            self.revision += 1;
        }
    }

    pub fn clear_all(&mut self) {
        if !self.edits.is_empty() {
            self.edits.clear();
            self.revision += 1;
        }
    }

    pub fn apply(&self, key: &UiKey, style: &mut ElementStyle) {
        if let Some(edit) = self.edits.get(key) {
            edit.apply(style);
        }
    }

    /// Every edit as [`StyleEdit::patch`], one line per element in key
    /// order, each after a comment naming its key.
    pub fn patch(&self) -> String {
        let mut edits: Vec<_> = self.edits.iter().collect();
        edits.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        edits
            .into_iter()
            .map(|(key, edit)| format!("// {key}\n{}\n", edit.patch()))
            .collect()
    }
}
