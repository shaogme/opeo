use opeo::{OResult, Out, opeo_try};

fn propagate<'slot, 'borrow>(
    result: OResult<'static, u8>,
    mut out: Out<'slot, 'borrow, ()>,
) -> OResult<'slot, u8> {
    let value = opeo_try!(out, result);
    OResult::success(value)
}

fn main() {}
