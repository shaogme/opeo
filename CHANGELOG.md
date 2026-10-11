# Changelog

## 1.0.0 — 2026-10-11

### Breaking changes

- Renamed `ResultOutExt::or_out`, `or_out_with`, and `or_out_into` to `store_err`, `store_err_with`, and `store_err_into`.
- Removed `OutEdit` and `Out::edit`; prepare the complete error before writing it with `Out::fail` or a `store_err_with` conversion.
- Removed `opeo_try!`; convert `Result` with `ResultOutExt` and convert `Option` with `ok_or(error)` followed by `ResultOutExt`.
- Kept `OResult`, `Failed`, `Caught`, `bail!`, and `ensure!` names and slot-branding behavior.
- Reorganized the README and moved async, borrowed-error, lifetime, and migration guidance into advanced guides.

See [the advanced migration guide](docs/advanced.md) or [the Chinese guide](docs/advanced_CN.md) for replacement examples.

## 0.2.0 — 2026-10-11

### Breaking changes

- Changed `OResult<'slot, T>` to an alias for `Result<T, Failed<'slot>>`; removed `OResult::success`, `OResult::failed`, and `into_result()`.
- Removed the `#[opeo]` attribute macro and hidden `__OpeoTry` bridge. The `opeo-macros` crate is no longer part of the workspace.
- Standard `Result` errors must be converted explicitly with `ResultOutExt` or `opeo_try!`; use native `?` for same-slot OPEO results.
- OPEO functions declare `Out<'slot, 'borrow, E>` and `OResult<'slot, T>` explicitly. Standard wrappers are written with `ErrSlot::call` or `call_async`.

### 迁移提示

- `OResult<'slot, T>` 现为 `Result<T, Failed<'slot>>` 的别名；已移除 `OResult::success`、`OResult::failed` 和 `into_result()`。
- `#[opeo]` 属性宏及隐藏的 `__OpeoTry` 桥接接口已移除，`opeo-macros` crate 不再属于工作区。
- 普通 `Result` 错误需通过 `ResultOutExt` 或 `opeo_try!` 显式写入槽位；同槽位 OPEO 结果使用原生 `?` 传播。
- OPEO 函数显式声明 `Out<'slot, 'borrow, E>` 与 `OResult<'slot, T>`。标准包装器通过 `ErrSlot::call` 或 `call_async` 编写。
