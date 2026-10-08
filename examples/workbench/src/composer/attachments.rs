//! Attachment chips, the 10 MiB limit, and fixture attachments (stream D).
//!
//! Attachments live in demo memory: a drop or a large paste copies the
//! bytes here, and Send hands their IDs to the run. Anything over
//! [`ATTACHMENT_LIMIT_BYTES`] is refused with a visible error and leaves
//! the draft as it was. A removed chip can be restored with Undo until the
//! text is edited again.

use std::path::Path;
use std::sync::Arc;

use accesskit::Role;
use quark::view;
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, svg_icon, text};
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{InputHooks, Insertion};
use quark_app::quark_ui::theme::Theme;

use super::Action;
use crate::contracts::{ATTACHMENT_LIMIT_BYTES, AttachmentId};

/// Pastes longer than this become a text attachment instead of text.
pub const PASTE_ATTACHMENT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    File,
    Text,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub id: AttachmentId,
    pub name: String,
    pub kind: Kind,
    pub bytes: Arc<[u8]>,
}

/// The attachments of the draft being edited, and the paste and drop
/// policy that fills them.
#[derive(Debug, Default)]
pub struct Attachments {
    pub items: Vec<Attachment>,
    /// Why the last attachment was refused, shown above the composer.
    pub error: Option<String>,
    /// Removed chips, newest last, each with the edit count it was removed
    /// at and where it stood.
    removed: Vec<(u64, usize, Attachment)>,
    /// Text edits so far; a removal is undoable until the next one.
    pub edits: u64,
    next_id: u32,
}

impl Attachments {
    /// Add `bytes` as `name`, or refuse it over the limit.
    pub fn add(&mut self, name: String, kind: Kind, bytes: Arc<[u8]>) -> bool {
        if bytes.len() > ATTACHMENT_LIMIT_BYTES {
            self.refuse(&name, bytes.len());
            return false;
        }
        self.next_id += 1;
        self.error = None;
        self.items.push(Attachment {
            id: AttachmentId(self.next_id),
            name,
            kind,
            bytes,
        });
        true
    }

    fn refuse(&mut self, name: &str, len: usize) {
        self.error = Some(format!(
            "{name} is {}, over the {} attachment limit",
            size_label(len),
            size_label(ATTACHMENT_LIMIT_BYTES)
        ));
    }

    /// Add the file at `path`, reading its size first so an oversized file
    /// is refused without being read.
    pub fn add_path(&mut self, path: &Path) -> bool {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into(),
        );
        match std::fs::metadata(path) {
            Ok(meta) if meta.len() > ATTACHMENT_LIMIT_BYTES as u64 => {
                self.refuse(&name, usize::try_from(meta.len()).unwrap_or(usize::MAX));
                false
            }
            Ok(_) => match std::fs::read(path) {
                Ok(bytes) => {
                    let kind = if is_image(&name) {
                        Kind::Image
                    } else {
                        Kind::File
                    };
                    self.add(name, kind, bytes.into())
                }
                Err(err) => self.fail(&name, &err),
            },
            Err(err) => self.fail(&name, &err),
        }
    }

    fn fail(&mut self, name: &str, err: &std::io::Error) -> bool {
        self.error = Some(format!("Could not attach {name}: {err}"));
        false
    }

    pub fn remove(&mut self, id: AttachmentId) {
        if let Some(at) = self.items.iter().position(|a| a.id == id) {
            let attachment = self.items.remove(at);
            self.removed.push((self.edits, at, attachment));
        }
    }

    /// Put back the last removed chip if nothing was typed since; returns
    /// whether it did.
    pub fn undo_remove(&mut self) -> bool {
        match self.removed.last() {
            Some((edits, _, _)) if *edits == self.edits => {}
            _ => return false,
        }
        let Some((_, at, attachment)) = self.removed.pop() else {
            return false;
        };
        self.items.insert(at.min(self.items.len()), attachment);
        true
    }

    /// Take the items for another draft, forgetting removals.
    pub fn take(&mut self) -> Vec<Attachment> {
        self.removed.clear();
        self.error = None;
        std::mem::take(&mut self.items)
    }

    pub fn ids(&self) -> Vec<AttachmentId> {
        self.items.iter().map(|a| a.id).collect()
    }
}

impl InputHooks for Attachments {
    fn paste(&mut self, pasted: Insertion) -> Insertion {
        match pasted {
            Insertion::Text(text) if text.len() > PASTE_ATTACHMENT_BYTES => {
                let name = format!("Pasted text ({})", size_label(text.len()));
                self.add(name, Kind::Text, Arc::from(text.into_bytes()));
                Insertion::Nothing
            }
            other => other,
        }
    }

    fn drop_path(&mut self, path: &Path) -> Insertion {
        self.add_path(path);
        Insertion::Nothing
    }
}

fn is_image(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// `len` bytes in the largest unit that keeps it at least 1, one decimal
/// past KB.
pub fn size_label(len: usize) -> String {
    const KIB: f64 = 1024.0;
    let len = len as f64;
    if len < KIB {
        format!("{len} B")
    } else if len < KIB * KIB {
        format!("{:.0} KB", len / KIB)
    } else {
        format!("{:.1} MB", len / (KIB * KIB))
    }
}

/// The chips above the editor, each with a keyboard-reachable remove
/// button.
pub fn chips(attachments: &Attachments, theme: &Theme) -> AnyElement {
    let colors = &theme.colors;
    view! {
        <div class="flex-row gap-[6]">
            for a in &attachments.items {
                <div accessibility_role={Role::Group} aria-label={a.name.clone()}
                     class="flex-row items-center gap-[6] pl-2 pr-1 h-7 rounded-[6]"
                     bg={colors.element_background} border={colors.border}>
                    {svg_icon(match a.kind {
                        Kind::Image => lucide::EYE,
                        Kind::File => lucide::FILE,
                        Kind::Text => lucide::FILE_CODE,
                    }, 14.0).color(colors.text_muted)}
                    <text class="text-xs" color={colors.text}>{a.name.clone()}</text>
                    <text class="text-xs" color={colors.text_muted}>{size_label(a.bytes.len())}</text>
                    <div id={format!("composer.attachment.remove.{}", a.id.0)}
                         accessibility_role={Role::Button}
                         aria-label={format!("Remove {}", a.name)}
                         class="w-[20] h-[20] items-center justify-center rounded-[4]"
                         hover_bg={colors.element_hover}
                         on:click={Action::RemoveAttachment(a.id)}>
                        {svg_icon(lucide::X, 12.0).color(colors.text_muted)}
                    </div>
                </div>
            }
        </div>
    }
    .into_any()
}
