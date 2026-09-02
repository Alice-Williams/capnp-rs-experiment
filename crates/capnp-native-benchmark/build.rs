use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use capnp_codegen::generate_requested_files;
use capnp_compiler::request::{compile_program, emit_compiled_schema};
use capnp_compiler::semantic::{ModuleSources, ResolveLimits};
use capnp_schema::CapnpVersion;

fn main() -> Result<(), Box<dyn Error>> {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest dir")?);
    let schemas = manifest.join("schemas");
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing output dir")?);
    for name in ["carsales", "catrank", "eval"] {
        generate(name, &schemas.join(format!("{name}.capnp")), &output)?;
    }
    println!("cargo:rerun-if-changed={}", schemas.display());
    Ok(())
}

fn generate(name: &str, source_path: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let entry = format!("/{name}.capnp");
    let mut sources = ModuleSources::default();
    sources.insert_explicit(&entry, fs::read_to_string(source_path)?);
    let program = sources.resolve(&entry, ResolveLimits::default());
    if !program.is_valid() {
        return Err(io::Error::other(format!("{program:#?}")).into());
    }
    let schema = compile_program(
        &program,
        CapnpVersion {
            major: 1,
            minor: 0,
            micro: 2,
        },
    )?;
    let generated = generate_requested_files(&schema)?
        .into_iter()
        .next()
        .ok_or("request contains no requested file")?;
    fs::write(output.join(format!("{name}.rs")), generated.source)?;
    fs::write(
        output.join(format!("{name}.request.bin")),
        emit_compiled_schema(&schema)?,
    )?;
    Ok(())
}
