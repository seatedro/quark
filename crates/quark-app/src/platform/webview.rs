//! Modal webviews (feature `webview`): a native browser window over a
//! parent window, for flows such as signing in to a website.
//!
//! - Open with `cx.window.webviews().open(url, options)`; the context's
//!   window becomes the parent and its input is blocked until the view
//!   closes. One modal per parent.
//! - Events arrive as [`AppEvent::WebView`](crate::AppEvent::WebView),
//!   with the context bound to the parent while it is open.
//! - Navigation is limited to exact `https` origins; scripts run only in
//!   allowed origins' committed documents, checked by an [`OriginGuard`].
//! - Engines: WebKitGTK 2.40+ (API 4.1) on Linux, WKWebView on macOS,
//!   WebView2 on Windows. Without one, opening fails with
//!   [`OpenError::Unsupported`].
//!
//! ```no_run
//! use quark_app::platform::webview::{
//!     AsyncScript, Origin, OriginGuard, Url, WebViewEvent, WebViewOptions, WebWindowOptions,
//! };
//! use quark_app::{AppEvent, EventContext};
//!
//! fn sign_in(cx: &mut EventContext) {
//!     let app = Origin::parse("https://app.example").unwrap();
//!     let options = WebWindowOptions::new(
//!         WebViewOptions::new([app.clone()]).evaluation_origins([app]),
//!     );
//!     let url = Url::parse("https://app.example/login").unwrap();
//!     let _view = cx.webviews().open(url, options);
//! }
//!
//! fn app_event(event: AppEvent, cx: &mut EventContext) {
//!     if let AppEvent::WebView(WebViewEvent::PageLoadFinished { view, document, .. }) = event {
//!         let guard = OriginGuard::new(Origin::parse("https://app.example").unwrap(), document);
//!         let script = AsyncScript::new("return await window.getToken();");
//!         let _ = cx.webviews().evaluate_script_event(view, script, guard);
//!     }
//! }
//! ```

pub use crate::runner::WebViews;
pub use quark_webview::{
    Appearance, AsyncScript, BlockReason, Capabilities, DataStore, DocumentId, ErrorDetail,
    EvalError, Evaluation, EvaluationId, EvaluationLimits, FailureStage, InvalidResult,
    NavigationError, NavigationId, OpenError, Origin, OriginError, OriginGuard, PageUrl,
    ParentRelationship, PlatformError, PopupPolicy, ProfileClear, ProfileError, ProfileId,
    ProfileMode, ScriptValue, Url, WebCloseReason, WebViewEvent, WebViewHandle, WebViewOptions,
    WebWindowOptions,
};
