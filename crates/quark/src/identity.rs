macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        ///
        /// The string is shared: clones (a cached subtree's replay) do not
        /// allocate.
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(::std::sync::Arc<str>);

        impl $name {
            pub fn new(value: impl Into<::std::sync::Arc<str>>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value.into())
            }
        }

        impl From<&str> for $name {
            /// Shared through a per-thread intern table: views name the
            /// same ids every frame, mostly from literals.
            fn from(value: &str) -> Self {
                Self($crate::identity::intern(value))
            }
        }

        impl From<::std::sync::Arc<str>> for $name {
            fn from(value: ::std::sync::Arc<str>) -> Self {
                Self(value)
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                <String as ::serde::Deserialize>::deserialize(deserializer).map(Self::from)
            }
        }
    };
}
pub(crate) use string_id;

/// Strings interned per thread before the table starts over; ids built
/// from changing text (formatted ids come in as `String` and are not
/// interned) cannot grow it without bound.
const INTERN_MAX: usize = 4096;

/// `value` as a shared string, allocated once per thread while the table
/// holds it. For names a view repeats every frame (ids, labels from
/// literals or stable data), so steady frames share them instead of
/// allocating.
pub fn intern(value: &str) -> ::std::sync::Arc<str> {
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::sync::Arc;
    thread_local! {
        static TABLE: RefCell<HashSet<Arc<str>>> = RefCell::new(HashSet::new());
    }
    TABLE
        .try_with(|table| {
            let mut table = table.borrow_mut();
            if let Some(shared) = table.get(value) {
                return shared.clone();
            }
            if table.len() >= INTERN_MAX {
                table.clear();
            }
            let shared: Arc<str> = value.into();
            table.insert(shared.clone());
            shared
        })
        .unwrap_or_else(|_| value.into())
}

/// 64-bit FNV-1a of `key`. Stable across runs and platforms, so ids derived
/// from it (focus ids, accessibility node ids) survive relaunches.
pub const fn stable_hash(key: &str) -> u64 {
    let bytes = key.as_bytes();
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    hash
}

string_id!(
    /// Stable identity for a retained semantic UI node.
    UiNodeId
);

string_id!(
    /// Stable sibling identity used when children are reordered.
    UiKey
);

string_id!(
    /// Stable identifier intended for harnesses, tests, and debug tooling.
    TestId
);
