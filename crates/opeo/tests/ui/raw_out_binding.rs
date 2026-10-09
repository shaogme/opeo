use opeo::opeo;

#[opeo(wrapper = parse_std)]
fn parse() -> Result<u32, ()> {
    let r#out = 1;
    Ok(r#out)
}

fn main() {}
