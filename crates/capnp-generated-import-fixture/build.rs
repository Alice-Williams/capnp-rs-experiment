use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::PathBuf;

use capnp_codegen::{GenerateOptions, generate_requested_file_with_options};
use capnp_compiler::request::compile_program;
use capnp_compiler::semantic::{ModuleSources, ResolveLimits};
use capnp_schema::CapnpVersion;

fn main() -> Result<(), Box<dyn Error>> {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest dir")?);
    let schemas = manifest.join("../../conformance/schemas");
    let mut sources = ModuleSources::default();
    for name in ["import-fixture", "wire-fixture", "language-fixture"] {
        sources.insert_explicit(
            format!("/{name}.capnp"),
            fs::read_to_string(schemas.join(format!("{name}.capnp")))?,
        );
    }
    let program = sources.resolve("/import-fixture.capnp", ResolveLimits::default());
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
    let file = schema
        .requested_files()
        .first()
        .ok_or("no requested file")?;
    let mut options = GenerateOptions::default();
    for import in &file.imports {
        let path = if import.name.contains("wire") {
            "capnp_generated_fixture::wire"
        } else {
            "capnp_generated_fixture::language"
        };
        options.import_paths.insert(import.id, path.to_owned());
    }
    let generated = generate_requested_file_with_options(&schema, file.id, &options)?;
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing output dir")?);
    fs::write(output.join("import_fixture.rs"), generated.source)?;
    println!("cargo:rerun-if-changed={}", schemas.display());
    Ok(())
}
