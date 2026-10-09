#![doc = include_str!("../README.md")]
#![doc = include_str!("../README_CN.md")]
#![no_std]

extern crate self as opeo;

use core::{fmt, hint::unreachable_unchecked, marker::PhantomData, ops::AsyncFnOnce, ptr::NonNull};

/// Converts a `Result` function into an OPEO entry and optionally generates a wrapper.
/// 将 `Result` 函数转换为 OPEO 入口，并可选择生成包装函数。
pub use opeo_macros::opeo;

#[must_use]
/// Proves that this call wrote an error to its slot.
/// 表示本次调用已向错误槽写入错误。
pub struct Failed<'slot>(PhantomData<fn(&'slot ()) -> &'slot ()>);

impl fmt::Debug for Failed<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Failed")
    }
}

/// A branded result whose error is stored in its `Out` slot.
/// 携带槽位品牌的结果，错误值保存在对应的 `Out` 槽中。
#[must_use = "propagate or handle OResult; failures are stored in its error slot"]
#[repr(transparent)]
pub struct OResult<'slot, T> {
    value: Result<T, ()>,
    slot: PhantomData<fn(&'slot ()) -> &'slot ()>,
}

impl<'slot, T> OResult<'slot, T> {
    /// Creates a successful result.
    /// 创建成功结果。
    pub const fn success(value: T) -> Self {
        Self {
            value: Ok(value),
            slot: PhantomData,
        }
    }

    /// Returns `true` when this result contains a success value.
    /// 当结果包含成功值时返回 `true`。
    pub const fn is_ok(&self) -> bool {
        self.value.is_ok()
    }

    /// Returns `true` when this result represents a failure.
    /// 当结果表示失败时返回 `true`。
    pub const fn is_err(&self) -> bool {
        self.value.is_err()
    }

    /// Maps a success value while preserving a failure for this slot.
    /// 转换成功值，同时保留绑定到当前槽位的失败状态。
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> OResult<'slot, U> {
        match self.value {
            Ok(value) => OResult::success(map(value)),
            Err(()) => OResult {
                value: Err(()),
                slot: PhantomData,
            },
        }
    }

    /// Chains a success value into another result for the same slot.
    /// 将成功值继续传入绑定到同一槽位的结果计算。
    pub fn and_then<U>(self, next: impl FnOnce(T) -> OResult<'slot, U>) -> OResult<'slot, U> {
        match self.value {
            Ok(value) => next(value),
            Err(()) => OResult {
                value: Err(()),
                slot: PhantomData,
            },
        }
    }

    /// Creates a failure result from a real failure proof.
    /// 根据真实失败凭证创建失败结果。
    #[doc(hidden)]
    pub const fn failed(_failure: Failed<'slot>) -> Self {
        Self {
            value: Err(()),
            slot: PhantomData,
        }
    }

    /// Exposes the branded state to generated propagation code.
    /// 暴露品牌化的内部状态供生成代码传播。
    #[doc(hidden)]
    pub fn __into_result(self) -> Result<T, Failed<'slot>> {
        match self.value {
            Ok(value) => Ok(value),
            Err(()) => Err(Failed(PhantomData)),
        }
    }
}

/// Exposes standard `Result` type information to `#[opeo]`.
/// 为 `#[opeo]` 暴露标准 `Result` 的类型信息。
#[doc(hidden)]
pub trait __OpeoResultParts {
    /// The success type of the standard `Result`.
    /// 标准 `Result` 的成功类型。
    type Success;

    /// The error type of the standard `Result`.
    /// 标准 `Result` 的错误类型。
    type Error;
}

impl<T, E> __OpeoResultParts for Result<T, E> {
    type Success = T;

    type Error = E;
}

struct ErrState<E> {
    value: Option<E>,
}

#[repr(transparent)]
/// A write handle pointing to caller-owned error storage.
/// 指向调用方错误存储的写入句柄。
pub struct Out<'slot, 'borrow, E>(
    NonNull<ErrState<E>>,
    PhantomData<fn(&'slot ()) -> &'slot ()>,
    PhantomData<&'borrow mut E>,
);

impl<'slot, 'borrow, E> Out<'slot, 'borrow, E> {
    /// Writes an error and returns a failure proof; a new write replaces the old value.
    /// 写入错误并生成失败凭证；重复写入时新值替换旧值。
    pub fn fail(self, error: E) -> Failed<'slot> {
        self.edit(error).commit()
    }

    /// Writes an error into the slot and returns an editor for it.
    /// 将错误写入槽位，并返回用于编辑该错误的编辑器。
    pub fn edit(mut self, error: E) -> OutEdit<'slot, 'borrow, E> {
        let previous = self.state_mut().value.replace(error);
        OutEdit {
            out: self,
            previous,
            committed: false,
        }
    }

    /// Creates a child handle within this handle's mutable borrow.
    /// 在当前句柄的可变借用内创建可传递给下游的句柄。
    pub const fn reborrow<'next>(&'next mut self) -> Out<'slot, 'next, E> {
        Out(self.0, PhantomData, PhantomData)
    }

    const fn state_mut(&mut self) -> &mut ErrState<E> {
        // SAFETY: try_call creates this pointer during an exclusive borrow; the safe API cannot copy Out or let it outlive that borrow.
        // 安全性：该指针仅由 try_call 在独占借用期间构造；安全 API 不会复制 Out 或让它逃逸该借用。
        unsafe { self.0.as_mut() }
    }

    fn state(&self) -> &ErrState<E> {
        // SAFETY: Out's borrow brand prevents mutable aliases while this shared reference is live.
        // 安全性：Out 的借用品牌会在该共享引用存活期间阻止可变别名。
        unsafe { self.0.as_ref() }
    }
}

/// Temporarily edits the error in an `Out` slot.
/// 暂时编辑 `Out` 槽位中的错误。
#[must_use = "commit the edited error to produce a failure proof"]
pub struct OutEdit<'slot, 'borrow, E> {
    out: Out<'slot, 'borrow, E>,
    previous: Option<E>,
    committed: bool,
}

impl<'slot, 'borrow, E> OutEdit<'slot, 'borrow, E> {
    /// Borrows the edited error.
    /// 借用正在编辑的错误。
    pub fn get(&self) -> &E {
        match self.out.state().value.as_ref() {
            Some(error) => error,
            None => {
                // SAFETY: OutEdit writes an error before it is constructed and exposes no way to remove it.
                // 安全性：OutEdit 构造前会写入错误，且不提供移除错误的接口。
                unsafe { unreachable_unchecked() }
            }
        }
    }

    /// Mutably borrows the edited error.
    /// 可变借用正在编辑的错误。
    pub fn get_mut(&mut self) -> &mut E {
        match self.out.state_mut().value.as_mut() {
            Some(error) => error,
            None => {
                // SAFETY: OutEdit writes an error before it is constructed and exposes no way to remove it.
                // 安全性：OutEdit 构造前会写入错误，且不提供移除错误的接口。
                unsafe { unreachable_unchecked() }
            }
        }
    }

    /// Commits the edited error and returns its failure proof.
    /// 提交编辑后的错误，并返回对应的失败凭证。
    pub fn commit(mut self) -> Failed<'slot> {
        self.committed = true;
        drop(self.previous.take());
        Failed(PhantomData)
    }
}

impl<E> Drop for OutEdit<'_, '_, E> {
    fn drop(&mut self) {
        if !self.committed {
            let state = self.out.state_mut();
            let edited = state.value.take();
            state.value = self.previous.take();
            // Restore before dropping the edit so a panic in its destructor preserves any earlier failure proof.
            // 先恢复原值再析构编辑值，确保其析构发生 panic 时仍保留先前失败凭证对应的错误。
            drop(edited);
        }
    }
}

/// Owns error storage and keeps it uninitialized between calls.
/// 持有错误存储，并在每次调用之间保持槽位未初始化。
pub struct ErrSlot<E> {
    state: ErrState<E>,
}

impl<E> ErrSlot<E> {
    /// Creates an empty error slot.
    /// 创建一个空错误槽。
    pub const fn new() -> Self {
        Self {
            state: ErrState { value: None },
        }
    }

    /// Returns a borrowable error on failure.
    /// 调用 OPEO 函数，并将失败结果转换为可借用的错误。
    pub fn try_call<T>(
        &mut self,
        f: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> Result<T, Caught<'_, E>> {
        drop(self.state.value.take());
        let mut guard = InitGuard::new(&mut self.state);
        let out = Out(NonNull::from(guard.state_mut()), PhantomData, PhantomData);
        let result = f(out);

        match result.__into_result() {
            Ok(value) => {
                guard.drop_error();
                guard.disarm();
                Ok(value)
            }
            Err(_failure) => {
                if guard.state_mut().value.is_none() {
                    // SAFETY: A safely constructed failure can only use a proof that wrote this slot.
                    // 安全性：安全构造的失败结果只能来自已写入当前槽位的凭证。
                    unsafe { unreachable_unchecked() };
                }
                guard.disarm();
                Err(Caught {
                    state: &mut self.state,
                })
            }
        }
    }

    /// Returns a borrowable error from an asynchronous OPEO call.
    /// 调用异步 OPEO 函数，并在失败时返回可借用的错误。
    pub async fn try_call_async<T>(
        &mut self,
        f: impl for<'slot> AsyncFnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> Result<T, Caught<'_, E>> {
        drop(self.state.value.take());
        let mut guard = InitGuard::new(&mut self.state);
        let out = Out(NonNull::from(guard.state_mut()), PhantomData, PhantomData);
        let result = f(out).await;

        match result.__into_result() {
            Ok(value) => {
                guard.drop_error();
                guard.disarm();
                Ok(value)
            }
            Err(_failure) => {
                if guard.state_mut().value.is_none() {
                    // SAFETY: A safely constructed failure can only use a proof that wrote this slot.
                    // 安全性：安全构造的失败结果只能来自已写入当前槽位的凭证。
                    unsafe { unreachable_unchecked() };
                }
                guard.disarm();
                Err(Caught {
                    state: &mut self.state,
                })
            }
        }
    }

    /// Bridges an OPEO result to standard `Result<T, E>`.
    /// 将 OPEO 结果桥接为标准 `Result<T, E>`。
    pub fn call<T>(
        &mut self,
        f: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> Result<T, E> {
        match self.try_call(f) {
            Ok(value) => Ok(value),
            Err(caught) => Err(caught.take()),
        }
    }

    /// Bridges an asynchronous OPEO result to standard `Result<T, E>`.
    /// 将异步 OPEO 结果桥接为标准 `Result<T, E>`。
    pub async fn call_async<T>(
        &mut self,
        f: impl for<'slot> AsyncFnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> Result<T, E> {
        drop(self.state.value.take());
        let mut guard = InitGuard::new(&mut self.state);
        let out = Out(NonNull::from(guard.state_mut()), PhantomData, PhantomData);
        let result = f(out).await;

        match result.__into_result() {
            Ok(value) => {
                guard.drop_error();
                guard.disarm();
                Ok(value)
            }
            Err(_failure) => {
                if guard.state_mut().value.is_none() {
                    // SAFETY: A safely constructed failure can only use a proof that wrote this slot.
                    // 安全性：安全构造的失败结果只能来自已写入当前槽位的凭证。
                    unsafe { unreachable_unchecked() };
                }
                let error = match guard.state_mut().value.take() {
                    Some(error) => error,
                    None => {
                        // SAFETY: The preceding check guarantees that the slot contains an error.
                        // 安全性：前面的检查保证槽位中存在错误。
                        unsafe { unreachable_unchecked() }
                    }
                };
                guard.disarm();
                Err(error)
            }
        }
    }
}

impl<E> Default for ErrSlot<E> {
    fn default() -> Self {
        Self::new()
    }
}

/// Temporarily borrows the slot error and drops it when discarded.
/// 暂时持有槽位错误的借用；丢弃时会析构错误。
pub struct Caught<'slot, E> {
    state: &'slot mut ErrState<E>,
}

impl<E> fmt::Debug for Caught<'_, E>
where
    E: fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Caught").field(self.get()).finish()
    }
}

impl<E> Caught<'_, E> {
    /// Borrows the error stored in the slot.
    /// 借用槽位中的错误。
    pub const fn get(&self) -> &E {
        match self.state.value.as_ref() {
            Some(error) => error,
            None => {
                // SAFETY: try_call constructs Caught only while the error is initialized.
                // 安全性：Caught 只能由持有已初始化错误的 try_call 分支构造。
                unsafe { unreachable_unchecked() }
            }
        }
    }

    /// Mutably borrows the error stored in the slot.
    /// 可变借用槽位中的错误。
    pub fn get_mut(&mut self) -> &mut E {
        match self.state.value.as_mut() {
            Some(error) => error,
            None => {
                // SAFETY: Caught can only be constructed while the error is initialized.
                // 安全性：Caught 只能在错误已初始化时构造。
                unsafe { unreachable_unchecked() }
            }
        }
    }

    /// Takes ownership of the error from the slot.
    /// 从槽位取走错误。
    pub fn take(self) -> E {
        match self.state.value.take() {
            Some(error) => error,
            None => {
                // SAFETY: Caught's private construction path guarantees the value remains in the slot.
                // 安全性：Caught 的私有构造路径保证值仍在槽位中。
                unsafe { unreachable_unchecked() }
            }
        }
    }
}

impl<E> Drop for Caught<'_, E> {
    fn drop(&mut self) {
        drop(self.state.value.take());
    }
}

struct InitGuard<E> {
    state: NonNull<ErrState<E>>,
    armed: bool,
}

impl<E> InitGuard<E> {
    fn new(state: &mut ErrState<E>) -> Self {
        Self {
            state: NonNull::from(state),
            armed: true,
        }
    }

    fn state_mut(&mut self) -> &mut ErrState<E> {
        // SAFETY: InitGuard is the sole temporary owner of this pointer during ErrSlot::try_call.
        // 安全性：InitGuard 在 ErrSlot::try_call 期间是该指针的唯一临时所有者。
        unsafe { self.state.as_mut() }
    }

    fn drop_error(&mut self) {
        drop(self.state_mut().value.take());
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl<E> Drop for InitGuard<E> {
    fn drop(&mut self) {
        if self.armed {
            self.drop_error();
        }
    }
}

/// Converts standard `Result` errors into an `Out` slot.
/// 将标准 `Result` 错误写入 `Out` 的转换接口。
pub trait ResultOutExt<T, E> {
    /// Preserves the success value and stores an error of the same type.
    /// 保留成功值，并将同类型错误写入槽位。
    fn or_out<'slot, 'borrow>(self, out: Out<'slot, 'borrow, E>) -> OResult<'slot, T>;

    /// Converts and stores the error only when the result is a failure.
    /// 仅在失败时转换错误并写入槽位。
    fn or_out_with<'slot, 'borrow, E2>(
        self,
        out: Out<'slot, 'borrow, E2>,
        convert: impl FnOnce(E) -> E2,
    ) -> OResult<'slot, T>;

    /// Converts a failure with `From<E>` and stores it in the slot.
    /// 使用 `From<E>` 转换失败值并写入槽位。
    fn or_out_into<'slot, 'borrow, E2>(self, out: Out<'slot, 'borrow, E2>) -> OResult<'slot, T>
    where
        E2: From<E>;
}

impl<T, E> ResultOutExt<T, E> for Result<T, E> {
    fn or_out<'slot, 'borrow>(self, out: Out<'slot, 'borrow, E>) -> OResult<'slot, T> {
        self.or_out_with(out, |error| error)
    }

    fn or_out_with<'slot, 'borrow, E2>(
        self,
        out: Out<'slot, 'borrow, E2>,
        convert: impl FnOnce(E) -> E2,
    ) -> OResult<'slot, T> {
        match self {
            Ok(value) => OResult::success(value),
            Err(error) => OResult::failed(out.fail(convert(error))),
        }
    }

    fn or_out_into<'slot, 'borrow, E2>(self, out: Out<'slot, 'borrow, E2>) -> OResult<'slot, T>
    where
        E2: From<E>,
    {
        self.or_out_with(out, E2::from)
    }
}

/// Bridges standard and same-slot OPEO results.
/// 将标准 `Result` 或同槽位 `OResult` 转换为带品牌的结果。
#[doc(hidden)]
pub trait __OpeoTry<'slot, T, E> {
    /// Stores an error or propagates a same-slot failure proof.
    /// 将错误写入槽位，或直接传递已有的同槽位失败凭证。
    fn __opeo_try<'borrow>(self, out: Out<'slot, 'borrow, E>) -> OResult<'slot, T>;
}

impl<'slot, T, Source, Target> __OpeoTry<'slot, T, Target> for Result<T, Source>
where
    Target: From<Source>,
{
    fn __opeo_try<'borrow>(self, out: Out<'slot, 'borrow, Target>) -> OResult<'slot, T> {
        match self {
            Ok(value) => OResult::success(value),
            Err(error) => OResult::failed(out.fail(Target::from(error))),
        }
    }
}

impl<'slot, T, E> __OpeoTry<'slot, T, E> for OResult<'slot, T> {
    fn __opeo_try<'borrow>(self, _out: Out<'slot, 'borrow, E>) -> OResult<'slot, T> {
        self
    }
}

/// Stores an error in `Out` and returns early with failure.
/// 将错误写入 `Out` 并提前返回失败。
#[macro_export]
macro_rules! bail {
    ($out:expr, $error:expr $(,)?) => {
        return $crate::OResult::failed(($out).reborrow().fail($error));
    };
}

/// Stores an error and returns early when the condition is false.
/// 条件不满足时写入错误并提前返回。
#[macro_export]
macro_rules! ensure {
    ($out:expr, $condition:expr, $error:expr $(,)?) => {
        if !$condition {
            return $crate::OResult::failed(($out).reborrow().fail($error));
        }
    };
}

/// Bridges a `Result` or `Option` failure into the current OPEO error slot.
/// 将 `Result` 或 `Option` 失败桥接到当前 OPEO 错误槽。
#[macro_export]
macro_rules! __opeo_try_value {
    ($out:expr, $result:expr $(,)?) => {
        match $crate::OResult::__into_result($crate::__OpeoTry::__opeo_try(
            ($result),
            ($out).reborrow(),
        )) {
            ::core::result::Result::Ok(value) => value,
            ::core::result::Result::Err(failure) => {
                return $crate::OResult::failed(failure);
            }
        }
    };
}

/// Bridges a `Result` or `Option` failure into the current OPEO error slot.
/// 将 `Result` 或 `Option` 失败桥接到当前 OPEO 错误槽。
#[macro_export]
macro_rules! opeo_try {
    ($out:expr, $result:expr $(,)?) => {
        $crate::__opeo_try_value!($out, $result)
    };
    ($out:expr, $option:expr, $error:expr $(,)?) => {
        match $option {
            Some(value) => value,
            None => {
                return $crate::OResult::failed(($out).reborrow().fail($error));
            }
        }
    };
}
