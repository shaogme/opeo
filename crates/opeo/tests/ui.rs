#[test]
fn opeo_runtime_rejects_invalid_failure_states() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/*.rs");
}
