use std::fmt;
use std::ops::Deref;
use std::sync::{Arc, LazyLock};

use crate::layout::TextSpan;

/// The text and spans a [`TextLayout`](crate::TextLayout) was laid out
/// from: shared, immutable while shared, and stored with spare capacity.
/// Cloning shares it.
///
/// Its owner writes the next text over it once nothing else holds it (the
/// [`LayoutCache`](crate::LayoutCache) refilling an evicted layout, a
/// [`TextBlock`](crate::TextBlock) at its next revision), so text of another
/// length reuses the storage rather than allocating. Whoever still holds a
/// clone keeps the text it had: only [`Arc::get_mut`] grants the write.
#[derive(Clone)]
pub struct TextSource(Arc<SourceText>);

#[derive(Debug, Default)]
struct SourceText {
    text: String,
    spans: Vec<TextSpan>,
}

impl TextSource {
    /// A source holding copies of `text` and `spans`.
    pub fn new(text: &str, spans: &[TextSpan]) -> Self {
        Self(Arc::new(SourceText {
            text: text.to_owned(),
            spans: spans.to_vec(),
        }))
    }

    /// The empty source every new layout starts from. Shared, so it is
    /// never written over.
    pub(crate) fn empty() -> Self {
        static EMPTY: LazyLock<TextSource> = LazyLock::new(|| TextSource::new("", &[]));
        EMPTY.clone()
    }

    pub fn as_str(&self) -> &str {
        &self.0.text
    }

    pub fn spans(&self) -> &[TextSpan] {
        &self.0.spans
    }

    /// Whether `a` and `b` are the same storage.
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }

    /// Bytes of text it holds without growing.
    pub fn capacity(&self) -> usize {
        self.0.text.capacity()
    }

    /// Bytes it keeps: text and span capacity.
    pub fn storage_bytes(&self) -> usize {
        size_of::<SourceText>()
            + self.0.text.capacity()
            + self.0.spans.capacity() * size_of::<TextSpan>()
    }

    /// Whether nothing else holds this storage.
    pub(crate) fn is_unshared(&mut self) -> bool {
        Arc::get_mut(&mut self.0).is_some()
    }

    /// Writes `text` and `spans` over this storage when nothing else holds
    /// it, returning whether it did.
    pub(crate) fn overwrite(&mut self, text: &str, spans: &[TextSpan]) -> bool {
        let Some(own) = Arc::get_mut(&mut self.0) else {
            return false;
        };
        own.text.clear();
        own.text.push_str(text);
        own.spans.clear();
        own.spans.extend_from_slice(spans);
        true
    }

    /// Sets this to `text` and `spans`: in place when nothing else holds
    /// the storage, in new storage otherwise.
    pub(crate) fn set(&mut self, text: &str, spans: &[TextSpan]) {
        if !self.overwrite(text, spans) {
            *self = Self::new(text, spans);
        }
    }
}

impl Deref for TextSource {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for TextSource {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for TextSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for TextSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}
