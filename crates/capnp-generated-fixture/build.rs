use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use capnp_codegen::generate_requested_files;
use capnp_schema::{CompiledSchema, LoadLimits};

const ORACLE: &str = "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b";

fn main() -> Result<(), Box<dyn Error>> {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest dir")?);
    let fixtures = manifest.join("../../conformance/fixtures/cpp").join(ORACLE);
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing output dir")?);
    for (fixture, destination) in [
        ("compiler-request-wire-fixture.bin", "wire_fixture.rs"),
        ("compiler-request-evolution-v2.bin", "evolution_v2.rs"),
        ("compiler-request-import-fixture.bin", "import_fixture.rs"),
        (
            "compiler-request-language-fixture.bin",
            "language_fixture.rs",
        ),
        (
            "compiler-request-streaming-fixture.bin",
            "streaming_fixture.rs",
        ),
    ] {
        generate(&fixtures.join(fixture), &output.join(destination))?;
    }
    println!("cargo:rerun-if-changed={}", fixtures.display());
    Ok(())
}

fn generate(request: &Path, destination: &Path) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(request)?;
    let schema = CompiledSchema::from_code_generator_request(&bytes, LoadLimits::default())?;
    let files = generate_requested_files(&schema)?;
    let generated = files.first().ok_or("request contains no requested file")?;
    fs::write(destination, &generated.source)?;
    Ok(())
}
