//! **Optional** SMT-backed static verification of contracts (issue #27).
//!
//! Runtime contract checking (via `writ-lower` + a back end) enforces `ensures`
//! per input. This pass upgrades that to a proof *for all inputs* where it can:
//! it translates each contract into an SMT-LIB2 query and asks a solver to
//! discharge it. Contracts the solver cannot prove are **reported before
//! execution** rather than silently deferred to runtime.
//!
//! Two properties are load-bearing (both in #27's acceptance criteria):
//!
//! - **The pass is optional and non-blocking.** It emits only warnings, never
//!   errors, so it never rejects a program the runtime path would accept. A
//!   caller runs it explicitly (e.g. `writ`'s `verify` subcommand); nothing in
//!   `check` / `run` / `build` depends on it.
//! - **The solver is isolated behind a trait.** The default build needs no
//!   solver installed: the bundled [`Z3Cli`] shells out to a `z3` binary if one
//!   is present and otherwise reports itself [`unavailable`](Solver::available),
//!   in which case verification is skipped. Any other engine can be dropped in
//!   by implementing [`Solver`].
//!
//! Soundness note: integers are modelled as unbounded ℤ, so a proof assumes no
//! overflow (overflow traps at runtime; it never yields a wrong answer). The
//! supported fragment is documented in [`smt`].

mod smt;

use std::io::Write as _;
use std::process::{Command, Stdio};

use writ_ast::{Diagnostic, Item, Module};

pub use smt::{function_queries, Query};

/// A solver's verdict on one query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// The negated goal is unsatisfiable — the contract is **proved**.
    Unsat,
    /// A model exists — the contract can fail; the solver found a counterexample.
    Sat,
    /// The solver neither proved nor refuted it.
    Unknown,
}

/// An SMT solver. Kept deliberately small so the engine is swappable and the
/// default build depends on no particular solver.
pub trait Solver {
    /// Whether this solver can actually be used right now (e.g. the binary is
    /// installed). When `false`, verification is skipped.
    fn available(&self) -> bool;
    /// Decide one SMT-LIB2 script.
    fn solve(&self, script: &str) -> Answer;
}

/// A [`Solver`] that shells out to the `z3` command-line tool over stdin. Adds
/// no build dependency: if `z3` is not installed, [`available`](Solver::available)
/// is `false` and the pass is skipped.
#[derive(Debug, Default, Clone)]
pub struct Z3Cli;

impl Z3Cli {
    /// The solver binary to run: `$WRIT_SMT` if set, otherwise `z3`.
    fn binary() -> String {
        std::env::var("WRIT_SMT").unwrap_or_else(|_| "z3".to_string())
    }
}

impl Solver for Z3Cli {
    fn available(&self) -> bool {
        Command::new(Self::binary())
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn solve(&self, script: &str) -> Answer {
        // `z3 -in` reads an SMT-LIB2 script from stdin and prints the result.
        let mut child = match Command::new(Self::binary())
            .arg("-in")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return Answer::Unknown,
        };
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(script.as_bytes());
        }
        let Ok(output) = child.wait_with_output() else {
            return Answer::Unknown;
        };
        parse_answer(&String::from_utf8_lossy(&output.stdout))
    }
}

/// Parse a solver's `(check-sat)` reply — the first meaningful token.
fn parse_answer(stdout: &str) -> Answer {
    match stdout.split_whitespace().next() {
        Some("unsat") => Answer::Unsat,
        Some("sat") => Answer::Sat,
        _ => Answer::Unknown,
    }
}

/// Statically verify a module's contracts with `solver`, returning warnings for
/// every `ensures` clause that could not be proved. An empty result means every
/// verifiable clause was discharged (or the solver was unavailable — see below).
///
/// Diagnostics (all **warnings**, so the pass never blocks compilation):
/// - `V0001` — a function has `ensures` but lies outside the SMT fragment, so it
///   was not statically verified.
/// - `V0002` — the solver found a counterexample: the postcondition can fail.
/// - `V0003` — the solver could not decide the postcondition.
///
/// If the solver is unavailable, verification is skipped and the result is
/// empty; callers that want to surface that should check [`Solver::available`].
#[must_use]
pub fn verify(module: &Module, solver: &dyn Solver) -> Vec<Diagnostic> {
    if !solver.available() {
        return Vec::new();
    }
    verify_checked(module, solver)
}

/// Verify, and report whether the solver was available — probing availability
/// **exactly once**. Prefer this over pairing a separate [`Solver::available`]
/// call with [`verify`] (which probes again): a driver processing a program
/// should not spawn one `z3 --version` per module. Returns `(warnings,
/// available)`; when unavailable the warnings are empty.
#[must_use]
pub fn verify_reporting_availability(
    module: &Module,
    solver: &dyn Solver,
) -> (Vec<Diagnostic>, bool) {
    if !solver.available() {
        return (Vec::new(), false);
    }
    (verify_checked(module, solver), true)
}

/// The verification core, run only once the solver is known to be available.
/// Split out so the availability probe happens in exactly one place per entry
/// point.
fn verify_checked(module: &Module, solver: &dyn Solver) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for item in &module.items {
        let Item::Function(f) = item else { continue };
        if f.signature.ensures.is_empty() {
            continue;
        }
        let queries = function_queries(f);
        if queries.is_empty() {
            diagnostics.push(Diagnostic::warning(
                "V0001",
                f.signature.span,
                format!(
                    "function `{}` was not statically verified: its contract is outside the supported SMT fragment",
                    f.signature.name
                ),
            ));
            continue;
        }
        for q in queries {
            match solver.solve(&q.script) {
                Answer::Unsat => {} // Proved for all inputs.
                Answer::Sat => diagnostics.push(Diagnostic::warning(
                    "V0002",
                    q.clause_span,
                    "postcondition not proved: the solver found inputs that violate it",
                )),
                Answer::Unknown => diagnostics.push(Diagnostic::warning(
                    "V0003",
                    q.clause_span,
                    "postcondition could not be proved by the solver",
                )),
            }
        }
    }
    diagnostics
}
