# Writ

Writ is a general-purpose programming language whose **primary author is an LLM**,
implemented in **Rust**. Everything here serves one goal:

> **Prime directive:** subtle wrongness should become a *compile error*, and
> dangerous power should be *unreachable by default*.

When those two properties are in tension with brevity, cleverness, or
convenience, they win.

## Getting started

New to Writ? Start with **[docs/getting-started.md](docs/getting-started.md)** —
it takes you from building the toolchain to writing, running, compiling, and
verifying real programs, with a feature-by-feature tour.

```bash
cargo run -p writ-cli -- run examples/hello.writ
# Hello, Writ!
```

## The two pillars

Writ's safety rests on two **orthogonal** static checkers. They are independent
passes that never reference each other — either can be disabled without breaking
the other.

### Capabilities — *authority* ("what may this code do?")

- **No ambient authority.** An effect (file write, network, spawning a process)
  is reachable only if an **unforgeable capability token** was passed into the
  function as a parameter.
- Capability values are **un-constructible in user code** and **parameter-only**.
  A function with no capability parameter is sandboxed *by construction*, not by
  convention.
- Signatures declare effects with `uses {...}`. At every effect site the checker
  enforces that the caller **holds** the capability (authority) and that the
  signature **declared** the effect (honesty).
- Untrusted data is `Tainted<T>` and cannot reach a **sink** (shell, query)
  without passing a `sanitize` boundary.

### Contracts — *correctness* ("is the answer right?")

- `requires` (preconditions) and `ensures` (postconditions) attach to a
  signature.
- Enforced at runtime per input, and — where feasible — proven statically for
  all inputs via SMT.
- **Blame direction is load-bearing:** a failed precondition blames the
  **caller**; a failed postcondition blames the **implementation**.

Capabilities never speak about correctness; contracts never speak about
authority.

## Architecture

Pipeline: **source → lexer → parser → AST → checkers → lower → interpreter / C
codegen** (self-hosting later still).

| Crate | Role |
| --- | --- |
| `writ-ast` | Shared data types: the AST, plus `Span` and `Diagnostic`. Depends on nothing heavy. |
| `writ-lexer` | Text → tokens, carrying byte spans. |
| `writ-parser` | Tokens → AST, including `uses {...}` / `requires` / `ensures`. |
| `writ-check` | All static analysis: types, effects, capabilities, taint, contracts. Never imports a back end. |
| `writ-lower` | AST → AST: links multi-module programs and desugars contracts into one shared form both back ends consume. |
| `writ-interp` | Tree-walking evaluator. A back end, not the source of truth. |
| `writ-codegen` | The native back end: emits C, compiled by the system C compiler. Agrees with the interpreter on a differential corpus. |
| `writ-verify` | Optional SMT-backed contract proving (behind a solver trait). |
| `writ-cli` | A thin driver — wiring only. |

The dependency graph is **acyclic**; nothing depends on a back end (`writ-interp`
/ `writ-codegen`) or `writ-cli`.

## Using it

```bash
writ check file.writ   # static checks only (runs nothing)
writ run   file.writ   # check, then interpret `main`
writ build file.writ   # check, then compile to a native binary (via C)
writ verify file.writ  # check, then prove contracts via SMT (optional)
```

(Run through Cargo as `cargo run -p writ-cli -- <command> file.writ`, or
`cargo install --path crates/writ-cli` for a `writ` binary.) See
[docs/getting-started.md](docs/getting-started.md) for a full walkthrough.

## Building from source

```bash
cargo build
cargo test --all

# The CI gate — run all three before opening a PR:
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Work is driven by GitHub issues and
milestones `M0`–`M8`. One issue per branch per PR; a human merges.

## License

[MIT](LICENSE).
