use opeo::opeo;

#[opeo(wrapper = parse_std)]
fn parse(r#out: u32) -> Result<u32, ()> {
    Ok(r#out)
}

fn main() {}
