//! Maintainer tool: collect UI expression strings and apply localization wrappers without reformatting.
use std::{collections::BTreeMap, path::Path};
use syn::{spanned::Spanned, visit::Visit};
#[derive(Default)]
struct Collector {
    strings: BTreeMap<String, usize>,
    edits: Vec<(usize, usize, String)>,
}
fn chinese(value: &str) -> bool {
    value.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
}
impl<'a> Visit<'a> for Collector {
    fn visit_attribute(&mut self, _: &'a syn::Attribute) {}
    fn visit_item_mod(&mut self, item: &'a syn::ItemMod) {
        if item.ident != "tests" && item.ident != "control_tests" {
            syn::visit::visit_item_mod(self, item);
        }
    }
    fn visit_item_const(&mut self, _: &'a syn::ItemConst) {}
    fn visit_item_static(&mut self, _: &'a syn::ItemStatic) {}
    fn visit_expr_lit(&mut self, expr: &'a syn::ExprLit) {
        if let syn::Lit::Str(literal) = &expr.lit
            && chinese(&literal.value())
        {
            *self.strings.entry(literal.value()).or_default() += 1;
            let span = literal.span().byte_range();
            self.edits.push((span.start, span.end, format!("swoop_i18n::tr!({})", quote::quote!(#literal))));
        }
    }
    fn visit_macro(&mut self, expr: &'a syn::Macro) {
        let name = expr.path.segments.last().unwrap().ident.to_string();
        if matches!(name.as_str(), "tr" | "trf") {
            if let Some(proc_macro2::TokenTree::Literal(lit)) = expr.tokens.clone().into_iter().next()
                && let Ok(literal) = syn::parse_str::<syn::LitStr>(&lit.to_string())
            {
                *self.strings.entry(literal.value()).or_default() += 1;
            }
            if let Ok(arguments) = syn::parse::Parser::parse2(
                syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
                expr.tokens.clone(),
            ) {
                for argument in arguments.iter().skip(1) {
                    self.visit_expr(argument);
                }
            }
            return;
        }
        if name != "format" {
            return;
        }
        let Some(proc_macro2::TokenTree::Literal(lit)) = expr.tokens.clone().into_iter().next() else { return };
        let Ok(literal) = syn::parse_str::<syn::LitStr>(&lit.to_string()) else { return };
        if chinese(&literal.value()) {
            *self.strings.entry(literal.value()).or_default() += 1;
            let span = expr.path.span().byte_range();
            self.edits.push((span.start, span.end, "swoop_i18n::trf".into()));
        }
        if let Ok(arguments) = syn::parse::Parser::parse2(
            syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
            expr.tokens.clone(),
        ) {
            for argument in arguments.iter().skip(1) {
                self.visit_expr(argument);
            }
        }
    }
}
fn files(directory: &Path, result: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            files(&path, result);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            result.push(path);
        }
    }
}
fn main() {
    let apply = std::env::args().any(|arg| arg == "--apply");
    let mut paths = Vec::new();
    files(Path::new("crates/app/src"), &mut paths);
    paths.sort();
    let mut catalog = BTreeMap::new();
    for path in paths {
        if path.file_name().is_some_and(|p| p == "demo.rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        let syntax = syn::parse_file(&source).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut collector = Collector::default();
        collector.visit_file(&syntax);
        for (key, count) in collector.strings {
            *catalog.entry(key).or_insert(0) += count;
        }
        if apply && !collector.edits.is_empty() {
            let mut changed = source;
            collector.edits.sort_by_key(|e| std::cmp::Reverse(e.0));
            for (start, end, replacement) in collector.edits {
                changed.replace_range(start..end, &replacement);
            }
            std::fs::write(path, changed).unwrap();
        }
    }
    println!("{}", serde_json::to_string_pretty(&catalog).unwrap());
}
