//! The theme every window paints with: fixed, or following the desktop's
//! light or dark preference, with native window chrome to match.

use quark_ui::theme::{Theme, ThemeMode};
use winit::window::Theme as SystemTheme;

use super::adapter::Shared;
use super::{Redraw, UiAdapter, UiApp, UiContext, redraw_all};
use crate::{EventContext, WindowHandle};

/// Which theme the adapter paints with.
pub(super) enum ThemeChoice {
    /// One theme whatever the desktop prefers.
    Fixed,
    /// Follow the desktop's light or dark preference.
    System { light: Box<Theme>, dark: Box<Theme> },
}

impl Shared {
    /// The native theme (title bar, window decorations) that matches the
    /// painted one: forced to a fixed theme's mode, and left to the desktop
    /// when following it.
    fn native_theme(&self) -> Option<SystemTheme> {
        match self.theme_choice {
            ThemeChoice::Fixed => Some(match self.theme.mode {
                ThemeMode::Light => SystemTheme::Light,
                ThemeMode::Dark => SystemTheme::Dark,
            }),
            ThemeChoice::System { .. } => None,
        }
    }

    /// Give every open window [`Self::native_theme`]. Windows still opening
    /// get it from [`AppEvent::WindowOpened`](crate::AppEvent::WindowOpened).
    fn theme_native_windows(&self, cx: &EventContext) {
        for window in cx.windows() {
            self.theme_native_window(window, cx);
        }
    }

    pub(super) fn theme_native_window(&self, window: WindowHandle, cx: &EventContext) {
        if let Some(native) = cx.window_by_handle(window) {
            native.set_theme(self.native_theme());
        }
    }

    /// The desktop now prefers `system`: paint with the matching theme when
    /// following it. True when the theme follows.
    pub(super) fn follow_desktop(&mut self, system: SystemTheme) -> bool {
        let ThemeChoice::System { light, dark } = &self.theme_choice else {
            return false;
        };
        self.theme = match system {
            SystemTheme::Light => (**light).clone(),
            SystemTheme::Dark => (**dark).clone(),
        };
        true
    }
}

impl UiContext<'_, '_> {
    /// The theme the next frame paints with.
    pub fn theme(&self) -> &Theme {
        &self.shared.theme
    }

    /// Paint every window with `theme` from the next frame on, whatever
    /// the desktop prefers. Cached elements rebuild in the new colors; text
    /// layouts are reused unless the theme's font sizes changed.
    pub fn set_theme(&mut self, theme: Theme) {
        self.shared.theme_choice = ThemeChoice::Fixed;
        self.apply_theme(theme);
        self.shared.theme_native_windows(self.window);
    }

    /// Paint with `light` or `dark` as the desktop prefers, from the next
    /// frame on (dark while the preference is unknown).
    pub fn set_themes(&mut self, light: Theme, dark: Theme) {
        let theme = match self.window.theme() {
            Some(SystemTheme::Light) => light.clone(),
            _ => dark.clone(),
        };
        self.shared.theme_choice = ThemeChoice::System {
            light: Box::new(light),
            dark: Box::new(dark),
        };
        self.apply_theme(theme);
        self.shared.theme_native_windows(self.window);
    }

    fn apply_theme(&mut self, theme: Theme) {
        if self.shared.theme != theme {
            self.shared.theme = theme;
            redraw_all(Redraw::Theme, self.window);
        }
    }

    /// Change font families or ligatures live. Text is reshaped once with
    /// the new fonts (layout caches drop the old shapes); editors reshape
    /// on their next flush.
    pub fn set_fonts(&mut self, fonts: &quark_text::FontSettings) {
        let system = &mut self.window.text().system;
        let before = system.font_epoch();
        system.set_font_settings(fonts);
        if system.font_epoch() != before {
            // Every window shares the text system.
            self.window.request_redraw_all();
        }
    }
}

impl<U: UiApp> UiAdapter<U> {
    /// Paint with `theme` whatever the desktop prefers.
    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.shared.theme = theme;
        self.shared.theme_choice = ThemeChoice::Fixed;
        self
    }

    /// Paint with `light` or `dark` as the desktop prefers.
    pub fn with_themes(mut self, light: Theme, dark: Theme) -> Self {
        self.shared.theme = dark.clone();
        self.shared.theme_choice = ThemeChoice::System {
            light: Box::new(light),
            dark: Box::new(dark),
        };
        self
    }

    /// The theme the next frame paints with.
    pub fn theme(&self) -> &Theme {
        &self.shared.theme
    }
}
