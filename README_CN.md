# opeo

`opeo` 是一个 Rust 错误处理库：函数把错误值写入由调用方持有的存储，同时单独返回成功值。失败凭证带有类型化的槽位品牌，因此不能把一个槽位上的失败传播到另一个调用的槽位。

运行时 crate 使用 `no_std`。配套的 `opeo-macros` crate 提供 `#[opeo]` 属性宏，可将常规 `Result` 函数转换为 OPEO 函数，并默认生成供常规调用方使用的 `Result` 包装函数。

[English README](README.md)

## 工作方式

OPEO 函数接收 `Out` 句柄并返回 `OResult`：

```text
调用方持有 ErrSlot<E>
       │ 借出 Out<'slot, E>
       ▼
OPEO 函数返回 OResult<'slot, T>
       │ 失败时，E 存在槽位中，Failed<'slot> 证明该槽位已写入错误
       ▼
调用方收到成功值 T，或处理槽位中的错误
```

`OResult` 保存成功值或零大小的失败标记；错误本身写入 `ErrSlot<E>`。生命周期品牌将失败标记与实际收到错误的槽位关联起来。`ErrSlot::call` 将该接口桥接回 `Result<T, E>`；`ErrSlot::try_call` 则返回 `Caught` 借用，供调用方检查或取走槽位中的错误。

`OResult` 提供 `is_ok`、`is_err`、`map` 和 `and_then`，便于检查状态并组合成功路径；`and_then` 的后续计算必须返回绑定到同一槽位的 `OResult`。异步调用也可以用 `ErrSlot::try_call_async` 检查借用错误。

`Out::edit` 会将错误写入槽位并返回 `OutEdit`。编辑器可借用或修改错误；只有 `commit` 会生成 `Failed`。如果编辑器在提交前被丢弃，槽位会恢复为编辑开始前的值。

实现不会为错误槽分配堆内存，运行时也不依赖 `std`。错误类型 `E` 自身如何存储由它决定，因此它仍可能分配内存。在受测配置中，`OResult<T>` 与 `Result<T, ()>` 布局相同；调用方应依赖公开 API，而不要依赖私有表示细节。

## 安装

将运行时 crate 加入依赖即可。过程宏由 `opeo` 重新导出，无需直接依赖 `opeo-macros`：

```toml
[dependencies]
opeo = "0.1"
```

工作区使用 Rust 2024，并将 Rust 1.85 声明为最低版本。

## 使用 `#[opeo]` 快速开始

像往常一样编写返回 `Result<T, E>` 的函数，并添加 `#[opeo]`。宏会保留该函数作为 OPEO 入口，改写可失败的操作，并默认生成名为 `<函数名>_std` 的标准包装函数。只需要 OPEO 入口时，可设置 `wrapper = false` 关闭包装函数生成。

```rust
use opeo::{opeo, ErrSlot};

#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    Empty,
    InvalidNumber,
    Zero,
}

#[opeo]
fn parse_positive(input: &str) -> Result<u32, ParseError> {
    if input.is_empty() {
        return Err(ParseError::Empty);
    }

    let value = input
        .parse::<u32>()
        .map_err(|_| ParseError::InvalidNumber)?;
    if value == 0 {
        return Err(ParseError::Zero);
    }

    Ok(value)
}

let mut errors = ErrSlot::<ParseError>::new();
assert_eq!(errors.call(|out| parse_positive("42", out)), Ok(42));
assert_eq!(parse_positive_std("0"), Err(ParseError::Zero));
```

生成的 `parse_positive_std` 保留原有的 `Result<u32, ParseError>` 接口。如果调用方已经持有 `ErrSlot` 并希望走 OPEO 路径，可以直接调用 `parse_positive`。

## 检查或取走错误

失败时，`try_call` 返回 `Caught`。`get` 借用错误，`get_mut` 可变借用错误，`take` 将错误从槽位中移出。如果丢弃 `Caught` 时尚未取走错误，槽位中的错误也会被析构。异步 OPEO 调用可使用 `try_call_async` 获得相同的检查能力。

```rust
use opeo::{opeo, ErrSlot};

#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    InvalidNumber,
}

#[opeo]
fn parse_number(input: &str) -> Result<u32, ParseError> {
    input
        .parse::<u32>()
        .map_err(|_| ParseError::InvalidNumber)
}

let mut errors = ErrSlot::<ParseError>::new();
let caught = errors
    .try_call(|out| parse_number("not a number", out))
    .unwrap_err();
assert_eq!(caught.get(), &ParseError::InvalidNumber);
assert_eq!(caught.take(), ParseError::InvalidNumber);
```

## 手写 OPEO 函数

如果不使用属性宏，也可以直接使用 `Out`、`OResult` 和 `opeo_try!` 定义 OPEO 形式。该类 `?` 风格宏接收 `Result`；其源错误类型必须能转换为槽位错误类型，也可以先用 `map_err` 转换。

```rust
use opeo::{ErrSlot, OResult, Out, opeo_try};

#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    InvalidNumber,
}

fn parse_number<'slot, 'borrow>(
    input: &str,
    mut out: Out<'slot, 'borrow, ParseError>,
) -> OResult<'slot, u32> {
    let value = opeo_try!(
        out,
        input
            .parse::<u32>()
            .map_err(|_| ParseError::InvalidNumber)
    );
    OResult::success(value)
}

let mut errors = ErrSlot::<ParseError>::new();
assert_eq!(errors.call(|out| parse_number("17", out)), Ok(17));
```

`Out::reborrow` 会在当前句柄的可变借用内创建生命周期更短的句柄，可用于嵌套 OPEO 调用。这样调用方可以传播同一槽位上的 `OResult`，而借用检查器会阻止混用不同槽位的失败状态。

需要在提交前补充错误上下文时，可通过 `Out::edit` 原位修改槽位中的错误：

```rust
use opeo::{ErrSlot, OResult};

#[derive(Debug, PartialEq, Eq)]
struct ParseError {
    context: Option<&'static str>,
}

let mut errors = ErrSlot::<ParseError>::new();
let result = errors.call(|out| {
    let mut edit = out.edit(ParseError { context: None });
    edit.get_mut().context = Some("header");
    OResult::<()>::failed(edit.commit())
});

assert_eq!(result, Err(ParseError { context: Some("header") }));
```

## 公开 API

| 项目 | 用途 |
| --- | --- |
| `ErrSlot<E>` | 持有错误存储。用 `call`/`call_async` 返回标准 `Result`，或用 `try_call`/`try_call_async` 获取借用错误 `Caught`。 |
| `Out<'slot, 'borrow, E>` | 用 `fail` 写入并提交错误，用 `edit` 原位编辑待提交错误，用 `reborrow` 创建嵌套句柄。 |
| `OutEdit<'slot, 'borrow, E>` | 借用或修改槽位中的错误；用 `commit` 生成失败凭证，未提交时丢弃会恢复原值。 |
| `OResult<'slot, T>` | 返回成功值，或返回绑定到某个槽位的失败状态；支持状态检查和成功值组合。 |
| `Failed<'slot>` | 由 `Out::fail` 或 `OutEdit::commit` 生成并用于构造失败结果的凭证。 |
| `Caught<'slot, E>` | 借用已存储的错误；用 `get`/`get_mut` 检查或修改，用 `take` 获取所有权。 |
| `ResultOutExt` | 使用 `or_out`、`or_out_with` 或 `or_out_into` 将标准 `Result` 转成 `OResult`。 |
| `bail!` | 写入错误并从 OPEO 函数提前返回。 |
| `ensure!` | 条件为假时写入错误并提前返回。 |
| `opeo_try!` | 传播 `Result`、同槽位的 `OResult`，或带有显式错误值的 `Option`。 |

例如，可以在手写的 OPEO 函数中使用 `ResultOutExt` 桥接标准结果：

```rust
use opeo::{ErrSlot, ResultOutExt};

let mut errors = ErrSlot::<&str>::new();
let result = errors.call(|out| Err::<u32, _>("invalid input").or_out(out));
assert_eq!(result, Err("invalid input"));
```

导入时，`bail!` 也可以重命名为 `abort!`；`opeo_try!` 同样可以使用其他导入名称。

## 属性选项与 `Result` 别名

可以显式指定包装函数名称：

```rust
use opeo::{opeo, ErrSlot};

#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    InvalidNumber,
}

#[opeo(wrapper = parse_standard)]
fn parse(input: &str) -> Result<u32, ParseError> {
    input
        .parse::<u32>()
        .map_err(|_| ParseError::InvalidNumber)
}

let mut errors = ErrSlot::<ParseError>::new();
assert_eq!(errors.call(|out| parse("8", out)), Ok(8));
assert_eq!(parse_standard("9"), Ok(9));
```

如果只需要 OPEO 入口，可以显式关闭包装函数生成：

```rust
use opeo::{opeo, ErrSlot};

#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    InvalidNumber,
}

#[opeo(wrapper = false)]
fn parse(input: &str) -> Result<u32, ParseError> {
    input
        .parse::<u32>()
        .map_err(|_| ParseError::InvalidNumber)
}

let mut errors = ErrSlot::<ParseError>::new();
assert_eq!(errors.call(|out| parse("12", out)), Ok(12));
```

此时宏保留 `parse` 作为 OPEO 入口，不会生成默认的 `parse_std`。调用方可通过 `ErrSlot::call` 调用它并取得标准 `Result`。

如果返回类型是一个类型别名，且其路径最后一段不叫 `Result`，请同时提供成功类型和错误类型，以便宏确定 OPEO 函数签名：

```rust
use opeo::{opeo, ErrSlot};

#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    InvalidNumber,
}

type ParseResult = Result<u32, ParseError>;

#[opeo(ok = u32, error = ParseError)]
fn parse(input: &str) -> ParseResult {
    input
        .parse::<u32>()
        .map_err(|_| ParseError::InvalidNumber)
}

let mut errors = ErrSlot::<ParseError>::new();
assert_eq!(errors.call(|out| parse("8", out)), Ok(8));
```

`wrapper_attrs(path, ...)` 用于选择要复制到生成包装函数的自定义函数属性。`cfg`、`doc`、`deprecated`、`inline`、`cold`、`must_use` 和 `track_caller` 等标准属性会自动复制。会更改符号的属性不会复制到包装函数，因此原函数保留其符号属性，不会产生重复符号。

## 结构体方法

可以在固有 `impl` 块的实例方法上使用 `#[opeo]`。宏会保留该方法作为 OPEO 入口，并默认生成一个使用相同接收者的标准包装方法。包装方法默认命名为 `<方法名>_std`，也可以通过 `wrapper = ...` 指定名称，或通过 `wrapper = false` 关闭生成。异步方法会生成异步 OPEO 方法；启用包装函数时还会生成异步标准包装方法。生成 `const` 方法的包装函数时会保留 `const`；由于 OPEO 入口需要写入错误槽，因此它只能在运行时调用。

```rust
use core::convert::Infallible;
use opeo::{opeo, ErrSlot};

struct Counter {
    value: u32,
}

impl Counter {
    #[opeo]
    fn add(&self, amount: u32) -> Result<u32, Infallible> {
        Ok(self.value + amount)
    }
}

let counter = Counter { value: 5 };
assert_eq!(counter.add_std(3), Ok(8));

let mut errors = ErrSlot::<Infallible>::new();
assert_eq!(errors.call(|out| counter.add(4, out)), Ok(9));
```

## 支持范围与错误转换

`#[opeo]` 支持自由函数和固有 `impl` 块中的实例方法，包括异步和 `const` 函数及方法。返回类型必须是可解析为 `Result<T, E>` 的路径，或通过 `ok` 和 `error` 显式指定类型。生成异步包装器时，它会返回一个 Future；`ErrSlot::call_async` 可将异步 OPEO 调用桥接回 `Result<T, E>`。生成 `const` 函数或方法的包装器时，会保留其 `const`；OPEO 入口会失去 `const`，因为它需要写入运行时错误槽。函数或方法最多有六个显式输入参数；宏会为 OPEO 形式添加 `Out` 参数。`extern` 和可变参数函数不受支持。

函数体中的问号传播会将源错误写入槽位。对于 `Result<T, SourceError>`，槽位错误类型必须实现 `From<SourceError>`；需要自定义转换时使用 `map_err`。嵌套闭包会保留原本的 `?` 行为。对于 `Option<T>`，使用 `opeo_try!(out, option, error_value)` 显式提供错误值。

宏在转换后的函数体和生成参数列表中保留名称 `out` 作为内部用途。普通参数和局部变量请使用其他名称。

## Crate 与许可证

- [`opeo`](https://crates.io/crates/opeo) 包含 `no_std` 运行时、公开类型和导出宏。
- [`opeo-macros`](https://crates.io/crates/opeo-macros) 包含过程宏实现，并由 `opeo` 重新导出。

本项目采用 MIT 或 Apache-2.0 双许可证。
