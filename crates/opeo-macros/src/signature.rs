use proc_macro2::Span;
use quote::format_ident;
use syn::{
    Block, Error, ExprAsync, ExprClosure, FnArg, GenericParam, Ident, Item, ItemFn, Lifetime, Pat,
    PatIdent, Path, Result, ReturnType, Signature, Token, Type, parse_quote,
    punctuated::Punctuated,
    visit_mut::{self, VisitMut},
};

use crate::attributes::{OpeoArgs, WrapperConfig};

pub(super) struct SignaturePlan {
    original_output: ReturnType,
    source_result_type: Type,
    ok_type: Type,
    error_type: Type,
    wrapper_name: Option<Ident>,
    is_async: bool,
    is_const: bool,
    is_method: bool,
    slot_lifetime: Lifetime,
    borrow_lifetime: Lifetime,
}

impl SignaturePlan {
    pub(super) fn analyze(
        arguments: &OpeoArgs,
        function: &mut ItemFn,
        runtime_path: &Path,
    ) -> Result<Self> {
        let wrapper_name = match arguments.wrapper_config() {
            WrapperConfig::Default => Some(default_wrapper_name(&function.sig.ident)),
            WrapperConfig::Named(name) => Some(name.clone()),
            WrapperConfig::Disabled => None,
        };
        let is_async = function.sig.asyncness.is_some();
        let is_const = function.sig.constness.is_some();
        if function.sig.variadic.is_some() || function.sig.abi.is_some() {
            return Err(Error::new_spanned(
                function.sig.clone(),
                "#[opeo] does not support variadic or extern functions",
            ));
        }

        let original_output = function.sig.output.clone();
        let (explicit_ok_type, explicit_error_type) = arguments.explicit_types();
        let (ok_type, error_type) = result_types(
            &original_output,
            explicit_ok_type,
            explicit_error_type,
            runtime_path,
        )?;
        validate_input_bindings(&function.sig.inputs)?;
        let argument_count = validate_forwarded_arguments(&function.sig.inputs)?;
        validate_out_bindings(&mut function.block)?;
        if let Some(wrapper_name) = &wrapper_name {
            validate_wrapper_name(&function.sig.ident, wrapper_name)?;
        }
        if argument_count > 6 {
            return Err(Error::new_spanned(
                &function.sig.inputs,
                "#[opeo] functions and methods may have at most six explicit input parameters",
            ));
        }

        let slot_lifetime = fresh_lifetime(function, "__opeo_slot");
        let borrow_lifetime = fresh_lifetime(function, "__opeo_borrow");
        let is_method = function
            .sig
            .inputs
            .iter()
            .any(|input| matches!(input, FnArg::Receiver(_)));
        let source_result_type = match &original_output {
            ReturnType::Type(_, output_type) => output_type.as_ref().clone(),
            ReturnType::Default => {
                return Err(Error::new_spanned(
                    &original_output,
                    "#[opeo] requires a Result<T, E> return type",
                ));
            }
        };

        Ok(Self {
            original_output,
            source_result_type,
            ok_type,
            error_type,
            wrapper_name,
            is_async,
            is_const,
            is_method,
            slot_lifetime,
            borrow_lifetime,
        })
    }

    pub(super) fn original_output(&self) -> &ReturnType {
        &self.original_output
    }

    pub(super) fn source_result_type(&self) -> &Type {
        &self.source_result_type
    }

    pub(super) fn error_type(&self) -> &Type {
        &self.error_type
    }

    pub(super) fn wrapper_name(&self) -> Option<&Ident> {
        self.wrapper_name.as_ref()
    }

    pub(super) fn is_async(&self) -> bool {
        self.is_async
    }

    pub(super) fn is_const(&self) -> bool {
        self.is_const
    }

    pub(super) fn is_method(&self) -> bool {
        self.is_method
    }

    pub(super) fn slot_lifetime(&self) -> &Lifetime {
        &self.slot_lifetime
    }

    pub(super) fn borrow_lifetime(&self) -> &Lifetime {
        &self.borrow_lifetime
    }

    pub(super) fn prepare_opeo_signature(&self, function: &mut ItemFn, runtime_path: &Path) {
        let opeo_doc = match &self.wrapper_name {
            Some(wrapper_name) => format!(
                "The OPEO form accepts `Out` and returns `OResult`; see [`{wrapper_name}`] for the standard wrapper.\nOPEO 版本接收 `Out` 并返回 `OResult`；标准包装函数见 [`{wrapper_name}`]。"
            ),
            None => "The OPEO form accepts `Out` and returns `OResult`; no standard wrapper is generated.\nOPEO 版本接收 `Out` 并返回 `OResult`；不会生成标准包装函数。".to_owned(),
        };
        function.attrs.push(parse_quote!(#[doc = #opeo_doc]));
        let slot_lifetime = &self.slot_lifetime;
        let borrow_lifetime = &self.borrow_lifetime;
        function
            .sig
            .generics
            .params
            .insert(0, GenericParam::Lifetime(parse_quote!(#slot_lifetime)));
        function
            .sig
            .generics
            .params
            .insert(1, GenericParam::Lifetime(parse_quote!(#borrow_lifetime)));
        if self.is_const {
            function.sig.constness = None;
        }
        let ok_type = &self.ok_type;
        let error_type = &self.error_type;
        function.sig.output = parse_quote!(-> #runtime_path::OResult<#slot_lifetime, #ok_type>);
        function.sig.inputs.push(parse_quote!(
            mut out: #runtime_path::Out<#slot_lifetime, #borrow_lifetime, #error_type>
        ));
    }

    pub(super) fn wrapper_argument_idents(signature: &Signature) -> Vec<Ident> {
        wrapper_argument_idents(signature)
    }

    pub(super) fn fresh_local_ident(signature: &Signature, base: &str) -> Ident {
        fresh_local_ident(signature, base)
    }
}

fn result_types(
    output: &ReturnType,
    explicit_ok_type: Option<Type>,
    explicit_error_type: Option<Type>,
    runtime_path: &Path,
) -> Result<(Type, Type)> {
    if let (Some(ok_type), Some(error_type)) = (explicit_ok_type, explicit_error_type) {
        let ReturnType::Type(_, output_type) = output else {
            return Err(Error::new_spanned(
                output,
                "#[opeo] requires a Result<T, E> return type or a result alias",
            ));
        };
        if !matches!(output_type.as_ref(), Type::Path(_)) {
            return Err(Error::new_spanned(
                output_type,
                "explicit `ok` and `error` types require a path return type",
            ));
        }
        return Ok((ok_type, error_type));
    }

    let ReturnType::Type(_, output_type) = output else {
        return Err(Error::new_spanned(
            output,
            "#[opeo] requires a Result<T, E> return type",
        ));
    };
    if !matches!(output_type.as_ref(), Type::Path(_)) {
        return Err(Error::new_spanned(
            output_type,
            "#[opeo] requires a path return type that resolves to `Result<T, E>`",
        ));
    }

    let is_result_path = match output_type.as_ref() {
        Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Result"),
        _ => false,
    };
    if !is_result_path {
        return Err(Error::new_spanned(
            output_type,
            "`#[opeo]` cannot infer the success and error types from this path; use a return path whose final segment is named `Result`, or specify both `ok = Type` and `error = Type`.",
        ));
    }

    let success_type: Type = parse_quote!(
        <#output_type as #runtime_path::__OpeoResultParts>::Success
    );
    let error_type: Type = parse_quote!(
        <#output_type as #runtime_path::__OpeoResultParts>::Error
    );
    Ok((success_type, error_type))
}

fn validate_wrapper_name(original_name: &Ident, wrapper_name: &Ident) -> Result<()> {
    if original_name == wrapper_name {
        return Err(Error::new_spanned(
            wrapper_name,
            "the generated standard wrapper must have a different name from the OPEO function",
        ));
    }
    Ok(())
}

fn default_wrapper_name(function_name: &Ident) -> Ident {
    let name = function_name.to_string();
    let name = name.trim_start_matches("r#");
    format_ident!("{}_std", name, span = function_name.span())
}

fn validate_out_bindings(block: &mut Block) -> Result<()> {
    let mut validator = OutBindingValidator { error: None };
    validator.visit_block_mut(block);
    validator.error.map_or(Ok(()), Err)
}

fn validate_input_bindings(inputs: &Punctuated<FnArg, Token![,]>) -> Result<()> {
    let mut validator = OutBindingValidator { error: None };
    for input in inputs {
        if let FnArg::Typed(argument) = input {
            let mut pattern = argument.pat.as_ref().clone();
            validator.visit_pat_mut(&mut pattern);
        }
    }
    validator.error.map_or(Ok(()), Err)
}

struct OutBindingValidator {
    error: Option<Error>,
}

impl VisitMut for OutBindingValidator {
    fn visit_pat_ident_mut(&mut self, pattern: &mut PatIdent) {
        if self.error.is_none() && ident_matches(&pattern.ident, "out") {
            self.error = Some(Error::new(
                pattern.ident.span(),
                "`out` is reserved by #[opeo] in the generated function",
            ));
        }
        visit_mut::visit_pat_ident_mut(self, pattern);
    }

    fn visit_expr_closure_mut(&mut self, _expression: &mut ExprClosure) {}

    fn visit_expr_async_mut(&mut self, _expression: &mut ExprAsync) {}

    fn visit_item_mut(&mut self, _item: &mut Item) {}
}

fn validate_forwarded_arguments(inputs: &Punctuated<FnArg, Token![,]>) -> Result<usize> {
    let mut argument_count = 0;
    for (index, input) in inputs.iter().enumerate() {
        match input {
            FnArg::Receiver(receiver) if index != 0 => {
                return Err(Error::new_spanned(
                    receiver,
                    "a method receiver must be the first #[opeo] parameter",
                ));
            }
            FnArg::Receiver(_) => {}
            FnArg::Typed(_) => argument_count += 1,
        }
    }
    Ok(argument_count)
}

fn ident_matches(ident: &Ident, expected: &str) -> bool {
    ident.to_string().trim_start_matches("r#") == expected
}

fn fresh_local_ident(signature: &Signature, base: &str) -> Ident {
    let mut suffix = 0;

    loop {
        let candidate = if suffix == 0 {
            base.to_owned()
        } else {
            format!("{base}_{suffix}")
        };
        let collides = signature.inputs.iter().any(|input| match input {
            FnArg::Typed(argument) => match argument.pat.as_ref() {
                Pat::Ident(pattern) => ident_matches(&pattern.ident, &candidate),
                _ => false,
            },
            FnArg::Receiver(_) => false,
        }) || signature
            .generics
            .params
            .iter()
            .any(|parameter| match parameter {
                GenericParam::Type(parameter) => parameter.ident == candidate,
                GenericParam::Const(parameter) => parameter.ident == candidate,
                GenericParam::Lifetime(_) => false,
            });

        if !collides {
            return format_ident!("{candidate}");
        }

        suffix += 1;
    }
}

fn wrapper_argument_idents(signature: &Signature) -> Vec<Ident> {
    let reserved_arguments = signature
        .inputs
        .iter()
        .filter_map(|input| match input {
            FnArg::Typed(argument) => match argument.pat.as_ref() {
                Pat::Ident(pattern) if pattern.by_ref.is_none() && pattern.subpat.is_none() => {
                    Some(pattern.ident.clone())
                }
                _ => None,
            },
            FnArg::Receiver(_) => None,
        })
        .collect::<Vec<_>>();
    let mut arguments = Vec::with_capacity(signature.inputs.len());
    for (index, input) in signature
        .inputs
        .iter()
        .filter(|input| matches!(input, FnArg::Typed(_)))
        .enumerate()
    {
        let simple_ident = match input {
            FnArg::Typed(argument) => match argument.pat.as_ref() {
                Pat::Ident(pattern) if pattern.by_ref.is_none() && pattern.subpat.is_none() => {
                    Some(pattern.ident.clone())
                }
                _ => None,
            },
            FnArg::Receiver(_) => None,
        };
        if let Some(ident) = simple_ident {
            arguments.push(ident);
            continue;
        }

        let base = format!("__opeo_arg_{index}");
        let mut name = base.clone();
        let mut suffix = 0;
        while reserved_arguments.iter().any(|argument| argument == &name)
            || arguments.iter().any(|argument| argument == &name)
            || signature
                .generics
                .params
                .iter()
                .any(|parameter| match parameter {
                    GenericParam::Type(parameter) => parameter.ident == name,
                    GenericParam::Const(parameter) => parameter.ident == name,
                    GenericParam::Lifetime(_) => false,
                })
        {
            suffix += 1;
            name = format!("{base}_{suffix}");
        }
        arguments.push(format_ident!("{name}"));
    }
    arguments
}

fn fresh_lifetime(function: &ItemFn, base: &str) -> Lifetime {
    let mut function = function.clone();
    let mut collector = LifetimeCollector { names: Vec::new() };
    collector.visit_item_fn_mut(&mut function);
    let mut name = base.to_owned();
    let mut suffix = 0;
    while collector.names.iter().any(|lifetime| lifetime == &name) {
        suffix += 1;
        name = format!("{base}_{suffix}");
    }
    Lifetime::new(&format!("'{name}"), Span::call_site())
}

struct LifetimeCollector {
    names: Vec<String>,
}

impl VisitMut for LifetimeCollector {
    fn visit_lifetime_mut(&mut self, lifetime: &mut Lifetime) {
        self.names.push(lifetime.ident.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        default_wrapper_name, result_types, validate_out_bindings, validate_wrapper_name,
        wrapper_argument_idents,
    };
    use quote::quote;
    use syn::{Result, parse_quote};

    fn ok_or_return<T>(result: Result<T>) -> Option<T> {
        assert!(result.is_ok(), "expected successful parser result");
        result.ok()
    }

    fn err_or_return<T>(result: Result<T>) -> Option<syn::Error> {
        assert!(result.is_err(), "expected parser error");
        result.err()
    }

    #[test]
    fn result_types_uses_associated_types_for_qualified_result_paths() {
        let output = parse_quote!(-> core::result::Result<u32, ParseError>);
        let runtime_path = parse_quote!(::opeo);

        let Some((success_type, error_type)) =
            ok_or_return(result_types(&output, None, None, &runtime_path))
        else {
            return;
        };

        assert_eq!(
            quote!(#success_type).to_string(),
            "< core :: result :: Result < u32 , ParseError > as :: opeo :: __OpeoResultParts > :: Success"
        );
        assert_eq!(
            quote!(#error_type).to_string(),
            "< core :: result :: Result < u32 , ParseError > as :: opeo :: __OpeoResultParts > :: Error"
        );
    }

    #[test]
    fn result_types_requires_explicit_types_for_aliases() {
        let output = parse_quote!(-> MyResult);
        let error = result_types(&output, None, None, &parse_quote!(::opeo));
        let Some(error) = err_or_return(error) else {
            return;
        };

        assert!(
            error
                .to_string()
                .contains("final segment is named `Result`")
        );
    }

    #[test]
    fn result_types_uses_explicit_types_for_opaque_aliases() {
        let output = parse_quote!(-> MyResult);

        let Some((ok_type, error_type)) = ok_or_return(result_types(
            &output,
            Some(parse_quote!(Option<u8>)),
            Some(parse_quote!(CustomError)),
            &parse_quote!(::opeo),
        )) else {
            return;
        };

        assert_eq!(quote!(#ok_type).to_string(), "Option < u8 >");
        assert_eq!(quote!(#error_type).to_string(), "CustomError");
    }

    #[test]
    fn result_types_accepts_explicit_types_for_aliases() {
        let output = parse_quote!(-> std::io::Result<u8>);

        let Some((ok_type, error_type)) = ok_or_return(result_types(
            &output,
            Some(parse_quote!(u8)),
            Some(parse_quote!(std::io::Error)),
            &parse_quote!(::opeo),
        )) else {
            return;
        };

        assert_eq!(quote!(#ok_type).to_string(), "u8");
        assert_eq!(quote!(#error_type).to_string(), "std :: io :: Error");
    }
    #[test]
    fn default_wrapper_name_appends_std() {
        let function = parse_quote!(parse);

        assert_eq!(default_wrapper_name(&function).to_string(), "parse_std");
    }
    #[test]
    fn wrapper_arguments_preserve_names_and_avoid_collisions() {
        let signature = parse_quote!(
            fn parse(value: u8, (left, right): (u8, u8), __opeo_arg_1: bool)
        );
        let arguments = wrapper_argument_idents(&signature);

        assert_eq!(
            arguments
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["value", "__opeo_arg_1_1", "__opeo_arg_1"]
        );
    }
    #[test]
    fn wrapper_name_must_differ_from_original_name() {
        let original = parse_quote!(parse);
        let wrapper = parse_quote!(parse);

        let Some(error) = err_or_return(validate_wrapper_name(&original, &wrapper)) else {
            return;
        };

        assert!(error.to_string().contains("must have a different name"));
    }
    #[test]
    fn out_binding_is_rejected_in_rewritten_scopes() {
        let mut block = parse_quote!({
            let (value, out) = (1, 2);
            let _ = value;
            Ok::<(), ()>(())
        });

        let Some(error) = err_or_return(validate_out_bindings(&mut block)) else {
            return;
        };

        assert!(error.to_string().contains("`out` is reserved"));
    }

    #[test]
    fn out_bindings_inside_unrewritten_closures_are_allowed() {
        let mut block = parse_quote!({
            let closure = |out| out;
            let _ = closure(1);
            Ok::<(), ()>(())
        });

        assert!(validate_out_bindings(&mut block).is_ok());
    }
}
