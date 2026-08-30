use std::env;
use std::fs;
use std::process::ExitCode;

use capnp_compiler::{ParseLimits, parse_schema_bytes};

fn main() -> ExitCode {
    let mut valid = true;
    for path in env::args_os().skip(1) {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("{}: {error}", path.to_string_lossy());
                valid = false;
                continue;
            }
        };
        match parse_schema_bytes(&bytes, ParseLimits::default()) {
            Ok(tree) if tree.is_valid() => {}
            Ok(tree) => {
                for diagnostic in tree.diagnostics {
                    eprintln!(
                        "{}:{}-{}: {}",
                        path.to_string_lossy(),
                        diagnostic.range.start,
                        diagnostic.range.end,
                        diagnostic.message
                    );
                }
                valid = false;
            }
            Err(error) => {
                eprintln!("{}: {error}", path.to_string_lossy());
                valid = false;
            }
        }
    }
    if valid {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
