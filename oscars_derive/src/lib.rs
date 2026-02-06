extern crate quote;
extern crate syn;
extern crate synstructure;

use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    Ident,
};
use synstructure::{decl_derive, AddBounds, Structure};

decl_derive! {
    [Trace, attributes(oscars_gc, unsafe_ignore_trace)] =>
    /// Derive the `Trace` trait.
    derive_trace
}

/// Derives the `Trace` trait.
#[allow(clippy::too_many_lines)]
fn derive_trace(mut s: Structure<'_>) -> proc_macro2::TokenStream {
    struct EmptyTrace {
        copy: bool,
        drop: bool,
    }

    impl Parse for EmptyTrace {
        fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
            let i: Ident = input.parse()?;

            if i != "empty_trace" && i != "unsafe_empty_trace" && i != "unsafe_no_drop" {
                let msg = format!(
                    "expected token \"empty_trace\", \"unsafe_empty_trace\" or \"unsafe_no_drop\", found {i:?}"
                );
                return Err(syn::Error::new_spanned(i.clone(), msg));
            }

            Ok(Self {
                copy: i == "empty_trace",
                drop: i == "empty_trace" || i != "unsafe_no_drop",
            })
        }
    }

    let mut drop = true;

    for attr in &s.ast().attrs {
        if attr.path().is_ident("oscars") {
            let trace = match attr.parse_args::<EmptyTrace>() {
                Ok(t) => t,
                Err(e) => return e.into_compile_error(),
            };

            if trace.copy {
                s.add_where_predicate(syn::parse_quote!(Self: Copy));
            }

            if !trace.drop {
                drop = false;
                continue;
            }

            return s.unsafe_bound_impl(
                quote!(::oscars::Trace),
                quote! {
                    #[inline(always)]
                    unsafe fn trace(&self, _color: ::oscars::TraceColor) {}
                    #[inline]
                    fn run_finalizer(&self) {
                        ::oscars::Finalize::finalize(self)
                    }
                },
            );
        }
    }

    s.filter(|bi| {
        !bi.ast()
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("unsafe_ignore_trace"))
    });
    let trace_body = s.each(|bi| quote!(::oscars::Trace::trace(#bi, color)));
    let trace_other_body = s.each(|bi| quote!(mark(#bi)));

    s.add_bounds(AddBounds::Fields);
    let trace_impl = s.unsafe_bound_impl(
        quote!(::oscars::Trace),
        quote! {
            #[inline]
            unsafe fn trace(&self, color: ::oscars::TraceColor) {
                #[allow(dead_code)]
                fn mark<T: ::oscars::Trace + ?Sized>(it: &T, color: oscars::TraceColor) {
                    unsafe {
                        ::oscars::Trace::trace(it, color);
                    }
                }
                match *self { #trace_body }
            }
            #[inline]
            fn run_finalizer(&self) {
                ::oscars::Finalize::finalize(self);
                #[allow(dead_code)]
                fn mark<T: ::oscars::Trace + ?Sized>(it: &T) {
                    unsafe {
                        ::oscars::Trace::run_finalizer(it);
                    }
                }
                match *self { #trace_other_body }
            }
        },
    );

    // We also implement drop to prevent unsafe drop implementations on this
    // type and encourage people to use Finalize. This implementation will
    // call `Finalize::finalize` if it is safe to do so.
    let drop_impl = if drop {
        s.unbound_impl(
            quote!(::core::ops::Drop),
            quote! {
                #[allow(clippy::inline_always)]
                #[inline(always)]
                fn drop(&mut self) {
                    ::oscars::Finalize::finalize(self);
                }
            },
        )
    } else {
        quote!()
    };

    quote! {
        #trace_impl
        #drop_impl
    }
}

decl_derive! {
    [Finalize] =>
    /// Derive the `Finalize` trait.
    derive_finalize
}

/// Derives the `Finalize` trait.
#[allow(clippy::needless_pass_by_value)]
fn derive_finalize(s: Structure<'_>) -> proc_macro2::TokenStream {
    s.unbound_impl(quote!(::oscars::Finalize), quote!())
}
