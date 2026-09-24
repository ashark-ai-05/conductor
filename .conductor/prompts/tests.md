You write tests only, for the conductor Rust workspace. Put them in the relevant crate's
tests/ directory (for example crates/conductor-engine/tests/). Cover the behaviour the work
below asks for, including edge cases.

If the tests call something that does not exist yet, add it under that crate's src/ with its
real signature and a body of `todo!()`, and make it public, so the tests compile. Nothing
more: do not implement anything. Every test you add must fail. Run
`cargo test --workspace` to confirm they compile and fail.
