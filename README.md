# opeo

`opeo` is a Rust error-handling library that stores errors in caller-owned storage while returning success values directly. A lifetime-branded failure proof ties each failed result to the slot that received its error.

The runtime is `no_std` and has no heap allocation for the error slot. `OResult<'slot, T>` is an alias for `Result<T, Failed<'slot>>`. The `Failed` constructor is private: safe code obtains a failure proof through `Out::fail` or `OutEdit::commit`.

[简体中文 README](README_CN.md)

## How it works

An OPEO function receives an `Out` handle and returns a standard `Result` whose error is branded for that slot:

```text
caller owns ErrSlot<E>
       │ lends Out<'slot, E>
       ▼
OPEO function returns Result<T, Failed<'slot>>
       │ on failure, E is stored in the slot and Failed<'slot> proves it
       ▼
caller receives T or handles the stored error
```

`ErrSlot::call` bridges an OPEO function back to `Result<T, E>`. `ErrSlot::try_call` instead returns a `Caught` borrow so the caller can inspect or take the error. Async functions can be bridged with `call_async` or inspected with `try_call_async`.

Use `Ok(value)` for success and `Err(out.fail(error))` for failure. A nested OPEO function using the same slot can be propagated with native `?`. A regular `Result<T, E>` must first be converted explicitly with `ResultOutExt` or `opeo_try!`.

## Install

Add the runtime crate to your package:

```toml
[dependencies]
opeo = "0.2"
```

The workspace uses Rust 2024 and declares Rust 1.85 as its minimum version.

## Write an OPEO function

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

`ResultOutExt::or_out` stores an error of the same type, `or_out_with` applies a conversion closure, and `or_out_into` uses `From<E>`. Each method returns `OResult`, so `?` then propagates the branded failure.

For an `Option<T>`, supply the error explicitly with `opeo_try!(out, option, error)`. The two-argument `opeo_try!(out, result)` form bridges a standard `Result`; same-slot OPEO results should use native `?`.

## Propagate a same-slot result

The alias behaves as a normal `Result`, while `Failed<'slot>` keeps the slot brand in the error type:

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

A failure from a different slot cannot be propagated because its `Failed` lifetime does not match the return type.

## Write a standard `Result` wrapper

Use `ErrSlot::call` wherever a standard `Result` interface is useful:

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

## Methods, traits, and async functions

OPEO functions are ordinary Rust functions, methods, and trait methods. They use the same `Out` and `OResult` signature and standard `Result` operations:

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

An `async fn` can use `.await` and return `OResult` as usual. Call it with `ErrSlot::call_async` or `try_call_async`.

## Runtime API

| Item | Purpose |
| --- | --- |
| `ErrSlot<E>` | Owns error storage. Use `call`/`call_async` for a standard `Result`, or `try_call`/`try_call_async` for a borrowed `Caught` error. |
| `Out<'slot, 'borrow, E>` | Writes errors with `fail`, edits a pending error with `edit`, and lends nested handles with `reborrow`. |
| `OutEdit<'slot, 'borrow, E>` | Borrows or modifies the stored error; `commit` produces its failure proof. Dropping it uncommitted restores the previous error. |
| `OResult<'slot, T>` | Alias for `Result<T, Failed<'slot>>`. |
| `Failed<'slot>` | Private-construction proof returned by `Out::fail` or `OutEdit::commit`. |
| `Caught<'slot, E>` | Borrows the stored error; `get`/`get_mut` inspect it and `take` moves it out. |
| `ResultOutExt` | Converts a standard `Result` with `or_out`, `or_out_with`, or `or_out_into`. |
| `bail!` | Writes an error and returns early from an OPEO function. |
| `ensure!` | Returns early with an error when a condition is false. |
| `opeo_try!` | Bridges a standard `Result`, or an `Option` with an explicit error. |

`Out::edit` can add context before committing the stored error:

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

## Changes in 0.2

`OResult<'slot, T>` is now a type alias for `Result<T, Failed<'slot>>`; `OResult::success`, `OResult::failed`, and `into_result()` were removed. Return `Ok(value)` or `Err(out.fail(error))` instead.

The `#[opeo]` attribute macro and hidden `__OpeoTry` bridge were removed. Convert standard `Result` errors explicitly with `ResultOutExt` or `opeo_try!`, and use native `?` for a same-slot OPEO result. The `opeo-macros` crate is no longer part of the workspace.

## License

This project is dual-licensed under MIT or Apache-2.0.
