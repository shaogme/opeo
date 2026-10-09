#![deny(unused_must_use)]

use opeo::{ErrSlot, OResult, Out};

fn fail<'slot, 'borrow>(out: Out<'slot, 'borrow, ()>) -> OResult<'slot, ()> {
    OResult::failed(out.fail(()))
}

fn main() {
    let mut slot = ErrSlot::<()>::new();
    let _ = slot.try_call(|mut out| {
        fail(out.reborrow());
        OResult::success(())
    });
}
