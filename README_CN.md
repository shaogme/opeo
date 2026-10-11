# opeo

`opeo` 是一个 Rust 错误处理库：错误写入由调用方持有的存储，成功值则直接返回。失败凭证带有生命周期品牌，因此每次失败都绑定到实际接收错误的槽位。

运行时使用 `no_std`，错误槽不需要堆分配。`OResult<'slot, T>` 是 `Result<T, Failed<'slot>>` 的别名。`Failed` 的构造器是私有的；安全代码只能通过 `Out::fail` 或 `OutEdit::commit` 获得失败凭证。

[English README](README.md)

## 工作方式

OPEO 函数接收 `Out` 句柄，并返回错误类型绑定到该槽位的标准 `Result`：

```text
调用方持有 ErrSlot<E>
       │ 借出 Out<'slot, E>
       ▼
OPEO 函数返回 Result<T, Failed<'slot>>
       │ 失败时，E 写入槽位，Failed<'slot> 证明该槽位已写入错误
       ▼
调用方收到 T，或处理槽位中的错误
```

`ErrSlot::call` 会将 OPEO 函数桥接回 `Result<T, E>`。`ErrSlot::try_call` 则返回 `Caught` 借用，供调用方检查或取走错误。异步函数可通过 `call_async` 桥接，也可用 `try_call_async` 检查借用错误。

成功时返回 `Ok(value)`，失败时返回 `Err(out.fail(error))`。调用同槽位的 OPEO 函数时，可用原生 `?` 传播结果。普通 `Result<T, E>` 必须先通过 `ResultOutExt` 或 `opeo_try!` 显式转换。

## 安装

将运行时 crate 加入依赖：

```toml
[dependencies]
opeo = "0.2"
```

工作区使用 Rust 2024，并将 Rust 1.85 声明为最低版本。

## 编写 OPEO 函数

```rust
use opeo::{bail, ensure, ErrSlot, OResult, Out, ResultOutExt};

#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    Empty,
    InvalidNumber,
    Zero,
}

fn parse_positive<'slot, 'borrow>(
    input: &str,
    mut out: Out<'slot, 'borrow, ParseError>,
) -> OResult<'slot, u32> {
    if input.is_empty() {
        bail!(out, ParseError::Empty);
    }

    let value = input
        .parse::<u32>()
        .map_err(|_| ParseError::InvalidNumber)
        .or_out_into(out.reborrow())?;
    ensure!(out, value > 0, ParseError::Zero);
    Ok(value)
}

let mut errors = ErrSlot::<ParseError>::new();
assert_eq!(errors.call(|out| parse_positive("42", out)), Ok(42));
assert_eq!(errors.call(|out| parse_positive("0", out)), Err(ParseError::Zero));
```

`ResultOutExt::or_out` 将同类型错误写入槽位，`or_out_with` 使用转换闭包，`or_out_into` 使用 `From<E>` 转换。每个方法都会返回 `OResult`，之后可用 `?` 传播带品牌的失败凭证。

传播 `Option<T>` 时，使用 `opeo_try!(out, option, error)` 显式提供错误。两参数形式 `opeo_try!(out, result)` 用于桥接普通 `Result`；同槽位 OPEO 结果应使用原生 `?`。

## 传播同槽位结果

该别名按普通 `Result` 工作，同时 `Failed<'slot>` 会在错误类型中保留槽位品牌：

```rust
use opeo::{OResult, Out};

struct ParseError;

fn parse_leaf<'slot, 'borrow>(out: Out<'slot, 'borrow, ParseError>) -> OResult<'slot, u32> {
    Err(out.fail(ParseError))
}

fn parse_nested<'slot, 'borrow>(mut out: Out<'slot, 'borrow, ParseError>) -> OResult<'slot, u32> {
    let value = parse_leaf(out.reborrow())?;
    Ok(value)
}
```

不同槽位的失败凭证无法传播，因为它的生命周期与返回类型中的槽位品牌不匹配。

## 编写标准 `Result` 包装器

需要标准 `Result` 接口时，可使用 `ErrSlot::call`：

```rust
use opeo::{ErrSlot, OResult, Out, ResultOutExt};

#[derive(Debug, PartialEq, Eq)]
struct ParseError;

fn parse_positive<'slot, 'borrow>(
    input: &str,
    mut out: Out<'slot, 'borrow, ParseError>,
) -> OResult<'slot, u32> {
    let value = input
        .parse::<u32>()
        .map_err(|_| ParseError)
        .or_out(out.reborrow())?;
    Ok(value)
}

fn parse_positive_std(input: &str) -> Result<u32, ParseError> {
    let mut errors = ErrSlot::<ParseError>::new();
    errors.call(|out| parse_positive(input, out))
}

assert_eq!(parse_positive_std("17"), Ok(17));
```

## 方法、trait 与异步函数

OPEO 函数就是普通 Rust 函数、方法和 trait 方法，使用相同的 `Out` 与 `OResult` 签名以及标准 `Result` 操作：

```rust
use opeo::{OResult, Out};

struct Meter(u32);
struct ParseError;

impl Meter {
    fn add<'slot, 'borrow>(
        &self,
        amount: u32,
        _out: Out<'slot, 'borrow, ParseError>,
    ) -> OResult<'slot, u32> {
        Ok(self.0 + amount)
    }
}
```

`async fn` 可以使用 `.await` 并照常返回 `OResult`。调用时使用 `ErrSlot::call_async` 或 `try_call_async`。

## 运行时 API

| 项目 | 用途 |
| --- | --- |
| `ErrSlot<E>` | 持有错误存储。用 `call`/`call_async` 返回标准 `Result`，或用 `try_call`/`try_call_async` 获取借用错误 `Caught`。 |
| `Out<'slot, 'borrow, E>` | 用 `fail` 写入错误，用 `edit` 编辑待提交错误，用 `reborrow` 借出嵌套句柄。 |
| `OutEdit<'slot, 'borrow, E>` | 借用或修改槽位错误；`commit` 生成失败凭证，未提交时丢弃会恢复先前错误。 |
| `OResult<'slot, T>` | `Result<T, Failed<'slot>>` 的别名。 |
| `Failed<'slot>` | 由 `Out::fail` 或 `OutEdit::commit` 返回、无法由外部构造的凭证。 |
| `Caught<'slot, E>` | 借用槽位中的错误；用 `get`/`get_mut` 检查，用 `take` 取走。 |
| `ResultOutExt` | 使用 `or_out`、`or_out_with` 或 `or_out_into` 转换普通 `Result`。 |
| `bail!` | 写入错误并从 OPEO 函数提前返回。 |
| `ensure!` | 条件为假时写入错误并提前返回。 |
| `opeo_try!` | 桥接普通 `Result`，或为 `Option` 显式提供错误。 |

`Out::edit` 可以在提交槽位错误前补充上下文：

```rust
use opeo::{ErrSlot, OResult};

#[derive(Debug, PartialEq, Eq)]
struct ContextError(Option<&'static str>);

let mut errors = ErrSlot::<ContextError>::new();
let result = errors.call(|out| {
    let mut edit = out.edit(ContextError(None));
    edit.get_mut().0 = Some("header");
    Err::<(), _>(edit.commit())
});
assert_eq!(result, Err(ContextError(Some("header"))));
```

## 0.2 版本的变更

`OResult<'slot, T>` 现在是 `Result<T, Failed<'slot>>` 的类型别名；`OResult::success`、`OResult::failed` 和 `into_result()` 已移除。成功时返回 `Ok(value)`，失败时返回 `Err(out.fail(error))`。

`#[opeo]` 属性宏和隐藏的 `__OpeoTry` 桥接接口已移除。普通 `Result` 错误需通过 `ResultOutExt` 或 `opeo_try!` 显式转换；同槽位 OPEO 结果使用原生 `?`。`opeo-macros` crate 已从工作区移除。

## 许可证

本项目采用 MIT 或 Apache-2.0 双许可证。
