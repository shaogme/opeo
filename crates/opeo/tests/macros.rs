use std::{
    future::Future,
    num::ParseIntError,
    pin::pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    thread,
};

use opeo::{
    ErrSlot, OResult, Out, bail, bail as abort, ensure, opeo, opeo_try, opeo_try as try_opeo,
};

#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    Empty,
    Number(ParseIntError),
    Zero,
    Missing,
}

impl From<ParseIntError> for ParseError {
    fn from(error: ParseIntError) -> Self {
        Self::Number(error)
    }
}

struct Counter {
    value: u32,
}

impl Counter {
    #[opeo]
    fn add_parsed(&self, input: &str) -> Result<u32, ParseError> {
        let amount = input.parse::<u32>()?;
        Ok(self.value + amount)
    }

    #[opeo]
    fn nested_leaf(&self, amount: u32) -> Result<u32, ParseError> {
        if amount == 0 {
            return Err(ParseError::Zero);
        }
        Ok(self.value + amount)
    }

    #[opeo]
    fn nested_through_wrapper(&self, amount: u32) -> Result<u32, ParseError> {
        let value = self.nested_leaf_std(amount)?;
        Ok(value + 1)
    }

    #[opeo]
    fn nested_through_same_slot(&self, amount: u32) -> Result<u32, ParseError> {
        let value = opeo_try!(out, self.nested_leaf(amount, out.reborrow()));
        Ok(value + 1)
    }

    #[opeo(wrapper = false)]
    fn add_without_wrapper(&self, amount: u32) -> Result<u32, ParseError> {
        Ok(self.value + amount)
    }

    #[opeo(wrapper = increment_std)]
    fn increment(&mut self, amount: u32) -> Result<u32, ParseError> {
        if amount == 0 {
            return Err(ParseError::Zero);
        }
        self.value += amount;
        Ok(self.value)
    }

    #[opeo(wrapper = consume_std)]
    fn consume(self) -> Result<u32, ParseError> {
        Ok(self.value)
    }

    #[opeo]
    async fn add_async(&self, input: &str) -> Result<u32, ParseError> {
        let amount = parse_async(input).await?;
        Ok(self.value + amount)
    }

    #[opeo]
    const fn add_const(&self, amount: u32) -> Result<u32, ParseError> {
        if amount == 0 {
            Err(ParseError::Zero)
        } else {
            Ok(self.value + amount)
        }
    }
}

async fn parse_async(input: &str) -> Result<u32, ParseIntError> {
    input.parse()
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

const CONST_METHOD_RESULT: Result<u32, ParseError> = Counter { value: 5 }.add_const_std(7);

type ParseResult<T, E> = Result<T, E>;
type OptionalParseResult<T, E> = Result<Option<T>, E>;
type ReversedParseResult<E, T> = Result<T, E>;

mod qualified_result_aliases {
    pub type Parsed<T, E> = core::result::Result<T, E>;
}

fn make_parse_result(input: &str) -> Result<u32, ParseError> {
    input.parse().map_err(ParseError::from)
}

macro_rules! result_from_macro {
    ($expression:expr) => {
        $expression
    };
}

macro_rules! parse_with_opeo_try {
    ($out:expr, $input:expr) => {
        opeo_try!($out, $input.parse::<u32>().map_err(ParseError::from))
    };
}

#[opeo(wrapper = parse_helper_tail_std)]
fn parse_helper_tail(input: &str) -> Result<u32, ParseError> {
    make_parse_result(input)
}

#[opeo(wrapper = parse_helper_return_std)]
fn parse_helper_return(input: &str) -> Result<u32, ParseError> {
    return make_parse_result(input);
}

#[opeo(wrapper = parse_result_branches_std)]
fn parse_result_branches(input: Option<&str>) -> Result<u32, ParseError> {
    match input {
        Some(value) => make_parse_result(value),
        None => Err(ParseError::Missing),
    }
}

#[opeo(wrapper = parse_alias_variant_std, ok = u32, error = ParseError)]
fn parse_alias_variant(valid: bool) -> ParseResult<u32, ParseError> {
    if valid {
        ParseResult::Ok(12)
    } else {
        ParseResult::Err(ParseError::Empty)
    }
}

#[opeo(wrapper = parse_macro_result_std)]
fn parse_macro_result(input: &str) -> Result<u32, ParseError> {
    result_from_macro!(make_parse_result(input))
}

#[opeo(wrapper = parse_macro_with_try_std)]
fn parse_macro_with_try(input: &str) -> Result<u32, ParseError> {
    let value = parse_with_opeo_try!(out, input);
    Ok(value)
}

#[opeo(wrapper = parse_number_std)]
fn parse_number(input: &str) -> Result<u32, ParseError> {
    if input.is_empty() {
        bail!(out, ParseError::Empty);
    }

    let number = input.parse::<u32>()?;
    ensure!(out, number > 0, ParseError::Zero);
    let number = opeo_try!(out, Some(number), ParseError::Missing);
    Ok(number)
}

#[opeo(wrapper = parse_text_std)]
fn parse_text<'input>(input: &'input str) -> Result<&'input str, ParseError> {
    if input.is_empty() {
        bail!(out, ParseError::Empty);
    }
    Ok(input)
}

#[opeo(wrapper = parse_slot_argument_std)]
fn parse_slot_argument(slot: u8, __opeo_err_slot: u8) -> Result<u16, ParseError> {
    Ok(u16::from(slot) + u16::from(__opeo_err_slot))
}

#[opeo(wrapper = sum_six_std)]
fn sum_six(a: u8, b: u8, c: u8, d: u8, e: u8, f: u8) -> Result<u16, ParseError> {
    Ok(u16::from(a) + u16::from(b) + u16::from(c) + u16::from(d) + u16::from(e) + u16::from(f))
}

#[opeo(wrapper = parse_alias_std)]
fn parse_alias(input: Option<u32>) -> Result<u32, ParseError> {
    let number = try_opeo!(out, input, ParseError::Missing);
    if number == 0 {
        abort!(out, ParseError::Zero);
    }
    Ok(number)
}

#[opeo(wrapper = parse_typed_ok_std, ok = u32, error = ParseError)]
fn parse_typed_ok(valid: bool) -> ParseResult<u32, ParseError> {
    if valid {
        return Ok::<u32, ParseError>(7);
    }
    Ok::<u32, ParseError>(9)
}

#[opeo(wrapper = parse_optional_result_std, ok = Option<u32>, error = ParseError)]
fn parse_optional_result(valid: bool) -> OptionalParseResult<u32, ParseError> {
    if valid {
        Ok(Some(19))
    } else {
        Err(ParseError::Empty)
    }
}

#[opeo(wrapper = parse_qualified_alias_std, ok = u32, error = ParseError)]
fn parse_qualified_alias(valid: bool) -> qualified_result_aliases::Parsed<u32, ParseError> {
    if valid {
        Ok(21)
    } else {
        Err(ParseError::Missing)
    }
}

#[opeo(wrapper = parse_io_result_std)]
fn parse_io_result(valid: bool) -> std::io::Result<u8> {
    if valid {
        Ok(29)
    } else {
        Err(std::io::Error::other("invalid input"))
    }
}

#[opeo(ok = u32, error = ParseError)]
fn parse_reversed_alias(valid: bool) -> ReversedParseResult<ParseError, u32> {
    if valid {
        Ok(31)
    } else {
        Err(ParseError::Empty)
    }
}

#[opeo]
fn parse_with_default_wrapper(input: &str) -> Result<u32, ParseError> {
    Ok(input.len() as u32)
}

#[opeo(wrapper = false)]
fn parse_without_wrapper(input: &str) -> Result<u32, ParseError> {
    input.parse::<u32>().map_err(ParseError::from)
}

#[opeo(wrapper = sum_pair_std)]
fn sum_pair((left, right): (u8, u8)) -> Result<u16, ParseError> {
    Ok(u16::from(left) + u16::from(right))
}

#[opeo(wrapper = flatten_nested_try_std)]
fn flatten_nested_try(
    value: Result<Result<u32, ParseError>, ParseError>,
) -> Result<u32, ParseError> {
    value?
}

#[opeo(wrapper = flatten_nested_try_return_std)]
fn flatten_nested_try_return(
    value: Result<Result<u32, ParseError>, ParseError>,
) -> Result<u32, ParseError> {
    return value?;
}

#[opeo(wrapper = parse_exported_std)]
#[unsafe(export_name = "opeo_integration_original_symbol")]
fn parse_exported(value: u8) -> Result<u8, ParseError> {
    Ok(value)
}

#[opeo(wrapper = parse_closure_std)]
fn parse_in_closure(input: &str) -> Result<usize, ParseError> {
    let parse = || -> Result<usize, ParseIntError> { input.parse() };
    let parsed = parse()?;
    Ok(parsed)
}

#[derive(Debug, PartialEq, Eq)]
struct GenericFailure;

#[opeo(wrapper = clone_std)]
fn clone_value<T>(value: T) -> Result<T, GenericFailure>
where
    T: Clone,
{
    Ok(value.clone())
}

#[opeo(wrapper = higher_ranked_lifetimes_std)]
fn higher_ranked_lifetimes<F>(_callback: F) -> Result<(), ParseError>
where
    F: for<'__opeo_slot, '__opeo_borrow> Fn(&'__opeo_slot (), &'__opeo_borrow ()),
{
    Ok(())
}

fn nested_failure<'slot, 'borrow>(mut out: Out<'slot, 'borrow, ParseError>) -> OResult<'slot, ()> {
    let value = opeo_try!(out, nested_leaf(out.reborrow()));
    assert_eq!(value, 3);
    OResult::success(())
}

fn nested_leaf<'slot, 'borrow>(out: Out<'slot, 'borrow, ParseError>) -> OResult<'slot, u8> {
    OResult::failed(out.fail(ParseError::Empty))
}

fn missing_value<'slot, 'borrow>(
    mut out: Out<'slot, 'borrow, ParseError>,
) -> OResult<'slot, usize> {
    let _value = opeo_try!(out, None::<usize>, ParseError::Missing);
    OResult::success(0)
}

#[test]
fn attribute_macro_builds_opeo_and_standard_functions() {
    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(|out| parse_number("42", out)), Ok(42));
    assert_eq!(parse_number_std("42"), Ok(42));
    assert!(matches!(
        parse_number_std("nope"),
        Err(ParseError::Number(_))
    ));
    assert_eq!(parse_number_std("0"), Err(ParseError::Zero));
    assert_eq!(parse_number_std(""), Err(ParseError::Empty));
    assert_eq!(parse_text_std(""), Err(ParseError::Empty));
}

#[test]
fn attribute_macro_can_omit_standard_wrappers() {
    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(|out| parse_without_wrapper("42", out)), Ok(42));
    assert!(matches!(
        slot.call(|out| parse_without_wrapper("invalid", out)),
        Err(ParseError::Number(_))
    ));

    let counter = Counter { value: 5 };
    assert_eq!(slot.call(|out| counter.add_without_wrapper(7, out)), Ok(12));
}

#[test]
fn attribute_macro_supports_struct_instance_methods() {
    let mut counter = Counter { value: 5 };
    assert_eq!(counter.add_parsed_std("7"), Ok(12));
    assert!(matches!(
        counter.add_parsed_std("invalid"),
        Err(ParseError::Number(_))
    ));

    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(|out| counter.add_parsed("8", out)), Ok(13));
    assert_eq!(counter.increment_std(3), Ok(8));
    assert_eq!(counter.increment_std(0), Err(ParseError::Zero));
    assert_eq!(slot.call(|out| counter.increment(2, out)), Ok(10));
    assert_eq!(counter.consume_std(), Ok(10));
}

#[test]
fn opeo_methods_can_call_other_opeo_methods_through_wrapper_or_same_slot() {
    let counter = Counter { value: 5 };
    assert_eq!(counter.nested_through_wrapper_std(3), Ok(9));
    assert_eq!(counter.nested_through_wrapper_std(0), Err(ParseError::Zero));

    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(
        slot.call(|out| counter.nested_through_same_slot(3, out)),
        Ok(9)
    );
    assert_eq!(
        slot.call(|out| counter.nested_through_same_slot(0, out)),
        Err(ParseError::Zero)
    );
}

#[test]
fn attribute_macro_supports_async_methods() {
    let counter = Counter { value: 5 };
    assert_eq!(block_on(counter.add_async_std("7")), Ok(12));
    assert!(matches!(
        block_on(counter.add_async_std("invalid")),
        Err(ParseError::Number(_))
    ));

    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(
        block_on(slot.call_async(async |out| counter.add_async("8", out).await)),
        Ok(13)
    );
    assert!(matches!(
        block_on(slot.call_async(async |out| counter.add_async("invalid", out).await)),
        Err(ParseError::Number(_))
    ));
}

#[test]
fn attribute_macro_preserves_const_standard_wrappers() {
    assert_eq!(CONST_METHOD_RESULT, Ok(12));

    let counter = Counter { value: 5 };
    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(|out| counter.add_const(3, out)), Ok(8));
    assert_eq!(
        slot.call(|out| counter.add_const(0, out)),
        Err(ParseError::Zero)
    );
}

#[test]
fn attribute_macro_preserves_input_lifetimes() {
    let input = String::from("value");
    assert_eq!(parse_text_std(&input), Ok("value"));

    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(|out| parse_text(&input, out)), Ok("value"));
}

#[test]
fn attribute_macro_supports_renamed_error_macros() {
    let mut slot = ErrSlot::<ParseError>::new();

    assert_eq!(slot.call(|out| parse_alias(Some(7), out)), Ok(7));
    assert_eq!(
        slot.call(|out| parse_alias(None, out)),
        Err(ParseError::Missing)
    );
    assert_eq!(parse_alias_std(Some(0)), Err(ParseError::Zero));
}

#[test]
fn attribute_macro_rewrites_typed_ok_values_and_accepts_result_aliases() {
    let mut slot = ErrSlot::<ParseError>::new();

    assert_eq!(slot.call(|out| parse_typed_ok(true, out)), Ok(7));
    assert_eq!(slot.call(|out| parse_typed_ok(false, out)), Ok(9));
    assert_eq!(parse_typed_ok_std(true), Ok(7));
    assert_eq!(parse_typed_ok_std(false), Ok(9));
}

#[test]
fn attribute_macro_supports_aliases_with_transformed_success_types() {
    let mut slot = ErrSlot::<ParseError>::new();

    assert_eq!(
        slot.call(|out| parse_optional_result(true, out)),
        Ok(Some(19))
    );
    assert_eq!(
        slot.call(|out| parse_optional_result(false, out)),
        Err(ParseError::Empty)
    );
    assert_eq!(parse_optional_result_std(true), Ok(Some(19)));
    assert_eq!(parse_optional_result_std(false), Err(ParseError::Empty));
}

#[test]
fn attribute_macro_accepts_qualified_generic_result_aliases() {
    assert_eq!(parse_qualified_alias_std(true), Ok(21));
    assert_eq!(parse_qualified_alias_std(false), Err(ParseError::Missing));
}

#[test]
fn attribute_macro_supports_result_aliases_with_defaulted_error_types() {
    assert!(matches!(parse_io_result_std(true), Ok(29)));
    assert!(parse_io_result_std(false).is_err());
}

#[test]
fn attribute_macro_supports_result_aliases_with_reordered_generic_parameters() {
    assert_eq!(parse_reversed_alias_std(true), Ok(31));
    assert_eq!(parse_reversed_alias_std(false), Err(ParseError::Empty));
}

#[test]
fn attribute_macro_generates_default_wrapper_names() {
    assert_eq!(parse_with_default_wrapper_std("abc"), Ok(3));
}

#[test]
fn attribute_macro_forwards_destructured_parameters() {
    assert_eq!(sum_pair_std((7, 8)), Ok(15));

    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(|out| sum_pair((9, 10), out)), Ok(19));
}

#[test]
fn attribute_macro_bridges_nested_result_question_marks() {
    assert_eq!(flatten_nested_try_std(Ok(Ok(29))), Ok(29));
    assert_eq!(
        flatten_nested_try_std(Ok(Err(ParseError::Missing))),
        Err(ParseError::Missing)
    );
    assert_eq!(
        flatten_nested_try_std(Err(ParseError::Zero)),
        Err(ParseError::Zero)
    );

    assert_eq!(flatten_nested_try_return_std(Ok(Ok(31))), Ok(31));
    assert_eq!(
        flatten_nested_try_return_std(Ok(Err(ParseError::Missing))),
        Err(ParseError::Missing)
    );
    assert_eq!(
        flatten_nested_try_return_std(Err(ParseError::Zero)),
        Err(ParseError::Zero)
    );
}

#[test]
fn attribute_macro_bridges_result_expressions_and_alias_variants() {
    assert_eq!(parse_helper_tail_std("13"), Ok(13));
    assert!(matches!(
        parse_helper_tail_std("bad"),
        Err(ParseError::Number(_))
    ));
    assert_eq!(parse_helper_return_std("14"), Ok(14));
    assert_eq!(parse_result_branches_std(Some("15")), Ok(15));
    assert_eq!(parse_result_branches_std(None), Err(ParseError::Missing));
    assert_eq!(parse_alias_variant_std(true), Ok(12));
    assert_eq!(parse_alias_variant_std(false), Err(ParseError::Empty));
    assert_eq!(parse_macro_result_std("16"), Ok(16));
    assert_eq!(parse_macro_with_try_std("17"), Ok(17));
    assert!(matches!(
        parse_macro_with_try_std("bad"),
        Err(ParseError::Number(_))
    ));
}

#[test]
fn attribute_macro_does_not_duplicate_symbol_export_attributes() {
    assert_eq!(parse_exported_std(9), Ok(9));
}

#[test]
fn attribute_macro_supports_generic_functions() {
    assert_eq!(clone_std(String::from("copy")), Ok(String::from("copy")));
    assert_eq!(higher_ranked_lifetimes_std(|_, _| {}), Ok(()));
}

#[test]
fn generated_wrapper_does_not_shadow_a_slot_argument() {
    assert_eq!(parse_slot_argument_std(83, 4), Ok(87));
}

#[test]
fn attribute_macro_accepts_six_input_parameters() {
    assert_eq!(sum_six_std(1, 2, 3, 4, 5, 6), Ok(21));

    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(|out| sum_six(1, 2, 3, 4, 5, 6, out)), Ok(21));
}

#[test]
fn attribute_macro_leaves_nested_closure_question_marks_alone() {
    assert_eq!(parse_closure_std("27"), Ok(27));
    assert!(matches!(
        parse_closure_std("bad"),
        Err(ParseError::Number(_))
    ));
}

#[test]
fn nested_failure_propagates_through_reborrow() {
    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(nested_failure), Err(ParseError::Empty));
}

#[test]
fn opeo_try_supports_result_and_option_paths() {
    fn parse<'slot, 'borrow>(
        input: &str,
        mut out: Out<'slot, 'borrow, ParseError>,
    ) -> OResult<'slot, usize> {
        let value = opeo_try!(out, input.parse::<usize>().map_err(ParseError::from));
        let present = opeo_try!(out, Some(value), ParseError::Missing);
        OResult::success(present)
    }

    let mut slot = ErrSlot::<ParseError>::new();
    assert_eq!(slot.call(|out| parse("8", out)), Ok(8));
    assert!(matches!(
        slot.call(|out| parse("bad", out)),
        Err(ParseError::Number(_))
    ));
    assert_eq!(slot.call(missing_value), Err(ParseError::Missing));
}
