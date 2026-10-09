// Local patch (quark): navigation observation and policy hooks for the
// existing `WryNavigationDelegate`. wry's own callbacks keep running; these
// hooks only add information wry does not forward (frame info, navigation
// identity, provisional and failure callbacks) and let the embedder cancel a
// navigation before wry's policy runs.

use objc2_foundation::{
  NSError, NSURLAuthenticationChallenge, NSURLCredential, NSURLSessionAuthChallengeDisposition,
};
use objc2_web_kit::{WKNavigation, WKNavigationAction, WKNavigationResponse, WKWebView};

/// The completion handler of `webView:didReceiveAuthenticationChallenge:completionHandler:`.
pub type AuthChallengeCompletion =
  block2::DynBlock<dyn Fn(NSURLSessionAuthChallengeDisposition, *mut NSURLCredential)>;

/// Embedder hooks on wry's navigation delegate. All run on the main thread,
/// inside the WebKit callback. Every method defaults to wry's behavior.
pub trait NavigationHooks {
  /// Runs before wry's navigation policy. `false` cancels the navigation;
  /// `true` continues into wry's download and navigation handler logic.
  fn decide_action(&self, _webview: &WKWebView, _action: &WKNavigationAction) -> bool {
    true
  }

  /// Runs before wry's response policy. `false` cancels the response;
  /// `true` continues into wry's MIME and download logic.
  fn decide_response(&self, _webview: &WKWebView, _response: &WKNavigationResponse) -> bool {
    true
  }

  /// `webView:didStartProvisionalNavigation:`.
  fn provisional_started(&self, _webview: &WKWebView, _navigation: Option<&WKNavigation>) {}

  /// `webView:didReceiveServerRedirectForProvisionalNavigation:`.
  fn server_redirect(&self, _webview: &WKWebView, _navigation: Option<&WKNavigation>) {}

  /// `webView:didFailProvisionalNavigation:withError:`.
  fn provisional_failed(
    &self,
    _webview: &WKWebView,
    _navigation: Option<&WKNavigation>,
    _error: &NSError,
  ) {
  }

  /// `webView:didCommitNavigation:`, after wry's own handling.
  fn committed(&self, _webview: &WKWebView, _navigation: Option<&WKNavigation>) {}

  /// `webView:didFinishNavigation:`, after wry's own handling.
  fn finished(&self, _webview: &WKWebView, _navigation: Option<&WKNavigation>) {}

  /// `webView:didFailNavigation:withError:`.
  fn failed(&self, _webview: &WKWebView, _navigation: Option<&WKNavigation>, _error: &NSError) {}

  /// `webView:didReceiveAuthenticationChallenge:completionHandler:`.
  /// Return `true` only after calling `completion` exactly once. `false`
  /// makes wry answer `PerformDefaultHandling`, WebKit's normal validation.
  fn authentication_challenge(
    &self,
    _webview: &WKWebView,
    _challenge: &NSURLAuthenticationChallenge,
    _completion: &AuthChallengeCompletion,
  ) -> bool {
    false
  }
}
