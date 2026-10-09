use core::marker::PhantomData;
use opeo::OResult;

fn fabricate<'slot>() -> OResult<'slot, u8> {
    OResult {
        value: Err(()),
        slot: PhantomData,
    }
}

fn main() {}
