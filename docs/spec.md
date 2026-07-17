# The Writ Language Specification

> **Status:** skeleton. This is a *living document* — sections are filled in as
> the corresponding milestones land. Every feature is framed in terms of Writ's
> two orthogonal pillars.

Writ is a general-purpose language whose primary author is an LLM. Its design is
governed by one prime directive:

> Subtle wrongness should become a **compile error**, and dangerous power should
> be **unreachable by default**.

The specification is organized around the two pillars that make those properties
hold. They are **orthogonal**: capabilities govern *authority* (what code may
do) and never speak about correctness; contracts govern *correctness* (whether
the answer is right) and never speak about authority.

- [1. Syntax](#1-syntax)
- [2. Type system](#2-type-system)
- [3. Capabilities](#3-capabilities-authority)
- [4. Contracts](#4-contracts-correctness)

---

## 1. Syntax

*Stub.* The concrete grammar of Writ: lexical structure (tokens, literals,
identifiers, keywords), expressions and operator precedence, statements, and the
declaration forms.

Signatures are the load-bearing surface of the language — they declare a
function's effects via `uses {...}` and its contracts via `requires` / `ensures`
— so the grammar gives those clauses first-class syntax rather than treating
them as annotations.

*To be specified:* lexical grammar, expression grammar with precedence, statement
forms, function and signature syntax.

---

## 2. Type system

*Stub.* Writ is strongly and statically typed with **no implicit coercion** and
**non-null by default**. The goal is to turn "plausible but wrong" into "doesn't
compile."

Effects live in the type system: a signature tells you what a function can do.
Sum types with **exhaustive `match`** ensure every case is handled.

*To be specified:* primitive and compound types, sum types and exhaustiveness,
the effect rows carried by signatures, and the absence of implicit conversions.

---

## 3. Capabilities (authority)

*Stub.* The authority pillar — "what may this code do?"

- **No ambient authority.** An effect (file write, network, spawning a process)
  is reachable only if an **unforgeable capability token** was passed into the
  function as a parameter.
- Capability values are **un-constructible in user code** and **parameter-only**.
  A function with no capability parameter is sandboxed *by construction*.
- Signatures declare effects with `uses {...}`. At every **effect site** the
  checker enforces two things: the caller **holds** the capability (authority)
  and the signature **declared** the effect (honesty).
- Untrusted data is `Tainted<T>` and cannot reach a **sink** (shell, query)
  without passing a `sanitize` boundary.

*To be specified:* capability types (`Cap<T>`), the root capability and
narrowing (`grant`), escape semantics (capture / return / storage), the honesty
check, and taint tracking.

---

## 4. Contracts (correctness)

*Stub.* The correctness pillar — "is the answer right?"

- `requires` (preconditions) and `ensures` (postconditions) attach to a
  signature.
- Enforced at runtime per input, and — where feasible — proven statically for
  all inputs via SMT.
- **Blame direction is load-bearing:** a failed precondition blames the
  **caller**; a failed postcondition blames the **implementation**. Diagnostics
  preserve this direction because a generate-check-repair loop relies on it.

*To be specified:* the contract expression language, runtime checking with
blame, and the optional SMT-backed static verification pass.

---

## Appendix: diagnostics are an API

Diagnostics are consumed by models, not just humans. Every diagnostic carries a
stable **code**, a **span**, and a **message**, is **serializable**, and output
is **deterministic** (same input → same diagnostics, same order). The
specification records the stable code for each rule alongside the rule itself.
