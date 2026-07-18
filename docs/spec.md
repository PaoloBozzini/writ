# The Writ Language Specification

> **Status:** a *living document* — it tracks what the implementation actually
> does, and grows as the language does. Every feature is framed in terms of Writ's
> two orthogonal pillars. For a practical, example-driven introduction, see
> [getting-started.md](getting-started.md).

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

Writ's grammar is small and regular. Signatures are the load-bearing surface of
the language — they declare a function's effects via `uses {...}` and its
contracts via `requires` / `ensures` — so those clauses are first-class grammar,
not annotations.

### Lexical structure

- **Comments** are line comments: `//` to the end of the line. There are no block
  comments.
- **Whitespace** separates tokens and is otherwise insignificant.
- **Identifiers** begin with a letter or `_` and continue with letters, digits,
  or `_`.
- **Keywords** (reserved): `fn`, `let`, `mut`, `return`, `if`, `else`, `match`,
  `type`, `import`, `export`, `uses`, `requires`, `ensures`, `true`, `false`.
- **Literals**:
  - *integer* — a run of decimal digits, within the range of a signed 64-bit
    integer;
  - *text* — `"..."`, with the escapes `\"`, `\\`, `\n`, `\t` decoded;
  - *boolean* — `true` or `false`.
- **Operators**: `+ - * / %`, `== != < <= > >=`, `&& || !`, `=`, and the arrows
  `->` (return type) and `=>` (match arm). `|` separates sum-type variants.
- **Punctuation**: `( ) { }`, `,`, `:`, `;`, `.`.

The lexer never aborts on malformed input: a bad character or literal produces a
diagnostic and the scan recovers, so one bad byte does not hide later errors.

### Expressions

Binary operators are left-associative, with these precedence levels from lowest
to highest binding:

1. `||`
2. `&&`
3. `==`, `!=`
4. `<`, `<=`, `>`, `>=`
5. `+`, `-`
6. `*`, `/`, `%`

Unary `-` (negation) and `!` (logical not) are prefix and bind tighter than any
binary operator; parentheses group. The primary expressions are literals and
identifiers; a **call** `f(a, b, ...)`, optionally with type arguments
`f<T>(...)` (used by `grant`); **member access** `a.b` (an imported module's
item, e.g. `math.add`); a **`match`**; and a parenthesized expression.

A `match` scrutinizes a value against **patterns**:

```
match o {
    Some(x) => x,
    None    => 0,
    _       => -1,
}
```

Each arm's body is an expression, and all arms must agree on a type. A pattern is
`_` (wildcard), an identifier (a binding, or a nullary-variant name), or a
variant `Name(p, ...)` with nested sub-patterns. A `match` on a sum type must be
**exhaustive** (see §2).

### Statements

A block `{ ... }` is a sequence of statements:

- **binding** — `let name = e;` or `let name: T = e;` (immutable);
- **expression** — `e;` (evaluated for its value or effect, e.g. a `print` call);
- **return** — `return e;` or `return;`;
- **conditional** — `if cond { ... }`, with optional `else if` / `else`; the
  condition must be `Bool`.

### Declarations

A source file (a **module**) begins with zero or more `import` declarations,
followed by function and type declarations.

- **Import** — `import name` brings the sibling module `name` into scope.
- **Type** — `[export] type Name[<G, ...>] = V | V | ...` declares a sum type
  with optional generic parameters. Each variant is a name, optionally with
  positional payload types: `Some(T)`, `Pair(Int, Int)`, or a nullary `None`.
- **Function** — `[export] fn name(params) [-> T] [clauses] { body }`, where a
  parameter is `name: Type`. The **signature clauses** — `uses { E, ... }`,
  `requires <expr>`, and `ensures <expr>` — sit between the return type and the
  body, **in any order**, and `requires` / `ensures` may each appear more than
  once.

`export` makes a top-level item visible to modules that import it; without it the
item is private to its module.

---

## 2. Type system

Writ is **strongly and statically typed**, with **no implicit coercion** and **no
null value**. Every expression has exactly one type, known at compile time; a
value is never silently widened, narrowed, or nil. The goal is to turn "plausible
but wrong" into "does not compile."

### Types

The **ground types** are:

- `Int` — a signed 64-bit integer. Arithmetic is **checked**: overflow and
  division by zero are errors, never wraparound.
- `Bool` — `true` or `false`.
- `Text` — a sequence of Unicode scalar values (below).
- `Unit` — the type of a statement, and of a function with no return type; it has
  a single value.

A **type expression** is a name with optional type arguments — `Int`,
`Option<Int>`, `Cap<Write>`, `Tainted<Text>` — or a **function type**
`fn(P, ...) -> R`. The head of a named type is a ground type, a declared sum type,
or a built-in constructor: `Cap<E>` (a capability for authority `E`, §3) or
`Tainted<T>` (untrusted data, §3). A name with no built-in rule is kept opaque so
a later pass can give it meaning.

There are **no implicit conversions**: `Int` and `Bool` never interconvert, and
combining or comparing mismatched types is a compile error, not a coercion.

### Sum types and exhaustiveness

A sum type enumerates named variants, each optionally carrying positional payload
values, and may be generic:

```
type Option<T> = Some(T) | None
```

Payload types are **instantiated** at each use: `Some(3)` is `Option<Int>` and
`Some("x")` is `Option<Text>`, so those two are distinct and incompatible. In a
`match`, a variant pattern binds its payload at the instantiated type — `Some(x)`
on an `Option<Int>` binds `x: Int` — and a pattern from a *different* sum type is
refused.

A `match` on a sum type must be **exhaustive**: every variant is covered, or a
wildcard `_` / catch-all binding is present. A missing case is a compile error
that names the uncovered variants — exhaustiveness is a compile-time guarantee,
not a runtime check. A binding may not repeat within one pattern (`Pair(x, x)` is
rejected).

### The prelude

Two sum types are always in scope, with no declaration or import — a **prelude**:

```
type Option<T> = Some(T) | None
type Result<T, E> = Ok(T) | Err(E)
```

`Option<T>` models a value that may be absent (Writ has no null); `Result<T, E>`
models a success (`Ok`) or a failure carrying a reason (`Err`). They are ordinary
generic sum types — the checker and back ends give them no special treatment. A
program that declares its own type of the same name **shadows** the prelude one,
so nothing is forced on a program that wants its own.

### Text

`Text` is a sequence of **Unicode scalar values**, so the text built-ins are
character-based, not byte-based: `text_len` counts scalar values, `char_at(s, i)`
returns the i-th one as a one-character `Text`, `substring(s, start, end)` slices
the half-open character range, `concat` joins, and `char_code` / `code_char`
convert between a character and its scalar value. Out-of-range `char_at` /
`substring` / `char_code` is a runtime error. The interpreter and the native back
end implement identical semantics (the C back end carries a small UTF-8 decoder).

### Functions and signatures

A signature gives each parameter a type and, if the function returns a value, a
return type. A call is checked for arity and for each argument's type, and its
result takes the return type. `main` may take only capability parameters — it is
where authority enters a program (§3). A signature also carries the **effect** and
**contract** clauses the two pillars act on: effects live in the type system, so a
signature tells you what a function may *do*, not just what it computes.

A top-level function can be passed as a **value** — a *higher-order* function
takes a parameter of function type `fn(P, ...) -> R` and calls it. Only **pure**
functions (those with an empty `uses {...}`) may be used as values: an effectful
function passed as a value would let its effects be performed at a call site the
honesty and authority passes cannot see, so it is refused.

### Built-in functions

A small set of built-ins is always in scope (each shadowable by a user function
of the same name). The pure ones take no capability:

- `print(x)` — write one line; accepts any type, returns `Unit`.
- `concat(Text, Text) -> Text`, `text_len(Text) -> Int`,
  `char_at(Text, Int) -> Text`, `substring(Text, Int, Int) -> Text`,
  `char_code(Text) -> Int`, `code_char(Int) -> Text`.
- `sanitize(Tainted<T>, fn(T) -> Bool) -> Option<T>` — the taint boundary (§3):
  applies the validator and returns `Some` if it accepts, else `None`.
- `grant<A>(Cap<..>) -> Cap<A>` — capability narrowing (§3).

The effectful built-ins take a capability and declare an effect:

- `read_file(Cap<Read>, Text) -> Text` — `uses { Read }`.
- `write_file(Cap<Write>, Text, Text)` — `uses { Write }`.

### Diagnostics

Type errors use stable `T00xx` codes, each with an exact span — for example
`T0001` (type mismatch), `T0004` (wrong argument count), `T0005` (return type
mismatch), `T0006` (non-exhaustive match, naming the missing variants), `T0010` /
`T0011` (duplicate function / variant name), and `T0012` (a pattern from the wrong
sum type). Output is deterministic: the same source yields the same diagnostics in
the same order.

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
  without passing a `sanitize` boundary — which applies a caller-supplied
  validation rule and yields `Some` (accepted) or `None` (rejected).

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

This is enforced **structurally**, on the *value's capability-hood*, not on
surface syntax: wrapping a capability in a compound expression (a `match`, say)
does not launder it, and the result of `grant<A>(cap)` is itself a capability, so
**binding it to a local is refused** (`E0202`) — narrow and forward it inline, in
argument position (`write(grant<Write>(root), ..)`). The invariant "no local ever
holds a capability" then holds by construction.

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
- **The interpreter has no ambient authority.** Effectful built-ins are
  **capability-gated**, never ambient. `read_file` / `write_file` (the first real
  effects) each take a `Cap<Read>` / `Cap<Write>` and declare `uses { Read }` /
  `uses { Write }`, so the honesty and authority checks apply to them exactly as
  to any call: a function with no matching capability cannot name the effect
  (E0301), and a function that reaches one without declaring it is refused
  (E0101). A sandboxed function — one handed no capability — therefore cannot
  touch the filesystem, by construction. Pure built-ins (`print` to an in-memory
  buffer, the text operations) take no capability because they perform no effect.
  The capability is checked statically; at runtime the token carries no data and
  the effect is performed only because the caller was proven to hold authority.

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
