#[test]
fn opeo_macro_reports_invalid_inputs() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/*.rs");
}
