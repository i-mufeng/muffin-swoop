use proc_macro::TokenStream;
use quote::quote;
use syn::{
    LitStr,
    parse::{Parse, ParseStream},
    parse_macro_input,
};

struct Message {
    source: LitStr,
    arguments: proc_macro2::TokenStream,
}
impl Parse for Message {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        Ok(Self { source: input.parse()?, arguments: input.parse()? })
    }
}
fn english(source: &LitStr) -> syn::Result<LitStr> {
    static CATALOG: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    let catalog = CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../../i18n/locales/en-US.json")).expect("valid English catalog")
    });
    let translated = catalog.get(source.value()).and_then(|v| v.as_str()).ok_or_else(|| {
        syn::Error::new(source.span(), "missing English translation in crates/i18n/locales/en-US.json")
    })?;
    Ok(LitStr::new(translated, source.span()))
}
#[proc_macro]
pub fn tr(input: TokenStream) -> TokenStream {
    let source = parse_macro_input!(input as LitStr);
    let translated = match english(&source) {
        Ok(value) => value,
        Err(error) => return error.to_compile_error().into(),
    };
    quote! { ::swoop_i18n::text(#source, #translated) }.into()
}
#[proc_macro]
pub fn trf(input: TokenStream) -> TokenStream {
    let Message { source, arguments } = parse_macro_input!(input as Message);
    let translated = match english(&source) {
        Ok(value) => value,
        Err(error) => return error.to_compile_error().into(),
    };
    quote! {
        if ::swoop_i18n::language() == ::swoop_i18n::Language::English {
            format!(#translated #arguments)
        } else { format!(#source #arguments) }
    }
    .into()
}
