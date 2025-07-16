// src/lib.rs
use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse2};
use synstructure::Structure;

#[proc_macro_derive(Managable, attributes(field))]
pub fn derive_managable(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    derive_managable_impl(input.into()).into()
}

fn derive_managable_impl(input: TokenStream) -> TokenStream {
    let input: DeriveInput = parse2(input).expect("failed to parse derive input");
    let ident = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let structure = Structure::new(&input);

    let body = structure.each_variant(|variant| {
        let mut scans = vec![];
        for (field, binding) in variant.ast().fields.iter().zip(variant.bindings()) {
            if field.attrs.iter().any(|attr| attr.path().is_ident("field")) {
                scans.push(quote! { scanner.scan_field(#binding); });
            } else if field
                .attrs
                .iter()
                .any(|attr| attr.path().is_ident("nullable"))
            {
                scans.push(quote! { scanner.scan_nullable(#binding); });
            } else {
                scans.push(quote! { scanner.scan_nested(#binding); });
            }
        }
        quote! { #(#scans)* }
    });

    quote! {
        unsafe impl #impl_generics refrigerator::Managable for #ident #ty_generics #where_clause {
            fn scan_nested(&self, scanner: &mut refrigerator::Scanner) {
                match *self {
                    #body
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::derive_managable_impl;
    use quote::quote;

    #[test]
    fn test_enum_macro_output() {
        let input = quote! {
            enum List<T> {
                Cons {
                    head: T,
                    #[field]
                    tail: Field<Box<List<T>>>,
                },
                Nil,
            }
        };

        let output = derive_managable_impl(input);
        println!("{}", output);
    }

    #[test]
    fn test_struct_macro_output() {
        let input = quote! {
            struct Point {
                x: i32,
                y: i32,
                #[nullable]
                z: Nullable<i32>,
            }
        };

        let output = derive_managable_impl(input);
        println!("{}", output);
    }
}
