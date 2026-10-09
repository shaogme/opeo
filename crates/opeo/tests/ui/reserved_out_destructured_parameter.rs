use opeo::opeo;

#[opeo]
fn parse((out, value): (u8, u8)) -> Result<u8, ()> {
    Ok(out + value)
}

fn main() {}
