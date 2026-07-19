//! `writ` driver: load a program from files, check it, and run it.
//!
//! A program is a root `.writ` file plus the sibling files its `import`s name.
//! This crate is the thin wiring layer — all language logic lives in the
//! pipeline crates.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use writ_ast::{Diagnostic, Item, Module, Span};
use writ_interp::{Interpreter, RuntimeError, Value};

/// A loaded program: its modules keyed by name, and the root module's name.
pub struct Program {
    pub modules: BTreeMap<String, Module>,
    pub root: String,
}

/// A module name is the file stem (`math.writ` → `math`).
fn module_name(path: &Path) -> String {
    path.file_stem()
        .map_or_else(|| "main".to_string(), |s| s.to_string_lossy().into_owned())
}

/// Load the root file and, transitively, the sibling files its imports name.
/// Load/parse diagnostics are returned alongside the (possibly partial) program.
#[must_use]
pub fn load_program(root_path: &Path) -> (Program, Vec<Diagnostic>) {
    let dir = root_path
        .parent()
        .map_or_else(|| Path::new(".").to_path_buf(), Path::to_path_buf);
    let root = module_name(root_path);
    let mut modules = BTreeMap::new();
    let mut diagnostics = Vec::new();

    let mut queue = vec![(root.clone(), root_path.to_path_buf())];
    while let Some((name, path)) = queue.pop() {
        if modules.contains_key(&name) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            diagnostics.push(Diagnostic::error(
                "D0001",
                Span::new(0, 0),
                format!(
                    "cannot read module `{name}` (expected file `{}`)",
                    path.display()
                ),
            ));
            continue;
        };
        let parsed = writ_parser::parse(&src);
        diagnostics.extend(parsed.diagnostics);
        for import in &parsed.module.imports {
            if !modules.contains_key(&import.name) {
                queue.push((
                    import.name.clone(),
                    dir.join(format!("{}.writ", import.name)),
                ));
            }
        }
        modules.insert(name, parsed.module);
    }

    (Program { modules, root }, diagnostics)
}

/// The independent check passes, in run order. Each is its own module in
/// `writ-check` with no cross-pass imports, so any subset can run alone.
pub const PASSES: &[&str] = &[
    "resolution",
    "types",
    "effects",
    "authority",
    "capabilities",
    "taint",
];

/// Run every static check over a program.
#[must_use]
pub fn check(program: &Program) -> Vec<Diagnostic> {
    check_passes(program, &[])
}

/// Run only the named passes (an empty selection runs them all), demonstrating
/// that the passes are independent — each can run without the others. Returns
/// diagnostics in a stable order.
#[must_use]
pub fn check_passes(program: &Program, passes: &[String]) -> Vec<Diagnostic> {
    let enabled = |name: &str| passes.is_empty() || passes.iter().any(|p| p == name);
    let mut diagnostics = Vec::new();
    // The resolver runs over the module graph: it checks import existence,
    // cross-module *visibility* (private items refused), and acyclicity — facts
    // that only make sense before flattening.
    if enabled("resolution") {
        diagnostics.extend(writ_check::check_resolution(&program.modules));
    }
    // Every other pass runs over the **linked** program — the same single module
    // the back ends execute. A cross-module call is an ordinary call there, so
    // types, effects, authority, and taint apply across boundaries just as they
    // do locally (issue #96), and the checked artifact is the executed one
    // (issue #103).
    let linked = writ_lower::link(&program.modules, &program.root);
    if enabled("types") {
        diagnostics.extend(writ_check::check_types(&linked));
    }
    if enabled("effects") {
        diagnostics.extend(writ_check::check_effects(&linked));
    }
    if enabled("authority") {
        diagnostics.extend(writ_check::check_authority(&linked));
    }
    if enabled("capabilities") {
        diagnostics.extend(writ_check::check_capabilities(&linked));
    }
    if enabled("taint") {
        diagnostics.extend(writ_check::check_taint(&linked));
    }
    diagnostics
}

/// Run a checked program's `main` via the interpreter, returning the lines it
/// printed. Multi-module programs are linked into one module (functions
/// qualified by module) first, and `main` is handed a root capability for each
/// of its capability parameters.
///
/// # Errors
/// Returns a [`RuntimeError`] if there is no `main`, or execution fails. Any
/// output printed **before** a failure is discarded by this signature; use
/// [`run_collecting`] to keep it.
pub fn run(program: &Program) -> Result<Vec<String>, RuntimeError> {
    match run_collecting(program) {
        (output, None) => Ok(output),
        (_, Some(err)) => Err(err),
    }
}

/// Run `main`, returning **both** the lines printed and any runtime error.
///
/// Partial-output semantics (issue #152): a `print` that ran before a runtime
/// error **is preserved**, matching the native binary — which streams to stdout
/// and flushes on trap. So a failing program prints identically on both engines
/// up to the point of failure; the error diagnostic is a separate channel
/// (stderr from the CLI). The returned error is `None` on success.
#[must_use]
pub fn run_collecting(program: &Program) -> (Vec<String>, Option<RuntimeError>) {
    let linked = writ_lower::link(&program.modules, &program.root);
    // Desugar contracts into shared `Check` nodes (the one place contract
    // semantics live) before handing the program to a back end.
    let lowered = writ_lower::lower(&linked);
    let interp = match Interpreter::new(&lowered) {
        Ok(i) => i,
        Err(e) => return (Vec::new(), Some(e)),
    };
    let Some(main) = lowered.items.iter().find_map(|it| match it {
        Item::Function(f) if f.signature.name == "main" => Some(f),
        _ => None,
    }) else {
        return (
            Vec::new(),
            Some(RuntimeError::new(Span::new(0, 0), "no `main` function")),
        );
    };
    let args = main
        .signature
        .params
        .iter()
        .map(|p| {
            if p.ty.name == "Cap" {
                let authority =
                    p.ty.args
                        .first()
                        .map_or_else(|| "Root".to_string(), |a| a.name.clone());
                Value::Capability { authority }
            } else {
                Value::Unit
            }
        })
        .collect();
    // Take the buffered output regardless of success, so prints made before a
    // failure survive.
    let err = interp.call("main", args).err();
    (interp.output(), err)
}

/// Statically verify a program's contracts with the bundled `z3`-CLI solver.
///
/// This is the **optional** SMT pass (issue #27): it returns only warnings and
/// is never part of `check` / `run` / `build`, so it cannot block a program the
/// runtime path accepts. Returns `(diagnostics, solver_available)`; when no
/// solver is installed, the diagnostics are empty and the flag is `false`.
///
/// Verification runs over the **linked** module — the same artifact `check`,
/// `run`, and `build` consume (issue #157) — not the raw per-module ASTs, so a
/// cross-module contract sees the same program the other passes do. Solver
/// availability is probed **once** per invocation.
#[must_use]
pub fn verify(program: &Program) -> (Vec<Diagnostic>, bool) {
    let solver = writ_verify::Z3Cli;
    // Verify the linked artifact (not lowered: `requires` / `ensures` still live
    // on signatures pre-lowering, which is what the verifier reads).
    let linked = writ_lower::link(&program.modules, &program.root);
    writ_verify::verify_reporting_availability(&linked, &solver)
}

/// Why a native build failed: a construct the back end cannot emit, an I/O
/// problem, or the system C compiler rejecting the generated source.
#[derive(Debug)]
pub enum BuildError {
    /// The program uses a construct the C back end does not support yet.
    Codegen(writ_codegen::CodegenError),
    /// Reading, writing, or spawning failed.
    Io(std::io::Error),
    /// The C compiler exited non-zero (its stderr is included).
    Compiler(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::Codegen(e) => {
                write!(
                    f,
                    "codegen: {} (at {}..{})",
                    e.message, e.span.start, e.span.end
                )
            }
            BuildError::Io(e) => write!(f, "io: {e}"),
            BuildError::Compiler(msg) => write!(f, "C compiler failed: {msg}"),
        }
    }
}

impl From<std::io::Error> for BuildError {
    fn from(e: std::io::Error) -> Self {
        BuildError::Io(e)
    }
}

/// The C compiler to invoke: `$CC` if set, otherwise `cc`.
fn c_compiler() -> String {
    std::env::var("CC").unwrap_or_else(|_| "cc".to_string())
}

/// Compile a checked program to a standalone native binary at `out_path`.
///
/// The build is **hermetic and deterministic** (issue #30): the whole pipeline
/// is one command from source to binary, and identical source yields
/// byte-identical output. Determinism holds because
///
/// - codegen is a pure function of the AST — items are emitted in module order
///   with no clock, RNG, hash-map iteration, or ambient input; and
/// - the C compiler is invoked with reproducible flags and a `-ffile-prefix-map`
///   that strips the (volatile) build directory, so the binary does not embed
///   where it was built.
///
/// The pipeline is: link the modules, lower contracts into shared `Check`
/// nodes, emit C beside the binary (`out_path` with a `.c` extension), then hand
/// the C to the system compiler. Returns the path to the emitted C source.
///
/// # Errors
/// Returns a [`BuildError`] if codegen rejects a construct, file I/O fails, or
/// the C compiler exits non-zero.
pub fn build(program: &Program, out_path: &Path) -> Result<PathBuf, BuildError> {
    let linked = writ_lower::link(&program.modules, &program.root);
    let lowered = writ_lower::lower(&linked);
    let c_src = writ_codegen::emit_c(&lowered).map_err(BuildError::Codegen)?;

    let c_path = out_path.with_extension("c");
    std::fs::write(&c_path, &c_src)?;

    let mut cmd = Command::new(c_compiler());
    cmd.arg("-O2").arg("-g0");
    // Strip the absolute build directory from anything the compiler might embed,
    // so a binary built at `/tmp/a` and one built at `/home/b` are identical.
    if let Some(dir) = c_path.parent() {
        if !dir.as_os_str().is_empty() {
            cmd.arg(format!("-ffile-prefix-map={}=.", dir.display()));
        }
    }
    let output = cmd.arg("-o").arg(out_path).arg(&c_path).output()?;
    if !output.status.success() {
        return Err(BuildError::Compiler(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(c_path)
}
