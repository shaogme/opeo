# opeo

`opeo` is a Rust error-handling library that keeps an error value in caller-owned storage while a function returns its success value separately. A typed failure proof connects the returned state to the error slot, so a failure cannot be propagated from a different call's slot.

The runtime crate is `no_std`. The companion `opeo-macros` crate provides `#[opeo]`, which turns a conventional `Result` function into an OPEO function and generates a standard `Result` wrapper by default for callers that prefer the usual interface.

[简体中文 README](README_CN.md)

## How it works

An OPEO function receives an `Out` handle and returns `OResult`:

```text
caller owns ErrSlot<E>
       │ lends Out<'slot, E>
       ▼
OPEO function returns OResult<'slot, T>
       │ on failure, E is in the slot and Failed<'slot> proves it
       ▼
caller receives success T or handles the error in the slot
```

`OResult` stores either a success value or a zero-sized failure marker; the error itself is written into the `ErrSlot<E>`. Its lifetime brand ties the failure marker to the slot that received the error. `ErrSlot::call` bridges this interface back to `Result<T, E>`, while `ErrSlot::try_call` returns a `Caught` borrow so the caller can inspect or take the stored error.

`OResult` provides `is_ok`, `is_err`, `map`, and `and_then` for inspecting the state and composing success paths. The closure passed to `and_then` must return an `OResult` branded for the same slot. Asynchronous calls can use `ErrSlot::try_call_async` to inspect a borrowed error as well.

`Out::edit` writes an error into the slot and returns an `OutEdit`. The editor can borrow or modify the error; only `commit` produces a `Failed` proof. Dropping the editor before commit restores the slot's previous value.

The implementation uses no heap allocation for the slot and keeps the runtime independent of `std`. The error type `E` controls its own storage and may, of course, allocate. `OResult<T>` has the same layout as `Result<T, ()>` in the tested configurations; callers should rely on the public API rather than private representation details.

## Install

Add the runtime crate to your package. The procedural macro is re-exported by `opeo`, so a direct dependency on `opeo-macros` is not needed:

```toml
[dependencies]
opeo = "0.1"
```

The workspace uses Rust 2024 and declares Rust 1.85 as its minimum version.

## Quick start with `#[opeo]`

Write a normal function returning `Result<T, E>` and mark it with `#[opeo]`. The macro keeps the function as the OPEO entry point, rewrites fallible operations, and adds a standard wrapper named `<function>_std` by default. Set `wrapper = false` to omit the wrapper when you only need the OPEO entry point.

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

The generated `parse_positive_std` has the original `Result<u32, ParseError>` interface. Call `parse_positive` directly when the caller already owns an `ErrSlot` and wants the OPEO path.

## Inspect or take an error

`try_call` gives access to a `Caught` value on failure. `get` borrows the error, `get_mut` mutably borrows it, and `take` moves it out of the slot. If a `Caught` value is dropped without taking the error, the stored error is dropped too. Use `try_call_async` for the same inspection behavior with an asynchronous OPEO call.

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

## Write OPEO functions directly

Use `Out`, `OResult`, and `opeo_try!` when you want to define the OPEO form without the attribute macro. The `?`-style macro accepts a `Result`; its source error must convert into the slot's error type, or you can convert it with `map_err` first.

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

`Out::reborrow` creates a shorter mutable borrow for nested OPEO calls. This lets a caller propagate an `OResult` from the same slot while the borrow checker prevents mixing failure states from unrelated slots.

Use `Out::edit` to enrich an error in place before committing it:

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

## Public API

| Item | Purpose |
| --- | --- |
| `ErrSlot<E>` | Owns the error storage. Use `call`/`call_async` for a standard `Result`, or `try_call`/`try_call_async` for a borrowed `Caught` error. |
| `Out<'slot, 'borrow, E>` | Writes and commits an error with `fail`, edits a pending error in place with `edit`, and creates nested handles with `reborrow`. |
| `OutEdit<'slot, 'borrow, E>` | Borrows or modifies the error in the slot; `commit` produces a failure proof, while dropping it uncommitted restores the previous value. |
| `OResult<'slot, T>` | Returns a success value or a failure branded for one slot; supports state checks and success value composition. |
| `Failed<'slot>` | Proof produced by `Out::fail` or `OutEdit::commit` and used to construct a failure result. |
| `Caught<'slot, E>` | Borrows the stored error; use `get`/`get_mut` to inspect or modify it and `take` to own it. |
| `ResultOutExt` | Converts a standard `Result` to `OResult` with `or_out`, `or_out_with`, or `or_out_into`. |
| `bail!` | Writes an error and returns early from an OPEO function. |
| `ensure!` | Returns early with an error when a condition is false. |
| `opeo_try!` | Propagates a `Result`, an `OResult` from the same slot, or an `Option` with an explicit error value. |

For example, `ResultOutExt` can bridge a standard result in a manually written OPEO function:

```rust
use opeo::{ErrSlot, ResultOutExt};

let mut errors = ErrSlot::<&str>::new();
let result = errors.call(|out| Err::<u32, _>("invalid input").or_out(out));
assert_eq!(result, Err("invalid input"));
```

The `bail!` macro is also available under the alias `abort!` when renamed at import. `opeo_try!` may similarly be imported under another name.

## Attribute options and `Result` aliases

The wrapper name can be set explicitly:

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

To generate only the OPEO entry point, disable the wrapper explicitly:

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

The macro keeps `parse` as the OPEO entry point and does not generate `parse_std`. Call it through `ErrSlot::call` when you need a standard `Result` at the call site.

If the return type is a type alias whose final path segment is not named `Result`, provide both success and error types so the macro can determine the OPEO signature:

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

`wrapper_attrs(path, ...)` selects custom function attributes to copy to the generated wrapper. Standard attributes such as `cfg`, `doc`, `deprecated`, `inline`, `cold`, `must_use`, and `track_caller` are copied automatically. Symbol-changing attributes are not copied to the wrapper, so the original function keeps its symbol attributes without creating a duplicate symbol.

## Struct methods

Apply `#[opeo]` to an instance method in an inherent `impl` block. The macro keeps the method as the OPEO entry point and generates a standard wrapper method with the same receiver by default. The wrapper name defaults to `<method>_std`, can be set with `wrapper = ...`, or can be disabled with `wrapper = false`. Async methods produce an async OPEO method and, when enabled, an async standard wrapper. Generated wrappers for const methods keep `const`; the OPEO entry is runtime-only because it writes to an error slot.

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

## Supported functions and error conversion

`#[opeo]` supports free functions and instance methods in inherent `impl` blocks, including async and const functions and methods. The return type must be a path that resolves to `Result<T, E>` or use explicit `ok` and `error` types. Generated async wrappers return a future, and `ErrSlot::call_async` bridges async OPEO calls back to `Result<T, E>`. Generated wrappers for const functions and methods remain const; the OPEO entry is not const because it writes to runtime error storage. A function or method may have up to six explicit input parameters; the macro adds the `Out` parameter to the OPEO form. `extern` and variadic functions are rejected.

Question-mark propagation in the function body stores the source error in the slot. For a `Result<T, SourceError>`, the slot error type must implement `From<SourceError>`; use `map_err` when a custom conversion is needed. Nested closures keep their own normal `?` behavior. For `Option<T>`, use `opeo_try!(out, option, error_value)` to provide the error explicitly.

The macro reserves the identifier `out` in the transformed function body and generated parameter list. Give ordinary inputs and local bindings different names.

## Crates and license

- [`opeo`](https://crates.io/crates/opeo) contains the `no_std` runtime, public types, and exported macros.
- [`opeo-macros`](https://crates.io/crates/opeo-macros) contains the procedural macro implementation and is re-exported by `opeo`.

This project is dual-licensed under MIT or Apache-2.0.
