//! App-defined payloads carried through the element tree.
//!
//! Quark does not know the hosting app's action enum or focus enum. Elements
//! carry a type-erased [`Action`] and an opaque [`FocusId`]; the app converts
//! its own types with `From` impls and downcasts actions when it dispatches.

use std::any::Any;
use std::fmt;
use std::rc::Rc;

/// Object-safe view of an app action value. Implemented for every
/// `Any + Debug + PartialEq` type, so apps never implement it by hand.
pub trait ActionPayload: Any + fmt::Debug {
    fn as_any(&self) -> &dyn Any;
    fn eq_dyn(&self, other: &dyn ActionPayload) -> bool;
}

impl<T: Any + fmt::Debug + PartialEq> ActionPayload for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn eq_dyn(&self, other: &dyn ActionPayload) -> bool {
        other.as_any().downcast_ref::<T>() == Some(self)
    }
}

/// A type-erased, cheaply clonable app action.
///
/// Apps write `impl From<MyAction> for quark_ui::Action` once, pass
/// `MyAction::Foo.into()` to builders such as `on_click`, and recover the
/// value with [`Action::downcast_ref`] in their dispatch loop.
#[derive(Clone)]
pub struct Action(Rc<dyn ActionPayload>);

impl Action {
    pub fn new<T: Any + fmt::Debug + PartialEq>(value: T) -> Self {
        Self(Rc::new(value))
    }

    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        // Deref explicitly: `Rc<dyn ActionPayload>` must not resolve
        // `as_any` against the `Rc` itself.
        (*self.0).as_any().downcast_ref::<T>()
    }

    pub fn is<T: Any>(&self) -> bool {
        self.downcast_ref::<T>().is_some()
    }
}

impl PartialEq for Action {
    fn eq(&self, other: &Self) -> bool {
        (*self.0).eq_dyn(&*other.0)
    }
}

impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}

/// Identity of a focusable target, shared with `quark::FocusTree` and
/// accessibility focus. Apps map their own focus enum to it
/// (`impl From<MyFocus> for FocusId`) or derive it from a stable key with
/// [`FocusId::from_key`].
pub use quark::FocusId;

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum Demo {
        Open(u32),
        Close,
    }

    #[test]
    fn action_round_trips_and_compares() {
        let a = Action::new(Demo::Open(3));
        assert_eq!(a.downcast_ref::<Demo>(), Some(&Demo::Open(3)));
        assert_eq!(a, Action::new(Demo::Open(3)));
        assert_ne!(a, Action::new(Demo::Close));
        assert_ne!(a, Action::new(3u32));
        assert!(!a.is::<u32>());
        assert_eq!(format!("{a:?}"), "Open(3)");
    }
}
