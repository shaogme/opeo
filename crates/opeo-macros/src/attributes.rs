use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{
    Attribute, Error, Ident, LitBool, Meta, Path, Result, Token, Type,
    parse::{Parse, ParseStream, Parser},
    punctuated::Punctuated,
};

pub(super) struct OpeoArgs {
    wrapper: WrapperConfig,
    ok_type: Option<Type>,
    error_type: Option<Type>,
    wrapper_attributes: Vec<Path>,
}

pub(super) enum WrapperConfig {
    Default,
    Named(Ident),
    Disabled,
}

impl Parse for OpeoArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut wrapper = None;
        let mut ok_type = None;
        let mut error_type = None;
        let mut wrapper_attributes = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            if key == "wrapper_attrs" {
                if wrapper_attributes.is_some() {
                    return Err(Error::new_spanned(
                        key,
                        "`wrapper_attrs` may only be specified once",
                    ));
                }
                let content;
                syn::parenthesized!(content in input);
                let selected = Punctuated::<Path, Token![,]>::parse_terminated(&content)?
                    .into_iter()
                    .collect::<Vec<_>>();
                if let Some(forbidden) = selected
                    .iter()
                    .find(|path| wrapper_attribute_path_is_forbidden(path))
                {
                    return Err(Error::new_spanned(
                        forbidden,
                        "symbol attributes cannot be copied to the generated wrapper",
                    ));
                }
                wrapper_attributes = Some(selected);
            } else {
                input.parse::<Token![=]>()?;
                match key.to_string().as_str() {
                    "wrapper" => {
                        if wrapper.is_some() {
                            return Err(Error::new_spanned(
                                key,
                                "`wrapper` may only be specified once",
                            ));
                        }
                        wrapper = Some(if input.peek(LitBool) {
                            let enabled: LitBool = input.parse()?;
                            if enabled.value {
                                WrapperConfig::Default
                            } else {
                                WrapperConfig::Disabled
                            }
                        } else {
                            WrapperConfig::Named(input.parse()?)
                        });
                    }
                    "ok" => {
                        if ok_type.is_some() {
                            return Err(Error::new_spanned(key, "`ok` may only be specified once"));
                        }
                        ok_type = Some(input.parse()?);
                    }
                    "error" => {
                        if error_type.is_some() {
                            return Err(Error::new_spanned(
                                key,
                                "`error` may only be specified once",
                            ));
                        }
                        error_type = Some(input.parse()?);
                    }
                    _ => {
                        return Err(Error::new_spanned(
                            key,
                            "expected `wrapper`, `ok`, `error`, or `wrapper_attrs(...)`",
                        ));
                    }
                }
            }

            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        if ok_type.is_some() != error_type.is_some() {
            return Err(Error::new(
                Span::call_site(),
                "`ok = Type` and `error = Type` must be specified together",
            ));
        }

        Ok(Self {
            wrapper: wrapper.unwrap_or(WrapperConfig::Default),
            ok_type,
            error_type,
            wrapper_attributes: wrapper_attributes.unwrap_or_default(),
        })
    }
}

impl OpeoArgs {
    pub(super) fn wrapper_config(&self) -> &WrapperConfig {
        &self.wrapper
    }

    pub(super) fn explicit_types(&self) -> (Option<Type>, Option<Type>) {
        (self.ok_type.clone(), self.error_type.clone())
    }

    pub(super) fn wrapper_attribute_paths(&self) -> &[Path] {
        &self.wrapper_attributes
    }
}

pub(super) struct WrapperAttributeFilter;

impl WrapperAttributeFilter {
    pub(super) fn select(attributes: &[Attribute], requested: &[Path]) -> Result<Vec<Attribute>> {
        attributes
            .iter()
            .filter_map(|attribute| {
                wrapper_meta(&attribute.meta, requested)
                    .transpose()
                    .map(|meta| {
                        meta.map(|meta| {
                            let mut attribute = attribute.clone();
                            attribute.meta = meta;
                            attribute
                        })
                    })
            })
            .collect()
    }
}

fn wrapper_meta(meta: &Meta, requested: &[Path]) -> Result<Option<Meta>> {
    match meta {
        Meta::List(list) if list.path.is_ident("cfg_attr") => {
            let mut nested = parse_meta_list(&list.tokens)?.into_iter();
            let Some(condition) = nested.next() else {
                return Ok(Some(meta.clone()));
            };
            let retained = nested
                .map(|meta| wrapper_meta(&meta, requested))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            if retained.is_empty() {
                Ok(None)
            } else {
                let mut filtered = list.clone();
                filtered.tokens = quote!(#condition, #(#retained),*);
                Ok(Some(Meta::List(filtered)))
            }
        }
        _ if !wrapper_attribute_is_forbidden(meta)
            && (wrapper_attribute_is_safe(meta)
                || requested.iter().any(|path| paths_match(path, meta.path()))) =>
        {
            Ok(Some(meta.clone()))
        }
        _ => Ok(None),
    }
}

fn paths_match(left: &Path, right: &Path) -> bool {
    quote!(#left).to_string() == quote!(#right).to_string()
}

fn wrapper_attribute_is_forbidden(meta: &Meta) -> bool {
    wrapper_attribute_path_is_forbidden(meta.path())
}

fn wrapper_attribute_path_is_forbidden(path: &Path) -> bool {
    let Some(ident) = path.get_ident() else {
        return false;
    };
    matches!(
        ident.to_string().as_str(),
        "unsafe" | "export_name" | "no_mangle" | "link_section" | "naked"
    )
}

fn wrapper_attribute_is_safe(meta: &Meta) -> bool {
    let Some(ident) = meta.path().get_ident() else {
        return false;
    };
    matches!(
        ident.to_string().as_str(),
        "cfg" | "doc" | "deprecated" | "inline" | "cold" | "must_use" | "track_caller"
    )
}

fn parse_meta_list(tokens: &TokenStream) -> Result<Punctuated<Meta, Token![,]>> {
    Punctuated::<Meta, Token![,]>::parse_terminated.parse2(tokens.clone())
}

#[cfg(test)]
mod tests {
    use super::{OpeoArgs, WrapperAttributeFilter, parse_meta_list, wrapper_meta};
    use quote::quote;
    use syn::{Attribute, Meta, Result, parse_quote, parse2};

    fn ok_or_return<T>(result: Result<T>) -> Option<T> {
        assert!(result.is_ok(), "expected successful parser result");
        result.ok()
    }

    fn err_or_return<T>(result: Result<T>) -> Option<syn::Error> {
        assert!(result.is_err(), "expected parser error");
        result.err()
    }

    fn list_or_return(meta: Option<&Meta>) -> Option<&syn::MetaList> {
        match meta {
            Some(Meta::List(list)) => Some(list),
            _ => None,
        }
    }

    #[test]
    fn wrapper_drops_symbol_attributes_and_keeps_other_attributes() {
        let attributes: Vec<Attribute> = vec![
            parse_quote!(#[unsafe(export_name = "stable_name")]),
            parse_quote!(#[unsafe(no_mangle)]),
            parse_quote!(#[inline]),
            parse_quote!(#[custom::instrument]),
        ];

        let requested = vec![parse_quote!(custom::instrument)];
        let Some(filtered) = ok_or_return(WrapperAttributeFilter::select(&attributes, &requested))
        else {
            return;
        };

        assert_eq!(filtered.len(), 2);
        assert!(
            filtered
                .first()
                .is_some_and(|attribute| attribute.path().is_ident("inline"))
        );
        let selected_path = filtered.get(1).map(|attribute| {
            let path = attribute.path();
            quote!(#path).to_string()
        });
        assert_eq!(selected_path.as_deref(), Some("custom :: instrument"));
    }
    #[test]
    fn wrapper_drops_lint_relaxation_metadata() {
        let metadata: Meta = parse_quote!(allow(dead_code));

        let result = wrapper_meta(&metadata, &[]);
        assert!(result.is_ok());
        assert!(result.is_ok_and(|meta| meta.is_none()));
    }
    #[test]
    fn wrapper_keeps_only_selected_function_attributes() {
        let attributes: Vec<Attribute> = vec![
            parse_quote!(#[cfg(unix)]),
            parse_quote!(#[doc = "generated wrapper"]),
            parse_quote!(#[custom::instrument]),
            parse_quote!(#[cfg_attr(feature = "fast", custom::marker, cold)]),
        ];

        let Some(filtered) = ok_or_return(WrapperAttributeFilter::select(&attributes, &[])) else {
            return;
        };

        assert_eq!(filtered.len(), 3);
        assert!(
            filtered
                .first()
                .is_some_and(|attribute| attribute.path().is_ident("cfg"))
        );
        assert!(
            filtered
                .get(1)
                .is_some_and(|attribute| attribute.path().is_ident("doc"))
        );
        assert!(
            filtered
                .get(2)
                .is_some_and(|attribute| matches!(attribute.meta, Meta::List(_)))
        );
        let Some(list) = list_or_return(filtered.get(2).map(|attribute| &attribute.meta)) else {
            return;
        };
        let Some(nested) = ok_or_return(parse_meta_list(&list.tokens)) else {
            return;
        };
        assert_eq!(nested.len(), 2);
        assert!(
            nested
                .iter()
                .any(|meta| { matches!(meta, Meta::Path(path) if path.is_ident("cold")) })
        );
        assert!(
            !nested
                .iter()
                .any(|meta| { matches!(meta, Meta::List(list) if list.path.is_ident("custom")) })
        );
    }
    #[test]
    fn wrapper_selects_nested_attributes_inside_cfg_attr() {
        let attributes: Vec<Attribute> = vec![parse_quote!(
            #[cfg_attr(feature = "fast", tracing::instrument, custom::marker)]
        )];
        let requested = vec![parse_quote!(tracing::instrument)];

        let Some(filtered) = ok_or_return(WrapperAttributeFilter::select(&attributes, &requested))
        else {
            return;
        };

        assert!(
            filtered
                .first()
                .is_some_and(|attribute| matches!(attribute.meta, Meta::List(_)))
        );
        let Some(list) = list_or_return(filtered.first().map(|attribute| &attribute.meta)) else {
            return;
        };
        let Some(nested) = ok_or_return(parse_meta_list(&list.tokens)) else {
            return;
        };
        assert_eq!(nested.len(), 2);
        assert!(
            nested
                .iter()
                .any(|meta| { quote!(#meta).to_string() == "tracing :: instrument" })
        );
        assert!(
            !nested
                .iter()
                .any(|meta| { quote!(#meta).to_string() == "custom :: marker" })
        );
    }
    #[test]
    fn wrapper_filters_symbol_attributes_inside_cfg_attr() {
        let attributes: Vec<Attribute> = vec![parse_quote!(
            #[cfg_attr(
                target_os = "linux",
                export_name = "conditional_name",
                unsafe(no_mangle),
                inline
            )]
        )];

        let Some(filtered) = ok_or_return(WrapperAttributeFilter::select(&attributes, &[])) else {
            return;
        };

        assert!(
            filtered
                .first()
                .is_some_and(|attribute| matches!(attribute.meta, Meta::List(_)))
        );
        let Some(list) = list_or_return(filtered.first().map(|attribute| &attribute.meta)) else {
            return;
        };
        let Some(nested) = ok_or_return(parse_meta_list(&list.tokens)) else {
            return;
        };
        assert_eq!(nested.len(), 2);
        assert!(
            nested
                .iter()
                .any(|meta| { matches!(meta, Meta::Path(path) if path.is_ident("inline")) })
        );
        assert!(
            !nested
                .iter()
                .any(|meta| { matches!(meta, Meta::List(list) if list.path.is_ident("unsafe")) })
        );
    }
    #[test]
    fn attribute_arguments_require_both_alias_types() {
        let Some(error) = err_or_return(parse2::<OpeoArgs>(quote!(wrapper = wrapped, ok = u8)))
        else {
            return;
        };

        assert!(
            error.to_string().contains("must be specified together"),
            "unexpected parser error: {error}"
        );
    }

    #[test]
    fn attribute_arguments_allow_default_wrapper_and_selected_attributes() {
        let Some(arguments) = ok_or_return(parse2::<OpeoArgs>(quote!(wrapper_attrs(
            tracing::instrument
        )))) else {
            return;
        };

        assert!(matches!(arguments.wrapper, super::WrapperConfig::Default));
        assert_eq!(arguments.wrapper_attribute_paths().len(), 1);
        let path = arguments.wrapper_attribute_paths().first();
        assert_eq!(
            path.map(|path| quote!(#path).to_string()).as_deref(),
            Some("tracing :: instrument")
        );
    }

    #[test]
    fn attribute_arguments_reject_symbol_attribute_copying() {
        let Some(error) = err_or_return(parse2::<OpeoArgs>(quote!(wrapper_attrs(export_name))))
        else {
            return;
        };

        assert!(
            error
                .to_string()
                .contains("symbol attributes cannot be copied")
        );
    }
}
