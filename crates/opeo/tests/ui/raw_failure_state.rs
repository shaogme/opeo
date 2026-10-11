use core::marker::PhantomData;
use opeo::{Failed, OResult};

fn fabricate<'slot>() -> OResult<'slot, u8> {
    Err(Failed(PhantomData))
}

fn main() {}
