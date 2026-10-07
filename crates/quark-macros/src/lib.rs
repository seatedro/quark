//! Procedural macros for Quark, re-exported by the `quark` crate.
//!
//! - `view!` lowers HTML/JSX-like markup to builder calls on whatever
//!   `div()`, `text()`, and component types are in scope (quark-ui's
//!   `element` module provides them). `docs/guide/writing-views.md` is the
//!   reference, and `tests/view_macro.rs` covers each lowering rule.
//! - `#[derive(Props)]` gives a component a typed builder, so it can be
//!   written as `<Component prop={..}>` with required props checked at
//!   compile time.
//! - `#[derive(Store)]` generates a `FooStore` with one
//!   `quark::reactive::Signal` per field of `Foo`; `#[store(flatten)]`
//!   nests another store and `#[store(skip)]` leaves a field out.
use proc_macro::TokenStream;
use quote::quote;

mod classes;
mod props;
mod store;
mod suggest;
mod view;

#[proc_macro_derive(Store, attributes(store))]
pub fn derive_store(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    match store::derive_store_impl(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// A typed props builder for a component; see the crate docs.
#[proc_macro_derive(Props, attributes(prop))]
pub fn derive_props(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    match props::derive(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Markup lowered to builder calls; see `docs/guide/writing-views.md`.
#[proc_macro]
pub fn view(input: TokenStream) -> TokenStream {
    let input: view::ast::ViewInput = match syn::parse(input) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error().into(),
    };
    let emit = view::emit::Emit::new(input.scale.clone());
    let tokens = emit.root(&input.root);
    // Any error replaces the whole expansion, so a rejected input never
    // compiles into partial or silently altered UI.
    match emit.into_errors() {
        // A block, so several `compile_error!`s still form one expression.
        Some(err) => {
            let errors = err.to_compile_error();
            quote!({ #errors }).into()
        }
        None => tokens.into(),
    }
}

/// Every `class` vocabulary entry applied to the builder it names. Used by
/// the test that keeps the vocabulary in step with the builder API.
#[doc(hidden)]
#[proc_macro]
pub fn __class_vocabulary(_input: TokenStream) -> TokenStream {
    classes::vocabulary_check().into()
}
