use opeo::OResult;

fn propagate<'slot>(result: OResult<'static, u8>) -> OResult<'slot, u8> {
    let value = result?;
    Ok(value)
}

fn main() {}
