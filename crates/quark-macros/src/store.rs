//! `#[derive(Store)]`: a parallel `XStore` struct where each field is a
//! `Signal<T>` handle (or a nested `YStore` for `#[store(flatten)]` fields).

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Ident, Result};

enum FieldKind {
    Leaf,
    Flatten,
    Skip,
}

fn parse_store_field_attr(attrs: &[syn::Attribute]) -> Result<FieldKind> {
    let mut kind: Option<(FieldKind, proc_macro2::Span)> = None;
    for attr in attrs {
        if !attr.path().is_ident("store") {
            continue;
        }
        attr.parse_nested_meta(|m| {
            let (new_kind, label) = if m.path.is_ident("flatten") {
                (FieldKind::Flatten, "flatten")
            } else if m.path.is_ident("skip") {
                (FieldKind::Skip, "skip")
            } else {
                return Err(
                    m.error("unknown `#[store(...)]` attribute; supported: `flatten`, `skip`")
                );
            };
            if let Some((_, prev_span)) = kind {
                let mut err = syn::Error::new(
                    m.path.span(),
                    format!(
                        "conflicting `#[store({label})]` — field already has a store attribute"
                    ),
                );
                err.combine(syn::Error::new(
                    prev_span,
                    "previous `#[store(...)]` attribute here",
                ));
                return Err(err);
            }
            kind = Some((new_kind, m.path.span()));
            Ok(())
        })?;
    }
    Ok(kind.map(|(k, _)| k).unwrap_or(FieldKind::Leaf))
}

/// Struct-level `#[store(default)]`: returns whether `new_default` is wanted.
fn parse_store_struct_attr(attrs: &[syn::Attribute]) -> Result<bool> {
    let mut default = false;
    for attr in attrs {
        if !attr.path().is_ident("store") {
            continue;
        }
        attr.parse_nested_meta(|m| {
            if m.path.is_ident("default") {
                default = true;
                Ok(())
            } else {
                Err(m.error("unknown struct-level `#[store(...)]` attribute; supported: `default`"))
            }
        })?;
    }
    Ok(default)
}

fn flatten_store_type(ty: &syn::Type) -> Result<syn::Type> {
    let syn::Type::Path(tp) = ty else {
        return Err(syn::Error::new_spanned(
            ty,
            "`#[store(flatten)]` expects a named struct type (e.g. `Foo` or `path::to::Foo`); \
             arrays, tuples, references, and generics are not supported",
        ));
    };
    let mut new_path = tp.clone();
    let last = new_path.path.segments.last_mut().unwrap();
    last.ident = Ident::new(&format!("{}Store", last.ident), last.ident.span());
    Ok(syn::Type::Path(new_path))
}

pub(crate) fn derive_store_impl(input: syn::DeriveInput) -> Result<TokenStream2> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "`#[derive(Store)]` cannot be applied to a generic type yet; \
             consider wrapping concrete instantiations instead",
        ));
    }

    let name = &input.ident;
    let store_name = Ident::new(&format!("{name}Store"), name.span());
    let vis = &input.vis;

    let fields = match &input.data {
        syn::Data::Struct(s) => match &s.fields {
            syn::Fields::Named(n) => &n.named,
            syn::Fields::Unnamed(_) => {
                return Err(syn::Error::new_spanned(
                    &input.ident,
                    "`#[derive(Store)]` requires a struct with named fields; \
                     tuple structs are not supported",
                ));
            }
            syn::Fields::Unit => {
                return Err(syn::Error::new_spanned(
                    &input.ident,
                    "`#[derive(Store)]` on a unit struct is meaningless — no fields to store",
                ));
            }
        },
        syn::Data::Enum(_) => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "`#[derive(Store)]` does not support enums; \
                 consider making the enum a leaf field of a struct that derives `Store`",
            ));
        }
        syn::Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "`#[derive(Store)]` does not support unions",
            ));
        }
    };

    let mut decls = Vec::new();
    let mut inits = Vec::new();
    let mut snapshot_fields = Vec::new();
    let mut any_skipped = false;

    for field in fields {
        let fname = field.ident.as_ref().unwrap();
        let fty = &field.ty;
        let fvis = &field.vis;

        match parse_store_field_attr(&field.attrs)? {
            FieldKind::Skip => {
                any_skipped = true;
            }
            FieldKind::Leaf => {
                decls.push(quote! {
                    #fvis #fname: ::quark::reactive::Signal<#fty>,
                });
                inits.push(quote! {
                    #fname: store.create(initial.#fname),
                });
                snapshot_fields.push(quote! {
                    #fname: store.read(self.#fname),
                });
            }
            FieldKind::Flatten => {
                let store_ty = flatten_store_type(fty)?;
                decls.push(quote! {
                    #fvis #fname: #store_ty,
                });
                inits.push(quote! {
                    #fname: <#store_ty>::new(store, initial.#fname),
                });
                snapshot_fields.push(quote! {
                    #fname: self.#fname.snapshot(store),
                });
            }
        }
    }

    // `snapshot()` can only reconstruct the original when every field is
    // represented in the store. If any field is `#[store(skip)]`, we can't
    // fill it in — omit the method.
    let snapshot_impl = if any_skipped {
        quote! {}
    } else {
        quote! {
            /// Read every signal and reconstruct the original plain struct.
            pub fn snapshot(&self, store: &::quark::reactive::SignalStore) -> #name {
                #name {
                    #(#snapshot_fields)*
                }
            }
        }
    };

    // `new_default` is opt-in: an unconditional `where Name: Default` bound
    // is trivially false on a non-`Default` struct and fails to compile.
    let new_default_impl = if parse_store_struct_attr(&input.attrs)? {
        quote! {
            /// Create a store initialized from `Original::default()`.
            pub fn new_default(store: &::quark::reactive::SignalStore) -> Self {
                Self::new(store, <#name as ::core::default::Default>::default())
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #[derive(Clone, Copy, Debug)]
        #vis struct #store_name {
            #(#decls)*
        }

        impl #store_name {
            /// Create a new store by consuming an initial value, allocating
            /// signals for each leaf field in the provided `SignalStore`.
            pub fn new(
                store: &::quark::reactive::SignalStore,
                initial: #name,
            ) -> Self {
                Self {
                    #(#inits)*
                }
            }

            #new_default_impl
            #snapshot_impl
        }
    })
}
