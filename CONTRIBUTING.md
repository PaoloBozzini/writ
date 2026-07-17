# Contributing to Writ

Writ's primary author is an LLM, so these conventions are written to be followed
by humans and models alike. The rules exist to preserve one property: **local
reasoning**. A change to one function must never be able to silently break a
distant one.

## The gate

Every change must pass all three checks before a PR. This is exactly what CI
runs:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

Never weaken a check to make a build pass — no removing `-D warnings`, no
`#[allow]` to silence a real lint, no deleting a failing test.

## Workflow

- **Issues drive the work.** Pick up work through the tracker. Priority order:
  `priority:p0` > `p1` > `p2`, then milestone `M0` … `M8`.
- **One issue per branch per PR.** Branch names: `feat/<n>-<slug>`,
  `fix/<n>-<slug>`, `chore/…`, `docs/…`.
- **Conventional commits:** e.g. `feat(lexer): add byte spans to tokens`. Keep
  commits small and logical.
- **PRs** state what changed, why, how it was verified, the acceptance criteria
  as a ticked checklist, and `Closes #<n>`.
- **Stay in scope.** Tempting adjacent work becomes a **new issue**, never scope
  creep in the current PR.

## Testing rules

- **Test-first.** Write the failing test that encodes the acceptance criteria
  before the implementation.
- **Negative tests are mandatory for safety features.** For anything touching
  capabilities, contracts, or effects, include a test proving that code which
  *should be refused* is refused, with the right diagnostic. The rejection is
  the feature.
- **Golden/snapshot tests** for the lexer and parser.
- **Determinism.** Tests and builds must be reproducible. Never depend on
  ordering, timing, or environment.

## Architecture invariants

- The crate dependency graph is **acyclic**. Nothing depends on `writ-interp` or
  `writ-cli`.
- The **checkers never import the interpreter**, and the interpreter never does
  static analysis.
- **No ambient authority and no global mutable state** — no `static mut`, global
  registries, or hidden singletons.

## Errors are an API

Diagnostics are consumed by models, not just humans. Use the single shared
`Diagnostic` type from `writ-ast`; every diagnostic carries a stable **code**, a
**span**, and a **message**, is **serializable**, and output is **deterministic**
(same input, same diagnostics, same order).

## Guardrails

- Never merge PRs, never push to `main`, never force-push.
- Never rewrite or delete someone else's work.
- When genuinely blocked — ambiguous spec, a decision only a human should make —
  stop and ask on the issue with a specific question. Prefer stopping over
  shipping something wrong.
