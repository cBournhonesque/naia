use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::DataStruct;

// TODO: for types that implement Serialize/Deserialize (get via attribute)
//  derive Serde here ourselves with bincode (or another library defined via attribute)

#[allow(clippy::format_push_string)]
pub fn derive_serde_struct(struct_: &DataStruct, struct_name: &Ident) -> TokenStream {
    let mut ser_body = quote! {};
    let mut de_body = quote! {};

    for field in &struct_.fields {
        let field_name = field.ident.as_ref().expect("expected field to have a name");
        ser_body = quote! {
            #ser_body
            self.#field_name.ser(writer);
        };
        de_body = quote! {
            #de_body
            #field_name: Serde::de(reader)?,
        };
    }
    quote! {
        impl Serde for #struct_name {
             fn ser(&self, writer: &mut dyn naia_serde::BitWrite) {
                #ser_body
             }
             fn de(reader: &mut naia_serde::BitReader) -> std::result::Result<Self, naia_serde::SerdeErr> {
                std::result::Result::Ok(Self {
                    #de_body
                })
             }

        }
    }
}
