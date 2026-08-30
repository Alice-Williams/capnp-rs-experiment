use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

use capnp_codegen::{GenerateOptions, generate_requested_file_with_options};
use capnp_schema::{CompiledSchema, LoadLimits};

const ORACLE: &str = "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b";

fn main() -> Result<(), Box<dyn Error>> {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest dir")?);
    let request = manifest
        .join("../../conformance/fixtures/cpp")
        .join(ORACLE)
        .join("compiler-request-import-fixture.bin");
    let schema =
        CompiledSchema::from_code_generator_request(&fs::read(&request)?, LoadLimits::default())?;
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
    println!("cargo:rerun-if-changed={}", request.display());
    Ok(())
}
