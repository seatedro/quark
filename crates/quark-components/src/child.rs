use quark_ui::element::{AnyElement, IntoAnyElement};

/// One child of a component written in `view!`. Text keeps its string, so
/// a component can use it as well as paint it: a [`Button`](crate::Button)
/// with only text children takes its accessible name from them.
pub enum Child {
    Text(String),
    Element(AnyElement),
}

impl Child {
    /// The text of `children`, joined; `None` if any child is not text.
    pub fn text_of(children: &[Child]) -> Option<String> {
        children
            .iter()
            .map(|child| match child {
                Child::Text(text) => Some(text.as_str()),
                Child::Element(_) => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(|parts| parts.concat())
    }
}

impl From<&str> for Child {
    fn from(text: &str) -> Self {
        Child::Text(text.to_owned())
    }
}

impl From<String> for Child {
    fn from(text: String) -> Self {
        Child::Text(text)
    }
}

impl From<AnyElement> for Child {
    fn from(element: AnyElement) -> Self {
        Child::Element(element)
    }
}

impl IntoAnyElement for Child {
    fn into_any(self) -> AnyElement {
        match self {
            Child::Text(text) => text.into_any(),
            Child::Element(element) => element,
        }
    }
}
