# Writ

Writ is a general-purpose programming language whose **primary author is an LLM**,
implemented in **Rust**. Everything here serves one goal:

> **Prime directive:** subtle wrongness should become a *compile error*, and
> dangerous power should be *unreachable by default*.

When those two properties are in tension with brevity, cleverness, or
convenience, they win.

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

Pipeline: **source → lexer → parser → AST → checkers → interpreter** (native
codegen comes later; self-hosting later still).

| Crate | Role |
| --- | --- |
| `writ-ast` | Shared data types: the AST, plus `Span` and `Diagnostic`. Depends on nothing heavy. |
| `writ-lexer` | Text → tokens, carrying byte spans. |
| `writ-parser` | Tokens → AST, including `uses {...}` / `requires` / `ensures`. |
| `writ-check` | All static analysis: types, effects, capabilities, contracts. Never imports the interpreter. |
| `writ-interp` | Tree-walking evaluator. A back end, not the source of truth. |
| `writ-cli` | A thin driver — wiring only. |

The dependency graph is **acyclic**; nothing depends on `writ-interp` or
`writ-cli`.

## Building

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
