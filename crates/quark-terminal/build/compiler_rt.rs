//! Post-processing of the ELF archive's bundled compiler-rt.
//!
//! The static libghostty-vt bundles Zig's compiler-rt as one member,
//! `compiler_rt.o`, which also defines C library functions (`memcpy`,
//! `bcmp`, `sin`, ...) as weak hidden symbols, and Ghostty's own code
//! defines a `memset` the same way (src/quirks_memset.zig, to replace
//! compiler-rt's). A link pulls those members in (Rust code alone
//! references `bcmp` and `memcpy`), and a definition in a linked object
//! beats the C library's shared one, so every slice comparison and copy in
//! the program ran compiler-rt's portable loops (its `bcmp` compares a byte
//! at a time, about nine times slower than glibc's on 640 bytes).
//! [`prefer_libc`] renames those definitions so references from other
//! members and the program bind to the C library, while a member's calls
//! to its own definitions keep their targets. Ghostty does the same to
//! compiler-rt in Darwin archives (src/build/libsystem_override.sh, which
//! needs a Darwin host), and its list of functions is the one below.
//!
//! Zig 0.16 also sometimes writes `compiler_rt.o` with some section symbols
//! pointing at no section, which ld.bfd rejects ("undefined reference to
//! `no symbol'" from debug info, "overlapping FDEs" from `.eh_frame`). The
//! object Zig made it from, `compiler_rt_zcu.o` beside it in Zig's cache,
//! has the same contents intact, so such a member is replaced with it.

use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// C library functions the archive defines whose C library versions the
/// program should use. All of them are in glibc, musl, and libSystem.
const LIBC_SYMBOLS: &[&str] = &[
    "bcmp",
    "memcmp",
    "memcpy",
    "memmove",
    "memset",
    "strlen",
    "__memcpy_chk",
    "__memmove_chk",
    "__memset_chk",
    "__strcat_chk",
    "__strcpy_chk",
    "ceil",
    "ceilf",
    "ceill",
    "cos",
    "cosf",
    "cosl",
    "exp",
    "exp2",
    "exp2f",
    "exp2l",
    "expf",
    "expl",
    "fabs",
    "fabsf",
    "fabsl",
    "floor",
    "floorf",
    "floorl",
    "fma",
    "fmaf",
    "fmal",
    "fmax",
    "fmaxf",
    "fmaxl",
    "fmin",
    "fminf",
    "fminl",
    "fmod",
    "fmodf",
    "fmodl",
    "log",
    "log10",
    "log10f",
    "log10l",
    "log2",
    "log2f",
    "log2l",
    "logf",
    "logl",
    "round",
    "roundf",
    "roundl",
    "sin",
    "sinf",
    "sinl",
    "sqrt",
    "sqrtf",
    "sqrtl",
    "tan",
    "tanf",
    "tanl",
    "trunc",
    "truncf",
    "truncl",
];

/// The prefix renamed definitions get.
const RENAMED: &str = "__ghostty_vt_";

const MAGIC: &[u8] = b"!<arch>\n";
const HEADER: usize = 60;

/// Rewrites the GNU archive at `archive` so no member defines any of
/// [`LIBC_SYMBOLS`] and its compiler-rt member is intact, then rebuilds the
/// archive's symbol index with `zig ranlib`.
pub fn prefer_libc(archive: &Path, zig: &str) {
    let data = fs::read(archive)
        .unwrap_or_else(|e| panic!("quark-terminal: read {}: {e}", archive.display()));
    assert!(
        data.starts_with(MAGIC),
        "quark-terminal: {} is not an ar archive",
        archive.display()
    );
    let mut out = MAGIC.to_vec();
    let mut long_names: &[u8] = &[];
    let mut changed = false;
    let mut pos = MAGIC.len();
    while pos < data.len() {
        let header = &data[pos..pos + HEADER];
        let size: usize = field(&header[48..58]).parse().expect("archive member size");
        let raw = &data[pos + HEADER..pos + HEADER + size];
        pos += HEADER + size + (size & 1);
        let name = field(&header[..16]);
        let mut body = Cow::Borrowed(raw);
        match name {
            // The symbol index names old member offsets; ranlib rebuilds it.
            "/" | "/SYM64/" => continue,
            "//" => long_names = raw,
            _ => {
                let full = member_name(name, long_names);
                let compiler_rt = Path::new(&full).file_name() == Some("compiler_rt.o".as_ref());
                let mut object = raw.to_vec();
                if compiler_rt {
                    object = intact_compiler_rt(object, &full);
                }
                if !rename_definitions(&mut object) && compiler_rt {
                    // An ELF this cannot edit links as it is, just slower.
                    println!(
                        "cargo:warning=quark-terminal: left compiler-rt's C library \
                         functions in {} (not 64-bit little-endian ELF)",
                        archive.display()
                    );
                }
                if object != raw {
                    changed = true;
                    body = object.into();
                }
            }
        }
        let mut header = header.to_vec();
        header[48..58].copy_from_slice(format!("{:<10}", body.len()).as_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(&body);
        if body.len() % 2 == 1 {
            out.push(b'\n');
        }
    }
    if !changed {
        // Nothing to fix; keep the archive and its index as Zig wrote them.
        return;
    }
    fs::write(archive, &out)
        .unwrap_or_else(|e| panic!("quark-terminal: write {}: {e}", archive.display()));
    let mut ranlib = Command::new(zig);
    ranlib.arg("ranlib").arg(archive);
    let status = ranlib
        .status()
        .unwrap_or_else(|e| panic!("quark-terminal: could not run {ranlib:?}: {e}"));
    assert!(
        status.success(),
        "quark-terminal: {ranlib:?} failed: {status}"
    );
}

/// A header field without its space padding.
fn field(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes)
        .expect("ASCII ar header")
        .trim_end()
}

/// A member's full name: `name/` inline, or `/offset` into the `//` table,
/// where names end in `/\n`.
fn member_name(name: &str, long_names: &[u8]) -> String {
    match name.strip_prefix('/').and_then(|n| n.parse::<usize>().ok()) {
        Some(offset) => {
            let rest = &long_names[offset..];
            let end = rest
                .windows(2)
                .position(|w| w == b"/\n")
                .unwrap_or(rest.len());
            String::from_utf8_lossy(&rest[..end]).into_owned()
        }
        None => name.trim_end_matches('/').to_owned(),
    }
}

/// `object`, or the intact copy of it in Zig's cache when it is corrupt
/// (see the top of this file). `path` is the member's name in the archive,
/// the object's path in Zig's cache.
fn intact_compiler_rt(object: Vec<u8>, path: &str) -> Vec<u8> {
    if !Elf::parse(&object).is_some_and(|elf| elf.has_detached_section_symbol()) {
        return object;
    }
    let zcu = PathBuf::from(path).with_file_name("compiler_rt_zcu.o");
    let intact = fs::read(&zcu)
        .ok()
        .filter(|zcu| Elf::parse(zcu).is_some_and(|elf| !elf.has_detached_section_symbol()));
    intact.unwrap_or_else(|| {
        panic!(
            "quark-terminal: Zig wrote a corrupt {path} (section symbols without a section) \
             and {} is missing or corrupt too. Delete {} so Zig builds it again.",
            zcu.display(),
            Path::new(path)
                .parent()
                .unwrap_or(Path::new(path))
                .display()
        )
    })
}

/// Renames the global definitions of [`LIBC_SYMBOLS`] in an ELF object,
/// writing the new names into a copy of the string table appended to the
/// file. Returns false, leaving `object` alone, if it is not 64-bit
/// little-endian ELF.
fn rename_definitions(object: &mut Vec<u8>) -> bool {
    let Some(elf) = Elf::parse(object) else {
        return false;
    };
    let symtab = elf.symtab();
    let strtab = elf.section(symtab.link);
    let mut names = object[strtab.offset..][..strtab.size].to_vec();
    let mut renames = Vec::new();
    for at in symtab.entries() {
        let sym = Sym::read(object, at);
        let bind = sym.info >> 4;
        // STB_GLOBAL or STB_WEAK, defined (not SHN_UNDEF).
        if !matches!(bind, 1 | 2) || sym.shndx == 0 {
            continue;
        }
        let name = c_str(&object[strtab.offset + sym.name as usize..]);
        if LIBC_SYMBOLS.iter().any(|s| s.as_bytes() == name) {
            renames.push((at, names.len() as u32));
            names.extend_from_slice(RENAMED.as_bytes());
            names.extend_from_slice(name);
            names.push(0);
        }
    }
    if renames.is_empty() {
        return true;
    }
    for (at, name) in renames {
        put(object, at, &name.to_le_bytes());
    }
    let offset = object.len() as u64;
    object.extend_from_slice(&names);
    put(object, strtab.header + 24, &offset.to_le_bytes());
    put(
        object,
        strtab.header + 32,
        &(names.len() as u64).to_le_bytes(),
    );
    true
}

/// The parts of a 64-bit little-endian ELF relocatable object read here.
struct Elf<'a> {
    data: &'a [u8],
    sections: Vec<Section>,
}

#[derive(Clone, Copy)]
struct Section {
    /// Offset of the section header in the file.
    header: usize,
    kind: u32,
    offset: usize,
    size: usize,
    link: usize,
    entsize: usize,
}

struct Sym {
    name: u32,
    info: u8,
    shndx: u16,
}

impl<'a> Elf<'a> {
    fn parse(data: &'a [u8]) -> Option<Self> {
        // ELFCLASS64, ELFDATA2LSB.
        if !data.starts_with(b"\x7fELF\x02\x01") {
            return None;
        }
        let shoff = u64_at(data, 0x28) as usize;
        let shentsize = u16_at(data, 0x3a) as usize;
        let mut shnum = u16_at(data, 0x3c) as usize;
        if shnum == 0 {
            // More than 0xff00 sections: the count is in section 0.
            shnum = u64_at(data, shoff + 32) as usize;
        }
        let sections = (0..shnum)
            .map(|i| {
                let header = shoff + i * shentsize;
                Section {
                    header,
                    kind: u32_at(data, header + 4),
                    offset: u64_at(data, header + 24) as usize,
                    size: u64_at(data, header + 32) as usize,
                    link: u32_at(data, header + 40) as usize,
                    entsize: u64_at(data, header + 56) as usize,
                }
            })
            .collect();
        Some(Self { data, sections })
    }

    fn section(&self, index: usize) -> Section {
        self.sections[index]
    }

    /// The SHT_SYMTAB section; a relocatable object has exactly one.
    fn symtab(&self) -> Section {
        *self
            .sections
            .iter()
            .find(|s| s.kind == 2)
            .expect("compiler_rt.o has no symbol table")
    }

    /// Whether a section symbol (STT_SECTION) names no section, the
    /// corruption described at the top of this file.
    fn has_detached_section_symbol(&self) -> bool {
        self.symtab().entries().skip(1).any(|at| {
            let sym = Sym::read(self.data, at);
            sym.info & 0xf == 3 && sym.shndx == 0
        })
    }
}

impl Section {
    /// File offsets of the table's entries.
    fn entries(self) -> impl Iterator<Item = usize> {
        (0..self.size / self.entsize).map(move |i| self.offset + i * self.entsize)
    }
}

impl Sym {
    fn read(data: &[u8], at: usize) -> Self {
        Self {
            name: u32_at(data, at),
            info: data[at + 4],
            shndx: u16_at(data, at + 6),
        }
    }
}

fn c_str(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())]
}

fn put(data: &mut [u8], at: usize, bytes: &[u8]) {
    data[at..at + bytes.len()].copy_from_slice(bytes);
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(data[at..at + 2].try_into().unwrap())
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().unwrap())
}
