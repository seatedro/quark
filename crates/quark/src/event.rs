/// Event categories the native platform can route without exposing DOM events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiEventKind {
    Click,
    PointerDown,
    PointerUp,
    PointerMove,
    PointerEnter,
    PointerLeave,
    Wheel,
    KeyDown,
    TextInput,
    Focus,
    Blur,
    /// A secondary click or the keyboard's context menu key.
    ContextMenu,
}

/// Capture/target/bubble phases borrowed from the web, expressed as native data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiEventPhase {
    Capture,
    Target,
    Bubble,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiEventPropagation {
    Continue,
    Stop,
}

/// Handler result: explicit propagation/default intent, no implicit exceptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UiEventResult {
    pub propagation: UiEventPropagation,
    pub default_prevented: bool,
}

impl UiEventResult {
    pub const fn continue_propagation() -> Self {
        Self {
            propagation: UiEventPropagation::Continue,
            default_prevented: false,
        }
    }

    pub const fn stop_propagation() -> Self {
        Self {
            propagation: UiEventPropagation::Stop,
            default_prevented: false,
        }
    }

    pub const fn prevent_default() -> Self {
        Self {
            propagation: UiEventPropagation::Continue,
            default_prevented: true,
        }
    }

    pub const fn stop_and_prevent_default() -> Self {
        Self {
            propagation: UiEventPropagation::Stop,
            default_prevented: true,
        }
    }

    pub const fn should_continue(self) -> bool {
        matches!(self.propagation, UiEventPropagation::Continue)
    }
}

impl Default for UiEventResult {
    fn default() -> Self {
        Self::continue_propagation()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiEventBinding {
    pub kind: UiEventKind,
    pub phase: UiEventPhase,
    pub default_result: UiEventResult,
}

impl UiEventBinding {
    pub fn new(kind: UiEventKind, phase: UiEventPhase) -> Self {
        Self {
            kind,
            phase,
            default_result: UiEventResult::continue_propagation(),
        }
    }

    pub fn with_result(mut self, result: UiEventResult) -> Self {
        self.default_result = result;
        self
    }
}
