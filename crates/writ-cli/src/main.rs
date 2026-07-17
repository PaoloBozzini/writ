//! `writ` — a thin driver over the compiler pipeline.
//!
//! Wiring only; no language logic lives here. The `run` / `check` / `build`
//! subcommands land in a later milestone. For now this is the empty scaffold
//! that anchors the top of the dependency graph (parser + check + interp).

fn main() {
    println!("writ: no subcommands yet — see the milestone backlog");
}
