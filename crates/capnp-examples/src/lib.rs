#![doc = "End-to-end native Cap'n Proto examples derived from the pinned C++ samples."]

use std::error::Error;
use std::io;
use std::sync::{Arc, OnceLock};

use capnp_compiler::request::compile_program;
use capnp_compiler::semantic::{ModuleSources, ResolveLimits};
use capnp_schema::{CapnpVersion, CompiledSchema};

pub mod addressbook {
    include!(concat!(env!("OUT_DIR"), "/addressbook.rs"));
}

#[allow(clippy::module_inception)]
pub mod calculator {
    include!(concat!(env!("OUT_DIR"), "/calculator.rs"));
}

pub mod address_book_example;
pub mod calculator_example;
pub mod platform_example;

/// The fallible result shared by the executable examples.
pub type ExampleResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

static ADDRESSBOOK_SCHEMA: OnceLock<Arc<CompiledSchema>> = OnceLock::new();
static CALCULATOR_SCHEMA: OnceLock<Arc<CompiledSchema>> = OnceLock::new();

/// Compiles the checked-in address-book schema with the native compiler.
pub fn addressbook_schema() -> ExampleResult<Arc<CompiledSchema>> {
    cached_schema(
        &ADDRESSBOOK_SCHEMA,
        "/addressbook.capnp",
        include_str!("../schemas/addressbook.capnp"),
    )
}

/// Compiles the checked-in calculator schema with the native compiler.
pub fn calculator_schema() -> ExampleResult<Arc<CompiledSchema>> {
    cached_schema(
        &CALCULATOR_SCHEMA,
        "/calculator.capnp",
        include_str!("../schemas/calculator.capnp"),
    )
}

fn cached_schema(
    cache: &OnceLock<Arc<CompiledSchema>>,
    entry: &str,
    source: &str,
) -> ExampleResult<Arc<CompiledSchema>> {
    if let Some(schema) = cache.get() {
        return Ok(Arc::clone(schema));
    }
    let schema = compile_schema(entry, source)?;
    match cache.set(Arc::clone(&schema)) {
        Ok(()) => Ok(schema),
        Err(schema) => Ok(cache.get().map_or(schema, Arc::clone)),
    }
}

fn compile_schema(entry: &str, source: &str) -> ExampleResult<Arc<CompiledSchema>> {
    let mut sources = ModuleSources::default();
    sources.insert_explicit(entry, source);
    let program = sources.resolve(entry, ResolveLimits::default());
    if !program.is_valid() {
        return Err(io::Error::other(format!(
            "checked-in example schema did not resolve: {program:#?}"
        ))
        .into());
    }
    Ok(Arc::new(compile_program(
        &program,
        CapnpVersion {
            major: 1,
            minor: 0,
            micro: 2,
        },
    )?))
}
