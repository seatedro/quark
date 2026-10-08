//! Reading Ghostty's `build.zig.zon` files and choosing which of their
//! dependencies the libghostty-vt build needs.
//!
//! Shared by build.rs, which fetches the chosen packages, and the crate's
//! unit tests (lib.rs includes this file under `cfg(test)`).

/// One entry of a manifest's `.dependencies`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    pub name: String,
    pub source: Source,
    /// Zig fetches a lazy dependency only when a build script asks for it.
    pub lazy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A tarball and its Zig package hash.
    Url { url: String, hash: String },
    /// A directory relative to the manifest.
    Path(String),
}

/// The Zig release series Ghostty's build needs.
pub const ZIG_SERIES: &str = "0.16";

/// Whether `zig version` output names a [`ZIG_SERIES`] release (a dev
/// build of it included). Other releases fail deep in Ghostty's build with
/// errors that do not mention the version.
pub fn zig_supported(version: &str) -> bool {
    version
        .strip_prefix(ZIG_SERIES)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['.', '-']))
}

/// Lazy dependencies that Ghostty's build script requests while configuring
/// `-Demit-lib-vt -Demit-themes=false`, found by building with Zig's
/// `--system` package directory holding only these. Zig resolves lazy
/// dependencies for the whole configured graph, so this includes some the
/// archive does not link (wuffs and its test images).
const LIB_VT_LAZY: &[&str] = &["highway", "simdutf", "translate_c", "wuffs", "pixels"];

/// Lazy dependencies requested only for some target OSes.
const LIB_VT_LAZY_BY_OS: &[(&str, &str)] = &[("macos", "zlib")];

/// Whether the libghostty-vt build for `target_os` (Cargo's
/// `CARGO_CFG_TARGET_OS`) needs `dep`. Every non-lazy dependency is needed.
pub fn needed(dep: &Dep, target_os: &str) -> bool {
    !dep.lazy
        || LIB_VT_LAZY.contains(&dep.name.as_str())
        || LIB_VT_LAZY_BY_OS
            .iter()
            .any(|&(os, name)| os == target_os && name == dep.name)
}

/// The `.dependencies` of a `build.zig.zon`. Fields other than `.url`,
/// `.hash`, `.path`, and `.lazy` are skipped.
pub fn parse_dependencies(zon: &str) -> Result<Vec<Dep>, String> {
    let tokens = tokenize(zon)?;
    let mut p = Parser { tokens, pos: 0 };
    p.expect(&Tok::Dot)?;
    p.expect(&Tok::Open)?;
    while !p.eat(&Tok::Close) {
        let field = p.field_name()?;
        if field == "dependencies" {
            return p.dependencies();
        }
        p.skip_value()?;
        p.eat(&Tok::Comma);
    }
    Ok(Vec::new())
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Dot,
    Eq,
    Open,
    Close,
    Comma,
    /// An identifier, `@"quoted"` identifier, number, or keyword.
    Word(String),
    Str(String),
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {}
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '.' => out.push(Tok::Dot),
            '=' => out.push(Tok::Eq),
            '{' => out.push(Tok::Open),
            '}' => out.push(Tok::Close),
            ',' => out.push(Tok::Comma),
            '"' => out.push(Tok::Str(string(&mut chars)?)),
            '@' if chars.peek() == Some(&'"') => {
                chars.next();
                out.push(Tok::Word(string(&mut chars)?));
            }
            c if c.is_alphanumeric() || c == '_' => {
                let mut word = c.to_string();
                while let Some(&c) = chars.peek() {
                    if !(c.is_alphanumeric() || c == '_') {
                        break;
                    }
                    word.push(c);
                    chars.next();
                }
                out.push(Tok::Word(word));
            }
            c => return Err(format!("unexpected character {c:?}")),
        }
    }
    Ok(out)
}

/// The rest of a string literal after its opening quote.
fn string(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<String, String> {
    let mut s = String::new();
    loop {
        match chars.next() {
            Some('"') => return Ok(s),
            Some('\\') => match chars.next() {
                Some('n') => s.push('\n'),
                Some(c) => s.push(c),
                None => break,
            },
            Some(c) => s.push(c),
            None => break,
        }
    }
    Err("unterminated string".into())
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn next(&mut self) -> Result<Tok, String> {
        let tok = self.tokens.get(self.pos).cloned().ok_or("unexpected end")?;
        self.pos += 1;
        Ok(tok)
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        let hit = self.tokens.get(self.pos) == Some(tok);
        self.pos += usize::from(hit);
        hit
    }

    fn expect(&mut self, tok: &Tok) -> Result<(), String> {
        match self.next()? {
            t if t == *tok => Ok(()),
            t => Err(format!("expected {tok:?}, found {t:?}")),
        }
    }

    /// `.name =`, returning the name.
    fn field_name(&mut self) -> Result<String, String> {
        self.expect(&Tok::Dot)?;
        let Tok::Word(name) = self.next()? else {
            return Err("expected a field name".into());
        };
        self.expect(&Tok::Eq)?;
        Ok(name)
    }

    /// A string, word, enum literal, or (nested) anonymous struct or list.
    fn skip_value(&mut self) -> Result<(), String> {
        match self.next()? {
            Tok::Str(_) | Tok::Word(_) => Ok(()),
            Tok::Dot => match self.next()? {
                Tok::Word(_) => Ok(()),
                Tok::Open => {
                    let mut depth = 1;
                    while depth > 0 {
                        match self.next()? {
                            Tok::Open => depth += 1,
                            Tok::Close => depth -= 1,
                            _ => {}
                        }
                    }
                    Ok(())
                }
                t => Err(format!("unexpected {t:?} after '.'")),
            },
            t => Err(format!("unexpected {t:?}")),
        }
    }

    fn dependencies(&mut self) -> Result<Vec<Dep>, String> {
        self.expect(&Tok::Dot)?;
        self.expect(&Tok::Open)?;
        let mut deps = Vec::new();
        while !self.eat(&Tok::Close) {
            let name = self.field_name()?;
            self.expect(&Tok::Dot)?;
            self.expect(&Tok::Open)?;
            let (mut url, mut hash, mut path, mut lazy) = (None, None, None, false);
            while !self.eat(&Tok::Close) {
                match self.field_name()?.as_str() {
                    "url" => url = Some(self.string()?),
                    "hash" => hash = Some(self.string()?),
                    "path" => path = Some(self.string()?),
                    "lazy" => lazy = self.next()? == Tok::Word("true".into()),
                    _ => self.skip_value()?,
                }
                self.eat(&Tok::Comma);
            }
            let source = match (url, hash, path) {
                (Some(url), Some(hash), None) => Source::Url { url, hash },
                (None, None, Some(path)) => Source::Path(path),
                _ => return Err(format!("dependency {name}: needs .url and .hash, or .path")),
            };
            deps.push(Dep { name, source, lazy });
            self.eat(&Tok::Comma);
        }
        Ok(deps)
    }

    fn string(&mut self) -> Result<String, String> {
        match self.next()? {
            Tok::Str(s) => Ok(s),
            t => Err(format!("expected a string, found {t:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Catches a Zig 0.17 build passing the check and failing later with a
    // misleading "build.zig: FileNotFound".
    #[test]
    fn only_zig_0_16_releases_are_supported() {
        let cases = [
            ("0.16.0", true),
            ("0.16.1-dev.42+abc123", true),
            ("0.17.0", false),
            ("0.15.2", false),
            ("0.160.0", false),
        ];
        for (version, supported) in cases {
            assert_eq!(zig_supported(version), supported, "{version}");
        }
    }

    /// Ghostty's manifest shapes: comments (with braces and quotes), field
    /// order varying, quoted names, and non-dependency fields around them.
    const FIXTURE: &str = r#".{
    .name = .ghostty,
    .version = "1.3.2-dev",
    .paths = .{""},
    .fingerprint = 0x64407a2a0b4147e5,
    .dependencies = .{
        // External translate-c }{ "not a string
        .translate_c = .{
            .lazy = true,
            .url = "https://example.org/translate_c.tar.gz",
            .hash = "translate_c-0.0.0-AAA",
        },
        .uucode = .{
            .url = "https://example.org/uucode.tar.gz",
            .hash = "uucode-0.2.0-BBB",
        },
        .vaxis = .{ .url = "https://example.org/vaxis.tar.gz", .hash = "vaxis-CCC", .lazy = true },
        .highway = .{ .path = "./pkg/highway", .lazy = true },
        .zlib = .{ .path = "./pkg/zlib", .lazy = true },
        .apple_sdk = .{ .path = "./pkg/apple-sdk" },
        .@"odd-name" = .{ .path = "./pkg/odd", .lazy = false },
    },
}
"#;

    fn dump(deps: &[Dep]) -> String {
        deps.iter()
            .map(|d| {
                let source = match &d.source {
                    Source::Url { url, hash } => format!("{url} {hash}"),
                    Source::Path(p) => p.clone(),
                };
                format!("{} {source}{}", d.name, if d.lazy { " lazy" } else { "" })
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn parses_ghostty_manifest_dependencies() {
        let deps = parse_dependencies(FIXTURE).unwrap();
        assert_eq!(
            dump(&deps),
            "translate_c https://example.org/translate_c.tar.gz translate_c-0.0.0-AAA lazy\n\
             uucode https://example.org/uucode.tar.gz uucode-0.2.0-BBB\n\
             vaxis https://example.org/vaxis.tar.gz vaxis-CCC lazy\n\
             highway ./pkg/highway lazy\n\
             zlib ./pkg/zlib lazy\n\
             apple_sdk ./pkg/apple-sdk\n\
             odd-name ./pkg/odd"
        );
    }

    #[test]
    fn selects_non_lazy_and_lib_vt_lazy_dependencies_per_os() {
        let deps = parse_dependencies(FIXTURE).unwrap();
        let selected = |os: &str| {
            deps.iter()
                .filter(|d| needed(d, os))
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        for (os, expected) in [
            ("linux", "translate_c uucode highway apple_sdk odd-name"),
            (
                "macos",
                "translate_c uucode highway zlib apple_sdk odd-name",
            ),
        ] {
            assert_eq!(selected(os), expected, "{os}");
        }
    }

    #[test]
    fn rejects_dependency_with_url_but_no_hash() {
        let err = parse_dependencies(
            r#".{ .dependencies = .{ .x = .{ .url = "https://example.org/x.tar.gz" } } }"#,
        )
        .unwrap_err();
        assert_eq!(err, "dependency x: needs .url and .hash, or .path");
    }
}
