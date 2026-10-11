use std::{
    mem::forget,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use opeo::{ErrSlot, OResult};

fn trigger_test_unwind() -> ! {
    resume_unwind(Box::new("intentional callback unwind"))
}

#[derive(Debug, PartialEq, Eq)]
enum TestError {
    Basic(u8),
}

#[test]
fn call_returns_success_without_touching_the_slot() {
    let mut slot = ErrSlot::<TestError>::new();

    assert_eq!(slot.call(|_| Ok(17)), Ok(17));
    assert_eq!(slot.call(|_| Ok(23)), Ok(23));
}

#[test]
fn call_catches_the_written_error() {
    let mut slot = ErrSlot::<TestError>::new();

    let result = slot.call(|out| Err::<(), _>(out.fail(TestError::Basic(7))));

    assert_eq!(result, Err(TestError::Basic(7)));
}

#[test]
fn caught_can_be_inspected_then_taken_once() {
    let mut slot = ErrSlot::<TestError>::new();
    let result = slot.try_call(|out| Err::<(), _>(out.fail(TestError::Basic(9))));
    assert!(result.is_err());
    let Some(caught) = result.err() else {
        return;
    };

    assert_eq!(caught.get(), &TestError::Basic(9));
    assert_eq!(caught.take(), TestError::Basic(9));
    assert_eq!(slot.call(|_| Ok(31)), Ok(31));
}

#[test]
fn failure_types_support_debug_and_unwrap() {
    let mut slot = ErrSlot::<TestError>::new();
    let result = slot.try_call(|_| Ok(19));
    assert!(result.is_ok());
    let Some(value) = result.ok() else {
        return;
    };

    assert_eq!(value, 19);

    let result = slot.try_call(|out| {
        let failure = out.fail(TestError::Basic(13));
        assert_eq!(format!("{failure:?}"), "Failed");
        Err::<(), _>(failure)
    });
    assert!(result.is_err());
    let Some(caught) = result.err() else {
        return;
    };

    assert_eq!(format!("{caught:?}"), "Caught(Basic(13))");
}

#[test]
fn dropping_caught_drops_error_and_releases_the_slot() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut slot = ErrSlot::<DropCounter>::new();
    {
        let result = slot.try_call(|out| Err::<(), _>(out.fail(DropCounter(Arc::clone(&drops)))));
        assert!(result.is_err());
        let Some(caught) = result.err() else {
            return;
        };
        assert_eq!(Arc::strong_count(&drops), 2);
        drop(caught);
    }

    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(matches!(slot.call(|_| Ok(41)), Ok(41)));
}

#[test]
fn taking_caught_transfers_error_ownership() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut slot = ErrSlot::<DropCounter>::new();
    let result = slot.try_call(|out| Err::<(), _>(out.fail(DropCounter(Arc::clone(&drops)))));
    assert!(result.is_err());
    let Some(caught) = result.err() else {
        return;
    };

    let error = caught.take();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(error);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn dropping_slot_cleans_error_when_caught_was_forgotten() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut slot = ErrSlot::<DropCounter>::new();
    let result = slot.try_call(|out| Err::<(), _>(out.fail(DropCounter(Arc::clone(&drops)))));
    assert!(result.is_err());
    let Some(caught) = result.err() else {
        return;
    };

    forget(caught);
    drop(slot);

    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn a_new_call_clears_an_error_from_a_forgotten_caught_value() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut slot = ErrSlot::<DropCounter>::new();
    let result = slot.try_call(|out| Err::<(), _>(out.fail(DropCounter(Arc::clone(&drops)))));
    assert!(result.is_err());
    let Some(caught) = result.err() else {
        return;
    };

    forget(caught);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert!(matches!(slot.call(|_| Ok(99)), Ok(99)));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn panic_after_fail_drops_error_and_releases_the_slot() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut slot = ErrSlot::<DropCounter>::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = slot.try_call(|out| -> OResult<'_, ()> {
            let _failure = out.fail(DropCounter(Arc::clone(&drops)));
            trigger_test_unwind();
        });
    }));

    assert!(result.is_err());
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(matches!(slot.call(|_| Ok(51)), Ok(51)));
}

#[test]
fn panic_before_fail_leaves_slot_usable() {
    let mut slot = ErrSlot::<TestError>::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = slot.try_call(|_| -> OResult<'_, ()> { trigger_test_unwind() });
    }));

    assert!(result.is_err());
    assert_eq!(slot.call(|_| Ok(61)), Ok(61));
}

#[test]
fn successful_return_drops_written_error_and_cleans_slot() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut slot = ErrSlot::<DropCounter>::new();
    let result = slot.call(|out| {
        let _failure = out.fail(DropCounter(Arc::clone(&drops)));
        Ok(71)
    });

    assert!(matches!(result, Ok(71)));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(matches!(slot.call(|_| Ok(72)), Ok(72)));
}

#[test]
fn repeated_writes_replace_and_drop_the_previous_error() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut slot = ErrSlot::<DropCounter>::new();
    let result = slot.call(|mut out| {
        let _first = out.reborrow().fail(DropCounter(Arc::clone(&drops)));
        let second = out.fail(DropCounter(Arc::clone(&drops)));
        Err::<(), _>(second)
    });

    assert!(result.is_err());
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    drop(result);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    assert!(matches!(slot.call(|_| Ok(81)), Ok(81)));
}

struct DropCounter(Arc<AtomicUsize>);

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
