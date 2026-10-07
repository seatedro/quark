//! Native open and save file dialogs through rfd, parented to the calling
//! window. The dialog runs without blocking the event loop; the result
//! arrives as [`AppEvent::FileDialogClosed`].
//!
//! Linux uses the XDG desktop portal (no GTK), so it needs a running portal
//! such as xdg-desktop-portal-gtk, -gnome, or -kde.
//!
//! [`AppEvent::FileDialogClosed`]: crate::AppEvent::FileDialogClosed

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use winit::window::Window;

use crate::runner::{AppEvent, EventSink};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileDialogKind {
    #[default]
    OpenFile,
    OpenFiles,
    OpenFolder,
    Save,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFilter {
    pub name: String,
    /// Without the dot: `["png", "jpg"]`.
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileDialog {
    pub kind: FileDialogKind,
    pub title: Option<String>,
    pub directory: Option<PathBuf>,
    /// Suggested file name for [`FileDialogKind::Save`].
    pub file_name: Option<String>,
    pub filters: Vec<FileFilter>,
}

/// Matches a dialog to its [`AppEvent::FileDialogClosed`].
///
/// [`AppEvent::FileDialogClosed`]: crate::AppEvent::FileDialogClosed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DialogId(pub(crate) u64);

type Pending = Pin<Box<dyn Future<Output = Vec<PathBuf>> + Send>>;

/// Builds the dialog on the calling (main) thread, which macOS requires, then
/// waits for it on a worker thread.
pub(crate) fn open(dialog: FileDialog, id: DialogId, parent: Option<&Window>, events: EventSink) {
    let mut native = rfd::AsyncFileDialog::new();
    if let Some(title) = dialog.title {
        native = native.set_title(title);
    }
    if let Some(directory) = dialog.directory {
        native = native.set_directory(directory);
    }
    if let Some(file_name) = dialog.file_name {
        native = native.set_file_name(file_name);
    }
    for filter in &dialog.filters {
        native = native.add_filter(filter.name.clone(), &filter.extensions);
    }
    if let Some(parent) = parent {
        native = native.set_parent(parent);
    }

    let pending: Pending = match dialog.kind {
        FileDialogKind::OpenFile => {
            let picked = native.pick_file();
            Box::pin(async move { picked.await.into_iter().map(path).collect() })
        }
        FileDialogKind::OpenFiles => {
            let picked = native.pick_files();
            Box::pin(async move { picked.await.into_iter().flatten().map(path).collect() })
        }
        FileDialogKind::OpenFolder => {
            let picked = native.pick_folder();
            Box::pin(async move { picked.await.into_iter().map(path).collect() })
        }
        FileDialogKind::Save => {
            let picked = native.save_file();
            Box::pin(async move { picked.await.into_iter().map(path).collect() })
        }
    };

    let spawned = std::thread::Builder::new()
        .name("quark-file-dialog".to_owned())
        .spawn(move || {
            let paths = pollster::block_on(pending);
            events.send(AppEvent::FileDialogClosed { id, paths });
        });
    if let Err(error) = spawned {
        tracing::warn!("could not start the file dialog thread: {error}");
    }
}

fn path(handle: rfd::FileHandle) -> PathBuf {
    handle.path().to_path_buf()
}
