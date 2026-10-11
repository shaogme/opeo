use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{
    Error, File, FnArg, GenericParam, Ident, Item, ItemFn, Pat, Path, Result, TraitItemFn,
    Visibility, parse_quote, parse2,
};

use crate::{
    attributes::{OpeoArgs, WrapperAttributeFilter},
    rewrite::RewriteTry,
    signature::SignaturePlan,
};

pub(super) struct Expansion;

impl Expansion {
    pub(super) fn expand(attribute: OpeoArgs, item: TokenStream) -> Result<TokenStream> {
        if let Ok(function) = parse2::<ItemFn>(item.clone()) {
            return Self::expand_function(attribute, function);
        }

        let function = parse2::<TraitItemFn>(item)?;
        Self::expand_trait_function(attribute, function)
    }

    fn expand_function(attribute: OpeoArgs, function: ItemFn) -> Result<TokenStream> {
        let runtime_path = Self::opeo_crate_path()?;
        Self::expand_opeo_with_path(attribute, function, runtime_path)
    }

    fn expand_trait_function(
        attribute: OpeoArgs,
        trait_function: TraitItemFn,
    ) -> Result<TokenStream> {
        let runtime_path = Self::opeo_crate_path()?;
        let has_default = trait_function.default.is_some();
        let function = ItemFn {
            attrs: trait_function.attrs,
            vis: Visibility::Inherited,
            sig: trait_function.sig,
            block: Box::new(trait_function.default.unwrap_or_else(|| parse_quote!({}))),
        };
        let expansion = Self::expand_opeo_with_path(attribute, function, runtime_path)?;
        let generated: File = parse2(expansion)?;
        let mut trait_items = Vec::with_capacity(generated.items.len());

        for (index, item) in generated.items.into_iter().enumerate() {
            let Item::Fn(function) = item else {
                return Err(Error::new_spanned(
                    item,
                    "#[opeo] generated an unsupported item in a trait method",
                ));
            };
            let attributes = function.attrs;
            let mut signature = function.sig;
            let body = function.block;

            if index == 0 && !has_default {
                for input in &mut signature.inputs {
                    if let FnArg::Typed(argument) = input
                        && let Pat::Ident(pattern) = argument.pat.as_mut()
                    {
                        pattern.mutability = None;
                    }
                }
                trait_items.push(quote!(#(#attributes)* #signature;));
            } else {
                trait_items.push(quote!(#(#attributes)* #signature #body));
            }
        }

        Ok(quote!(#(#trait_items)*))
    }

    fn expand_opeo_with_path(
        attribute: OpeoArgs,
        mut function: ItemFn,
        runtime_path: Path,
    ) -> Result<TokenStream> {
        let original_function = function.clone();
        let plan = SignaturePlan::analyze(&attribute, &mut function, &runtime_path)?;
        Self::rewrite_function_body(&plan, &mut function, &runtime_path);
        Self::prepare_opeo_entry(&plan, &mut function, &runtime_path);
        let wrapper = Self::build_wrapper(
            &attribute,
            &original_function,
            &function,
            &plan,
            &runtime_path,
        )?;

        Ok(Self::combine_expansion(&function, wrapper))
    }

    fn prepare_opeo_entry(plan: &SignaturePlan, function: &mut ItemFn, runtime_path: &Path) {
        plan.prepare_opeo_signature(function, runtime_path);
    }

    fn rewrite_function_body(plan: &SignaturePlan, function: &mut ItemFn, runtime_path: &Path) {
        let mut rewrite = RewriteTry::new(runtime_path.clone(), plan.source_result_type().clone());
        rewrite.visit_function_body(&mut function.block);
        function
            .block
            .stmts
            .insert(0, parse_quote!(let _ = &mut out;));
    }

    fn build_wrapper(
        arguments: &OpeoArgs,
        original_function: &ItemFn,
        opeo_function: &ItemFn,
        plan: &SignaturePlan,
        runtime_path: &Path,
    ) -> Result<TokenStream> {
        let Some(wrapper_name) = plan.wrapper_name() else {
            return Ok(quote!());
        };
        let wrapper_attributes = WrapperAttributeFilter::select(
            &original_function.attrs,
            arguments.wrapper_attribute_paths(),
        )?;
        let mut wrapper_signature = if plan.is_const() {
            let mut signature = original_function.sig.clone();
            signature.ident = wrapper_name.clone();
            signature
        } else {
            let mut signature = opeo_function.sig.clone();
            signature.ident = wrapper_name.clone();
            let slot_lifetime = plan.slot_lifetime();
            let borrow_lifetime = plan.borrow_lifetime();
            signature.generics.params = signature
                .generics
                .params
                .into_iter()
                .filter(|parameter| match parameter {
                    GenericParam::Lifetime(lifetime) => {
                        lifetime.lifetime != *slot_lifetime && lifetime.lifetime != *borrow_lifetime
                    }
                    GenericParam::Type(_) | GenericParam::Const(_) => true,
                })
                .collect();
            for input in &mut signature.inputs {
                if let FnArg::Typed(argument) = input {
                    if let Pat::Ident(pattern) = argument.pat.as_mut() {
                        pattern.mutability = None;
                    }
                }
            }
            signature.inputs.pop();
            signature.output = plan.original_output().clone();
            signature
        };

        let call_arguments = SignaturePlan::wrapper_argument_idents(&wrapper_signature);
        let mut arguments = call_arguments.iter();
        for input in &mut wrapper_signature.inputs {
            if let FnArg::Typed(input) = input {
                let Some(argument) = arguments.next() else {
                    return Err(Error::new_spanned(
                        input,
                        "could not create a wrapper argument for this parameter",
                    ));
                };
                *input.pat = parse_quote!(#argument);
            }
        }

        let original_name = &opeo_function.sig.ident;
        let wrapper_doc = format!(
            "Standard `Result` wrapper; see [`{original_name}`] for the OPEO form.\n标准 `Result` 包装函数；OPEO 版本见 [`{original_name}`]。"
        );
        let call_arguments = call_arguments.iter();
        let call_target = if plan.is_method() {
            quote!(self.#original_name)
        } else {
            quote!(#original_name)
        };
        let call = if wrapper_signature.unsafety.is_some() {
            quote!(unsafe { #call_target(#(#call_arguments,)* out) })
        } else {
            quote!(#call_target(#(#call_arguments,)* out))
        };
        let call = if plan.is_async() {
            quote!(#call.await)
        } else {
            call
        };
        let slot_ident = SignaturePlan::fresh_local_ident(&wrapper_signature, "__opeo_err_slot");
        let error_type = plan.error_type();
        let wrapper_body = if plan.is_const() {
            let body = &original_function.block;
            quote!(#body)
        } else if plan.is_async() {
            quote!({
                let mut #slot_ident = #runtime_path::ErrSlot::<#error_type>::new();
                #slot_ident.call_async(async move |out| { #call }).await
            })
        } else {
            quote!({
                let mut #slot_ident = #runtime_path::ErrSlot::<#error_type>::new();
                #slot_ident.call(|out| #call)
            })
        };

        Ok(quote! {
            #(#wrapper_attributes)*
            #[doc = #wrapper_doc]
            #wrapper_signature
            #wrapper_body
        })
    }

    fn combine_expansion(function: &ItemFn, wrapper: TokenStream) -> TokenStream {
        quote! {
            #function
            #wrapper
        }
    }

    fn opeo_crate_path() -> Result<Path> {
        match crate_name("opeo") {
            Ok(FoundCrate::Itself) => Ok(parse_quote!(::opeo)),
            Ok(FoundCrate::Name(name)) => {
                let ident = Ident::new(&name, Span::call_site());
                Ok(parse_quote!(::#ident))
            }
            Err(error) => Err(Error::new(
                Span::call_site(),
                format!("could not resolve the `opeo` crate: {error}"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Expansion;
    use crate::attributes::OpeoArgs;
    use quote::quote;
    use syn::{Expr, File, Item, ItemFn, Lit, Meta, Result, parse_quote, parse2};

    fn ok_or_return<T>(result: Result<T>) -> Option<T> {
        assert!(result.is_ok(), "expected successful parser result");
        result.ok()
    }

    #[test]
    fn generated_functions_have_distinct_documentation() {
        let Some(attribute) = ok_or_return(parse2::<OpeoArgs>(quote!(wrapper = parse_std))) else {
            return;
        };
        let function: ItemFn = parse_quote! {
            /// User-provided function documentation.
            /// 用户提供的函数说明。
            pub fn parse(input: &str) -> Result<u32, ParseError> {
                Ok(input.len() as u32)
            }
        };
        let Some(expansion) = ok_or_return(Expansion::expand_opeo_with_path(
            attribute,
            function,
            parse_quote!(::opeo),
        )) else {
            return;
        };
        let Some(file) = ok_or_return(parse2::<File>(expansion)) else {
            return;
        };
        let original = file.items.iter().find_map(|item| match item {
            Item::Fn(function) if function.sig.ident == "parse" => Some(function),
            _ => None,
        });
        let wrapper = file.items.iter().find_map(|item| match item {
            Item::Fn(function) if function.sig.ident == "parse_std" => Some(function),
            _ => None,
        });
        let Some(original) = original else {
            return;
        };
        let Some(wrapper) = wrapper else {
            return;
        };
        let original_docs = doc_attribute_values(original);
        let wrapper_docs = doc_attribute_values(wrapper);

        assert!(original_docs.contains("User-provided function documentation."));
        assert!(original_docs.contains("The OPEO form accepts"));
        assert!(!original_docs.contains("Standard `Result` wrapper"));
        assert!(wrapper_docs.contains("User-provided function documentation."));
        assert!(wrapper_docs.contains("Standard `Result` wrapper"));
        assert!(wrapper_docs.contains("`parse`"));
        assert!(!wrapper_docs.contains("The OPEO form accepts"));
    }
    #[test]
    fn disabled_wrapper_expands_to_only_the_opeo_function() {
        let Some(attribute) = ok_or_return(parse2::<OpeoArgs>(quote!(wrapper = false))) else {
            return;
        };
        let function: ItemFn = parse_quote! {
            pub fn parse(input: &str) -> Result<u32, ParseError> {
                Ok(input.len() as u32)
            }
        };
        let Some(expansion) = ok_or_return(Expansion::expand_opeo_with_path(
            attribute,
            function,
            parse_quote!(::opeo),
        )) else {
            return;
        };
        let Some(file) = ok_or_return(parse2::<File>(expansion)) else {
            return;
        };

        assert_eq!(file.items.len(), 1);
        let Some(Item::Fn(function)) = file.items.first() else {
            return;
        };
        let docs = doc_attribute_values(function);
        assert!(docs.contains("no standard wrapper is generated"));
        assert!(!docs.contains("see [`"));
    }

    fn doc_attribute_values(function: &ItemFn) -> String {
        function
            .attrs
            .iter()
            .filter_map(|attribute| match &attribute.meta {
                Meta::NameValue(value) if value.path.is_ident("doc") => match &value.value {
                    Expr::Lit(expression) => match &expression.lit {
                        Lit::Str(value) => Some(value.value()),
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
