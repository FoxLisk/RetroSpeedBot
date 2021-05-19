// #![feature(trace_macros)]
//
// trace_macros!(true);

extern crate proc_macro;
use proc_macro::TokenStream;
use proc_macro2::{Spacing, Span, TokenTree as TokenTree2};
use quote::quote;
use quote::TokenStreamExt;
use syn::parse_macro_input;

#[proc_macro]
pub fn model(input: TokenStream) -> TokenStream {
    let tokens = parse_macro_input!(input as syn::ItemStruct);
    let name = &tokens.ident;
    let fields = &tokens.fields;

    let mut field_names = Vec::with_capacity(fields.len());
    let mut field_idents = Vec::with_capacity(fields.len());
    let mut field_params = quote! {};
    let self_tt = TokenTree2::Ident(proc_macro2::Ident::new("self", Span::call_site()));
    let dot_tt = TokenTree2::Punct(proc_macro2::Punct::new('.', Spacing::Alone));
    let comma_tt = TokenTree2::Punct(proc_macro2::Punct::new(',', Spacing::Alone));
    for field in fields {
        field_names.push(format!("{} = ?", field.ident.as_ref().unwrap().to_string()));
        field_idents.push(field.ident.as_ref().unwrap().clone());

        let tt = TokenTree2::Ident(field.ident.as_ref().unwrap().clone());
        field_params.append(self_tt.clone());
        field_params.append(dot_tt.clone());
        field_params.append(tt);
        field_params.append(comma_tt.clone());
    }

    let values_str = format!(" {} ", field_names.join(", "));
    let update_str = format!(" UPDATE {} SET {} WHERE id = ?", name, values_str);

    let expanded = quote! {
        #tokens

        impl #name {
            async fn save(&self, pool: &SqlitePool) -> sqlx::Result<()> {
            let q = sqlx::query![
                    #update_str,
                    #field_params // #field_params ends with a trailing comma, which sucks but /shrug
                    self.id
            ];

                debug!("Updating {:?}", self);

                match q.execute(pool).await {
                    Ok(_) => Ok(()),
                    Err(e) => Err(e),
                }
            }
        }

    };

    println!("{:?}", expanded.to_string());

    TokenStream::from(expanded)
}
