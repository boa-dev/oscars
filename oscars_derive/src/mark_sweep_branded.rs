use quote::quote;
use synstructure::{AddBounds, Structure, decl_derive};

decl_derive! {
    [Trace, attributes(oscars_gc, unsafe_ignore_trace)] =>
    /// Derive the `Trace` trait for mark_sweep_branded collector.
    derive_trace
}

fn derive_trace(mut s: Structure<'_>) -> proc_macro2::TokenStream {
    s.filter(|bi| {
        !bi.ast()
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("unsafe_ignore_trace"))
    });
    
    let trace_body = s.each(|bi| {
        quote!(::oscars::collectors::mark_sweep_branded::Trace::trace(#bi, color))
    });

    s.add_bounds(AddBounds::Fields);
    s.bound_impl(
        quote!(::oscars::collectors::mark_sweep_branded::Trace),
        quote! {
            #[inline]
            fn trace(&self, color: &::oscars::collectors::mark_sweep_branded::TraceColor) {
                match *self { #trace_body }
            }
        },
    )
}

decl_derive! {
    [Finalize] =>
    /// Derive the `Finalize` trait for mark_sweep_branded collector
    derive_finalize
}

fn derive_finalize(s: Structure<'_>) -> proc_macro2::TokenStream {
    s.unbound_impl(
        quote!(::oscars::collectors::mark_sweep_branded::Finalize),
        quote!(),
    )
}
