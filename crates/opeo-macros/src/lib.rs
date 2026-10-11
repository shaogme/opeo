use proc_macro::TokenStream as ProcMacroTokenStream;
use proc_macro2::TokenStream;
use syn::parse_macro_input;

mod attributes;
mod expansion;
mod rewrite;
mod signature;

/// Generates an OPEO function or method, including trait methods, and a standard wrapper from a `Result` function.
/// 从标准 `Result` 函数或方法（包括 trait 方法）生成 OPEO 入口和标准包装入口。
/// Generic error types are preserved, so `E: From<SourceError>` can adapt errors to the caller's slot type.
/// 泛型错误类型会被保留，因此可用 `E: From<SourceError>` 将错误转换为调用方的槽位类型。
///
/// `wrapper` defaults to `<function>_std`; set it to `false` to omit the wrapper. Use
/// `wrapper_attrs(...)` to copy selected non-symbol attributes to the generated wrapper.
/// `wrapper` 默认生成为 `<函数名>_std`；设为 `false` 可关闭包装函数。可用 `wrapper_attrs(...)` 选择复制到包装函数的非符号属性。
#[proc_macro_attribute]
pub fn opeo(attribute: ProcMacroTokenStream, item: ProcMacroTokenStream) -> ProcMacroTokenStream {
    let attribute = parse_macro_input!(attribute as attributes::OpeoArgs);
    let item = TokenStream::from(item);

    match expansion::Expansion::expand(attribute, item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}
