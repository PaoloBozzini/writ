//! Effectful built-in functions, shared by the honesty (`effects`) and authority
//! passes.
//!
//! Most built-ins (`print`, `sanitize`, the text operations) are pure. File I/O
//! is the first that performs a real effect, so a call to it must be an **effect
//! site** exactly like a call to a user function that declares `uses {...}`.
//! Both passes fold this table into the effect map they build from user
//! signatures. It is plain data — reading it is not a cross-pass import, so the
//! passes stay independent of each other.

/// Built-in name → the effects a call to it performs.
pub const EFFECTFUL_BUILTINS: &[(&str, &[&str])] =
    &[("read_file", &["Read"]), ("write_file", &["Write"])];
