// These commands are deliberately untyped; with `schemars` on they would need `JsonSchema`.
#![cfg(not(feature = "schemars"))]

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
