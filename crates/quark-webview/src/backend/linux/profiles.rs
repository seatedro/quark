//! Data stores. An ephemeral view gets a WebKit context of its own, so its
//! cookies, storage, caches, and service workers live in memory and die with
//! it. A persistent profile keeps one context per process over its own
//! directory, `$XDG_DATA_HOME/<app>/webview/<purpose>`, so two contexts never
//! share files.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;
use gtk::glib::thread_guard::ThreadGuard;
use webkit2gtk::{
    WebContextExt, WebsiteDataManagerExt, WebsiteDataManagerExtManual, WebsiteDataTypes,
};

use crate::PlatformError;
use crate::backend::ClearRequest;
use crate::profile::{ProfileError, ProfileId};

#[derive(Default)]
pub(super) struct Profiles {
    persistent: HashMap<ProfileId, Profile>,
}

struct Profile {
    wry: wry::WebContext,
    /// WebKit's context under `wry`, known once a view used it (wry does
    /// not expose it otherwise).
    native: Option<webkit2gtk::WebContext>,
}

impl Profiles {
    /// The context for `profile`, creating its directory on first use.
    pub(super) fn context(
        &mut self,
        profile: &ProfileId,
    ) -> Result<&mut wry::WebContext, ProfileError> {
        if !self.persistent.contains_key(profile) {
            let directory = directory(profile)?;
            create_private(&directory)
                .map_err(|error| platform("create the profile directory", error))?;
            let wry = wry::WebContext::new(Some(directory));
            self.persistent
                .insert(profile.clone(), Profile { wry, native: None });
        }
        Ok(&mut self
            .persistent
            .get_mut(profile)
            .expect("inserted above")
            .wry)
    }

    /// Record the WebKit context a view of `profile` was built on.
    pub(super) fn built(&mut self, profile: &ProfileId, native: &webkit2gtk::WebContext) {
        if let Some(entry) = self.persistent.get_mut(profile) {
            entry.native = Some(native.clone());
        }
    }

    /// Delete everything `request.profile` stored. No view uses it.
    ///
    /// With a context alive in this process, WebKit's network process may
    /// hold the files open, so WebKit removes the data and the answer waits
    /// until a fetch finds nothing left. Otherwise no process here touched
    /// the directory, and removing it is enough.
    pub(super) fn clear(&mut self, request: ClearRequest, owed: &Rc<Cell<usize>>) {
        let ClearRequest { profile, sink } = request;
        let Some(manager) = self
            .persistent
            .get(&profile)
            .and_then(|entry| entry.native.as_ref())
            .and_then(|context| context.website_data_manager())
        else {
            let result = directory(&profile).and_then(|directory| {
                match std::fs::remove_dir_all(&directory) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        Err(platform("remove the profile directory", error))
                    }
                    _ => Ok(()),
                }
            });
            sink.finished(result);
            return;
        };
        owed.set(owed.get() + 1);
        let owed = Rc::clone(owed);
        let verify = manager.clone();
        // WebKit's binding wants a Send callback, though it runs on this
        // thread; the guard carries the GObject across that bound.
        let then = ThreadGuard::new(move |cleared: Result<(), glib::Error>| {
            if let Err(error) = cleared {
                owed.set(owed.get() - 1);
                sink.finished(Err(platform("clear the profile", error)));
                return;
            }
            verify.fetch(
                WebsiteDataTypes::ALL,
                None::<&gio::Cancellable>,
                move |records| {
                    owed.set(owed.get() - 1);
                    sink.finished(match records {
                        Ok(records) if records.is_empty() => Ok(()),
                        Ok(_) => Err(ProfileError::Platform(PlatformError::new(
                            "clear the profile",
                            "website data remained after clearing",
                        ))),
                        Err(error) => Err(platform("verify the profile is empty", error)),
                    });
                },
            );
        });
        manager.clear(
            WebsiteDataTypes::ALL,
            glib::TimeSpan(0),
            None::<&gio::Cancellable>,
            move |cleared| {
                (then.into_inner())(cleared);
            },
        );
    }
}

/// Where `profile` keeps its data. The id's parts are validated to be plain
/// names, so they cannot climb out of the data directory.
fn directory(profile: &ProfileId) -> Result<PathBuf, ProfileError> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".local/share"))
        })
        .ok_or_else(|| {
            ProfileError::Platform(PlatformError::new(
                "find the data directory",
                "no XDG_DATA_HOME or HOME",
            ))
        })?;
    Ok(data_home
        .join(profile.app())
        .join("webview")
        .join(profile.purpose()))
}

/// Create `directory` and its `webview` parent readable by the user only.
fn create_private(directory: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    if let Some(parent) = directory.parent() {
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn platform(context: &'static str, error: impl std::fmt::Display) -> ProfileError {
    ProfileError::Platform(PlatformError::new(context, error.to_string()))
}
