# Advanced guide

[README](../README.md) · [简体中文进阶指南](advanced_CN.md)

## Converting errors

An OPEO function returns `OResult<'slot, T>`, an alias for `Result<T, Failed<'slot>>`. A failure value is a proof tied to the slot, while the actual error is stored by `Out`:

```rust,ignore
return Err(out.fail(ParseError::InvalidNumber));
```

For a regular `Result<T, E>`, import `ResultOutExt` and choose the conversion that describes the error mapping:

```rust,ignore
let number = input
    .parse::<u32>()
    .store_err_with(out.reborrow(), |_| ParseError::InvalidNumber)?;
```

`store_err` writes an error of the slot's type, `store_err_with` maps it with a closure, and `store_err_into` uses `From<E>`. The conversion closure runs only when the input result is `Err`. For `Option<T>`, use the standard conversion first:

```rust,ignore
let value = optional_value
    .ok_or(ParseError::Missing)
    .store_err(out.reborrow())?;
```

A nested OPEO function using the same slot already returns the right failure brand, so propagate it directly with `?`.

## Slot and borrow lifetimes

`Out<'slot, 'borrow, E>` keeps two separate guarantees. `'slot` brands `Failed` so a failure from another slot cannot be propagated. `'borrow` limits the mutable borrow used to pass a reborrowed handle to nested code. Most function signatures can leave the second lifetime anonymous:

```rust,ignore
fn parse<'slot>(out: Out<'slot, '_, ParseError>) -> OResult<'slot, u32> {
    // ...
}
```

Keeping both lifetimes is important: shortening a nested borrow must not change the slot brand.

## Returning or inspecting errors

Use `ErrSlot::call` and `call_async` when the caller wants an ordinary `Result<T, E>`. Use `try_call` or `try_call_async` when the caller needs to inspect the error in the slot:

```rust,ignore
match slot.try_call(|out| parse(input, out)) {
    Ok(value) => use_value(value),
    Err(caught) => log_error(caught.get()),
}
```

`Caught` is a guard for the stored error. `get` and `get_mut` borrow it, and `take` moves it out. Dropping the guard drops the error. If the guard is forgotten, the slot still owns the error; dropping the slot or starting its next call cleans it up.

Async OPEO functions use the same return type and conversion methods. Await them through `call_async` or `try_call_async`:

```rust,ignore
let result = slot
    .call_async(async |out| parse_async(input, out).await)
    .await;
```

## Early returns and error context

`bail!(out, error)` writes an error and returns early. `ensure!(out, condition, error)` does so only when the condition is false. Both are optional conveniences; the equivalent explicit form is `return Err(out.fail(error));`.

Build contextual errors before writing them. For example, `store_err_with` can attach a field name while converting a parser error:

```rust,ignore
let value = parse_field(input)
    .store_err_with(out.reborrow(), |source| ParseError::Field {
        field: "header",
        source,
    })?;
```

## Migrating from 0.2

This release makes a breaking API cleanup:

| Previous API | Replacement |
| --- | --- |
| `or_out(out)` | `store_err(out)` |
| `or_out_with(out, convert)` | `store_err_with(out, convert)` |
| `or_out_into(out)` | `store_err_into(out)` |
| `opeo_try!(out, result)` | `result.store_err_into(out.reborrow())?` |
| `opeo_try!(out, option, error)` | `option.ok_or(error).store_err(out.reborrow())?` |
| `Out::edit(error)` and `OutEdit::commit()` | Construct the final error first, then call `out.fail(error)` or convert with `store_err_with` |

`OutEdit`'s rollback-on-drop behavior has been removed. Code that added context to an error after storing it should move that transformation into the conversion closure so the complete error is ready before it enters the slot. `OResult`, `Failed`, `Caught`, `bail!`, and `ensure!` keep their names.
