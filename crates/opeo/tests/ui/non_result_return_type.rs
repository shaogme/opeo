use opeo::opeo;

struct NotAResult<T, E>(T, E);

#[opeo(wrapper = invalid_std)]
fn invalid() -> NotAResult<u8, ()> {
    NotAResult(1, ())
}

fn main() {}
