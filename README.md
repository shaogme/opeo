# opeo

`opeo` stores errors in caller-owned storage and returns successful values directly. A lifetime-branded `Failed` value proves that an error was written to the matching slot. The runtime is `no_std` and does not allocate for error storage.

[简体中文 README](README_CN.md) · [Advanced guide](docs/advanced.md) · [进阶指南](docs/advanced_CN.md)

## Install

```toml
[dependencies]
opeo = "1"
```

The crate uses Rust 2024 and requires Rust 1.85 or newer.

## Quick start

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

An OPEO function returns `OResult<'slot, T>`, which is an alias for `Result<T, Failed<'slot>>`. Write a failure with `out.fail(error)`. Convert a regular `Result` with `store_err`, `store_err_with`, or `store_err_into`, then use `?`. Convert `Option` with standard `ok_or(error)` before calling the same methods. A nested OPEO call using the same slot propagates with native `?`.

`ErrSlot::call` returns a standard `Result<T, E>`. For asynchronous calls and borrowed error inspection, see the [advanced guide](docs/advanced.md).

## License

This project is dual-licensed under MIT or Apache-2.0.
