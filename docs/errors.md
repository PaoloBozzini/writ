# The Writ diagnostic format

Diagnostics are Writ's **API to the model**: every stage — lexer, parser, type
checker, effect/honesty checker, capability + authority checker, taint checker,
contract checker, module resolver, and the runtime — reports through **one
shared type** ([`writ_ast::Diagnostic`]) and serializes to **one JSON schema**.
Output is **deterministic**: the same input yields the same diagnostics, in the
same order, byte for byte.

## Schema

Each diagnostic is a JSON object:

```json
{
  "code": "E0301",
  "severity": "error",
  "span": { "start": 778, "end": 792 },
  "message": "…",
  "blame": "implementation"
}
```

| Field | Type | Notes |
| --- | --- | --- |
| `code` | string | Stable identifier for the rule (see below). Keyed on by repair loops; never reused for a different rule. |
| `severity` | `"error"` \| `"warning"` | Only `error` blocks acceptance. |
| `span` | `{ "start": int, "end": int }` | Half-open byte range into the source. |
| `message` | string | Human-facing, precise, actionable. May be reworded without changing `code`. |
| `blame` | `"caller"` \| `"implementation"` | **Optional** — present only on contract diagnostics. A failed precondition blames the `caller`; a failed postcondition blames the `implementation`. |

Fields always appear in this order. The `message` is escaped (`"`, `\`, control
characters as `\u00xx`) so the output is always valid JSON. A run's diagnostics
are emitted as a JSON array (`writ check` / `writ run` print exactly this).

## Code prefixes

| Prefix | Producer |
| --- | --- |
| `L####` | lexer |
| `P####` | parser |
| `T####` | type checker |
| `E01##` | effect / honesty checker |
| `E02##` | capability (parameter-only / escape) |
| `E03##` | authority (effect-site) checker |
| `E04##` | taint checker |
| `R####` | module resolver |
| `T0008`/`T0009` | capability narrowing (`grant`) |
| `C####` | contract violations (runtime; carry `blame`) |
| `D####` | driver (file loading) |
| `E1000` | generic runtime error |

Codes are stable: a code identifies one rule for the life of the language, so a
generate-check-repair loop can branch on it without parsing prose.

## Runtime errors and partial output

A program that fails at runtime behaves identically under `writ run` (the
interpreter) and a `writ build` native binary:

- **Output goes to stdout; the error goes to stderr.** Both engines print the
  lines produced *before* the failure to stdout, then emit **one** machine-
  readable diagnostic to stderr and exit non-zero. Output printed before the
  failure is **preserved**, not discarded — a repair loop sees how far the
  program got.
- **The native diagnostic carries the stable `code`** (and, for a contract
  failure, the `blame`) as a one-line JSON object, the same load-bearing fields
  `writ run` emits. Native binaries carry no source spans, so the native line
  omits `span`; the interpreter's includes it. Ordinary runtime errors use
  `E1000`; a failed precondition/postcondition uses `C0001`/`C0002` with
  `blame` `caller`/`implementation` on both engines.
