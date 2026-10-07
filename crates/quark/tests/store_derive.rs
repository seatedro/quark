//! Integration tests for `#[derive(Store)]`.

use quark::Store;
use quark::reactive::SignalStore;

#[derive(Debug, Clone, Default, PartialEq, Store)]
#[store(default)]
pub struct Pane {
    pub scroll_px: f32,
    pub hovered: Option<usize>,
    pub filter: String,
}

#[derive(Debug, Clone, Default, PartialEq, Store)]
pub struct PaneDebug {
    pub last_primitive_count: usize,
    pub last_frame_us: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Store)]
pub struct Workspace {
    pub ready: bool,
    #[store(flatten)]
    pub pane: Pane,
    #[store(flatten)]
    pub debug: PaneDebug,
    #[store(skip)]
    pub callbacks: Vec<String>, // stays out of the store
}

#[test]
fn flatten_nests_stores() {
    let store = SignalStore::default();
    let ws = WorkspaceStore::new(
        &store,
        Workspace {
            ready: true,
            pane: Pane {
                scroll_px: 7.0,
                hovered: None,
                filter: "bar".into(),
            },
            debug: PaneDebug {
                last_primitive_count: 1000,
                last_frame_us: 16_666,
            },
            callbacks: vec!["ignored".into()],
        },
    );

    assert!(store.read(ws.ready));
    assert_eq!(store.read(ws.pane.scroll_px), 7.0);
    assert_eq!(store.read(ws.pane.filter), "bar");
    assert_eq!(store.read(ws.debug.last_frame_us), 16_666);
}

#[test]
fn snapshot_reflects_writes() {
    let store = SignalStore::default();
    let initial = Pane {
        scroll_px: 12.5,
        hovered: Some(9),
        filter: "hello".into(),
    };
    let pane = PaneStore::new(&store, initial.clone());
    assert_eq!(pane.snapshot(&store), initial);

    store.write(pane.scroll_px, 99.0);
    store.write(pane.filter, "world".into());
    assert_eq!(
        pane.snapshot(&store),
        Pane {
            scroll_px: 99.0,
            hovered: Some(9),
            filter: "world".into(),
        }
    );
}

#[test]
fn snapshot_recurses_through_flatten() {
    let store = SignalStore::default();
    // Workspace has #[store(skip)] for callbacks, so snapshot is NOT generated.
    #[derive(Debug, Clone, Default, PartialEq, quark::Store)]
    struct Outer {
        pub flag: bool,
        #[store(flatten)]
        pub inner: Pane,
    }

    let initial = Outer {
        flag: true,
        inner: Pane {
            scroll_px: 1.5,
            hovered: None,
            filter: "x".into(),
        },
    };
    let outer = OuterStore::new(&store, initial.clone());
    assert_eq!(outer.snapshot(&store), initial);
}

#[test]
fn store_default_constructs_from_default() {
    let store = SignalStore::default();
    let pane = PaneStore::new_default(&store);
    assert_eq!(pane.snapshot(&store), Pane::default());
}

/// Regression: the derive used to emit `new_default` with a `where Self:
/// Default` bound, which fails to compile for structs without `Default`.
#[test]
fn non_default_struct_derives_store() {
    #[derive(Debug, Clone, PartialEq)]
    struct Handle(u32);

    #[derive(Debug, Clone, PartialEq, Store)]
    struct Session {
        handle: Handle,
        title: String,
    }

    let store = SignalStore::default();
    let initial = Session {
        handle: Handle(7),
        title: "main".into(),
    };
    let session = SessionStore::new(&store, initial.clone());
    assert_eq!(session.snapshot(&store), initial);
}
