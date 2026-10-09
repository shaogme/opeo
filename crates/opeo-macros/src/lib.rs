use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use quote::{format_ident, quote};
use syn::{
    Attribute, Block, Error, Expr, ExprBlock, ExprBreak, ExprCall, ExprForLoop, ExprLoop,
    ExprMacro, ExprPath, ExprWhile, FnArg, GenericParam, Ident, ItemFn, Lifetime, LitBool, Meta,
    Pat, PatIdent, Path, ReturnType, Stmt, StmtMacro, Token, TraitItemFn, Type,
    parse::{Parse, ParseStream, Parser},
    parse_macro_input, parse_quote,
    punctuated::Punctuated,
    visit_mut::{self, VisitMut},
};

/// Generates an OPEO function or method, including trait methods, and a standard wrapper from a `Result` function.
/// 从标准 `Result` 函数或方法（包括 trait 方法）生成 OPEO 入口和标准包装入口。
/// Generic error types are preserved, so `E: From<SourceError>` can adapt errors to the caller's slot type.
/// 泛型错误类型会被保留，因此可用 `E: From<SourceError>` 将错误转换为调用方的槽位类型。
///
/// `wrapper` defaults to `<function>_std`; set it to `false` to omit the wrapper. Use
/// `wrapper_attrs(...)` to copy selected non-symbol attributes to the generated wrapper.
/// `wrapper` 默认生成为 `<函数名>_std`；设为 `false` 可关闭包装函数。可用 `wrapper_attrs(...)` 选择复制到包装函数的非符号属性。
#[proc_macro_attribute]
pub fn opeo(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let attribute = parse_macro_input!(attribute as OpeoArgs);
    let item = proc_macro2::TokenStream::from(item);

    if let Ok(function) = syn::parse2::<ItemFn>(item.clone()) {
        return match expand_opeo(attribute, function) {
            Ok(tokens) => tokens.into(),
            Err(error) => error.into_compile_error().into(),
        };
    }

    match syn::parse2::<TraitItemFn>(item)
        .and_then(|function| expand_trait_opeo(attribute, function))
    {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

struct OpeoArgs {
    wrapper: WrapperConfig,
    ok_type: Option<Type>,
    error_type: Option<Type>,
    wrapper_attributes: Vec<Path>,
}

enum WrapperConfig {
    Default,
    Named(Ident),
    Disabled,
}

impl Parse for OpeoArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
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
                proc_macro2::Span::call_site(),
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

fn expand_opeo(attribute: OpeoArgs, function: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let runtime_path = opeo_crate_path()?;
    expand_opeo_with_path(attribute, function, runtime_path)
}

fn expand_trait_opeo(
    attribute: OpeoArgs,
    trait_function: TraitItemFn,
) -> syn::Result<proc_macro2::TokenStream> {
    let runtime_path = opeo_crate_path()?;
    let has_default = trait_function.default.is_some();
    let function = ItemFn {
        attrs: trait_function.attrs,
        vis: syn::Visibility::Inherited,
        sig: trait_function.sig,
        block: Box::new(trait_function.default.unwrap_or_else(|| parse_quote!({}))),
    };
    let expansion = expand_opeo_with_path(attribute, function, runtime_path)?;
    let generated: syn::File = syn::parse2(expansion)?;
    let mut trait_items = Vec::with_capacity(generated.items.len());

    for (index, item) in generated.items.into_iter().enumerate() {
        let syn::Item::Fn(function) = item else {
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
) -> syn::Result<proc_macro2::TokenStream> {
    let original_function = function.clone();
    let wrapper_name = match attribute.wrapper {
        WrapperConfig::Default => Some(default_wrapper_name(&function.sig.ident)),
        WrapperConfig::Named(name) => Some(name),
        WrapperConfig::Disabled => None,
    };
    let requested_wrapper_attributes = attribute.wrapper_attributes;
    let is_async = function.sig.asyncness.is_some();
    let is_const = function.sig.constness.is_some();
    if function.sig.variadic.is_some() || function.sig.abi.is_some() {
        return Err(Error::new_spanned(
            function.sig,
            "#[opeo] does not support variadic or extern functions",
        ));
    }

    let original_output = function.sig.output.clone();
    let (ok_type, error_type) = result_types(
        &original_output,
        attribute.ok_type,
        attribute.error_type,
        &runtime_path,
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
    let slot_lifetime = fresh_lifetime(&function, "__opeo_slot");
    let borrow_lifetime = fresh_lifetime(&function, "__opeo_borrow");
    let is_method = function
        .sig
        .inputs
        .iter()
        .any(|input| matches!(input, FnArg::Receiver(_)));
    let opeo_doc = match &wrapper_name {
        Some(wrapper_name) => format!(
            "The OPEO form accepts `Out` and returns `OResult`; see [`{wrapper_name}`] for the standard wrapper.\nOPEO 版本接收 `Out` 并返回 `OResult`；标准包装函数见 [`{wrapper_name}`]。"
        ),
        None => "The OPEO form accepts `Out` and returns `OResult`; no standard wrapper is generated.\nOPEO 版本接收 `Out` 并返回 `OResult`；不会生成标准包装函数。".to_owned(),
    };
    function.attrs.push(parse_quote!(#[doc = #opeo_doc]));
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
    if is_const {
        function.sig.constness = None;
    }
    function.sig.output = parse_quote!(-> #runtime_path::OResult<#slot_lifetime, #ok_type>);
    let mut visitor = RewriteTry::new(runtime_path.clone());
    visitor.visit_function_body(&mut function.block);
    function
        .block
        .stmts
        .insert(0, parse_quote!(let _ = &mut out;));
    function.sig.inputs.push(parse_quote!(
        mut out: #runtime_path::Out<#slot_lifetime, #borrow_lifetime, #error_type>
    ));

    let wrapper = if let Some(wrapper_name) = wrapper_name {
        let wrapper_attributes =
            wrapper_attributes(&original_function.attrs, &requested_wrapper_attributes)?;
        let mut wrapper_signature = if is_const {
            let mut signature = original_function.sig.clone();
            signature.ident = wrapper_name.clone();
            signature
        } else {
            let mut signature = function.sig.clone();
            signature.ident = wrapper_name;
            signature.generics.params = signature
                .generics
                .params
                .into_iter()
                .filter(|parameter| match parameter {
                    GenericParam::Lifetime(lifetime) => {
                        lifetime.lifetime != slot_lifetime && lifetime.lifetime != borrow_lifetime
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
            signature.output = original_output;
            signature
        };

        let call_arguments = wrapper_argument_idents(&wrapper_signature);
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

        let original_name = &function.sig.ident;
        let wrapper_doc = format!(
            "Standard `Result` wrapper; see [`{original_name}`] for the OPEO form.\n标准 `Result` 包装函数；OPEO 版本见 [`{original_name}`]。"
        );
        let call_arguments = call_arguments.iter();
        let call_target = if is_method {
            quote!(self.#original_name)
        } else {
            quote!(#original_name)
        };
        let call = if wrapper_signature.unsafety.is_some() {
            quote!(unsafe { #call_target(#(#call_arguments,)* out) })
        } else {
            quote!(#call_target(#(#call_arguments,)* out))
        };
        let call = if is_async { quote!(#call.await) } else { call };
        let slot_ident = fresh_local_ident(&wrapper_signature, "__opeo_err_slot");
        let wrapper_body = if is_const {
            let body = &original_function.block;
            quote!(#body)
        } else if is_async {
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

        quote! {
            #(#wrapper_attributes)*
            #[doc = #wrapper_doc]
            #wrapper_signature
            #wrapper_body
        }
    } else {
        quote!()
    };

    Ok(quote! {
        #function
        #wrapper
    })
}

fn result_types(
    output: &ReturnType,
    explicit_ok_type: Option<Type>,
    explicit_error_type: Option<Type>,
    runtime_path: &Path,
) -> syn::Result<(Type, Type)> {
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

fn validate_wrapper_name(original_name: &syn::Ident, wrapper_name: &syn::Ident) -> syn::Result<()> {
    if original_name == wrapper_name {
        return Err(Error::new_spanned(
            wrapper_name,
            "the generated standard wrapper must have a different name from the OPEO function",
        ));
    }
    Ok(())
}

fn default_wrapper_name(function_name: &syn::Ident) -> syn::Ident {
    let name = function_name.to_string();
    let name = name.trim_start_matches("r#");
    format_ident!("{}_std", name, span = function_name.span())
}

fn validate_out_bindings(block: &mut Block) -> syn::Result<()> {
    let mut validator = OutBindingValidator { error: None };
    validator.visit_block_mut(block);
    validator.error.map_or(Ok(()), Err)
}

fn validate_input_bindings(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::Token![,]>,
) -> syn::Result<()> {
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

    fn visit_expr_closure_mut(&mut self, _expression: &mut syn::ExprClosure) {}

    fn visit_expr_async_mut(&mut self, _expression: &mut syn::ExprAsync) {}

    fn visit_item_mut(&mut self, _item: &mut syn::Item) {}
}

fn validate_forwarded_arguments(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::Token![,]>,
) -> syn::Result<usize> {
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

fn fresh_local_ident(signature: &syn::Signature, base: &str) -> syn::Ident {
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

fn wrapper_argument_idents(signature: &syn::Signature) -> Vec<Ident> {
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

fn fresh_lifetime(function: &ItemFn, base: &str) -> syn::Lifetime {
    let mut function = function.clone();
    let mut collector = LifetimeCollector { names: Vec::new() };
    collector.visit_item_fn_mut(&mut function);
    let mut name = base.to_owned();
    let mut suffix = 0;
    while collector.names.iter().any(|lifetime| lifetime == &name) {
        suffix += 1;
        name = format!("{base}_{suffix}");
    }
    syn::Lifetime::new(&format!("'{name}"), proc_macro2::Span::call_site())
}

struct LifetimeCollector {
    names: Vec<String>,
}

impl VisitMut for LifetimeCollector {
    fn visit_lifetime_mut(&mut self, lifetime: &mut Lifetime) {
        self.names.push(lifetime.ident.to_string());
    }
}

struct RewriteTry {
    runtime_path: Path,
    rewrite_question_marks: bool,
}

impl VisitMut for RewriteTry {
    fn visit_expr_mut(&mut self, expression: &mut Expr) {
        if let Expr::Try(try_expression) = expression
            && self.rewrite_question_marks
        {
            self.visit_expr_mut(&mut try_expression.expr);
            let inner = &try_expression.expr;
            let runtime_path = &self.runtime_path;
            *expression = parse_quote!(#runtime_path::__opeo_try_value!(out, #inner));
            return;
        }
        visit_mut::visit_expr_mut(self, expression);
    }

    fn visit_expr_closure_mut(&mut self, _expression: &mut syn::ExprClosure) {}

    fn visit_expr_async_mut(&mut self, _expression: &mut syn::ExprAsync) {}

    fn visit_expr_try_block_mut(&mut self, expression: &mut syn::ExprTryBlock) {
        let rewrite_question_marks = self.rewrite_question_marks;
        self.rewrite_question_marks = false;
        self.visit_block_mut(&mut expression.block);
        self.rewrite_question_marks = rewrite_question_marks;
    }

    fn visit_expr_return_mut(&mut self, expression: &mut syn::ExprReturn) {
        if let Some(value) = &mut expression.expr {
            self.rewrite_result_tail(value);
            self.visit_expr_mut(value);
        }
    }

    fn visit_item_mut(&mut self, _item: &mut syn::Item) {}
}

impl RewriteTry {
    fn new(runtime_path: Path) -> Self {
        Self {
            runtime_path,
            rewrite_question_marks: true,
        }
    }

    fn visit_function_body(&mut self, block: &mut Block) {
        self.rewrite_block_tail(block);
        self.visit_block_mut(block);
    }

    fn rewrite_block_tail(&mut self, block: &mut Block) {
        if let Some(Stmt::Expr(expression, _)) = block.stmts.last_mut() {
            self.rewrite_result_tail(expression);
        }
    }

    fn rewrite_result_tail(&mut self, expression: &mut Expr) {
        match expression {
            Expr::Call(call)
                if is_result_variant_call(call, "Err") || is_result_variant_call(call, "Ok") =>
            {
                self.rewrite_result_call(expression);
            }
            Expr::Macro(macro_expression)
                if macro_expression
                    .mac
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "__opeo_try_value") =>
            {
                self.rewrite_result_expression(expression);
            }
            Expr::Macro(macro_expression)
                if macro_expression
                    .mac
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "opeo_try") =>
            {
                let runtime_path = &self.runtime_path;
                let value = expression.clone();
                *expression = parse_quote!(#runtime_path::OResult::success(#value));
            }
            Expr::Macro(macro_expression) if is_diverging_macro(macro_expression) => {}
            Expr::Block(block) => self.rewrite_block_tail(&mut block.block),
            Expr::If(if_expression) => {
                self.rewrite_block_tail(&mut if_expression.then_branch);
                if let Some((_, else_expression)) = &mut if_expression.else_branch {
                    self.rewrite_result_tail(else_expression);
                }
            }
            Expr::Match(match_expression) => {
                for arm in &mut match_expression.arms {
                    self.rewrite_result_tail(&mut arm.body);
                }
            }
            Expr::Paren(paren) => self.rewrite_result_tail(&mut paren.expr),
            Expr::Group(group) => self.rewrite_result_tail(&mut group.expr),
            Expr::Unsafe(unsafe_expression) => {
                self.rewrite_block_tail(&mut unsafe_expression.block);
            }
            Expr::Loop(loop_expression) if !loop_has_exit(loop_expression) => {}
            Expr::Break(break_expression) => {
                if let Some(value) = &mut break_expression.expr {
                    self.rewrite_result_tail(value);
                }
            }
            Expr::Continue(_) => {}
            Expr::Return(_) => {}
            _ => self.rewrite_result_expression(expression),
        }
    }

    fn rewrite_result_expression(&self, expression: &mut Expr) {
        let runtime_path = &self.runtime_path;
        let result = expression.clone();
        *expression = parse_quote!(#runtime_path::__OpeoTry::__opeo_try(
            #result,
            out.reborrow(),
        ));
    }

    fn rewrite_result_call(&mut self, expression: &mut Expr) {
        let Expr::Call(call) = expression else {
            return;
        };
        if call.args.len() != 1 {
            return;
        }

        let is_error = is_result_variant_call(call, "Err");
        let Some(value) = call.args.pop() else {
            return;
        };
        let runtime_path = &self.runtime_path;
        if is_error {
            *expression = parse_quote!(#runtime_path::OResult::failed(
                out.reborrow().fail(#value)
            ));
        } else {
            *expression = parse_quote!(#runtime_path::OResult::success(#value));
        }
    }
}

fn loop_has_exit(loop_expression: &ExprLoop) -> bool {
    let target_label = loop_expression
        .label
        .as_ref()
        .map(|label| label.name.ident.clone());
    let mut visitor = LoopExitVisitor {
        target_label,
        nested_loop_depth: 0,
        nested_labels: Vec::new(),
        found: false,
    };
    let mut body = loop_expression.body.clone();
    visitor.visit_block_mut(&mut body);
    visitor.found
}

struct LoopExitVisitor {
    target_label: Option<Ident>,
    nested_loop_depth: usize,
    nested_labels: Vec<Ident>,
    found: bool,
}

impl VisitMut for LoopExitVisitor {
    fn visit_expr_break_mut(&mut self, expression: &mut ExprBreak) {
        let exits_target = match &expression.label {
            Some(label) => {
                self.target_label.as_ref() == Some(&label.ident)
                    && !self.nested_labels.contains(&label.ident)
            }
            None => self.nested_loop_depth == 0,
        };
        self.found |= exits_target;
        if let Some(value) = &mut expression.expr {
            self.visit_expr_mut(value);
        }
    }

    fn visit_expr_loop_mut(&mut self, expression: &mut ExprLoop) {
        self.visit_nested_loop(expression.label.as_ref().map(|label| &label.name.ident));
        visit_mut::visit_expr_loop_mut(self, expression);
        self.nested_loop_depth -= 1;
        if expression.label.is_some() {
            self.nested_labels.pop();
        }
    }

    fn visit_expr_for_loop_mut(&mut self, expression: &mut ExprForLoop) {
        self.visit_nested_loop(expression.label.as_ref().map(|label| &label.name.ident));
        visit_mut::visit_expr_for_loop_mut(self, expression);
        self.nested_loop_depth -= 1;
        if expression.label.is_some() {
            self.nested_labels.pop();
        }
    }

    fn visit_expr_while_mut(&mut self, expression: &mut ExprWhile) {
        self.visit_nested_loop(expression.label.as_ref().map(|label| &label.name.ident));
        visit_mut::visit_expr_while_mut(self, expression);
        self.nested_loop_depth -= 1;
        if expression.label.is_some() {
            self.nested_labels.pop();
        }
    }

    fn visit_expr_block_mut(&mut self, expression: &mut ExprBlock) {
        if let Some(label) = &expression.label {
            self.nested_labels.push(label.name.ident.clone());
            visit_mut::visit_expr_block_mut(self, expression);
            self.nested_labels.pop();
        } else {
            visit_mut::visit_expr_block_mut(self, expression);
        }
    }

    fn visit_expr_closure_mut(&mut self, _expression: &mut syn::ExprClosure) {}

    fn visit_expr_async_mut(&mut self, _expression: &mut syn::ExprAsync) {}

    fn visit_expr_macro_mut(&mut self, _expression: &mut ExprMacro) {
        self.found = true;
    }

    fn visit_stmt_macro_mut(&mut self, _statement: &mut StmtMacro) {
        self.found = true;
    }

    fn visit_item_mut(&mut self, _item: &mut syn::Item) {}
}

impl LoopExitVisitor {
    fn visit_nested_loop(&mut self, label: Option<&Ident>) {
        self.nested_loop_depth += 1;
        if let Some(label) = label {
            self.nested_labels.push(label.clone());
        }
    }
}

fn is_diverging_macro(expression: &ExprMacro) -> bool {
    expression.mac.path.segments.last().is_some_and(|segment| {
        matches!(
            segment.ident.to_string().as_str(),
            "panic" | "unreachable" | "todo" | "unimplemented"
        )
    })
}

fn opeo_crate_path() -> syn::Result<Path> {
    match crate_name("opeo") {
        Ok(FoundCrate::Itself) => Ok(parse_quote!(::opeo)),
        Ok(FoundCrate::Name(name)) => {
            let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
            Ok(parse_quote!(::#ident))
        }
        Err(error) => Err(Error::new(
            proc_macro2::Span::call_site(),
            format!("could not resolve the `opeo` crate: {error}"),
        )),
    }
}

fn is_result_variant_call(call: &ExprCall, variant: &str) -> bool {
    let Expr::Path(ExprPath {
        qself: None, path, ..
    }) = call.func.as_ref()
    else {
        return false;
    };
    path.segments
        .last()
        .is_some_and(|segment| segment.ident == variant)
}

fn wrapper_attributes(attributes: &[Attribute], requested: &[Path]) -> syn::Result<Vec<Attribute>> {
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

fn wrapper_meta(meta: &Meta, requested: &[Path]) -> syn::Result<Option<Meta>> {
    match meta {
        Meta::List(list) if list.path.is_ident("cfg_attr") => {
            let mut nested = parse_meta_list(&list.tokens)?.into_iter();
            let Some(condition) = nested.next() else {
                return Ok(Some(meta.clone()));
            };
            let retained = nested
                .map(|meta| wrapper_meta(&meta, requested))
                .collect::<syn::Result<Vec<_>>>()?
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

fn parse_meta_list(tokens: &proc_macro2::TokenStream) -> syn::Result<Punctuated<Meta, Token![,]>> {
    Punctuated::<Meta, Token![,]>::parse_terminated.parse2(tokens.clone())
}

#[cfg(test)]
mod tests {
    use super::{
        Attribute, Meta, OpeoArgs, RewriteTry, expand_opeo_with_path, is_diverging_macro,
        loop_has_exit, parse_meta_list, result_types, validate_out_bindings, validate_wrapper_name,
        wrapper_attributes, wrapper_meta,
    };
    use quote::quote;
    use syn::{Expr, ExprLoop, ExprMacro, parse_quote};

    fn ok_or_return<T>(result: syn::Result<T>) -> Option<T> {
        assert!(result.is_ok(), "expected successful parser result");
        result.ok()
    }

    fn err_or_return<T>(result: syn::Result<T>) -> Option<syn::Error> {
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
    fn loop_exit_detection_only_counts_breaks_that_exit_target() {
        let loop_with_inner_break: ExprLoop = parse_quote!(loop {
            while true {
                break;
            }
        });
        let loop_with_exit: ExprLoop = parse_quote!(loop {
            if should_stop {
                break Ok::<u32, ParseError>(1);
            }
        });
        let loop_with_macro_exit: ExprLoop = parse_quote!(loop {
            if should_stop {
                break_loop_with_result!(Ok::<u32, ParseError>(1));
            }
        });
        let labeled_loop_with_exit: ExprLoop = parse_quote!('outer: loop {
            loop {
                break 'outer Ok(1);
            }
        });

        assert!(!loop_has_exit(&loop_with_inner_break));
        assert!(loop_has_exit(&loop_with_exit));
        assert!(loop_has_exit(&loop_with_macro_exit));
        assert!(loop_has_exit(&labeled_loop_with_exit));
    }

    #[test]
    fn tail_rewriting_recurses_through_parentheses_and_leaves_diverging_macros_alone() {
        let mut rewrite = RewriteTry::new(parse_quote!(::opeo));
        let mut parenthesized_tail: Expr = parse_quote!((Ok(7)));
        rewrite.rewrite_result_tail(&mut parenthesized_tail);
        let rewritten = quote!(#parenthesized_tail).to_string();
        assert!(rewritten.contains("OResult :: success (7)"));

        let panic_macro: ExprMacro = parse_quote!(panic!("stop"));
        let custom_macro: ExprMacro = parse_quote!(custom::never_returns!());
        assert!(is_diverging_macro(&panic_macro));
        assert!(!is_diverging_macro(&custom_macro));
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
        let Some(filtered) = ok_or_return(wrapper_attributes(&attributes, &requested)) else {
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

        let Some(filtered) = ok_or_return(wrapper_attributes(&attributes, &[])) else {
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

        let Some(filtered) = ok_or_return(wrapper_attributes(&attributes, &requested)) else {
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

        let Some(filtered) = ok_or_return(wrapper_attributes(&attributes, &[])) else {
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
    fn generated_functions_have_distinct_documentation() {
        let Some(attribute) = ok_or_return(syn::parse2::<OpeoArgs>(quote!(wrapper = parse_std)))
        else {
            return;
        };
        let function: syn::ItemFn = parse_quote! {
            /// User-provided function documentation.
            /// 用户提供的函数说明。
            pub fn parse(input: &str) -> Result<u32, ParseError> {
                Ok(input.len() as u32)
            }
        };
        let Some(expansion) = ok_or_return(expand_opeo_with_path(
            attribute,
            function,
            parse_quote!(::opeo),
        )) else {
            return;
        };
        let Some(file) = ok_or_return(syn::parse2::<syn::File>(expansion)) else {
            return;
        };
        let original = file.items.iter().find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "parse" => Some(function),
            _ => None,
        });
        let wrapper = file.items.iter().find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "parse_std" => Some(function),
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
        let Some(attribute) = ok_or_return(syn::parse2::<OpeoArgs>(quote!(wrapper = false))) else {
            return;
        };
        let function: syn::ItemFn = parse_quote! {
            pub fn parse(input: &str) -> Result<u32, ParseError> {
                Ok(input.len() as u32)
            }
        };
        let Some(expansion) = ok_or_return(expand_opeo_with_path(
            attribute,
            function,
            parse_quote!(::opeo),
        )) else {
            return;
        };
        let Some(file) = ok_or_return(syn::parse2::<syn::File>(expansion)) else {
            return;
        };

        assert_eq!(file.items.len(), 1);
        let Some(syn::Item::Fn(function)) = file.items.first() else {
            return;
        };
        let docs = doc_attribute_values(function);
        assert!(docs.contains("no standard wrapper is generated"));
        assert!(!docs.contains("see [`"));
    }

    fn doc_attribute_values(function: &syn::ItemFn) -> String {
        function
            .attrs
            .iter()
            .filter_map(|attribute| match &attribute.meta {
                Meta::NameValue(value) if value.path.is_ident("doc") => match &value.value {
                    syn::Expr::Lit(expression) => match &expression.lit {
                        syn::Lit::Str(value) => Some(value.value()),
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn attribute_arguments_require_both_alias_types() {
        let Some(error) =
            err_or_return(syn::parse2::<OpeoArgs>(quote!(wrapper = wrapped, ok = u8)))
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
        let Some(arguments) = ok_or_return(syn::parse2::<OpeoArgs>(quote!(wrapper_attrs(
            tracing::instrument
        )))) else {
            return;
        };

        assert!(matches!(arguments.wrapper, super::WrapperConfig::Default));
        assert_eq!(arguments.wrapper_attributes.len(), 1);
        let path = arguments.wrapper_attributes.first();
        assert_eq!(
            path.map(|path| quote!(#path).to_string()).as_deref(),
            Some("tracing :: instrument")
        );
    }

    #[test]
    fn attribute_arguments_reject_symbol_attribute_copying() {
        let Some(error) =
            err_or_return(syn::parse2::<OpeoArgs>(quote!(wrapper_attrs(export_name))))
        else {
            return;
        };

        assert!(
            error
                .to_string()
                .contains("symbol attributes cannot be copied")
        );
    }

    #[test]
    fn default_wrapper_name_appends_std() {
        let function = parse_quote!(parse);

        assert_eq!(
            super::default_wrapper_name(&function).to_string(),
            "parse_std"
        );
    }

    #[test]
    fn wrapper_arguments_preserve_names_and_avoid_collisions() {
        let signature = parse_quote!(
            fn parse(value: u8, (left, right): (u8, u8), __opeo_arg_1: bool)
        );
        let arguments = super::wrapper_argument_idents(&signature);

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
