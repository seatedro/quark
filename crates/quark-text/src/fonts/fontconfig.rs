//! fontconfig's answer for `system-ui`/`sans-serif` and `monospace`, with
//! the user's aliases and the locale applied. libfontconfig is opened at
//! run time, so a machine without it (or a static musl build) falls back
//! to well-known families instead of failing to start.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::PathBuf;
use std::sync::OnceLock;

use super::FontRole;
use super::platform::Candidate;

/// Families tried, in order, when fontconfig cannot answer: the common
/// GNOME and KDE defaults, then the most widely installed.
const UI_FALLBACKS: [&str; 3] = ["Cantarell", "Noto Sans", "DejaVu Sans"];
const MONO_FALLBACKS: [&str; 3] = ["Noto Sans Mono", "DejaVu Sans Mono", "Liberation Mono"];

pub(crate) fn candidates(role: FontRole, locale: &str) -> Vec<Candidate> {
    let (patterns, fallbacks): (&[&str], &[&str]) = match role {
        // Older fontconfig lacks the system-ui alias and answers with its
        // default family; sans-serif then says the same.
        FontRole::Ui => (&["system-ui", "sans-serif"], &UI_FALLBACKS),
        FontRole::Mono => (&["monospace"], &MONO_FALLBACKS),
    };
    let mut candidates: Vec<Candidate> = Vec::new();
    if let Some(fc) = Fontconfig::get() {
        for pattern in patterns {
            if let Some(found) = fc.match_pattern(pattern, locale)
                && !candidates.contains(&found)
            {
                candidates.push(found);
            }
        }
    }
    candidates.extend(fallbacks.iter().map(|name| Candidate::family(name, false)));
    candidates
}

type FcConfig = c_void;
type FcPattern = c_void;
/// `FcResultMatch`.
const MATCH: c_int = 0;
/// `FcMatchPattern`.
const MATCH_PATTERN: c_int = 0;
const RTLD_NOW: c_int = 2;

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

type InitFn = unsafe extern "C" fn() -> c_int;
type NameParseFn = unsafe extern "C" fn(*const u8) -> *mut FcPattern;
type ConfigSubstituteFn = unsafe extern "C" fn(*mut FcConfig, *mut FcPattern, c_int) -> c_int;
type DefaultSubstituteFn = unsafe extern "C" fn(*mut FcPattern);
type FontMatchFn =
    unsafe extern "C" fn(*mut FcConfig, *mut FcPattern, *mut c_int) -> *mut FcPattern;
type GetStringFn =
    unsafe extern "C" fn(*mut FcPattern, *const c_char, c_int, *mut *mut u8) -> c_int;
type PatternDestroyFn = unsafe extern "C" fn(*mut FcPattern);

/// The libfontconfig functions a match needs.
struct Fontconfig {
    init: InitFn,
    name_parse: NameParseFn,
    config_substitute: ConfigSubstituteFn,
    default_substitute: DefaultSubstituteFn,
    font_match: FontMatchFn,
    get_string: GetStringFn,
    pattern_destroy: PatternDestroyFn,
}

impl Fontconfig {
    /// The library, loaded and initialized once per process; `None` when it
    /// is missing or lacks a function.
    fn get() -> Option<&'static Self> {
        static FONTCONFIG: OnceLock<Option<Fontconfig>> = OnceLock::new();
        FONTCONFIG
            .get_or_init(|| {
                // SAFETY: loading a library and looking up symbols by name;
                // each pointer is null-checked and then given the signature
                // fontconfig.h declares for that symbol.
                let fc = unsafe { Self::load()? };
                // SAFETY: FcInit takes no arguments; 0 means it failed.
                (unsafe { (fc.init)() } != 0).then_some(fc)
            })
            .as_ref()
    }

    unsafe fn load() -> Option<Self> {
        // SAFETY: a valid C string and a valid flag.
        let lib = unsafe { dlopen(c"libfontconfig.so.1".as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            return None;
        }
        let sym = |name: &CStr| {
            // SAFETY: `lib` is a live handle; the library stays loaded for
            // the rest of the process (it is never closed).
            let ptr = unsafe { dlsym(lib, name.as_ptr()) };
            (!ptr.is_null()).then_some(ptr)
        };
        // SAFETY (each transmute): a non-null function pointer of the
        // symbol fontconfig.h declares with this signature.
        unsafe {
            Some(Self {
                init: std::mem::transmute::<*mut c_void, InitFn>(sym(c"FcInit")?),
                name_parse: std::mem::transmute::<*mut c_void, NameParseFn>(sym(c"FcNameParse")?),
                config_substitute: std::mem::transmute::<*mut c_void, ConfigSubstituteFn>(sym(
                    c"FcConfigSubstitute",
                )?),
                default_substitute: std::mem::transmute::<*mut c_void, DefaultSubstituteFn>(sym(
                    c"FcDefaultSubstitute",
                )?),
                font_match: std::mem::transmute::<*mut c_void, FontMatchFn>(sym(c"FcFontMatch")?),
                get_string: std::mem::transmute::<*mut c_void, GetStringFn>(sym(
                    c"FcPatternGetString",
                )?),
                pattern_destroy: std::mem::transmute::<*mut c_void, PatternDestroyFn>(sym(
                    c"FcPatternDestroy",
                )?),
            })
        }
    }

    /// The face fontconfig picks for `family` in `locale`, as `fc-match`
    /// would.
    fn match_pattern(&self, family: &str, locale: &str) -> Option<Candidate> {
        // fontconfig languages are lowercase RFC 3066 tags.
        let lang: String = locale
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .map(|c| {
                if c == '_' {
                    '-'
                } else {
                    c.to_ascii_lowercase()
                }
            })
            .collect();
        let name = if lang.is_empty() {
            family.to_owned()
        } else {
            format!("{family}:lang={lang}")
        };
        let name = CString::new(name).ok()?;
        // SAFETY: fontconfig calls in its documented order on patterns this
        // function owns: parse, substitute, match, read, destroy. A null
        // config means the current one FcInit loaded. Strings read from the
        // match are copied before it is destroyed.
        unsafe {
            let pattern = (self.name_parse)(name.as_ptr().cast());
            if pattern.is_null() {
                return None;
            }
            (self.config_substitute)(std::ptr::null_mut(), pattern, MATCH_PATTERN);
            (self.default_substitute)(pattern);
            let mut result = 0;
            let matched = (self.font_match)(std::ptr::null_mut(), pattern, &mut result);
            (self.pattern_destroy)(pattern);
            if matched.is_null() {
                return None;
            }
            let read = |object: &CStr| {
                let mut value: *mut u8 = std::ptr::null_mut();
                let found = (self.get_string)(matched, object.as_ptr(), 0, &mut value);
                (found == MATCH && !value.is_null())
                    .then(|| CStr::from_ptr(value.cast()).to_string_lossy().into_owned())
            };
            let candidate = Candidate {
                family: read(c"family"),
                path: read(c"file").map(PathBuf::from),
                post_script_name: read(c"postscriptname"),
                from_api: true,
            };
            (self.pattern_destroy)(matched);
            (candidate.family.is_some() || candidate.path.is_some()).then_some(candidate)
        }
    }
}
