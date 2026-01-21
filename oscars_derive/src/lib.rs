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
    [Trace, attributes(boa_gc, unsafe_ignore_trace)] =>
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
        if attr.path().is_ident("boa_gc") {
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
                quote!(::boa_gc::Trace),
                quote! {
                    #[inline(always)]
                    unsafe fn trace(&self, _tracer: &mut ::boa_gc::Tracer) {}
                    #[inline(always)]
                    unsafe fn trace_non_roots(&self) {}
                    #[inline]
                    fn run_finalizer(&self) {
                        ::boa_gc::Finalize::finalize(self)
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
    let trace_body = s.each(|bi| quote!(::boa_gc::Trace::trace(#bi, tracer)));
    let trace_other_body = s.each(|bi| quote!(mark(#bi)));

    s.add_bounds(AddBounds::Fields);
    let trace_impl = s.unsafe_bound_impl(
        quote!(::boa_gc::Trace),
        quote! {
            #[inline]
            unsafe fn trace(&self, tracer: &mut ::boa_gc::Tracer) {
                #[allow(dead_code)]
                let mut mark = |it: &dyn ::boa_gc::Trace| {
                    // SAFETY: The implementor must ensure that `trace` is correctly implemented.
                    unsafe {
                        ::boa_gc::Trace::trace(it, tracer);
                    }
                };
                match *self { #trace_body }
            }
            #[inline]
            unsafe fn trace_non_roots(&self) {
                #[allow(dead_code)]
                fn mark<T: ::boa_gc::Trace + ?Sized>(it: &T) {
                    // SAFETY: The implementor must ensure that `trace_non_roots` is correctly implemented.
                    unsafe {
                        ::boa_gc::Trace::trace_non_roots(it);
                    }
                }
                match *self { #trace_other_body }
            }
            #[inline]
            fn run_finalizer(&self) {
                ::boa_gc::Finalize::finalize(self);
                #[allow(dead_code)]
                fn mark<T: ::boa_gc::Trace + ?Sized>(it: &T) {
                    unsafe {
                        ::boa_gc::Trace::run_finalizer(it);
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
                    if ::boa_gc::finalizer_safe() {
                        ::boa_gc::Finalize::finalize(self);
                    }
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
    s.unbound_impl(quote!(::boa_gc::Finalize), quote!())
}
