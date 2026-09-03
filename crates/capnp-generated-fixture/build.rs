use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use capnp_codegen::generate_requested_files;
use capnp_compiler::request::compile_program;
use capnp_compiler::semantic::{ModuleSources, ResolveLimits};
use capnp_schema::CapnpVersion;

fn main() -> Result<(), Box<dyn Error>> {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest dir")?);
    let schemas = manifest.join("../../conformance/schemas");
    let mut sources = ModuleSources::default();
    for name in [
        "wire-fixture",
        "evolution-v1",
        "evolution-v2",
        "import-fixture",
        "language-fixture",
        "streaming-fixture",
    ] {
        sources.insert_explicit(
            format!("/{name}.capnp"),
            fs::read_to_string(schemas.join(format!("{name}.capnp")))?,
        );
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing output dir")?);
    for (schema, destination) in [
        ("wire-fixture", "wire_fixture.rs"),
        ("evolution-v1", "evolution_v1.rs"),
        ("evolution-v2", "evolution_v2.rs"),
        ("import-fixture", "import_fixture.rs"),
        ("language-fixture", "language_fixture.rs"),
        ("streaming-fixture", "streaming_fixture.rs"),
    ] {
        generate(&sources, schema, &output.join(destination))?;
    }
    println!("cargo:rerun-if-changed={}", schemas.display());
    Ok(())
}

fn generate(sources: &ModuleSources, name: &str, destination: &Path) -> Result<(), Box<dyn Error>> {
    let entry = format!("/{name}.capnp");
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
    let files = generate_requested_files(&schema)?;
    let generated = files.first().ok_or("request contains no requested file")?;
    fs::write(destination, &generated.source)?;
    Ok(())
}
