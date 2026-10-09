use opeo::opeo;

#[opeo(wrapper = parse_std)]
fn parse() -> Result<(), ()> {
    let out = ();
    let _ = out;
    Ok(())
}

fn main() {}
