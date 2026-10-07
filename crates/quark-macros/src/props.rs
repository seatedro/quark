//! `#[derive(Props)]`: a typed builder for a component, used by
//! `view!`'s `<Component prop=..>` form and callable directly as
//! `Component::builder().prop(..).build()`.
//!
//! Field attributes:
//! - none: a required prop. Leaving it out is a compile error naming it.
//! - `#[prop(default)]` / `#[prop(default = expr)]`: optional, starting at
//!   `Default::default()` / `expr`.
//! - `#[prop(optional)]` on an `Option<T>` field: optional, and the setter
//!   takes `T`.
//! - `#[prop(into)]`: the setter takes `impl Into<T>`.
//!
//! Required props are tracked in the builder's type: each is a type
//! parameter that is `()` until set and `__{Name}PropSet` after, and `build()` is
//! only callable once every one is set. A per-prop marker trait with
//! `#[diagnostic::on_unimplemented]` turns a missing prop into
//! "missing required prop `x` on `<Component>`".

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Expr, Fields, GenericArgument, Ident, PathArguments, Result, Type};

struct Prop {
    name: Ident,
    ty: Type,
    docs: Vec<syn::Attribute>,
    kind: Kind,
    into: bool,
}

enum Kind {
    Required,
    Default(Option<Expr>),
    /// `Option<T>` field; carries `T`.
    Optional(Type),
}

pub(crate) fn derive(input: DeriveInput) -> Result<TokenStream2> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(
            input.ident.span(),
            "`#[derive(Props)]` needs a struct with named fields",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new(
            input.ident.span(),
            "`#[derive(Props)]` needs a struct with named fields",
        ));
    };
    let props = fields
        .named
        .iter()
        .map(parse_prop)
        .collect::<Result<Vec<_>>>()?;

    let name = &input.ident;
    let vis = &input.vis;
    let builder = format_ident!("{name}Builder");
    let (_, ty_generics, where_clause) = input.generics.split_for_impl();
    let struct_params: Vec<&syn::GenericParam> = input.generics.params.iter().collect();
    let struct_args: Vec<TokenStream2> = input
        .generics
        .params
        .iter()
        .map(|p| match p {
            syn::GenericParam::Type(t) => {
                let i = &t.ident;
                quote!(#i)
            }
            syn::GenericParam::Lifetime(l) => {
                let l = &l.lifetime;
                quote!(#l)
            }
            syn::GenericParam::Const(c) => {
                let i = &c.ident;
                quote!(#i)
            }
        })
        .collect();

    let required: Vec<&Prop> = props
        .iter()
        .filter(|p| matches!(p.kind, Kind::Required))
        .collect();
    let markers: Vec<Ident> = (0..required.len())
        .map(|i| format_ident!("__P{i}"))
        .collect();
    let set = format_ident!("__{name}PropSet");
    let unset: Vec<TokenStream2> = required.iter().map(|_| quote!(())).collect();

    // Builder fields: `Option<T>` for required props, `T` otherwise.
    let storage = props.iter().map(|p| {
        let n = &p.name;
        let ty = &p.ty;
        match p.kind {
            Kind::Required => quote!(#n: ::core::option::Option<#ty>,),
            _ => quote!(#n: #ty,),
        }
    });
    let initial = props.iter().map(|p| {
        let n = &p.name;
        match &p.kind {
            Kind::Required | Kind::Optional(_) => quote!(#n: ::core::option::Option::None,),
            Kind::Default(None) => quote!(#n: ::core::default::Default::default(),),
            Kind::Default(Some(e)) => quote!(#n: #e,),
        }
    });

    let setters = props.iter().map(|p| {
        let n = &p.name;
        let docs = &p.docs;
        let (param_ty, store) = match &p.kind {
            Kind::Optional(inner) => (inner, quote!(::core::option::Option::Some(value))),
            _ => (&p.ty, quote!(value)),
        };
        let (param, convert) = if p.into {
            (quote!(impl ::core::convert::Into<#param_ty>), quote!(let value: #param_ty = value.into();))
        } else {
            (quote!(#param_ty), quote!())
        };
        match p.kind {
            Kind::Required => {
                // Setting a required prop moves its marker from `()` to
                // the set marker; every other field moves across unchanged.
                let index = required.iter().position(|r| r.name == p.name).unwrap();
                let out_markers = markers.iter().enumerate().map(|(i, m)| {
                    if i == index { quote!(#set) } else { quote!(#m) }
                });
                let moves = props.iter().map(|q| {
                    let f = &q.name;
                    if q.name == p.name {
                        quote!(#f: ::core::option::Option::Some(#store),)
                    } else {
                        quote!(#f: self.#f,)
                    }
                });
                quote! {
                    #(#docs)*
                    #[inline]
                    pub fn #n(self, value: #param) -> #builder<#(#struct_args,)* #(#out_markers),*> {
                        #convert
                        #builder {
                            #(#moves)*
                            __quark_marker: ::core::marker::PhantomData,
                        }
                    }
                }
            }
            _ => quote! {
                #(#docs)*
                #[inline]
                pub fn #n(mut self, value: #param) -> Self {
                    #convert
                    self.#n = #store;
                    self
                }
            },
        }
    });

    let display = |p: &Prop| {
        let s = p.name.to_string();
        match s.strip_prefix("on_") {
            Some(event) => format!("on:{event}"),
            None => s,
        }
    };
    let require_traits: Vec<Ident> = required
        .iter()
        .map(|p| format_ident!("__{}Requires_{}", name, p.name))
        .collect();
    let trait_defs = required.iter().zip(&require_traits).map(|(p, t)| {
        let message = format!("missing required prop `{}` on `<{name}>`", display(p));
        let label = format!("`<{name}>` needs `{}={{..}}`", display(p));
        quote! {
            #[doc(hidden)]
            #[allow(non_camel_case_types)]
            #[diagnostic::on_unimplemented(message = #message, label = #label)]
            #vis trait #t {}
            impl #t for #set {}
        }
    });

    let finals = props.iter().map(|p| {
        let n = &p.name;
        match p.kind {
            Kind::Required => quote! {
                #n: match self.#n {
                    ::core::option::Option::Some(value) => value,
                    // The marker bounds on `build` make this unreachable.
                    ::core::option::Option::None => ::core::unreachable!(),
                },
            },
            _ => quote!(#n: self.#n,),
        }
    });

    let all_params = quote!(#(#struct_params,)* #(#markers),*);
    let all_args = quote!(#(#struct_args,)* #(#markers),*);
    let where_preds = where_clause.map(|w| {
        let p = &w.predicates;
        quote!(#p,)
    });
    let doc = format!(
        "Builder for [`{name}`]'s props; `view!` uses it for `<{name} ..>`. \
         Call `build()` once every required prop is set."
    );
    let builder_doc = format!("Start building a [`{name}`] from its props.");
    let build_doc = format!("The [`{name}`]. Compiles only once every required prop is set.");
    Ok(quote! {
        #[doc = #doc]
        #[must_use = "call `.build()` to get the component"]
        #vis struct #builder<#all_params> #where_clause {
            #(#storage)*
            __quark_marker: ::core::marker::PhantomData<(#(#markers,)*)>,
        }

        /// Marks a required prop as set in the builder's type.
        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        #vis struct #set;

        #(#trait_defs)*

        impl<#(#struct_params),*> #name #ty_generics #where_clause {
            #[doc = #builder_doc]
            pub fn builder() -> #builder<#(#struct_args,)* #(#unset),*> {
                #builder {
                    #(#initial)*
                    __quark_marker: ::core::marker::PhantomData,
                }
            }
        }

        impl<#all_params> #builder<#all_args> where #where_preds {
            #(#setters)*
        }

        impl<#all_params> #builder<#all_args> where #where_preds {
            // The marker bounds sit on the method, not the impl, so a
            // missing prop is an unsatisfied bound (E0277, which shows the
            // `on_unimplemented` message) instead of "no method `build`".
            #[doc = #build_doc]
            pub fn build(self) -> #name #ty_generics
            where
                #(#markers: #require_traits,)*
            {
                #name { #(#finals)* }
            }
        }
    })
}

fn parse_prop(field: &syn::Field) -> Result<Prop> {
    let name = field.ident.clone().expect("named field");
    let mut kind = Kind::Required;
    let mut into = false;
    let mut optional = false;
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("prop")) {
        attr.parse_nested_meta(|m| {
            if m.path.is_ident("into") {
                into = true;
            } else if m.path.is_ident("optional") {
                optional = true;
            } else if m.path.is_ident("default") {
                kind = if m.input.peek(syn::Token![=]) {
                    Kind::Default(Some(m.value()?.parse()?))
                } else {
                    Kind::Default(None)
                };
            } else {
                return Err(m.error(
                    "unknown `#[prop(..)]` option; supported: `default`, `default = expr`, \
                     `optional`, `into`",
                ));
            }
            Ok(())
        })?;
    }
    if optional {
        if !matches!(kind, Kind::Required) {
            return Err(syn::Error::new(
                name.span(),
                "`#[prop(optional)]` already defaults to `None`; drop `default`",
            ));
        }
        let inner = option_inner(&field.ty).ok_or_else(|| {
            syn::Error::new(
                field.ty.span(),
                "`#[prop(optional)]` needs an `Option<T>` field",
            )
        })?;
        kind = Kind::Optional(inner.clone());
    }
    let docs = field
        .attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .cloned()
        .collect();
    Ok(Prop {
        name,
        ty: field.ty.clone(),
        docs,
        kind,
        into,
    })
}

fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else { return None };
    let last = path.path.segments.last()?;
    if last.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    match args.args.first()? {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    }
}
