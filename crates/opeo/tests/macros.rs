use core::num::ParseIntError;
use std::{
    future::Future,
    pin::pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    thread,
};

use opeo::{ErrSlot, OResult, Out, ResultOutExt, bail, ensure, opeo_try};

#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    Empty,
    InvalidNumber,
    Missing,
    Zero,
    Source,
}

#[derive(Debug, PartialEq, Eq)]
struct SourceError;

impl From<SourceError> for ParseError {
    fn from(_error: SourceError) -> Self {
        Self::Source
    }
}

impl From<ParseIntError> for ParseError {
    fn from(_error: ParseIntError) -> Self {
        Self::InvalidNumber
    }
}

fn parse_number<'slot, 'borrow>(
    input: &str,
    mut out: Out<'slot, 'borrow, ParseError>,
) -> OResult<'slot, u32> {
    if input.is_empty() {
        bail!(out, ParseError::Empty);
    }

    let value = input.parse::<u32>().or_out_into(out.reborrow())?;
    ensure!(out, value > 0, ParseError::Zero);
    Ok(value)
}

fn parse_number_std(input: &str) -> Result<u32, ParseError> {
    let mut slot = ErrSlot::<ParseError>::new();
    slot.call(|out| parse_number(input, out))
}

fn parse_optional<'slot, 'borrow>(
    input: Option<u32>,
    mut out: Out<'slot, 'borrow, ParseError>,
) -> OResult<'slot, u32> {
    let value = opeo_try!(out, input, ParseError::Missing);
    ensure!(out, value > 0, ParseError::Zero);
    Ok(value)
}

fn parse_nested<'slot, 'borrow>(mut out: Out<'slot, 'borrow, ParseError>) -> OResult<'slot, u8> {
    let value = nested_leaf(out.reborrow())?;
    Ok(value)
}

fn nested_leaf<'slot, 'borrow>(out: Out<'slot, 'borrow, ParseError>) -> OResult<'slot, u8> {
    Err(out.fail(ParseError::Missing))
}

fn parse_with_try_macro<'slot, 'borrow>(
    input: &str,
    mut out: Out<'slot, 'borrow, ParseError>,
) -> OResult<'slot, u32> {
    let value = opeo_try!(out, input.parse::<u32>().map_err(ParseError::from));
    Ok(value)
}

fn parse_source_error<'slot, 'borrow, E>(
    value: u32,
    out: Out<'slot, 'borrow, E>,
) -> OResult<'slot, u32>
where
    E: From<SourceError>,
{
    let result = if value == 0 {
        Err(SourceError)
    } else {
        Ok(value)
    };
    result.or_out_into(out)
}

struct Counter {
    value: u32,
}

impl Counter {
    fn add<'slot, 'borrow>(
        &self,
        amount: u32,
        out: Out<'slot, 'borrow, ParseError>,
    ) -> OResult<'slot, u32> {
        if amount == 0 {
            return Err(out.fail(ParseError::Zero));
        }
        Ok(self.value + amount)
    }

    fn add_nested<'slot, 'borrow>(
        &self,
        amount: u32,
        mut out: Out<'slot, 'borrow, ParseError>,
    ) -> OResult<'slot, u32> {
        let value = self.add(amount, out.reborrow())?;
        Ok(value + 1)
    }
}

trait TraitCounter {
    fn add<'slot, 'borrow>(
        &self,
        amount: u32,
        _out: Out<'slot, 'borrow, ParseError>,
    ) -> OResult<'slot, u32> {
        Ok(amount + 1)
    }

    fn base<'slot, 'borrow>(&self, out: Out<'slot, 'borrow, ParseError>) -> OResult<'slot, u32>;
}

struct SimpleCounter(u32);

impl TraitCounter for SimpleCounter {
    fn base<'slot, 'borrow>(&self, _out: Out<'slot, 'borrow, ParseError>) -> OResult<'slot, u32> {
        Ok(self.0)
    }
}

async fn parse_async(input: &str) -> Result<u32, ParseIntError> {
    input.parse()
}

async fn parse_async_opeo<'slot, 'borrow>(
    input: &str,
    mut out: Out<'slot, 'borrow, ParseError>,
) -> OResult<'slot, u32> {
    let value = parse_async(input).await.or_out_into(out.reborrow())?;
    Ok(value)
}

struct ThreadWaker(thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<T>(future: impl Future<Output = T>) -> T {
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);

    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => thread::park(),
        }
    }
}

#[test]
fn result_conversion_and_control_flow_write_the_error_slot() {
    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(|out| parse_number("42", out)), Ok(42));
    assert_eq!(parse_number_std("7"), Ok(7));
    assert_eq!(parse_number_std(""), Err(ParseError::Empty));
    assert_eq!(parse_number_std("0"), Err(ParseError::Zero));
    assert_eq!(parse_number_std("invalid"), Err(ParseError::InvalidNumber));
}

#[test]
fn result_out_ext_supports_same_custom_and_from_conversions() {
    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(|out| Ok::<_, ParseError>(4).or_out(out)), Ok(4));
    assert_eq!(
        slot.call(|out| Err::<(), _>(ParseError::Missing).or_out(out)),
        Err(ParseError::Missing)
    );
    assert_eq!(
        slot.call(|out| {
            Err::<(), _>("invalid".to_owned()).or_out_with(out, |_| ParseError::InvalidNumber)
        }),
        Err(ParseError::InvalidNumber)
    );
    assert_eq!(
        slot.call(|out| Err::<(), _>(SourceError).or_out_into(out)),
        Err(ParseError::Source)
    );
}

#[test]
fn native_question_mark_propagates_same_slot_results() {
    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(|out| parse_optional(Some(12), out)), Ok(12));
    assert_eq!(
        slot.call(|out| parse_optional(None, out)),
        Err(ParseError::Missing)
    );
    assert_eq!(
        slot.call(|out| parse_optional(Some(0), out)),
        Err(ParseError::Zero)
    );
    assert_eq!(slot.call(parse_nested), Err(ParseError::Missing));
}

#[test]
fn opeo_try_bridges_standard_result_and_option_values() {
    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(|out| parse_with_try_macro("32", out)), Ok(32));
    assert_eq!(
        slot.call(|out| parse_with_try_macro("bad", out)),
        Err(ParseError::InvalidNumber)
    );
    assert_eq!(slot.call(|out| parse_optional(Some(3), out)), Ok(3));
}

#[test]
fn from_conversion_supports_generic_slot_errors() {
    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(|out| parse_source_error(5, out)), Ok(5));
    assert_eq!(
        slot.call(|out| parse_source_error(0, out)),
        Err(ParseError::Source)
    );
}

#[test]
fn native_results_work_in_methods_and_trait_bodies() {
    let counter = Counter { value: 5 };
    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(|out| counter.add(3, out)), Ok(8));
    assert_eq!(slot.call(|out| counter.add_nested(3, out)), Ok(9));
    assert_eq!(
        slot.call(|out| counter.add_nested(0, out)),
        Err(ParseError::Zero)
    );

    let trait_counter = SimpleCounter(41);
    assert_eq!(slot.call(|out| trait_counter.add(2, out)), Ok(3));
    assert_eq!(slot.call(|out| trait_counter.base(out)), Ok(41));
}

#[test]
fn result_conversion_works_in_async_functions() {
    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(
        block_on(slot.call_async(async |out| parse_async_opeo("46", out).await)),
        Ok(46)
    );
    assert_eq!(
        block_on(slot.call_async(async |out| parse_async_opeo("bad", out).await)),
        Err(ParseError::InvalidNumber)
    );
}
