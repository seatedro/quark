//! Lays out arbitrary UTF-8 at an arbitrary wrap width and checks the
//! layout's integrity and that hit-testing and carets stay in bounds.
//!
//! Input: two little-endian bytes of wrap width in 1/8 px (0 = no wrap),
//! then the text (invalid UTF-8 is replaced, so every input lays out).

#![no_main]

use std::sync::{Mutex, OnceLock};

use libfuzzer_sys::fuzz_target;
use quark_text::{FontSettings, TextParams, TextStyle, TextSystem};
use unicode_segmentation::UnicodeSegmentation;

fn system() -> std::sync::MutexGuard<'static, TextSystem> {
    static SYSTEM: OnceLock<Mutex<TextSystem>> = OnceLock::new();
    SYSTEM
        .get_or_init(|| Mutex::new(TextSystem::vendored_only(&FontSettings::default())))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fuzz_target!(|data: &[u8]| {
    let Some((head, body)) = data.split_first_chunk::<2>() else {
        return;
    };
    let wrap = match u16::from_le_bytes(*head) {
        0 => None,
        w => Some(f32::from(w) / 8.0),
    };
    let text = String::from_utf8_lossy(body);
    let params = TextParams::new(text.as_ref(), TextStyle::new(14.0)).wrap_width(wrap);
    let layout = system().layout(&params).expect("valid params lay out");
    assert_eq!(layout.verify_integrity(), Ok(()));

    let mut boundaries: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
    boundaries.push(text.len());
    let (width, height) = layout.size();
    let points = [
        (-10.0, -10.0),
        (width * 0.5, height * 0.5),
        (width + 10.0, height + 10.0),
        (width * 0.25, height * 0.75),
    ];
    for (x, y) in points {
        let b = layout.hit(x, y);
        assert!(boundaries.contains(&b), "hit({x}, {y}) = {b}");
    }
    for (b, _) in text.char_indices().chain([(text.len(), ' ')]) {
        let caret = layout.caret(b);
        assert!(caret.line < layout.line_count());
        assert!(caret.x.is_finite() && caret.y.is_finite());
    }
    for rect in layout.selection_rects(0..text.len()) {
        assert!(rect.x.is_finite() && rect.width >= 0.0 && rect.y >= 0.0);
    }
});
