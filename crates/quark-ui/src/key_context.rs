//! Key contexts and context predicates, GPUI style.
//!
//! An element names the context it gives its subtree with
//! `div().key_context("editor mode=insert")`: whitespace-separated
//! identifiers (`editor`) and `key=value` pairs (`mode=insert`). The
//! contexts on the path from the root to the focused element form the
//! *context path*, outermost first.
//!
//! A binding in [`KeyBindings`] may carry a [`KeyPredicate`] over that
//! path:
//!
//! | Predicate | Holds when the context entry... |
//! |---|---|
//! | `editor` | has the identifier `editor` |
//! | `mode == insert`, `mode != insert` | has (or lacks) `mode=insert` |
//! | `!p`, `p && q`, `p \|\| q`, `(p)` | as in Rust |
//! | `pane > editor` | has `editor`, inside an entry where `pane` holds |
//!
//! Binding operators from loosest: `>`, `||`, `&&`, then `==`/`!=` and `!`.
//!
//! A predicate is tested against each prefix of the path, so `editor`
//! matches wherever an `editor` context is on the path. A binding matches
//! at the deepest prefix it holds for, and the deepest match wins: a
//! binding for the focused editor beats one for the pane around it, and
//! both beat bindings without a predicate. Among matches at the same depth,
//! the binding added last wins, so user overrides are appended after the
//! defaults.

use std::fmt;
use std::str::FromStr;

use quark::{FocusId, SemanticFrame};

use crate::Action;
use crate::element::Binding;

/// One element's parsed key context: identifiers and `key=value` pairs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextEntry {
    ids: Vec<String>,
    pairs: Vec<(String, String)>,
}

impl ContextEntry {
    /// Parse `"editor mode=insert"`. Anything is accepted: each
    /// whitespace-separated word is an identifier or, with an `=`, a pair.
    pub fn parse(text: &str) -> Self {
        let mut entry = Self::default();
        for word in text.split_whitespace() {
            match word.split_once('=') {
                Some((key, value)) => entry.pairs.push((key.to_owned(), value.to_owned())),
                None => entry.ids.push(word.to_owned()),
            }
        }
        entry
    }

    pub fn has(&self, id: &str) -> bool {
        self.ids.iter().any(|candidate| candidate == id)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.as_str())
    }
}

/// A condition on the context path; see the [module docs](self).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyPredicate {
    Id(String),
    Eq(String, String),
    NotEq(String, String),
    Not(Box<KeyPredicate>),
    And(Box<KeyPredicate>, Box<KeyPredicate>),
    Or(Box<KeyPredicate>, Box<KeyPredicate>),
    /// The child holds at the path's last entry and the parent at an entry
    /// before it.
    Child(Box<KeyPredicate>, Box<KeyPredicate>),
}

impl KeyPredicate {
    /// Whether the predicate holds at the last entry of `path` (outermost
    /// first). An empty path satisfies nothing but negations.
    pub fn eval(&self, path: &[ContextEntry]) -> bool {
        let last = path.last();
        match self {
            Self::Id(id) => last.is_some_and(|entry| entry.has(id)),
            Self::Eq(key, value) => last.is_some_and(|entry| entry.get(key) == Some(value)),
            Self::NotEq(key, value) => !last.is_some_and(|entry| entry.get(key) == Some(value)),
            Self::Not(inner) => !inner.eval(path),
            Self::And(left, right) => left.eval(path) && right.eval(path),
            Self::Or(left, right) => left.eval(path) || right.eval(path),
            Self::Child(parent, child) => {
                child.eval(path) && (1..path.len()).any(|len| parent.eval(&path[..len]))
            }
        }
    }

    /// The longest prefix of `path` the predicate holds for, as its length
    /// (1 for the outermost entry); `None` when it holds for none.
    pub fn depth_of(&self, path: &[ContextEntry]) -> Option<usize> {
        (1..=path.len()).rev().find(|&len| self.eval(&path[..len]))
    }

    /// Whether some context path satisfies both predicates at once, so a
    /// key bound under both would be ambiguous. Decided by trying every
    /// path over the identifiers and values the predicates mention; when
    /// that is too many to try, assumes they overlap.
    pub fn overlaps(&self, other: &KeyPredicate) -> bool {
        let mut atoms = Atoms::default();
        self.collect(&mut atoms);
        other.collect(&mut atoms);
        let depth = 1 + self.nesting().max(other.nesting());
        let per_entry = atoms.entries_count();
        let Some(paths) = per_entry
            .checked_pow(depth as u32)
            .filter(|n| *n <= 1 << 16)
        else {
            return true;
        };
        let mut path = vec![ContextEntry::default(); depth];
        (0..paths).any(|mut code| {
            for entry in &mut path {
                *entry = atoms.entry(code % per_entry);
                code /= per_entry;
            }
            (1..=depth).any(|len| self.eval(&path[..len]) && other.eval(&path[..len]))
        })
    }

    fn collect(&self, atoms: &mut Atoms) {
        match self {
            Self::Id(id) => atoms.id(id),
            Self::Eq(key, value) | Self::NotEq(key, value) => atoms.pair(key, value),
            Self::Not(inner) => inner.collect(atoms),
            Self::And(l, r) | Self::Or(l, r) | Self::Child(l, r) => {
                l.collect(atoms);
                r.collect(atoms);
            }
        }
    }

    /// How many `>` deep the predicate reaches.
    fn nesting(&self) -> usize {
        match self {
            Self::Id(_) | Self::Eq(..) | Self::NotEq(..) => 0,
            Self::Not(inner) => inner.nesting(),
            Self::And(l, r) | Self::Or(l, r) => l.nesting().max(r.nesting()),
            Self::Child(l, r) => 1 + l.nesting().max(r.nesting()),
        }
    }
}

/// The identifiers and pair values predicates mention, which are all a
/// context entry can differ in as far as they can tell.
#[derive(Default)]
struct Atoms {
    ids: Vec<String>,
    /// Each key with its mentioned values; an entry may also have none of
    /// them.
    keys: Vec<(String, Vec<String>)>,
}

impl Atoms {
    fn id(&mut self, id: &str) {
        if !self.ids.iter().any(|known| known == id) {
            self.ids.push(id.to_owned());
        }
    }

    fn pair(&mut self, key: &str, value: &str) {
        let index = match self.keys.iter().position(|(known, _)| known == key) {
            Some(index) => index,
            None => {
                self.keys.push((key.to_owned(), Vec::new()));
                self.keys.len() - 1
            }
        };
        let values = &mut self.keys[index].1;
        if !values.iter().any(|known| known == value) {
            values.push(value.to_owned());
        }
    }

    /// Distinct entries: each identifier present or not, each key at one
    /// of its values or absent.
    fn entries_count(&self) -> usize {
        let ids = 1usize
            .checked_shl(self.ids.len() as u32)
            .unwrap_or(usize::MAX);
        self.keys
            .iter()
            .fold(ids, |n, (_, values)| n.saturating_mul(values.len() + 1))
    }

    fn entry(&self, mut code: usize) -> ContextEntry {
        let mut entry = ContextEntry::default();
        for id in &self.ids {
            if code & 1 == 1 {
                entry.ids.push(id.clone());
            }
            code >>= 1;
        }
        for (key, values) in &self.keys {
            let choice = code % (values.len() + 1);
            code /= values.len() + 1;
            if let Some(value) = values.get(choice) {
                entry.pairs.push((key.clone(), value.clone()));
            }
        }
        entry
    }
}

/// A predicate that does not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateError {
    pub source: String,
    pub message: &'static str,
}

impl fmt::Display for PredicateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid key predicate {:?}: {}",
            self.source, self.message
        )
    }
}

impl std::error::Error for PredicateError {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token<'a> {
    Ident(&'a str),
    Not,
    And,
    Or,
    Eq,
    NotEq,
    Child,
    Open,
    Close,
}

fn tokenize(text: &str) -> Result<Vec<Token<'_>>, &'static str> {
    let mut tokens = Vec::new();
    let mut rest = text;
    loop {
        rest = rest.trim_start();
        let Some(c) = rest.chars().next() else {
            return Ok(tokens);
        };
        let (token, len) = match c {
            '&' if rest.starts_with("&&") => (Token::And, 2),
            '|' if rest.starts_with("||") => (Token::Or, 2),
            '=' if rest.starts_with("==") => (Token::Eq, 2),
            '!' if rest.starts_with("!=") => (Token::NotEq, 2),
            '!' => (Token::Not, 1),
            '>' => (Token::Child, 1),
            '(' => (Token::Open, 1),
            ')' => (Token::Close, 1),
            c if is_ident_char(c) => {
                let len = rest.find(|c| !is_ident_char(c)).unwrap_or(rest.len());
                (Token::Ident(&rest[..len]), len)
            }
            _ => return Err("unexpected character"),
        };
        tokens.push(token);
        rest = &rest[len..];
    }
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// Precedence climbing over the tokens; looser operators first.
struct Parser<'a> {
    tokens: Vec<Token<'a>>,
    at: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token<'a>> {
        self.tokens.get(self.at)
    }

    fn eat(&mut self, token: &Token) -> bool {
        let found = self.peek() == Some(token);
        self.at += usize::from(found);
        found
    }

    fn child(&mut self) -> Result<KeyPredicate, &'static str> {
        let mut left = self.or()?;
        while self.eat(&Token::Child) {
            left = KeyPredicate::Child(Box::new(left), Box::new(self.or()?));
        }
        Ok(left)
    }

    fn or(&mut self) -> Result<KeyPredicate, &'static str> {
        let mut left = self.and()?;
        while self.eat(&Token::Or) {
            left = KeyPredicate::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<KeyPredicate, &'static str> {
        let mut left = self.unary()?;
        while self.eat(&Token::And) {
            left = KeyPredicate::And(Box::new(left), Box::new(self.unary()?));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<KeyPredicate, &'static str> {
        if self.eat(&Token::Not) {
            return Ok(KeyPredicate::Not(Box::new(self.unary()?)));
        }
        if self.eat(&Token::Open) {
            let inner = self.child()?;
            return if self.eat(&Token::Close) {
                Ok(inner)
            } else {
                Err("missing )")
            };
        }
        let Some(Token::Ident(name)) = self.peek().cloned() else {
            return Err("expected a context name");
        };
        self.at += 1;
        let equal = self.eat(&Token::Eq);
        if !equal && !self.eat(&Token::NotEq) {
            return Ok(KeyPredicate::Id(name.to_owned()));
        }
        let Some(Token::Ident(value)) = self.peek().cloned() else {
            return Err("expected a value after == or !=");
        };
        self.at += 1;
        let (key, value) = (name.to_owned(), value.to_owned());
        Ok(if equal {
            KeyPredicate::Eq(key, value)
        } else {
            KeyPredicate::NotEq(key, value)
        })
    }
}

impl FromStr for KeyPredicate {
    type Err = PredicateError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let error = |message| PredicateError {
            source: text.to_owned(),
            message,
        };
        let tokens = tokenize(text).map_err(error)?;
        let mut parser = Parser { tokens, at: 0 };
        let predicate = parser.child().map_err(error)?;
        if parser.at != parser.tokens.len() {
            return Err(error("unexpected trailing input"));
        }
        Ok(predicate)
    }
}

impl fmt::Display for KeyPredicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => f.write_str(id),
            Self::Eq(key, value) => write!(f, "{key} == {value}"),
            Self::NotEq(key, value) => write!(f, "{key} != {value}"),
            Self::Not(inner) => write!(f, "!({inner})"),
            Self::And(l, r) => write!(f, "({l} && {r})"),
            Self::Or(l, r) => write!(f, "({l} || {r})"),
            Self::Child(l, r) => write!(f, "({l} > {r})"),
        }
    }
}

/// The context path to `focus`: each semantic node on the way from the
/// root that has a key context, with its parsed entry, outermost first.
/// Empty with nothing focused.
pub fn context_path(
    semantic: &SemanticFrame,
    focus: Option<FocusId>,
) -> Vec<(usize, ContextEntry)> {
    let Some(start) = focus.and_then(|focus| semantic.node_for_focus(focus)) else {
        return Vec::new();
    };
    let nodes = semantic.nodes();
    let mut path: Vec<(usize, ContextEntry)> = semantic
        .ancestors_inclusive(start)
        .filter_map(|node| {
            let context = nodes[node].key_context.as_ref()?;
            Some((node, ContextEntry::parse(context.as_str())))
        })
        .collect();
    path.reverse();
    path
}

/// One key bound to an action, optionally under a predicate.
#[derive(Debug, Clone)]
pub struct KeyBinding {
    pub keys: Binding,
    pub predicate: Option<KeyPredicate>,
    pub action: Action,
}

/// Which binding a key press resolved to.
#[derive(Debug, Clone, Copy)]
pub struct KeyMatch<'a> {
    pub binding: &'a KeyBinding,
    /// How many path entries the match covers: 0 for a binding without a
    /// predicate, else the matched prefix's length.
    pub depth: usize,
}

/// A table of key bindings resolved against the context path. See the
/// [module docs](self) for precedence.
#[derive(Debug, Clone, Default)]
pub struct KeyBindings {
    bindings: Vec<KeyBinding>,
}

/// A binding or predicate string that does not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyBindingError {
    Keys(String),
    Predicate(PredicateError),
}

impl fmt::Display for KeyBindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keys(keys) => write!(f, "invalid key binding {keys:?}"),
            Self::Predicate(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for KeyBindingError {}

impl KeyBindings {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind `keys` (keymap format, one stroke) to `action` where
    /// `predicate` holds, or everywhere for `None`. Later bindings win
    /// ties, so add overrides after defaults.
    pub fn bind(
        &mut self,
        keys: &str,
        predicate: Option<&str>,
        action: impl Into<Action>,
    ) -> Result<(), KeyBindingError> {
        let keys = keys
            .parse::<Binding>()
            .map_err(|_| KeyBindingError::Keys(keys.to_owned()))?;
        let predicate = predicate
            .map(str::parse::<KeyPredicate>)
            .transpose()
            .map_err(KeyBindingError::Predicate)?;
        self.bindings.push(KeyBinding {
            keys,
            predicate,
            action: action.into(),
        });
        Ok(())
    }

    pub fn push(&mut self, binding: KeyBinding) {
        self.bindings.push(binding);
    }

    pub fn bindings(&self) -> &[KeyBinding] {
        &self.bindings
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// The binding `pressed` triggers on `path`: the deepest match, the
    /// last added among equals.
    pub fn resolve(&self, pressed: &Binding, path: &[ContextEntry]) -> Option<KeyMatch<'_>> {
        let mut best: Option<KeyMatch> = None;
        for binding in &self.bindings {
            if !binding.keys.matches(pressed) {
                continue;
            }
            let depth = match &binding.predicate {
                None => 0,
                Some(predicate) => match predicate.depth_of(path) {
                    Some(depth) => depth,
                    None => continue,
                },
            };
            if best.is_none_or(|best| depth >= best.depth) {
                best = Some(KeyMatch { binding, depth });
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(entries: &[&str]) -> Vec<ContextEntry> {
        entries
            .iter()
            .map(|text| ContextEntry::parse(text))
            .collect()
    }

    fn holds(predicate: &str, entries: &[&str]) -> bool {
        predicate
            .parse::<KeyPredicate>()
            .unwrap()
            .eval(&path(entries))
    }

    #[test]
    fn predicates_evaluate_at_the_innermost_context() {
        let cases = [
            ("editor", &["workspace", "editor"][..], true),
            ("workspace", &["workspace", "editor"], false),
            ("editor && !vim_normal", &["editor"], true),
            ("editor && !vim_normal", &["editor vim_normal"], false),
            ("mode == insert", &["editor mode=insert"], true),
            ("mode == insert", &["editor mode=normal"], false),
            ("mode != insert", &["editor"], true),
            ("a || b && c", &["a"], true),
            ("(a || b) && c", &["a"], false),
            ("pane > editor", &["pane", "split", "editor"], true),
            ("pane > editor", &["editor"], false),
            (
                "pane > editor && mode == insert",
                &["pane", "editor mode=insert"],
                true,
            ),
            ("!editor", &[], true),
        ];
        for (predicate, entries, expected) in cases {
            assert_eq!(
                holds(predicate, entries),
                expected,
                "{predicate} on {entries:?}"
            );
        }
    }

    #[test]
    fn malformed_predicates_are_rejected() {
        for text in [
            "",
            "editor &&",
            "(editor",
            "mode ==",
            "editor editor",
            "a & b",
        ] {
            assert!(text.parse::<KeyPredicate>().is_err(), "{text:?}");
        }
    }

    #[test]
    fn deepest_match_wins_then_the_last_added() {
        #[derive(Debug, Clone, PartialEq)]
        struct Named(&'static str);
        impl From<Named> for Action {
            fn from(named: Named) -> Self {
                Action::new(named)
            }
        }

        let mut keys = KeyBindings::new();
        keys.bind("mod+k", None, Named("global")).unwrap();
        keys.bind("mod+k", Some("workspace"), Named("workspace"))
            .unwrap();
        keys.bind("mod+k", Some("editor"), Named("editor")).unwrap();
        keys.bind("mod+k", Some("editor"), Named("editor override"))
            .unwrap();
        keys.bind("mod+k", Some("editor && mode == insert"), Named("insert"))
            .unwrap();

        let cases = [
            (&[][..], "global"),
            (&["workspace"], "workspace"),
            (&["workspace", "editor"], "editor override"),
            (&["workspace", "editor mode=insert"], "insert"),
            (&["workspace", "editor mode=insert", "popover"], "insert"),
        ];
        let pressed: Binding = "ctrl+k".parse().unwrap();
        for (entries, expected) in cases {
            let got = keys.resolve(&pressed, &path(entries)).unwrap();
            let name = got.binding.action.downcast_ref::<Named>().unwrap().0;
            assert_eq!(name, expected, "{entries:?}");
        }
    }

    #[test]
    fn overlap_means_one_context_path_satisfies_both() {
        let cases = [
            ("editor", "editor", true),
            ("editor", "pane", true),
            ("editor && !pane", "pane && !editor", false),
            ("editor && !vim_normal", "editor && vim_normal", false),
            ("mode == insert", "mode == normal", false),
            ("mode == insert", "mode != normal", true),
            ("pane > editor", "editor && !pane", true),
            ("pane > editor", "!editor", false),
        ];
        for (a, b, expected) in cases {
            let (pa, pb) = (a.parse::<KeyPredicate>().unwrap(), b.parse().unwrap());
            assert_eq!(pa.overlaps(&pb), expected, "{a} vs {b}");
        }
    }
}
