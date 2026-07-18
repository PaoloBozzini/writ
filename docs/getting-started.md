# Getting started with Writ

Writ is a small, strongly-typed language whose safety comes from two orthogonal
checkers: **capabilities** (what code is *allowed* to do) and **contracts**
(whether it returns the *right* answer). This guide takes you from an empty
directory to writing, running, compiling, and verifying real programs, then
tours the language feature by feature. Every snippet is a complete program you
can run.

- [Install the toolchain](#install-the-toolchain)
- [Your first program](#your-first-program)
- [The `writ` command](#the-writ-command)
- [A tour of the language](#a-tour-of-the-language)
- [Compiling to a native binary](#compiling-to-a-native-binary)
- [Static verification](#static-verification)
- [Diagnostics are an API](#diagnostics-are-an-api)
- [Where to go next](#where-to-go-next)

---

## Install the toolchain

Writ is implemented in Rust, so you need a recent Rust toolchain
(`rustup`/`cargo`). Clone the repository and build:

```bash
git clone https://github.com/PaoloBozzini/writ
cd writ
cargo build
```

Every command below is run through Cargo:

```bash
cargo run -p writ-cli -- <command> path/to/file.writ
```

If you would rather type `writ` directly, install the binary once:

```bash
cargo install --path crates/writ-cli   # provides a `writ` executable
writ run examples/hello.writ
```

The rest of this guide uses the short `writ <command>` form.

---

## Your first program

Create `hello.writ`:

```writ
// The smallest Writ program: print a greeting.
fn main() {
    print("Hello, Writ!");
}
```

Run it:

```bash
writ run hello.writ
```

```
Hello, Writ!
```

`main` is the entry point. `print` is a built-in that writes one line per call.
Statements end in `;`, and blocks are wrapped in `{ }`.

---

## The `writ` command

Writ has one pipeline — **source → lexer → parser → checkers → back end** — and
four subcommands that stop at different points:

| Command | What it does |
| --- | --- |
| `writ check <file>` | Run the static checkers only (types, effects, authority, taint, contracts). Reports diagnostics; runs nothing. |
| `writ run <file>` | Check, then interpret `main`, echoing whatever it prints. |
| `writ build <file>` | Check, then compile to a standalone native binary (via C) next to the source. |
| `writ verify <file>` | Check, then attempt to *prove* contracts for all inputs with an SMT solver (optional; see below). |

`writ check` never executes your program — the strongest sandbox is not running
code at all — so it is safe to run on untrusted source.

You can also run a single checker pass:

```bash
writ check file.writ types      # only the type checker
writ check file.writ authority  # only the capability authority pass
```

---

## A tour of the language

### Values and types

Writ has three ground types — `Int` (64-bit, with **checked overflow**), `Bool`,
and `Text` (a sequence of Unicode characters) — plus the sum types you declare
yourself. There is **no implicit coercion** and **no null**: a value is always
exactly its type.

```writ
fn main() {
    print(2 + 3 * 4);        // 14
    print(10 / 3);           // 3  (integer division)
    print(1 < 2 && 3 >= 3);  // true
    print(!false);           // true
    print(-5);               // -5
}
```

Arithmetic (`+ - * / %`) and comparisons (`< <= > >=`) work on `Int`; `==` and
`!=` compare two values of the *same* type; `&& || !` work on `Bool`. Mixing
types is a compile error, not a coercion:

```writ
fn main() {
    print(1 + "x");   // error: left operand must be `Int`, found `Text`
}
```

### Functions

A function declares typed parameters and, if it returns a value, a return type
after `->`. Values leave via an explicit `return`.

```writ
fn add(a: Int, b: Int) -> Int {
    return a + b;
}

fn main() {
    print(add(3, 4));   // 7
}
```

### Bindings

`let` introduces an immutable binding. Its type is inferred, or you can annotate
it — and an annotation that disagrees with the value is a compile error.

```writ
fn main() {
    let sum = add(3, 4);       // inferred `Int`
    let greeting: Text = "hi"; // annotated
    print(sum);
    print(greeting);
}

fn add(a: Int, b: Int) -> Int { return a + b; }
```

### Control flow

`if` is a statement; its condition must be `Bool`. `else` and `else if` chain as
usual.

```writ
fn sign(n: Int) -> Int {
    if n < 0 {
        return -1;
    } else if n > 0 {
        return 1;
    } else {
        return 0;
    }
}

fn main() {
    print(sign(-5));  // -1
    print(sign(3));   // 1
}
```

### Sum types and `match`

Sum types are the main compound type. They can be generic, and a `match` over
one must be **exhaustive** — forgetting a case is a compile error, not a runtime
surprise.

```writ
type Option<T> = Some(T) | None

fn unwrap_or(o: Option<Int>, fallback: Int) -> Int {
    return match o {
        Some(x) => x,       // `x` is bound to the payload, typed `Int`
        None    => fallback,
    };
}

fn main() {
    print(unwrap_or(Some(42), 0));  // 42
    print(unwrap_or(None, 7));      // 7
}
```

Payload types are inferred: `Some(3)` is an `Option<Int>`, and a pattern from a
different sum type — or a non-exhaustive match — is refused before the program
runs.

### Text operations

Text is a sequence of Unicode scalar values, so its operations are character-based:

```writ
fn main() {
    let s = concat("Writ", "!");
    print(s);                 // Writ!
    print(text_len(s));       // 5
    print(char_at(s, 0));     // W
    print(substring(s, 0, 4));// Writ
    print(char_code("A"));    // 65
    print(code_char(97));     // a
}
```

`text_len`, `char_at`, and `substring` count characters (not bytes); an
out-of-range access is a runtime error. `char_code`/`code_char` convert between a
character and its Unicode scalar value.

### Contracts — is the answer *right*?

A signature can carry a `requires` (precondition) and one or more `ensures`
(postconditions). **Blame is load-bearing**: a failed `requires` blames the
**caller** (it passed bad input); a failed `ensures` blames the
**implementation** (it computed a wrong answer). Contract predicates must be
pure.

```writ
fn half(n: Int) -> Int
    requires n > 0
    ensures result >= 0
{
    return n / 2;
}

fn main() {
    print(half(8));  // 4
}
```

Contracts are checked at runtime per call. `result` is bound to the return value
inside `ensures`. To go further and *prove* them for all inputs, see
[static verification](#static-verification).

### Capabilities — what may this code *do*?

Writ has **no ambient authority**. An effect — writing a file, say — is reachable
only if an unforgeable **capability token** was passed into the function as a
parameter. A function with no capability parameter is sandboxed *by
construction*.

Capabilities have the type `Cap<E>` for an authority `E` (e.g. `Cap<Write>`).
They are **parameter-only** and **second-class**: you can receive one and pass it
on as an argument, but you cannot construct, return, or store one. A signature
declares the effects it may perform with `uses { ... }`.

```writ
fn write_line(out: Cap<Write>, msg: Text) uses { Write } {
    return;
}

// Fine: `greet` holds the capability and forwards it.
fn greet(out: Cap<Write>) uses { Write } {
    write_line(out, "hi");
}
```

The checker enforces two things at every effect site: the caller **holds** a
matching capability (authority), and the signature **declared** the effect
(honesty). A "plausible but dangerous" helper is refused:

```writ
fn write_line(out: Cap<Write>, msg: Text) uses { Write } { return; }

// Refused: `sneaky` performs `Write` but holds no `Cap<Write>` and does not
// declare the effect.
fn sneaky(msg: Text) {
    write_line(msg, msg);
}
```

The runtime hands the **root capability**, `Cap<Root>`, to `main`. You narrow it
to a specific authority with `grant`:

```writ
fn write_line(out: Cap<Write>, msg: Text) uses { Write } { return; }

fn main(root: Cap<Root>) uses { Write } {
    write_line(grant<Write>(root), "authorized");
}
```

`main` may take only capability parameters — it is the sole place authority
enters a program.

### Taint — keeping untrusted data out of sinks

Untrusted input has the type `Tainted<T>`. A **sink** is a function that declares
a dangerous effect (`uses { Query }` or `uses { Shell }`). A tainted value cannot
reach a sink until it passes a `sanitize` boundary — wrapping it in a `match` or
any other expression does not launder it.

```writ
fn run_query(db: Cap<Query>, sql: Text) uses { Query } { return; }

fn handle(db: Cap<Query>, input: Tainted<Text>) uses { Query } {
    run_query(db, sanitize(input));   // OK — sanitized
    // run_query(db, input);          // would be refused (E0401)
}
```

`run_query` is a sink because it declares `uses { Query }`; passing the tainted
`input` straight to it is refused, and no wrapper (a `match`, say) can launder
the taint around the `sanitize` boundary.

### File I/O via capabilities

Reading and writing files are the first genuinely effectful built-ins, and they
are capability-gated like everything else:

```writ
fn main(root: Cap<Root>) uses { Read, Write } {
    write_file(grant<Write>(root), "greeting.txt", "hello");
    print(read_file(grant<Read>(root), "greeting.txt"));  // hello
}
```

- `write_file(cap: Cap<Write>, path: Text, contents: Text)` — `uses { Write }`
- `read_file(cap: Cap<Read>, path: Text) -> Text` — `uses { Read }`

A function without the matching capability cannot touch the filesystem — there is
no ambient path to it.

### Modules

A program is one or more files. Each file is a module named after its stem. An
item is private unless prefixed with `export`, and a qualified name reaches an
exported item in an imported module.

`math.writ`:

```writ
export fn add(a: Int, b: Int) -> Int {
    return a + b;
}
```

`app.writ`:

```writ
import math

fn main() {
    print(math.add(2, 3));  // 5
}
```

```bash
writ run app.writ
```

Cross-module calls are checked exactly like local ones: types, effects,
authority, and taint all apply across the boundary. Sum-type constructors are
global, so `Some` / `None` need no qualification.

---

## Compiling to a native binary

`writ build` compiles a checked program to a standalone binary (it emits C and
invokes the system C compiler):

```bash
writ build examples/factorial.writ
./examples/factorial
```

```
120
```

Builds are **hermetic and deterministic**: identical source produces
byte-identical output, independent of where it is built. The native binary
matches the interpreter's behavior — a differential test corpus keeps them in
step.

---

## Static verification

`writ verify` upgrades contracts from per-input runtime checks to **proofs for
all inputs**, where a solver can discharge them. It is optional and needs the
`z3` command-line solver on your `PATH` (or `$WRIT_SMT` set to a solver binary);
without one, the pass is skipped.

```bash
writ verify half.writ   # tries to prove every `ensures`
```

It reports only warnings, never blocking a program the runtime path accepts: an
`ensures` the solver refutes (a counterexample exists) or cannot decide is
reported before execution, rather than silently deferred to runtime.

---

## Diagnostics are an API

Every diagnostic — from any stage — is machine-readable, with a stable **code**,
a **span**, a **severity**, and a **message**, printed as deterministic JSON so a
generate-check-repair loop can consume it:

```json
[{"code":"E0301","severity":"error","span":{"start":24,"end":47},
  "message":"effect `Write` is performed here (via call to `write_file`) but function `sneaky` holds no `Cap<Write>` capability"}]
```

Contract diagnostics also carry a **blame** direction (`caller` /
`implementation`) — the load-bearing signal for knowing which side to fix.

---

## Where to go next

- **Examples:** the [`examples/`](../examples) directory — `hello.writ`,
  `factorial.writ`, the capability demo `reject_fs_write.writ`, the contract demo
  `reject_wrong_answer.writ`, and a multi-file program under `modules/`.
- **The standard library:** [`std/list.writ`](../std/list.writ), a generic list
  written in Writ.
- **Self-hosting:** [`bootstrap/lexer.writ`](../bootstrap/lexer.writ) — a lexer
  for Writ, written in Writ.
- **The specification:** [`docs/spec.md`](spec.md) frames every feature in terms
  of the two pillars.
- **Contributing:** [`CONTRIBUTING.md`](../CONTRIBUTING.md).
