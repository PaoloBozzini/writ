//! `writ` — the command-line entry point. Wiring only; the driver logic lives in
//! the `writ_cli` library.

use std::path::PathBuf;
use std::process::ExitCode;

use writ_ast::diagnostics_to_json;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(command), Some(file)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: writ <check|run|build> <file.writ>");
        return ExitCode::from(2);
    };
    let path = PathBuf::from(file);

    match command.as_str() {
        "check" => check(&path),
        "run" => run(&path),
        "build" => {
            // Native code generation is a later milestone (M6); there is no
            // back end to emit a binary yet.
            eprintln!("writ build: native code generation is not available yet (M6)");
            ExitCode::FAILURE
        }
        other => {
            eprintln!("unknown subcommand `{other}`; expected check | run | build");
            ExitCode::from(2)
        }
    }
}

/// Load + statically check a program, printing diagnostics as canonical JSON.
fn check(path: &std::path::Path) -> ExitCode {
    let (program, mut diagnostics) = writ_cli::load_program(path);
    diagnostics.extend(writ_cli::check(&program));
    report(&diagnostics)
}

/// Check, then (if clean) run `main`, echoing whatever it printed.
fn run(path: &std::path::Path) -> ExitCode {
    let (program, mut diagnostics) = writ_cli::load_program(path);
    diagnostics.extend(writ_cli::check(&program));
    if diagnostics.iter().any(writ_ast::Diagnostic::is_error) {
        return report(&diagnostics);
    }
    match writ_cli::run(&program) {
        Ok(output) => {
            for line in output {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            // Runtime errors serialize under the same machine-readable schema.
            println!("{}", diagnostics_to_json(&[e.to_diagnostic()]));
            ExitCode::FAILURE
        }
    }
}

/// Print diagnostics (machine-readable JSON) and choose an exit code.
fn report(diagnostics: &[writ_ast::Diagnostic]) -> ExitCode {
    if diagnostics.is_empty() {
        return ExitCode::SUCCESS;
    }
    println!("{}", diagnostics_to_json(diagnostics));
    if diagnostics.iter().any(writ_ast::Diagnostic::is_error) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
