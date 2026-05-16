#[test]
#[cfg(not(miri))]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("src/collectors/mark_sweep_branded/tests/ui/*.rs");
}
