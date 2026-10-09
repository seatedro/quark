//! The Chrome DevTools Protocol half of WebView2 evaluation, kept free of
//! COM so it builds and tests on every platform.
//!
//! `ExecuteScript` cannot await a Promise and folds exceptions into `null`,
//! so evaluation goes through `Runtime.callFunctionOn` with
//! `awaitPromise`. That call needs an execution context, and the only
//! acceptable one is the main frame's default (page world) context of the
//! committed document. [`MainContext`] follows `Runtime.executionContext*`
//! events to name that context by its process-unique id, so a numeric id
//! reused by a later document or another renderer process can never
//! receive the script.

use serde_json::{Value, json};

/// The CDP events [`MainContext::on_event`] reads. Subscribe to all of them
/// before `Runtime.enable`, so the replayed existing contexts arrive.
pub(crate) const CONTEXT_EVENTS: [&str; 3] = [
    "Runtime.executionContextCreated",
    "Runtime.executionContextDestroyed",
    "Runtime.executionContextsCleared",
];

/// The main frame's default execution context, as far as CDP has said.
#[derive(Debug, Default)]
pub(crate) struct MainContext {
    /// The main frame's CDP frame id, from `Page.getFrameTree`. Contexts are
    /// ignored until it is known.
    frame: Option<String>,
    current: Option<Context>,
    /// Set by a top-level navigation start: the old document's context no
    /// longer counts, even if CDP has not destroyed it yet.
    stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Context {
    id: i64,
    unique_id: Option<String>,
    origin: String,
}

/// Why no context can take a script now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unbound {
    /// No main-frame context of the committed document yet: it is still
    /// loading, or CDP has not reported it.
    NotReady,
    /// The main-frame context belongs to another origin.
    WrongOrigin,
    /// The runtime reports contexts without `uniqueId`, so a script cannot
    /// be pinned to one document.
    NoUniqueId,
}

impl MainContext {
    /// Records the main frame's id from a `Page.getFrameTree` response.
    /// Returns false when the response names no frame.
    pub(crate) fn set_frame_tree(&mut self, response: &str) -> bool {
        let tree: Value = serde_json::from_str(response).unwrap_or(Value::Null);
        match tree["frameTree"]["frame"]["id"].as_str() {
            Some(id) => {
                self.frame = Some(id.to_owned());
                true
            }
            None => false,
        }
    }

    /// A top-level navigation was accepted: revoke the current context.
    pub(crate) fn navigation_started(&mut self) {
        self.stale = true;
    }

    /// Applies one CDP event.
    pub(crate) fn on_event(&mut self, method: &str, params: &str) {
        let params: Value = serde_json::from_str(params).unwrap_or(Value::Null);
        match method {
            "Runtime.executionContextCreated" => {
                let context = &params["context"];
                let aux = &context["auxData"];
                let main_frame =
                    self.frame.is_some() && aux["frameId"].as_str() == self.frame.as_deref();
                let default = aux["isDefault"].as_bool() == Some(true);
                if let (true, true, Some(id), Some(origin)) = (
                    main_frame,
                    default,
                    context["id"].as_i64(),
                    context["origin"].as_str(),
                ) {
                    self.current = Some(Context {
                        id,
                        unique_id: context["uniqueId"].as_str().map(str::to_owned),
                        origin: origin.to_owned(),
                    });
                    self.stale = false;
                }
            }
            "Runtime.executionContextDestroyed" => {
                let destroyed_unique = params["executionContextUniqueId"].as_str();
                let destroyed_id = params["executionContextId"].as_i64();
                let gone = self
                    .current
                    .as_ref()
                    .is_some_and(|current| match destroyed_unique {
                        Some(unique) => current.unique_id.as_deref() == Some(unique),
                        None => Some(current.id) == destroyed_id,
                    });
                if gone {
                    self.current = None;
                }
            }
            "Runtime.executionContextsCleared" => self.current = None,
            _ => {}
        }
    }

    /// The `uniqueContextId` to evaluate in for a document of `origin`
    /// (serialized as a browser does, `https://host:port`).
    pub(crate) fn bind(&self, origin: &str) -> Result<&str, Unbound> {
        let current = match &self.current {
            Some(current) if !self.stale => current,
            _ => return Err(Unbound::NotReady),
        };
        if current.origin != origin {
            return Err(Unbound::WrongOrigin);
        }
        current.unique_id.as_deref().ok_or(Unbound::NoUniqueId)
    }
}

/// `Runtime.callFunctionOn` parameters running `function_declaration` with
/// `input` as its one string argument in the context `unique_context_id`.
/// The input travels as a CDP argument value, never as source.
pub(crate) fn call_function_on(
    function_declaration: &str,
    input: &str,
    unique_context_id: &str,
) -> String {
    json!({
        "functionDeclaration": function_declaration,
        "arguments": [{ "value": input }],
        "uniqueContextId": unique_context_id,
        "awaitPromise": true,
        "returnByValue": true,
        "silent": true,
        "userGesture": false,
    })
    .to_string()
}

/// What one `Runtime.callFunctionOn` produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CallOutcome {
    /// The wrapper's returned envelope string.
    Envelope(String),
    /// The call completed with a value that is not a string.
    NotAString,
    /// The wrapper itself threw. It catches the app body's exceptions, so
    /// this means page code broke a global the wrapper uses.
    WrapperFailed,
    /// The context went away before or during the call: the document
    /// changed.
    ContextGone,
    /// Any other protocol failure. The text is CDP's error message and may
    /// echo page data; keep it out of logs.
    Protocol(String),
}

/// Reads `CallDevToolsProtocolMethod`'s completion: whether the call
/// succeeded and the JSON it returned (a result object, or an error object
/// on failure).
pub(crate) fn call_outcome(succeeded: bool, json: &str) -> CallOutcome {
    let value: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    if !succeeded {
        let message = value["message"].as_str().unwrap_or_default();
        // Chromium's wording when a uniqueContextId names a destroyed
        // context or one from a replaced renderer.
        return if message.contains("context")
            && (message.contains("Cannot find") || message.contains("destroyed"))
        {
            CallOutcome::ContextGone
        } else {
            CallOutcome::Protocol(message.to_owned())
        };
    }
    if value.get("exceptionDetails").is_some() {
        return CallOutcome::WrapperFailed;
    }
    match (&value["result"]["type"], &value["result"]["value"]) {
        (Value::String(kind), Value::String(text)) if kind == "string" => {
            CallOutcome::Envelope(text.clone())
        }
        (Value::String(_), _) => CallOutcome::NotAString,
        _ => CallOutcome::Protocol("no result".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME_TREE: &str = r#"{"frameTree":{"frame":{"id":"MAIN","url":"https://a.test/"}}}"#;

    fn created(id: i64, unique: Option<&str>, origin: &str, frame: &str, default: bool) -> String {
        let mut context = json!({
            "id": id,
            "origin": origin,
            "name": "",
            "auxData": { "isDefault": default, "type": if default { "default" } else { "isolated" }, "frameId": frame },
        });
        if let Some(unique) = unique {
            context["uniqueId"] = unique.into();
        }
        json!({ "context": context }).to_string()
    }

    /// Feeds `(method, params)` events after the frame tree; returns what a
    /// script bound for `https://a.test` would get.
    fn bound(events: &[(&str, String)]) -> Result<String, Unbound> {
        let mut contexts = MainContext::default();
        contexts.set_frame_tree(FRAME_TREE);
        for (method, params) in events {
            if *method == "navigate" {
                contexts.navigation_started();
            } else {
                contexts.on_event(method, params);
            }
        }
        contexts.bind("https://a.test").map(str::to_owned)
    }

    /// A name, the events, and what binds afterwards.
    type Case = (
        &'static str,
        Vec<(&'static str, String)>,
        Result<&'static str, Unbound>,
    );

    const CREATED: &str = "Runtime.executionContextCreated";
    const DESTROYED: &str = "Runtime.executionContextDestroyed";

    #[test]
    fn only_the_live_main_frame_default_context_binds() {
        let main = || {
            (
                CREATED,
                created(1, Some("u1"), "https://a.test", "MAIN", true),
            )
        };
        let cases: Vec<Case> = vec![
            ("main default context", vec![main()], Ok("u1")),
            ("before any context", vec![], Err(Unbound::NotReady)),
            (
                "a cross-origin frame's context",
                vec![(
                    CREATED,
                    created(2, Some("u2"), "https://a.test", "CHILD", true),
                )],
                Err(Unbound::NotReady),
            ),
            (
                "an isolated world in the main frame",
                vec![(
                    CREATED,
                    created(2, Some("u2"), "https://a.test", "MAIN", false),
                )],
                Err(Unbound::NotReady),
            ),
            (
                "a frame's context after the main one",
                vec![
                    main(),
                    (
                        CREATED,
                        created(2, Some("u2"), "https://b.test", "CHILD", true),
                    ),
                ],
                Ok("u1"),
            ),
            (
                "another origin",
                vec![(
                    CREATED,
                    created(1, Some("u1"), "https://b.test", "MAIN", true),
                )],
                Err(Unbound::WrongOrigin),
            ),
            (
                "no uniqueId",
                vec![(CREATED, created(1, None, "https://a.test", "MAIN", true))],
                Err(Unbound::NoUniqueId),
            ),
            (
                "navigation started",
                vec![main(), ("navigate", String::new())],
                Err(Unbound::NotReady),
            ),
            (
                "the next document's context",
                vec![
                    main(),
                    ("navigate", String::new()),
                    (
                        CREATED,
                        created(1, Some("u9"), "https://a.test", "MAIN", true),
                    ),
                ],
                Ok("u9"),
            ),
            (
                "destroyed by unique id",
                vec![
                    main(),
                    (
                        DESTROYED,
                        json!({ "executionContextId": 1, "executionContextUniqueId": "u1" })
                            .to_string(),
                    ),
                ],
                Err(Unbound::NotReady),
            ),
            (
                "a reused numeric id destroyed in another process",
                vec![
                    main(),
                    (
                        DESTROYED,
                        json!({ "executionContextId": 1, "executionContextUniqueId": "other" })
                            .to_string(),
                    ),
                ],
                Ok("u1"),
            ),
            (
                "contexts cleared",
                vec![main(), ("Runtime.executionContextsCleared", "{}".into())],
                Err(Unbound::NotReady),
            ),
        ];
        for (name, events, expected) in cases {
            assert_eq!(
                bound(&events).as_deref().map_err(|e| *e),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn contexts_before_the_frame_tree_are_ignored() {
        let mut contexts = MainContext::default();
        contexts.on_event(
            CREATED,
            &created(1, Some("u1"), "https://a.test", "MAIN", true),
        );
        contexts.set_frame_tree(FRAME_TREE);
        assert_eq!(contexts.bind("https://a.test"), Err(Unbound::NotReady));
    }

    #[test]
    fn input_is_an_argument_value_not_source() {
        let input = r#"{"args":"\"});alert(1);//"}"#;
        let params: Value = serde_json::from_str(&call_function_on(
            "async function (quarkInput) {}",
            input,
            "u1",
        ))
        .unwrap();
        assert_eq!(
            params["functionDeclaration"],
            "async function (quarkInput) {}"
        );
        assert_eq!(params["arguments"], json!([{ "value": input }]));
        assert_eq!(params["uniqueContextId"], "u1");
        assert_eq!(
            (
                params["awaitPromise"].as_bool(),
                params["returnByValue"].as_bool()
            ),
            (Some(true), Some(true))
        );
    }

    #[test]
    fn call_results_map_to_outcomes() {
        for (succeeded, json, expected) in [
            (
                true,
                r#"{"result":{"type":"string","value":"{\"quark\":1}"}}"#,
                CallOutcome::Envelope(r#"{"quark":1}"#.into()),
            ),
            (
                true,
                r#"{"result":{"type":"string","value":""}}"#,
                CallOutcome::Envelope(String::new()),
            ),
            (
                true,
                r#"{"result":{"type":"object","value":null}}"#,
                CallOutcome::NotAString,
            ),
            (
                true,
                r#"{"result":{"type":"undefined"}}"#,
                CallOutcome::NotAString,
            ),
            (
                true,
                r#"{"result":{"type":"object","subtype":"error"},"exceptionDetails":{"exceptionId":1,"text":"Uncaught"}}"#,
                CallOutcome::WrapperFailed,
            ),
            (
                false,
                r#"{"code":-32000,"message":"Cannot find context with specified id"}"#,
                CallOutcome::ContextGone,
            ),
            (
                false,
                r#"{"code":-32000,"message":"Execution context was destroyed."}"#,
                CallOutcome::ContextGone,
            ),
            (
                false,
                r#"{"code":-32601,"message":"'Runtime.callFunctionOn' wasn't found"}"#,
                CallOutcome::Protocol("'Runtime.callFunctionOn' wasn't found".into()),
            ),
            (true, "not json", CallOutcome::Protocol("no result".into())),
        ] {
            assert_eq!(call_outcome(succeeded, json), expected, "{json}");
        }
    }
}
