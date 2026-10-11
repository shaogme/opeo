# opeo

`opeo` 将错误存入调用方持有的存储，并直接返回成功值。带生命周期品牌的 `Failed` 凭证证明错误已写入对应槽位。运行时使用 `no_std`，错误存储无需堆分配。

[English README](README.md) · [进阶指南](docs/advanced_CN.md) · [Advanced guide](docs/advanced.md)

## 安装

```toml
[dependencies]
opeo = "1"
```

crate 使用 Rust 2024，最低支持 Rust 1.85。

## 快速上手

```rust
use opeo::{ErrSlot, OResult, Out, ResultOutExt};

#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    InvalidNumber,
    Zero,
}

fn parse_positive<'slot>(
    input: &str,
    mut out: Out<'slot, '_, ParseError>,
) -> OResult<'slot, u32> {
    let value = input
        .parse::<u32>()
        .store_err_with(out.reborrow(), |_| ParseError::InvalidNumber)?;

    if value == 0 {
        return Err(out.fail(ParseError::Zero));
    }

    Ok(value)
}

fn parse_positive_std(input: &str) -> Result<u32, ParseError> {
    let mut errors = ErrSlot::<ParseError>::new();
    errors.call(|out| parse_positive(input, out))
}

assert_eq!(parse_positive_std("42"), Ok(42));
assert_eq!(parse_positive_std("0"), Err(ParseError::Zero));
assert_eq!(parse_positive_std("invalid"), Err(ParseError::InvalidNumber));
```

OPEO 函数返回 `OResult<'slot, T>`，它是 `Result<T, Failed<'slot>>` 的别名。使用 `out.fail(error)` 写入失败。普通 `Result` 通过 `store_err`、`store_err_with` 或 `store_err_into` 转换，然后使用 `?`。`Option` 先通过标准库的 `ok_or(error)` 转换，再使用相同方法。同槽位的嵌套 OPEO 调用可用原生 `?` 传播。

`ErrSlot::call` 返回标准 `Result<T, E>`。异步调用和借用错误检查请参阅[进阶指南](docs/advanced_CN.md)。

## 许可证

本项目采用 MIT 或 Apache-2.0 双许可证。
