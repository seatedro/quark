//! Spell checking for text fields that opt in.
//!
//! [`SpellDictionary`] wraps a Hunspell dictionary (`.aff` and `.dic`)
//! read by spellbook, a pure-Rust Hunspell port. Quark bundles none:
//! [`SpellDictionary::find`] looks in the system's dictionary folders
//! (`/usr/share/hunspell` and friends, `~/Library/Spelling`, `$DICPATH`),
//! and an app can ship its own and load it with
//! [`SpellDictionary::from_hunspell`].
//!
//! [`SpellChecker`] checks text on a worker thread so typing never waits
//! on it; an [`super::Editor`] given one ([`super::Editor::set_spellcheck`])
//! sends its text after each change and paints a wavy line under each
//! misspelled word the checker reports.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, RwLock};

use unicode_segmentation::UnicodeSegmentation;

/// A dictionary could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpellError {
    /// No `{locale}.aff` and `.dic` pair in any searched folder.
    NotFound {
        locale: String,
    },
    Read {
        path: PathBuf,
        message: String,
    },
    Parse {
        message: String,
    },
}

impl std::fmt::Display for SpellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { locale } => write!(f, "no Hunspell dictionary for {locale}"),
            Self::Read { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Parse { message } => write!(f, "dictionary: {message}"),
        }
    }
}

impl std::error::Error for SpellError {}

type AddHook = Box<dyn Fn(&str) + Send + Sync>;

/// A Hunspell dictionary plus the words the user added. Share it with
/// `Arc`: every checker and field using one sees words added through any.
pub struct SpellDictionary {
    words: RwLock<spellbook::Dictionary>,
    /// Told about each word the user adds, to save it.
    on_add: RwLock<Option<AddHook>>,
}

impl std::fmt::Debug for SpellDictionary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpellDictionary").finish_non_exhaustive()
    }
}

impl SpellDictionary {
    /// Parse a Hunspell dictionary from the text of its two files.
    pub fn from_hunspell(aff: &str, dic: &str) -> Result<Self, SpellError> {
        let words = spellbook::Dictionary::new(aff, dic).map_err(|e| SpellError::Parse {
            message: e.to_string(),
        })?;
        Ok(Self {
            words: RwLock::new(words),
            on_add: RwLock::new(None),
        })
    }

    /// The system's dictionary folders, most specific first.
    pub fn system_dirs() -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::env::var_os("DICPATH")
            .map(|paths| std::env::split_paths(&paths).collect())
            .unwrap_or_default();
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            dirs.push(home.join(".local/share/hunspell"));
            dirs.push(home.join("Library/Spelling"));
        }
        for dir in [
            "/usr/share/hunspell",
            "/usr/local/share/hunspell",
            "/usr/share/myspell",
            "/usr/share/myspell/dicts",
            "/Library/Spelling",
        ] {
            dirs.push(PathBuf::from(dir));
        }
        dirs
    }

    /// Load the dictionary for `locale` (`en-US` or `en_US`; a language
    /// alone matches any of its regions) from the first of `dirs` that has
    /// one.
    pub fn find(locale: &str, dirs: &[PathBuf]) -> Result<Self, SpellError> {
        let name = locale.replace('-', "_");
        let language = name.split('_').next().unwrap_or(&name).to_owned();
        for dir in dirs {
            let Some(stem) = dictionary_stem(dir, &name, &language) else {
                continue;
            };
            let read = |ext: &str| {
                let path = dir.join(format!("{stem}.{ext}"));
                std::fs::read_to_string(&path).map_err(|e| SpellError::Read {
                    path,
                    message: e.to_string(),
                })
            };
            return Self::from_hunspell(&read("aff")?, &read("dic")?);
        }
        Err(SpellError::NotFound {
            locale: locale.to_owned(),
        })
    }

    /// Call `hook` with each word the user adds (to save it for the next
    /// launch, where the app passes it to [`Self::add_word`] again).
    pub fn set_add_hook(&self, hook: impl Fn(&str) + Send + Sync + 'static) {
        if let Ok(mut slot) = self.on_add.write() {
            *slot = Some(Box::new(hook));
        }
    }

    /// Accept `word` from now on, without telling the add hook (words the
    /// app restores).
    pub fn add_word(&self, word: &str) {
        if let Ok(mut words) = self.words.write() {
            // Words are plain text, never `word/FLAGS` syntax.
            let _ = words.add(&word.replace('/', ""));
        }
    }

    /// The user asked to accept `word`: add it and tell the add hook.
    pub fn learn(&self, word: &str) {
        self.add_word(word);
        if let Ok(hook) = self.on_add.read()
            && let Some(hook) = hook.as_ref()
        {
            hook(word);
        }
    }

    pub fn check(&self, word: &str) -> bool {
        self.words.read().map_or(true, |words| words.check(word))
    }

    /// Likely corrections for `word`, best first.
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(words) = self.words.read() {
            words.suggest(word, &mut out);
        }
        out
    }

    /// Byte ranges of the misspelled words in `text`. Words with digits or
    /// that sit in a path, URL, or identifier (touching `/`, `_`, `.`,
    /// `@`, or `:` followed by text) are skipped.
    pub fn misspellings(&self, text: &str) -> Vec<Range<usize>> {
        let Ok(words) = self.words.read() else {
            return Vec::new();
        };
        let checker = words.checker();
        text.unicode_word_indices()
            .filter(|(at, word)| {
                let range = *at..*at + word.len();
                !word.chars().any(|c| c.is_ascii_digit() || c == '_')
                    && word.chars().any(char::is_alphabetic)
                    && !in_token(text, range)
                    && !checker.check(word)
            })
            .map(|(at, word)| at..at + word.len())
            .collect()
    }
}

/// Whether the word at `range` is part of a path, URL, address, or
/// identifier.
fn in_token(text: &str, range: Range<usize>) -> bool {
    let joined = |c: char| matches!(c, '/' | '_' | '\\' | '@');
    let before = text.get(..range.start).and_then(|s| s.chars().next_back());
    let mut after = text.get(range.end..).map(str::chars).into_iter().flatten();
    let next = after.next();
    let following = after.next();
    let continues = |c: Option<char>| c.is_some_and(|c| !c.is_whitespace());
    before.is_some_and(joined)
        || next.is_some_and(joined)
        || (matches!(next, Some('.' | ':')) && continues(following))
        || (matches!(before, Some('.')) && range.start >= 2)
}

/// The file stem in `dir` of the dictionary for `name` (`en_US`), or of
/// any region of `language`.
fn dictionary_stem(dir: &Path, name: &str, language: &str) -> Option<String> {
    let has = |stem: &str| {
        dir.join(format!("{stem}.aff")).is_file() && dir.join(format!("{stem}.dic")).is_file()
    };
    if has(name) {
        return Some(name.to_owned());
    }
    let mut stems: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| {
            let path = e.ok()?.path();
            (path.extension()? == "dic").then(|| path.file_stem()?.to_str().map(str::to_owned))?
        })
        .filter(|stem| stem == language || stem.starts_with(&format!("{language}_")))
        .filter(|stem| has(stem))
        .collect();
    stems.sort();
    stems.into_iter().next()
}

/// The misspellings in revision `rev` of a field's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpellResult {
    pub rev: u64,
    pub misspelled: Vec<Range<usize>>,
}

/// One text to check, and where its result goes.
struct Job {
    client: u64,
    rev: u64,
    text: Arc<str>,
    reply: Sender<SpellResult>,
}

static NEXT_CLIENT: AtomicU64 = AtomicU64::new(0);

/// Checks text on a worker thread. Each clone shares the worker and the
/// dictionary but gets its own results, so give each field a clone. A
/// field's requests that queue up while the worker is busy are skipped for
/// its newest one.
pub struct SpellChecker {
    dictionary: Arc<SpellDictionary>,
    requests: Sender<Job>,
    client: u64,
    reply: Sender<SpellResult>,
    results: Mutex<Receiver<SpellResult>>,
}

impl Clone for SpellChecker {
    fn clone(&self) -> Self {
        let (reply, results) = channel();
        Self {
            dictionary: self.dictionary.clone(),
            requests: self.requests.clone(),
            client: NEXT_CLIENT.fetch_add(1, Ordering::Relaxed),
            reply,
            results: Mutex::new(results),
        }
    }
}

impl std::fmt::Debug for SpellChecker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpellChecker").finish_non_exhaustive()
    }
}

impl SpellChecker {
    /// Start a worker over `dictionary`. It calls `wake` after each result
    /// so the app can repaint (pass its `Waker::wake`).
    pub fn spawn(
        dictionary: Arc<SpellDictionary>,
        wake: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        let (requests, jobs) = channel::<Job>();
        let words = dictionary.clone();
        std::thread::Builder::new()
            .name("quark-spellcheck".into())
            .spawn(move || {
                while let Ok(first) = jobs.recv() {
                    // Only each client's newest text matters.
                    let mut pending: Vec<Job> = vec![first];
                    for job in jobs.try_iter() {
                        pending.retain(|p| p.client != job.client);
                        pending.push(job);
                    }
                    for job in pending {
                        let misspelled = words.misspellings(&job.text);
                        let rev = job.rev;
                        // A client that is gone just misses its result.
                        let _ = job.reply.send(SpellResult { rev, misspelled });
                    }
                    wake();
                }
            })?;
        let (reply, results) = channel();
        Ok(Self {
            dictionary,
            requests,
            client: NEXT_CLIENT.fetch_add(1, Ordering::Relaxed),
            reply,
            results: Mutex::new(results),
        })
    }

    pub fn dictionary(&self) -> &Arc<SpellDictionary> {
        &self.dictionary
    }

    /// Check revision `rev` of a text.
    pub fn request(&self, rev: u64, text: Arc<str>) {
        let _ = self.requests.send(Job {
            client: self.client,
            rev,
            text,
            reply: self.reply.clone(),
        });
    }

    /// The newest result that has arrived, if any.
    pub fn poll(&self) -> Option<SpellResult> {
        self.results.lock().ok()?.try_iter().last()
    }

    /// Block until a result arrives.
    pub fn wait(&self) -> Option<SpellResult> {
        self.results.lock().ok()?.recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::super::{Editor, TextDecoration, TextEditCommand};
    use super::*;

    /// A seven-word test dictionary (vendored here, test only).
    fn dictionary() -> Arc<SpellDictionary> {
        let aff = include_str!("testdata/spell.aff");
        let dic = include_str!("testdata/spell.dic");
        Arc::new(SpellDictionary::from_hunspell(aff, dic).expect("test dictionary"))
    }

    /// The words of `text` at `ranges`.
    fn words<'a>(text: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
        ranges.iter().filter_map(|r| text.get(r.clone())).collect()
    }

    #[test]
    fn misspelled_words_are_found_and_others_skipped() {
        let cases: &[(&str, &[&str])] = &[
            ("the quick brwn fox", &["brwn"]),
            ("hello worlds, Hello", &[]),
            ("café cafe", &["cafe"]),
            ("the src/qwk.rs fox foo_bar qwk@fox.io", &[]),
            ("abc123 and v2", &["and"]),
            ("teh fox. Teh", &["teh", "Teh"]),
        ];
        let dictionary = dictionary();
        for (text, expected) in cases {
            let found = dictionary.misspellings(text);
            assert_eq!(words(text, &found), *expected, "{text}");
        }
    }

    #[test]
    fn suggestions_and_learned_words() {
        let dictionary = dictionary();
        assert_eq!(
            dictionary.suggest("wrold").first().map(String::as_str),
            Some("world")
        );
        let saved = Arc::new(Mutex::new(Vec::new()));
        let sink = saved.clone();
        dictionary.set_add_hook(move |word| sink.lock().expect("lock").push(word.to_owned()));
        dictionary.learn("quark");
        dictionary.add_word("wgpu");
        assert_eq!(
            words(
                "quark wgpu qurk",
                &dictionary.misspellings("quark wgpu qurk")
            ),
            ["qurk"]
        );
        assert_eq!(
            *saved.lock().expect("lock"),
            ["quark"],
            "only learned words reach the hook"
        );
    }

    // Catches the editor never sending its text to the worker, dropping
    // the result, or painting no squiggle under a misspelled word.
    #[test]
    fn an_editor_marks_misspellings_from_the_worker() {
        let mut text_system = quark_text::TextSystem::vendored_only(&Default::default());
        let checker = SpellChecker::spawn(dictionary(), || {}).expect("worker");
        let mut editor = Editor::default();
        editor.sync_size(400.0, 60.0);
        editor.set_spellcheck(Some(checker));
        editor.set_text("the qick fox jmps");
        editor.flush(&mut text_system);
        editor.wait_for_spelling();
        assert_eq!(
            words(editor.text(), editor.misspellings()),
            ["qick", "jmps"]
        );

        // The word being typed at the caret is not marked yet.
        let squiggles = |editor: &Editor| {
            editor
                .decoration_rects()
                .iter()
                .filter(|(kind, _)| *kind == TextDecoration::Misspelled)
                .count()
        };
        assert_eq!(squiggles(&editor), 1);
        editor.apply(TextEditCommand::InsertText(" ok".into()));
        editor.flush(&mut text_system);
        editor.wait_for_spelling();
        editor.flush(&mut text_system);
        assert_eq!(
            words(editor.text(), editor.misspellings()),
            ["qick", "jmps", "ok"]
        );
        assert_eq!(squiggles(&editor), 2);

        let issue = editor.spelling_at(6).expect("qick");
        assert_eq!(
            (
                issue.word.as_str(),
                issue.suggestions.first().map(String::as_str)
            ),
            ("qick", Some("quick"))
        );
        editor.learn_word("jmps");
        editor.flush(&mut text_system);
        editor.wait_for_spelling();
        assert_eq!(words(editor.text(), editor.misspellings()), ["qick", "ok"]);
    }
}
