//! COM glue for the WebView2 backend. Unverified on a real Windows
//! machine: it builds and passes clippy for `x86_64-pc-windows-msvc`, and
//! its ordering logic is tested through [`super::state`].
//!
//! Everything runs on the winit UI thread, a COM single-threaded
//! apartment. The environment and controller are created asynchronously,
//! so their completions arrive through winit's own message pump and
//! nothing here pumps messages itself (wry's synchronous construction
//! would pump inside `Backend::open`, and it quietly falls back to a
//! controller without InPrivate on older runtimes).
//!
//! Each view gets an owned top-level window with the parent as its owner,
//! and the parent is disabled while it is open, which is the native modal
//! relationship. Ephemeral views use InPrivate in their own user data
//! folder, deleted once the browser process exits; persistent profiles use
//! one folder per app and purpose.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use raw_window_handle::RawWindowHandle;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, ContentLoadingEventHandler,
    CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, DevToolsProtocolEventReceivedEventHandler,
    DocumentTitleChangedEventHandler, DownloadStartingEventHandler,
    LaunchingExternalUriSchemeEventHandler, NavigationCompletedEventHandler,
    NavigationStartingEventHandler, NewWindowRequestedEventHandler,
    PermissionRequestedEventHandler, ProcessFailedEventHandler,
    ServerCertificateErrorDetectedEventHandler, SourceChangedEventHandler,
    WindowCloseRequestedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{
    CloseHandle, E_POINTER, HWND, LPARAM, LRESULT, POINT, RECT, RPC_E_CHANGED_MODE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    COLOR_WINDOW, GetMonitorInfoW, HBRUSH, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect,
    GetWindowRect, IDC_ARROW, LoadCursorW, MINMAXINFO, RegisterClassExW, SW_SHOWNORMAL,
    SWP_NOACTIVATE, SWP_NOZORDER, SetForegroundWindow, SetWindowPos, ShowWindow, WINDOW_EX_STYLE,
    WM_CLOSE, WM_DPICHANGED, WM_GETMINMAXINFO, WM_MOVE, WM_SETFOCUS, WM_SIZE, WNDCLASSEXW,
    WS_OVERLAPPEDWINDOW,
};
use windows::core::{BOOL, HSTRING, Interface, PCWSTR, PWSTR, w};

use super::cdp::CONTEXT_EVENTS;
use super::state::{CdpCall, NewWindow, ViewState, WebError, test_trusted};
use crate::backend::{Backend, ClearRequest, EvalDispatch, NativeClose, NativeSink, OpenRequest};
use crate::policy::BlockReason;
use crate::profile::ProfileError;
use crate::{
    Appearance, Capabilities, DataStore, EvaluationId, OpenError, ParentRelationship,
    PlatformError, ProfileId, ProfileMode, WebViewHandle, WebWindowOptions,
};

const HOST_CLASS: PCWSTR = w!("QuarkWebView2Host");

pub(super) fn backend() -> Option<Box<dyn Backend>> {
    let root = std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("quark-webview"));
    if let Some(root) = &root {
        sweep_ephemeral(&root.join("ephemeral"));
    }
    Some(Box::new(WebView2 {
        views: HashMap::new(),
        root,
    }))
}

struct WebView2 {
    views: HashMap<WebViewHandle, Rc<View>>,
    /// `%LOCALAPPDATA%\quark-webview`.
    root: Option<PathBuf>,
}

thread_local! {
    /// Host windows to their views, for the window procedure.
    static HOSTS: RefCell<HashMap<isize, Weak<View>>> = RefCell::new(HashMap::new());
}

/// A user data folder and what happens to it after the view.
#[derive(Debug, Clone)]
enum DataDir {
    /// Deleted once the browser process lets go of it.
    Ephemeral(PathBuf),
    Persistent(PathBuf),
}

impl DataDir {
    fn path(&self) -> &Path {
        match self {
            Self::Ephemeral(path) | Self::Persistent(path) => path,
        }
    }

    fn ephemeral(&self) -> bool {
        matches!(self, Self::Ephemeral(_))
    }
}

struct View {
    state: RefCell<ViewState>,
    host: HWND,
    parent: HWND,
    /// Whether the parent was enabled before the modal disabled it.
    parent_was_enabled: bool,
    data: DataDir,
    /// Minimum client size in logical points.
    min_size: (f32, f32),
    native: RefCell<Option<Native>>,
    closing: Cell<bool>,
}

/// The engine objects, once the controller exists.
struct Native {
    environment: ICoreWebView2Environment,
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
    /// Unregister each event handler; run before the controller closes.
    removers: Vec<Box<dyn FnOnce()>>,
}

fn platform(context: &'static str, error: impl std::fmt::Display) -> PlatformError {
    PlatformError::new(context, error.to_string())
}

fn open_platform(context: &'static str, error: impl std::fmt::Display) -> OpenError {
    OpenError::Platform(platform(context, error))
}

/// The installed Evergreen runtime's version, or `None` without one.
fn runtime_version() -> Option<String> {
    let mut version = PWSTR::null();
    // SAFETY: a null browser folder asks for the installed runtime; the
    // out string is freed by `take_pwstr`.
    unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version) }.ok()?;
    let version = take_pwstr(version);
    (!version.is_empty()).then_some(version)
}

impl Backend for WebView2 {
    fn open(&mut self, request: OpenRequest) -> Result<(), OpenError> {
        let OpenRequest {
            view: handle,
            url,
            options,
            parent,
            sink,
        } = request;
        // Views whose open failed later are done; nothing else drops them.
        self.views.retain(|_, view| !view.closing.get());
        let RawWindowHandle::Win32(parent) = parent.window else {
            return Err(OpenError::UnsupportedParenting);
        };
        let parent = HWND(parent.hwnd.get() as *mut c_void);
        if runtime_version().is_none() {
            return Err(OpenError::UnsupportedRuntime);
        }
        // SAFETY: plain COM initialization on the calling (UI) thread.
        let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if com == RPC_E_CHANGED_MODE {
            return Err(open_platform(
                "initialize COM",
                "WebView2 needs a single-threaded apartment on the UI thread",
            ));
        }
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| open_platform("find the data folder", "LOCALAPPDATA is not set"))?;
        let data = match &options.view.data_store {
            DataStore::Ephemeral => DataDir::Ephemeral(ephemeral_dir(root)),
            DataStore::Persistent(profile) => DataDir::Persistent(profile_dir(root, profile)),
        };
        std::fs::create_dir_all(data.path())
            .map_err(|e| open_platform("create the data folder", e))?;

        let host = create_host(parent, &options)?;
        // SAFETY: both windows belong to this thread. `EnableWindow`
        // returns whether the window was disabled before.
        let parent_was_enabled = !unsafe { EnableWindow(parent, false) }.as_bool();
        let view = Rc::new(View {
            state: RefCell::new(ViewState::new(sink)),
            host,
            parent,
            parent_was_enabled,
            data,
            min_size: options.min_size,
            native: RefCell::new(None),
            closing: Cell::new(false),
        });
        HOSTS.with(|hosts| {
            hosts
                .borrow_mut()
                .insert(host.0 as isize, Rc::downgrade(&view))
        });
        if let Err(error) = start_environment(&view, url, options) {
            view.release(false);
            return Err(error);
        }
        self.views.insert(handle, view);
        Ok(())
    }

    fn evaluate(&mut self, view: WebViewHandle, dispatch: EvalDispatch) {
        let Some(view) = self.views.get(&view) else {
            return;
        };
        let call = view.state.borrow_mut().evaluate(dispatch);
        if let Some(call) = call {
            send_call(view, call);
        }
    }

    fn cancel(&mut self, view: WebViewHandle, evaluation: EvaluationId) {
        // CDP cannot interrupt a running function; the answer is dropped.
        if let Some(view) = self.views.get(&view) {
            view.state.borrow_mut().cancel(evaluation);
        }
    }

    fn close(&mut self, view: WebViewHandle) {
        if let Some(view) = self.views.remove(&view) {
            view.release(true);
        }
    }

    fn focus(&mut self, view: WebViewHandle) {
        if let Some(view) = self.views.get(&view) {
            // SAFETY: the host window belongs to this thread.
            unsafe {
                let _ = SetForegroundWindow(view.host);
            }
            view.move_focus();
        }
    }

    fn clear_profile(&mut self, request: ClearRequest) {
        let Some(root) = &self.root else {
            request.sink.finished(Err(ProfileError::Platform(platform(
                "find the data folder",
                "LOCALAPPDATA is not set",
            ))));
            return;
        };
        let dir = profile_dir(root, &request.profile);
        let sink = request.sink;
        // The browser process may hold files for a moment after its last
        // view closed; retry off the UI thread instead of blocking it.
        std::thread::spawn(move || {
            let mut result = Ok(());
            for _ in 0..40 {
                result = match std::fs::remove_dir_all(&dir) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    other => other,
                };
                if result.is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            sink.finished(
                result.map_err(|e| ProfileError::Platform(platform("delete the profile", e))),
            );
        });
    }

    fn shutdown(&mut self) {
        for (_, view) in self.views.drain() {
            view.release(true);
        }
    }
}

impl View {
    fn sink(&self) -> NativeSink {
        self.state.borrow().sink().clone()
    }

    fn webview(&self) -> Option<ICoreWebView2> {
        self.native
            .borrow()
            .as_ref()
            .map(|native| native.webview.clone())
    }

    fn controller(&self) -> Option<ICoreWebView2Controller> {
        self.native
            .borrow()
            .as_ref()
            .map(|native| native.controller.clone())
    }

    fn move_focus(&self) {
        if let Some(controller) = self.controller() {
            // SAFETY: a live controller on its own thread.
            let _ = unsafe { controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC) };
        }
    }

    /// Fit the browser to the host's client area.
    fn fit(&self) {
        let Some(controller) = self.controller() else {
            return;
        };
        let mut rect = RECT::default();
        // SAFETY: the host window and controller belong to this thread.
        unsafe {
            if GetClientRect(self.host, &mut rect).is_ok() {
                let _ = controller.SetBounds(rect);
            }
        }
    }

    /// The open failed after `open` returned.
    fn fail_open(&self, error: OpenError) {
        if self.closing.get() {
            return;
        }
        self.sink().open_failed(error);
        self.release(false);
    }

    /// Tear everything down once: handlers, controller, host window, the
    /// parent lock, then the data folder. With `report`, `destroyed` follows
    /// once the browser process is gone.
    fn release(&self, report: bool) {
        if self.closing.replace(true) {
            return;
        }
        self.state.borrow_mut().close();
        let native = self.native.borrow_mut().take();
        let environment = native.as_ref().map(|native| native.environment.clone());
        if let Some(native) = native {
            for remove in native.removers {
                remove();
            }
            // SAFETY: closing a controller on its thread.
            let _ = unsafe { native.controller.Close() };
        }
        HOSTS.with(|hosts| hosts.borrow_mut().remove(&(self.host.0 as isize)));
        // SAFETY: windows of this thread. Re-enable the owner before
        // destroying its modal, or Windows activates some other app.
        unsafe {
            if self.parent_was_enabled {
                let _ = EnableWindow(self.parent, true);
            }
            let _ = DestroyWindow(self.host);
        }
        let data = self.data.clone();
        let sink = report.then(|| self.sink());
        let finish = move || {
            if data.ephemeral() {
                let _ = std::fs::remove_dir_all(data.path());
            }
            if let Some(sink) = sink {
                sink.destroyed();
            }
        };
        match environment.and_then(|e| e.cast::<ICoreWebView2Environment5>().ok()) {
            Some(environment) => on_browser_exit(&environment, finish),
            None => finish(),
        }
    }
}

/// Run `finish` once the environment's browser process exits, which is
/// when its user data folder is free.
fn on_browser_exit(environment: &ICoreWebView2Environment5, finish: impl FnOnce() + 'static) {
    let finish = Rc::new(RefCell::new(Some(finish)));
    let token = Rc::new(Cell::new(0i64));
    let (finish_, token_, environment_) =
        (Rc::clone(&finish), Rc::clone(&token), environment.clone());
    let handler = webview2_com::BrowserProcessExitedEventHandler::create(Box::new(move |_, _| {
        // SAFETY: unregistering this handler on its own thread, which also
        // drops its reference to the environment.
        let _ = unsafe { environment_.remove_BrowserProcessExited(token_.get()) };
        if let Some(finish) = finish_.borrow_mut().take() {
            finish();
        }
        Ok(())
    }));
    let mut raw = 0;
    // SAFETY: registering an event handler on the environment's thread.
    match unsafe { environment.add_BrowserProcessExited(&handler, &mut raw) } {
        Ok(()) => token.set(raw),
        // Without the event, finish now; a folder still in use is left for
        // the next start's sweep.
        Err(_) => {
            if let Some(finish) = finish.borrow_mut().take() {
                finish();
            }
        }
    }
}

fn ephemeral_dir(root: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    // SAFETY: no preconditions.
    let pid = unsafe { GetCurrentProcessId() };
    root.join("ephemeral")
        .join(format!("{pid}-{}", NEXT.fetch_add(1, Ordering::Relaxed)))
}

/// One user data folder per app and purpose. `ProfileId` admits only
/// ASCII letters, digits, `.`, `_`, and `-`, and no leading dot, so both
/// parts are safe path components.
fn profile_dir(root: &Path, profile: &ProfileId) -> PathBuf {
    root.join("profiles")
        .join(profile.app())
        .join(profile.purpose())
}

/// Delete ephemeral folders left by processes that are gone (a crash, or a
/// browser that held files past exit). Folders are named `<pid>-<n>`.
fn sweep_ephemeral(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // SAFETY: no preconditions.
    let current = unsafe { GetCurrentProcessId() };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name
            .to_str()
            .and_then(|name| name.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<u32>().ok());
        if let Some(pid) = pid
            && pid != current
            && !process_alive(pid)
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn process_alive(pid: u32) -> bool {
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: the handle is closed before returning.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0;
        let alive = GetExitCodeProcess(process, &mut code).is_ok() && code == STILL_ACTIVE;
        let _ = CloseHandle(process);
        alive
    }
}

/// Create the hidden host window, owned by `parent` and centered on it.
fn create_host(parent: HWND, options: &WebWindowOptions) -> Result<HWND, OpenError> {
    // SAFETY: Win32 window creation on the UI thread; every pointer passed
    // outlives its call.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| open_platform("find the module", e))?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(host_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut c_void),
            lpszClassName: HOST_CLASS,
            ..Default::default()
        };
        // Fails harmlessly once the class exists.
        RegisterClassExW(&class);

        let dpi = match GetDpiForWindow(parent) {
            0 => 96,
            dpi => dpi,
        };
        let scale = |points: f32| (points * dpi as f32 / 96.0).round() as i32;
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: scale(options.size.0),
            bottom: scale(options.size.1),
        };
        let _ = AdjustWindowRectExForDpi(
            &mut frame,
            WS_OVERLAPPEDWINDOW,
            false,
            WINDOW_EX_STYLE(0),
            dpi,
        );
        let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);

        let mut owner = RECT::default();
        let _ = GetWindowRect(parent, &mut owner);
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let work = if GetMonitorInfoW(
            MonitorFromWindow(parent, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        )
        .as_bool()
        {
            monitor.rcWork
        } else {
            owner
        };
        let (width, height) = (
            width.min(work.right - work.left),
            height.min(work.bottom - work.top),
        );
        let x = (owner.left + (owner.right - owner.left - width) / 2)
            .clamp(work.left, work.right - width);
        let y = (owner.top + (owner.bottom - owner.top - height) / 2)
            .clamp(work.top, work.bottom - height);

        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            HOST_CLASS,
            &HSTRING::from(options.title.as_str()),
            WS_OVERLAPPEDWINDOW,
            x,
            y,
            width,
            height,
            Some(parent),
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| open_platform("create the host window", e))
    }
}

fn view_for(hwnd: HWND) -> Option<Rc<View>> {
    HOSTS.with(|hosts| {
        hosts
            .borrow()
            .get(&(hwnd.0 as isize))
            .and_then(Weak::upgrade)
    })
}

unsafe extern "system" fn host_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let Some(view) = view_for(hwnd) else {
        // SAFETY: default handling with the arguments Windows passed in.
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    };
    match message {
        WM_CLOSE => {
            // The session decides; it answers with `close`.
            view.sink().closed(NativeClose::User);
            return LRESULT(0);
        }
        WM_SIZE => view.fit(),
        WM_MOVE => {
            if let Some(controller) = view.controller() {
                // SAFETY: a live controller on its own thread.
                let _ = unsafe { controller.NotifyParentWindowPositionChanged() };
            }
        }
        WM_SETFOCUS => view.move_focus(),
        WM_DPICHANGED => {
            // SAFETY: Windows passes the suggested frame in lparam.
            unsafe {
                let suggested = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            return LRESULT(0);
        }
        WM_GETMINMAXINFO => {
            // SAFETY: Windows passes a MINMAXINFO in lparam; `hwnd` is ours.
            unsafe {
                let dpi = match GetDpiForWindow(hwnd) {
                    0 => 96,
                    dpi => dpi,
                };
                let scale = |points: f32| (points * dpi as f32 / 96.0).round() as i32;
                let mut frame = RECT {
                    left: 0,
                    top: 0,
                    right: scale(view.min_size.0),
                    bottom: scale(view.min_size.1),
                };
                let _ = AdjustWindowRectExForDpi(
                    &mut frame,
                    WS_OVERLAPPEDWINDOW,
                    false,
                    WINDOW_EX_STYLE(0),
                    dpi,
                );
                let info = &mut *(lparam.0 as *mut MINMAXINFO);
                info.ptMinTrackSize = POINT {
                    x: frame.right - frame.left,
                    y: frame.bottom - frame.top,
                };
            }
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: default handling with the arguments Windows passed in.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// Step one: the environment, with a user data folder of the view's own.
fn start_environment(
    view: &Rc<View>,
    url: url::Url,
    options: WebWindowOptions,
) -> Result<(), OpenError> {
    let environment_options = CoreWebView2EnvironmentOptions::default();
    // SAFETY: setters on a plain options object.
    unsafe {
        // Never sign pages in with the Windows account.
        environment_options.set_allow_single_sign_on_using_os_primary_account(false);
        environment_options.set_are_browser_extensions_enabled(false);
        environment_options.set_exclusive_user_data_folder_access(true);
    }
    let weak = Rc::downgrade(view);
    let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |result, environment| {
            let Some(view) = weak.upgrade() else {
                return Ok(());
            };
            match result.and_then(|()| environment.ok_or_else(|| E_POINTER.into())) {
                Ok(environment) => create_controller(&view, environment, url, options),
                Err(error) => {
                    view.fail_open(open_platform("create the WebView2 environment", error))
                }
            }
            Ok(())
        },
    ));
    // SAFETY: the folder string and options outlive the call; completion
    // arrives later on this thread.
    unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            &HSTRING::from(view.data.path()),
            &ICoreWebView2EnvironmentOptions::from(environment_options),
            &handler,
        )
    }
    .map_err(|e| open_platform("create the WebView2 environment", e))
}

/// Step two: the controller, with InPrivate for ephemeral views. Needs
/// `ICoreWebView2Environment10`; without it the runtime would silently
/// give a non-private controller.
fn create_controller(
    view: &Rc<View>,
    environment: ICoreWebView2Environment,
    url: url::Url,
    options: WebWindowOptions,
) {
    if view.closing.get() {
        return;
    }
    let Ok(environment10) = environment.cast::<ICoreWebView2Environment10>() else {
        return view.fail_open(OpenError::UnsupportedRuntime);
    };
    let ephemeral = view.data.ephemeral();
    // SAFETY: COM calls on the environment's thread.
    let controller_options = unsafe {
        environment10
            .CreateCoreWebView2ControllerOptions()
            .and_then(|controller_options| {
                controller_options.SetIsInPrivateModeEnabled(ephemeral)?;
                controller_options.SetProfileName(&HSTRING::from("quark"))?;
                Ok(controller_options)
            })
    };
    let controller_options = match controller_options {
        Ok(controller_options) => controller_options,
        Err(error) => {
            return view.fail_open(open_platform("configure the WebView2 controller", error));
        }
    };
    let weak = Rc::downgrade(view);
    let environment_ = environment.clone();
    let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
        move |result, controller| {
            let Some(view) = weak.upgrade() else {
                if let Some(controller) = controller {
                    // SAFETY: closing an orphaned controller on its thread.
                    let _ = unsafe { controller.Close() };
                }
                return Ok(());
            };
            if view.closing.get() {
                if let Some(controller) = controller {
                    // SAFETY: as above; the view was closed while opening.
                    let _ = unsafe { controller.Close() };
                }
                return Ok(());
            }
            match result.and_then(|()| controller.ok_or_else(|| E_POINTER.into())) {
                Ok(controller) => {
                    if let Err(error) = configure(&view, environment_, controller, &url, &options) {
                        view.fail_open(error);
                    }
                }
                Err(error) => {
                    view.fail_open(open_platform("create the WebView2 controller", error))
                }
            }
            Ok(())
        },
    ));
    // SAFETY: the host window and options outlive the call.
    let created = unsafe {
        environment10.CreateCoreWebView2ControllerWithOptions(
            view.host,
            &controller_options,
            &handler,
        )
    };
    if let Err(error) = created {
        view.fail_open(open_platform("create the WebView2 controller", error));
    }
}

/// Step three: verify the profile, apply settings, install every handler,
/// start CDP, show the window, then load the first URL.
fn configure(
    view: &Rc<View>,
    environment: ICoreWebView2Environment,
    controller: ICoreWebView2Controller,
    url: &url::Url,
    options: &WebWindowOptions,
) -> Result<(), OpenError> {
    let context =
        |what: &'static str| move |error: windows::core::Error| open_platform(what, error);
    // SAFETY: COM calls on the controller's thread throughout.
    unsafe {
        let webview = controller
            .CoreWebView2()
            .map_err(context("get the WebView2 view"))?;
        // Profiles (13) and certificate decisions (14) are required: without
        // 14 a TLS failure shows Chromium's interstitial, which offers to
        // proceed anyway.
        let webview13 = webview
            .cast::<ICoreWebView2_13>()
            .map_err(|_| OpenError::UnsupportedRuntime)?;
        let webview14 = webview
            .cast::<ICoreWebView2_14>()
            .map_err(|_| OpenError::UnsupportedRuntime)?;
        let profile = webview13
            .Profile()
            .map_err(context("get the WebView2 profile"))?;
        let mut private = BOOL::default();
        profile
            .IsInPrivateModeEnabled(&mut private)
            .map_err(context("read the profile mode"))?;
        if private.as_bool() != view.data.ephemeral() {
            return Err(OpenError::UnsupportedRuntime);
        }

        let settings = webview
            .Settings()
            .map_err(context("get the WebView2 settings"))?;
        let view_options = &options.view;
        settings
            .SetAreDevToolsEnabled(view_options.devtools)
            .map_err(context("configure WebView2"))?;
        // No page-to-app channel of any kind.
        settings
            .SetIsWebMessageEnabled(false)
            .map_err(context("configure WebView2"))?;
        settings
            .SetAreHostObjectsAllowed(false)
            .map_err(context("configure WebView2"))?;
        // Uncontrolled alert/confirm/prompt dialogs are dismissed.
        settings
            .SetAreDefaultScriptDialogsEnabled(false)
            .map_err(context("configure WebView2"))?;
        settings
            .SetIsStatusBarEnabled(false)
            .map_err(context("configure WebView2"))?;
        if let Some(agent) = &view_options.user_agent {
            settings
                .cast::<ICoreWebView2Settings2>()
                .and_then(|settings| settings.SetUserAgent(&HSTRING::from(agent.as_str())))
                .map_err(context("set the user agent"))?;
        }
        if let Ok(settings) = settings.cast::<ICoreWebView2Settings4>() {
            settings
                .SetIsPasswordAutosaveEnabled(false)
                .map_err(context("configure WebView2"))?;
            settings
                .SetIsGeneralAutofillEnabled(false)
                .map_err(context("configure WebView2"))?;
        }

        let scheme = match view_options.appearance {
            Appearance::Light => COREWEBVIEW2_PREFERRED_COLOR_SCHEME_LIGHT,
            Appearance::Dark => COREWEBVIEW2_PREFERRED_COLOR_SCHEME_DARK,
            _ => COREWEBVIEW2_PREFERRED_COLOR_SCHEME_AUTO,
        };
        let appearance = match profile.SetPreferredColorScheme(scheme) {
            Ok(()) => view_options.appearance,
            Err(_) => Appearance::System,
        };

        let removers = install_handlers(view, &webview, &webview14)
            .map_err(context("install WebView2 handlers"))?;
        *view.native.borrow_mut() = Some(Native {
            environment,
            controller: controller.clone(),
            webview: webview.clone(),
            removers,
        });
        start_cdp(view, &webview).map_err(context("start the DevTools protocol"))?;

        let _ = ShowWindow(view.host, SW_SHOWNORMAL);
        view.fit();
        controller
            .SetIsVisible(true)
            .map_err(context("show the WebView2 view"))?;
        view.move_focus();
        let mode = if view.data.ephemeral() {
            ProfileMode::Ephemeral
        } else {
            ProfileMode::Persistent
        };
        view.sink().opened(Capabilities::new(
            ParentRelationship::Native,
            mode,
            appearance,
        ));
        webview
            .Navigate(&HSTRING::from(url.as_str()))
            .map_err(context("load the first page"))?;
    }
    Ok(())
}

/// A string out-parameter read through `take_pwstr`.
unsafe fn read_string(read: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>) -> String {
    let mut value = PWSTR::null();
    match read(&mut value) {
        Ok(()) => take_pwstr(value),
        Err(_) => String::new(),
    }
}

fn source(webview: &ICoreWebView2) -> String {
    // SAFETY: reading the view's URL on its thread.
    unsafe { read_string(|out| webview.Source(out)) }
}

/// Register every event handler. Each handler holds the view weakly and
/// releases its `RefCell` borrow before calling back into WebView2, which
/// may raise further events synchronously.
unsafe fn install_handlers(
    view: &Rc<View>,
    webview: &ICoreWebView2,
    webview14: &ICoreWebView2_14,
) -> windows::core::Result<Vec<Box<dyn FnOnce()>>> {
    let mut removers: Vec<Box<dyn FnOnce()>> = Vec::new();
    macro_rules! register {
        ($target:expr, $add:ident, $remove:ident, $handler:expr) => {{
            let target = $target.clone();
            let handler = $handler;
            let mut token = 0i64;
            // SAFETY: registering on the view's thread; the token removes it.
            unsafe { target.$add(&handler, &mut token)? };
            removers.push(Box::new(move || {
                // SAFETY: as above.
                let _ = unsafe { target.$remove(token) };
            }));
        }};
    }

    let weak = Rc::downgrade(view);
    register!(
        webview,
        add_NavigationStarting,
        remove_NavigationStarting,
        NavigationStartingEventHandler::create(Box::new(move |_, args| {
            let (Some(view), Some(args)) = (weak.upgrade(), args) else {
                return Ok(());
            };
            // SAFETY: event arguments read on the view's thread.
            unsafe {
                let uri = read_string(|out| args.Uri(out));
                let mut redirected = BOOL::default();
                args.IsRedirected(&mut redirected)?;
                let mut native = 0;
                args.NavigationId(&mut native)?;
                let allow =
                    view.state
                        .borrow_mut()
                        .navigation_starting(native, &uri, redirected.as_bool());
                if !allow {
                    args.SetCancel(true)?;
                }
            }
            Ok(())
        }))
    );

    let weak = Rc::downgrade(view);
    register!(
        webview,
        add_FrameNavigationStarting,
        remove_FrameNavigationStarting,
        NavigationStartingEventHandler::create(Box::new(move |_, args| {
            let (Some(view), Some(args)) = (weak.upgrade(), args) else {
                return Ok(());
            };
            // SAFETY: as above.
            unsafe {
                let uri = read_string(|out| args.Uri(out));
                if !view.state.borrow().frame_navigation_starting(&uri) {
                    args.SetCancel(true)?;
                }
            }
            Ok(())
        }))
    );

    let weak = Rc::downgrade(view);
    register!(
        webview,
        add_NewWindowRequested,
        remove_NewWindowRequested,
        NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
            let (Some(view), Some(args)) = (weak.upgrade(), args) else {
                return Ok(());
            };
            // SAFETY: as above.
            unsafe {
                let uri = read_string(|out| args.Uri(out));
                // Handled with no new window: WebView2 opens nothing.
                args.SetHandled(true)?;
                let decision = view.state.borrow().new_window(&uri);
                if let (NewWindow::Navigate(url), Some(webview)) = (decision, view.webview()) {
                    webview.Navigate(&HSTRING::from(url.as_str()))?;
                }
            }
            Ok(())
        }))
    );

    let weak = Rc::downgrade(view);
    register!(
        webview,
        add_ContentLoading,
        remove_ContentLoading,
        ContentLoadingEventHandler::create(Box::new(move |sender, args| {
            let (Some(view), Some(args), Some(webview)) = (weak.upgrade(), args, sender) else {
                return Ok(());
            };
            // SAFETY: as above.
            unsafe {
                let mut error_page = BOOL::default();
                args.IsErrorPage(&mut error_page)?;
                let mut native = 0;
                args.NavigationId(&mut native)?;
                let source = source(&webview);
                let keep =
                    view.state
                        .borrow_mut()
                        .content_loading(native, &source, error_page.as_bool());
                if !keep {
                    webview.Stop()?;
                } else if !error_page.as_bool() {
                    // The main frame may be a new one after a process swap.
                    request_frame_tree(&view, &webview);
                }
            }
            Ok(())
        }))
    );

    let weak = Rc::downgrade(view);
    register!(
        webview,
        add_SourceChanged,
        remove_SourceChanged,
        SourceChangedEventHandler::create(Box::new(move |sender, args| {
            let (Some(view), Some(args), Some(webview)) = (weak.upgrade(), args, sender) else {
                return Ok(());
            };
            // SAFETY: as above.
            unsafe {
                let mut new_document = BOOL::default();
                args.IsNewDocument(&mut new_document)?;
                let source = source(&webview);
                view.state
                    .borrow()
                    .source_changed(new_document.as_bool(), &source);
            }
            Ok(())
        }))
    );

    let weak = Rc::downgrade(view);
    register!(
        webview,
        add_NavigationCompleted,
        remove_NavigationCompleted,
        NavigationCompletedEventHandler::create(Box::new(move |_, args| {
            let (Some(view), Some(args)) = (weak.upgrade(), args) else {
                return Ok(());
            };
            // SAFETY: as above.
            unsafe {
                let mut success = BOOL::default();
                args.IsSuccess(&mut success)?;
                let mut native = 0;
                args.NavigationId(&mut native)?;
                let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                args.WebErrorStatus(&mut status)?;
                let http_status = args
                    .cast::<ICoreWebView2NavigationCompletedEventArgs2>()
                    .ok()
                    .and_then(|args| {
                        let mut code = 0;
                        args.HttpStatusCode(&mut code).ok()?;
                        u16::try_from(code).ok().filter(|code| *code > 0)
                    });
                view.state.borrow_mut().navigation_completed(
                    native,
                    success.as_bool(),
                    web_error(status),
                    http_status,
                );
            }
            Ok(())
        }))
    );

    let weak = Rc::downgrade(view);
    register!(
        webview,
        add_DocumentTitleChanged,
        remove_DocumentTitleChanged,
        DocumentTitleChangedEventHandler::create(Box::new(move |sender, _| {
            let (Some(view), Some(webview)) = (weak.upgrade(), sender) else {
                return Ok(());
            };
            // SAFETY: as above.
            let title = unsafe { read_string(|out| webview.DocumentTitle(out)) };
            view.sink().title_changed(&title);
            Ok(())
        }))
    );

    let weak = Rc::downgrade(view);
    register!(
        webview,
        add_ProcessFailed,
        remove_ProcessFailed,
        ProcessFailedEventHandler::create(Box::new(move |_, args| {
            let (Some(view), Some(args)) = (weak.upgrade(), args) else {
                return Ok(());
            };
            let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
            // SAFETY: as above.
            unsafe { args.ProcessFailedKind(&mut kind)? };
            // A frame's or a utility process's failure leaves the document;
            // the main renderer or the browser going away ends it.
            if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED
                || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
            {
                view.state.borrow_mut().process_gone();
            }
            Ok(())
        }))
    );

    register!(
        webview,
        add_PermissionRequested,
        remove_PermissionRequested,
        PermissionRequestedEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                // Camera, microphone, location, notifications, clipboard:
                // all denied in v1.
                // SAFETY: as above.
                unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)? };
            }
            Ok(())
        }))
    );

    let weak = Rc::downgrade(view);
    register!(
        webview,
        add_WindowCloseRequested,
        remove_WindowCloseRequested,
        WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
            // `window.close()` from the page ends the flow like the user
            // closing the window.
            if let Some(view) = weak.upgrade() {
                view.sink().closed(NativeClose::User);
            }
            Ok(())
        }))
    );

    register!(
        webview14,
        add_ServerCertificateErrorDetected,
        remove_ServerCertificateErrorDetected,
        ServerCertificateErrorDetectedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else {
                return Ok(());
            };
            // SAFETY: as above.
            unsafe {
                let uri = read_string(|out| args.RequestUri(out));
                let host = url::Url::parse(&uri)
                    .ok()
                    .and_then(|url| {
                        url.host_str()
                            .map(|host| host.trim_matches(['[', ']']).to_owned())
                    })
                    .unwrap_or_default();
                let pem = match args.ServerCertificate() {
                    Ok(certificate) => read_string(|out| certificate.ToPemEncoding(out)),
                    Err(_) => String::new(),
                };
                // Only the smoke tests' one certificate, for its loopback
                // hosts, is let through. Anything else fails as
                // `NavigationError::Tls`, with no interstitial offering to
                // continue.
                let action = if test_trusted(&host, &pem, crate::test_trust()) {
                    COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW
                } else {
                    COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL
                };
                args.SetAction(action)?;
            }
            Ok(())
        }))
    );

    if let Ok(webview4) = webview.cast::<ICoreWebView2_4>() {
        let weak = Rc::downgrade(view);
        register!(
            webview4,
            add_DownloadStarting,
            remove_DownloadStarting,
            DownloadStartingEventHandler::create(Box::new(move |_, args| {
                let (Some(view), Some(args)) = (weak.upgrade(), args) else {
                    return Ok(());
                };
                // SAFETY: as above.
                unsafe {
                    args.SetCancel(true)?;
                    let uri = read_string(|out| args.DownloadOperation()?.Uri(out));
                    view.sink()
                        .blocked(url::Url::parse(&uri).ok(), BlockReason::Download);
                }
                Ok(())
            }))
        );
    }

    if let Ok(webview18) = webview.cast::<ICoreWebView2_18>() {
        let weak = Rc::downgrade(view);
        register!(
            webview18,
            add_LaunchingExternalUriScheme,
            remove_LaunchingExternalUriScheme,
            LaunchingExternalUriSchemeEventHandler::create(Box::new(move |_, args| {
                let (Some(view), Some(args)) = (weak.upgrade(), args) else {
                    return Ok(());
                };
                // Never handed to the operating system, and no prompt.
                // SAFETY: as above.
                unsafe {
                    args.SetCancel(true)?;
                    let uri = read_string(|out| args.Uri(out));
                    view.sink()
                        .blocked(url::Url::parse(&uri).ok(), BlockReason::ExternalProtocol);
                }
                Ok(())
            }))
        );
    }

    for event in CONTEXT_EVENTS {
        // SAFETY: as above.
        let receiver = unsafe { webview.GetDevToolsProtocolEventReceiver(&HSTRING::from(event))? };
        let weak = Rc::downgrade(view);
        register!(
            receiver,
            add_DevToolsProtocolEventReceived,
            remove_DevToolsProtocolEventReceived,
            DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                let (Some(view), Some(args)) = (weak.upgrade(), args) else {
                    return Ok(());
                };
                // SAFETY: as above.
                let params = unsafe { read_string(|out| args.ParameterObjectAsJson(out)) };
                let calls = view.state.borrow_mut().cdp_event(event, &params);
                for call in calls {
                    send_call(&view, call);
                }
                Ok(())
            }))
        );
    }

    Ok(removers)
}

fn web_error(status: COREWEBVIEW2_WEB_ERROR_STATUS) -> WebError {
    match status {
        COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN => WebError::None,
        COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT
        | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED
        | COREWEBVIEW2_WEB_ERROR_STATUS_CLIENT_CERTIFICATE_CONTAINS_ERRORS
        | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED
        | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID => WebError::Tls,
        COREWEBVIEW2_WEB_ERROR_STATUS_SERVER_UNREACHABLE
        | COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT
        | COREWEBVIEW2_WEB_ERROR_STATUS_ERROR_HTTP_INVALID_SERVER_RESPONSE
        | COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED
        | COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_RESET
        | COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED
        | COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT
        | COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED => WebError::Transport,
        COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED => WebError::Cancelled,
        other => WebError::Other(other.0),
    }
}

/// Subscribe-then-enable: the context receivers are registered already,
/// so `Runtime.enable` replays the existing contexts to them.
fn start_cdp(view: &Rc<View>, webview: &ICoreWebView2) -> windows::core::Result<()> {
    request_frame_tree(view, webview);
    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(())));
    // SAFETY: a CDP call on the view's thread; the strings outlive it.
    unsafe { webview.CallDevToolsProtocolMethod(w!("Runtime.enable"), w!("{}"), &handler) }
}

fn request_frame_tree(view: &Rc<View>, webview: &ICoreWebView2) {
    let weak = Rc::downgrade(view);
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json| {
            if let (Some(view), Ok(())) = (weak.upgrade(), result) {
                let calls = view.state.borrow_mut().frame_tree(&json);
                for call in calls {
                    send_call(&view, call);
                }
            }
            Ok(())
        }));
    // SAFETY: as above.
    let _ =
        unsafe { webview.CallDevToolsProtocolMethod(w!("Page.getFrameTree"), w!("{}"), &handler) };
}

fn send_call(view: &Rc<View>, call: CdpCall) {
    let CdpCall { evaluation, params } = call;
    let Some(webview) = view.webview() else {
        view.state
            .borrow_mut()
            .call_finished(evaluation, false, "{}");
        return;
    };
    let weak = Rc::downgrade(view);
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json| {
            if let Some(view) = weak.upgrade() {
                view.state
                    .borrow_mut()
                    .call_finished(evaluation, result.is_ok(), &json);
            }
            Ok(())
        }));
    // SAFETY: as above.
    let sent = unsafe {
        webview.CallDevToolsProtocolMethod(
            w!("Runtime.callFunctionOn"),
            &HSTRING::from(params),
            &handler,
        )
    };
    if sent.is_err() {
        view.state
            .borrow_mut()
            .call_finished(evaluation, false, "{}");
    }
}
