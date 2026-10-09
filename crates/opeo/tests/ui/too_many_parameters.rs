use opeo::opeo;

#[opeo(wrapper = sum_std)]
fn sum(a: u8, b: u8, c: u8, d: u8, e: u8, f: u8, g: u8) -> Result<u8, ()> {
    Ok(a + b + c + d + e + f + g)
}

fn main() {}
