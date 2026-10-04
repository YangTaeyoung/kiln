//! Compile translated format strings with Rust's normal formatting checks.
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use std::{collections::BTreeSet, sync::OnceLock};
use syn::{
    Expr, LitStr, Token,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
};
struct Input {
    source: LitStr,
    args: Punctuated<Expr, Token![,]>,
}
impl Parse for Input {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let source = input.parse()?;
        let args = if input.is_empty() {
            Punctuated::new()
        } else {
            input.parse::<Token![,]>()?;
            Punctuated::parse_terminated(input)?
        };
        Ok(Self { source, args })
    }
}
fn captures(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if bytes.get(i + 1) == Some(&b'{') {
                i += 2;
                continue;
            }
            if let Some(end) = text[i + 1..].find('}') {
                let field = &text[i + 1..i + 1 + end];
                let (name, spec) = field.split_once(':').unwrap_or((field, ""));
                if syn::parse_str::<syn::Ident>(name).is_ok() {
                    names.insert(name.into());
                }
                for part in spec.split('$').take(spec.matches('$').count()) {
                    let candidate = part
                        .rsplit(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .unwrap_or("");
                    if syn::parse_str::<syn::Ident>(candidate).is_ok() {
                        names.insert(candidate.into());
                    }
                }
                i += end + 2;
                continue;
            }
        }
        i += 1;
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captures_ignore_escaped_braces_and_positional_fields() {
        assert_eq!(
            captures("{{literal}} {} {0} {name} {error:#} {n:>width$.precision$}"),
            ["name", "error", "n", "width", "precision"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
    }
}
#[proc_macro]
pub fn trf(input: TokenStream) -> TokenStream {
    let Input { source, args } = syn::parse_macro_input!(input as Input);
    static MESSAGES: OnceLock<serde_json::Value> = OnceLock::new();
    let messages = MESSAGES.get_or_init(|| {
        serde_json::from_str(include_str!("../../kiln-common/locales/messages.json"))
            .expect("valid localization catalog")
    });
    let key = source.value();
    let entries = messages.get(&key);
    let translated = |language: &str| {
        LitStr::new(
            entries
                .and_then(|e| e.get(language))
                .and_then(|v| v.as_str())
                .unwrap_or(&key),
            source.span(),
        )
    };
    let en = translated("en");
    let ja = translated("ja");
    let zh = translated("zh-CN");
    let mut implicit = captures(&key);
    for arg in &args {
        if let Expr::Assign(assign) = arg {
            if let Expr::Path(path) = assign.left.as_ref() {
                if let Some(ident) = path.path.get_ident() {
                    implicit.remove(&ident.to_string());
                }
            }
        }
    }
    let args: Vec<_> = args.into_iter().collect();
    let named: Vec<_> = implicit
        .into_iter()
        .map(|s| {
            let id = format_ident!("{s}");
            quote!(#id = #id)
        })
        .collect();
    quote!({
        match ::kiln_common::i18n::language() {
            ::kiln_common::i18n::Language::Korean => format!(#source, #(#args,)* #(#named,)*),
            ::kiln_common::i18n::Language::English => format!(#en, #(#args,)* #(#named,)*),
            ::kiln_common::i18n::Language::Japanese => format!(#ja, #(#args,)* #(#named,)*),
            ::kiln_common::i18n::Language::ChineseSimplified => {
                format!(#zh, #(#args,)* #(#named,)*)
            }
        }
    })
    .into()
}
