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
- [5. Modules](#5-modules)

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

### Escape semantics (ARCH-02): capabilities are second-class

Making `Cap<T>` un-constructible and parameter-only controls how authority is
*introduced*. It must also control how authority *escapes*, or the "no
capability parameter ⇒ sandboxed" guarantee leaks. **Decision: capabilities are
second-class.** A `Cap<T>` value may only be:

- received as a function parameter, and
- passed on as an argument to another call.

It may **not**:

- be **returned** from a function,
- be **stored** in a data structure, or
- be **captured** by a closure.

This is the simplest provably-sound option, and today it is the *only* sound one:
the language has no closures, structs, or collections, so return position is the
sole escape channel that exists. Forbidding it keeps the invariant **"a function
with no capability parameter can reach no effects"** — authority flows strictly
downward through explicit arguments and can never be smuggled back up or stashed
for later. If first-class capabilities are ever wanted, the effect system would
have to track capability flow through captures and returns; that is deliberately
out of scope.

**Runtime unforgeability.** The interpreter represents a capability as an opaque
value that no surface syntax can construct — there is no literal, constructor, or
built-in that yields one. The only capability a program ever sees is the root
capability the runtime hands to `main`, and values narrowed from it via `grant`.
Because the checker also forbids escape, a captured or returned token cannot exist
at runtime either.

### Sandboxing (the check step and ambient authority)

Writ is **sandboxed by construction**, on two levels:

- **The check step executes nothing.** `writ check` reads the AST and analyzes
  it — it never runs the program, opens a file, or touches the network. There
  is no compile-time evaluation, so there is nothing to sandbox and nothing to
  escape: the strongest sandbox is not running code at all.
- **The interpreter has no ambient authority.** The only built-in is `print`
  (to an in-memory buffer). There is no filesystem or network primitive in the
  language for a program to call, so even `writ run` cannot reach the host. A
  program that names `read_file` or `http_get` is simply calling an unknown
  function; it fails closed, having touched nothing. When effectful built-ins
  are eventually added, they will take a `Cap<T>` and be governed by the
  capability checker — authority will still never be ambient.

*To be specified:* capability types (`Cap<T>`), the root capability and
narrowing (`grant`), the authority check at effect sites, and taint tracking.

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
- **Predicates are pure.** A `requires` / `ensures` predicate may not call an
  effectful function (one with a non-empty `uses {...}`). Contracts *assert*
  correctness; they must not *do* anything. Because the interpreter evaluates
  predicates at runtime, an effectful predicate would perform an effect the
  signature never declared — so it is a compile error (`E0102`). A predicate may
  freely call pure (effect-free) functions.

*To be specified:* the contract expression language, runtime checking with
blame, and the optional SMT-backed static verification pass.

---

## 5. Modules

A program is a collection of **named modules**. Each source file is one module;
the build driver assembles the module set from files. Modules give the language
multi-file programs — a prerequisite for anything larger than a toy, including
self-hosting the compiler in Writ.

Modules interact with **both pillars**, so they are a language-design concern,
not mere packaging. The governing principle is the same one the whole language
rests on: **nothing is visible or reachable across a boundary unless it is
explicitly handed across.** A module is the locality boundary — the
module-level twin of "unreachable by default."

### Imports and exports

- A file may begin with `import <name>` declarations, each naming another module
  to bring into scope. Imports come first, before any item.
- A top-level item (`fn` or `type`) is private to its module unless prefixed with
  `export`. Only exported items are visible to importers.
- A qualified access `module.item` reaches an item in an imported module.

Visibility is enforced by the resolver pass (`writ-check`), which is
self-contained and imports no other checker. Its rules, with stable diagnostic
codes:

- **`R0001`** — every `import` must name a module that exists.
- **`R0002`** — the base of a `module.member` access must be an imported module.
- **`R0003`** — the named item must exist in that module.
- **`R0004`** — the named item must be `export`ed. Using a **private** item
  across a boundary is refused — the module-level form of "unreachable by
  default."
- **`R0005`** — the import graph must be **acyclic**; a cycle is refused.

Diagnostics are emitted in deterministic (module-sorted) order, per the
diagnostics-are-an-API rule.

### Capabilities across boundaries

Modules require **no special capability machinery**, and that is by design.
Capabilities are **parameter-only and second-class** (§3): a `Cap<T>` can only
be received as a parameter and passed on as an argument — never returned,
stored, or captured. Those rules are *signature-local*, so they hold identically
whether a call stays within a module or crosses into an imported one.

The consequence is the property that matters: **a module cannot ambiently
re-export authority.** Importing a module grants access to its exported
*signatures*, never to any capability — there is no capability value a module
could expose, because none can be constructed, returned, or stored. Authority
still flows in exactly one way — downward, as an explicit argument at a call —
and a boundary changes nothing about that. A module with no capability parameter
threaded into it remains sandboxed by construction.

### Effects across boundaries

An exported signature's `uses {...}` set is its **honest, public declaration** of
what it may do. A caller in another module sees exactly that declaration, and the
authority check applies at the cross-module call's effect site just as it does
locally: the caller must hold a capability for each declared effect. The honesty
check, in turn, holds the *callee's* body to its own declaration within its own
module. Neither check needs to know a call crossed a boundary — effects are
carried on signatures, and signatures are what crosses.

*To be specified:* the module-naming scheme the driver derives from file paths,
whether re-exports are permitted, and selective/aliased imports.

---

## Pass architecture and annotations

The compiler runs several analysis passes over one shared, immutable AST: the
type checker, the effect system (inference + honesty), the capability authority
checker, and the contract checker. Later passes need earlier passes' results,
and a back end will need types. **Decision (ARCH-04): those results live in
per-pass side tables keyed by a stable node id — not in a mutated or re-wrapped
AST.**

- The AST is the **stable shared contract** and stays immutable. Each pass reads
  the AST and writes its facts into its own `Map<NodeId, _>` (for example
  `types: Map<NodeId, Type>`, `effects: Map<NodeId, EffectSet>`). `writ-ast`
  therefore stays dependency-light — spans, node types, and the [`NodeId`] key
  type only, with no imports of any checker.
- Rejected alternative: a distinct typed AST / HIR produced after checking. It
  attaches results directly to nodes but adds a second tree to keep in sync with
  the front end; the side-table approach keeps consumers decoupled and matches
  Writ's locality principle.

**Pass dependency rule.** Passes are otherwise independent (either pillar can be
disabled without the other):

- `types` depends on nothing.
- `effects` (inference + honesty) depends on nothing.
- `authority` consumes `effects` facts (does the caller hold a token for each
  effect performed?) — this is the one allowed cross-pass dependency.
- `contracts` depends on neither capabilities nor effects.
- A future `codegen` back end consumes `types`.

*Implementation note:* the `NodeId` key type exists in `writ-ast`; threading an
id onto every node is done when the first consumer that must resolve a fact **by
node identity** lands (side-table population), so the AST is not churned ahead of
need.

---

## Appendix: diagnostics are an API

Diagnostics are consumed by models, not just humans. Every diagnostic carries a
stable **code**, a **span**, and a **message**, is **serializable**, and output
is **deterministic** (same input → same diagnostics, same order). The
specification records the stable code for each rule alongside the rule itself.
