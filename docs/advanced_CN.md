# 进阶指南

[中文 README](../README_CN.md) · [English advanced guide](advanced.md)

## 错误转换

OPEO 函数返回 `OResult<'slot, T>`，即 `Result<T, Failed<'slot>>` 的别名。失败凭证绑定到槽位，实际错误由 `Out` 存储：

```rust,ignore
return Err(out.fail(ParseError::InvalidNumber));
```

普通 `Result<T, E>` 导入 `ResultOutExt` 后，选择能说明错误转换方式的方法：

```rust,ignore
let number = input
    .parse::<u32>()
    .store_err_with(out.reborrow(), |_| ParseError::InvalidNumber)?;
```

`store_err` 写入与槽位同类型的错误；`store_err_with` 通过闭包转换；`store_err_into` 使用 `From<E>`。只有输入结果为 `Err` 时才会调用转换闭包。处理 `Option<T>` 时，先使用标准库转换：

```rust,ignore
let value = optional_value
    .ok_or(ParseError::Missing)
    .store_err(out.reborrow())?;
```

同槽位的嵌套 OPEO 函数已经返回正确的失败品牌，直接用 `?` 传播即可。

## 槽位与借用生命周期

`Out<'slot, 'borrow, E>` 保留两项不同的保证：`'slot` 将 `Failed` 绑定到槽位，使其他槽位的失败无法传播；`'borrow` 限定传给下游代码的可变重借用范围。多数函数签名可以省略第二个生命周期名称：

```rust,ignore
fn parse<'slot>(out: Out<'slot, '_, ParseError>) -> OResult<'slot, u32> {
    // ...
}
```

保留两个生命周期很重要：缩短嵌套借用时，不应改变槽位品牌。

## 返回或检查错误

调用方需要普通 `Result<T, E>` 时使用 `ErrSlot::call` 或 `call_async`；需要检查槽位中的错误时使用 `try_call` 或 `try_call_async`：

```rust,ignore
match slot.try_call(|out| parse(input, out)) {
    Ok(value) => use_value(value),
    Err(caught) => log_error(caught.get()),
}
```

`Caught` 是槽位错误的守卫。`get` 和 `get_mut` 借用错误，`take` 将错误移出。丢弃守卫会析构错误。如果忘记守卫，错误仍由槽位持有；槽位析构或开始下一次调用时会清理它。

异步 OPEO 函数使用相同的返回类型和转换方法，通过 `call_async` 或 `try_call_async` 等待：

```rust,ignore
let result = slot
    .call_async(async |out| parse_async(input, out).await)
    .await;
```

## 提前返回与错误上下文

`bail!(out, error)` 写入错误并提前返回。`ensure!(out, condition, error)` 仅在条件不成立时这样做。两者都是可选的简写；显式形式是 `return Err(out.fail(error));`。

在写入前构造带上下文的错误。例如，可用 `store_err_with` 在转换解析错误时附加字段名：

```rust,ignore
let value = parse_field(input)
    .store_err_with(out.reborrow(), |source| ParseError::Field {
        field: "header",
        source,
    })?;
```

## 从 0.2 迁移

此版本清理 API，包含不兼容变更：

| 旧 API | 替代写法 |
| --- | --- |
| `or_out(out)` | `store_err(out)` |
| `or_out_with(out, convert)` | `store_err_with(out, convert)` |
| `or_out_into(out)` | `store_err_into(out)` |
| `opeo_try!(out, result)` | `result.store_err_into(out.reborrow())?` |
| `opeo_try!(out, option, error)` | `option.ok_or(error).store_err(out.reborrow())?` |
| `Out::edit(error)` 与 `OutEdit::commit()` | 先构造最终错误，再调用 `out.fail(error)` 或用 `store_err_with` 转换 |

`OutEdit` 的丢弃回滚行为已移除。过去在写入后补充错误上下文的代码，应将转换移入闭包，让完整错误在写入槽位前就准备好。`OResult`、`Failed`、`Caught`、`bail!` 和 `ensure!` 保留原名。
