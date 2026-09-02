//! Native implementation of the pinned C++ product benchmark scenarios.

#[allow(dead_code)]
mod carsales {
    include!(concat!(env!("OUT_DIR"), "/carsales.rs"));
}

#[allow(dead_code)]
mod catrank {
    include!(concat!(env!("OUT_DIR"), "/catrank.rs"));
}

#[allow(dead_code)]
mod eval {
    include!(concat!(env!("OUT_DIR"), "/eval.rs"));
}

mod cases;
mod common;

use std::error::Error;
use std::sync::Arc;

use capnp_schema::{CompiledSchema, LoadLimits};

use crate::cases::{CarSales, CatRank, Eval};
use crate::common::{Compression, Mode, run};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let case = arguments.next().ok_or(usage())?;
    let mode = arguments.next().ok_or(usage())?.parse::<Mode>()?;
    let reuse = arguments.next().ok_or(usage())?;
    let compression = arguments.next().ok_or(usage())?.parse::<Compression>()?;
    let iterations = arguments.next().ok_or(usage())?.parse::<u64>()?;
    if arguments.next().is_some() || reuse != "no-reuse" {
        return Err(usage().into());
    }

    let schema = match case.as_str() {
        "carsales" => schema(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/carsales.request.bin"
        )))?,
        "catrank" => schema(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/catrank.request.bin"
        )))?,
        "eval" => schema(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/eval.request.bin"
        )))?,
        _ => return Err(usage().into()),
    };
    let throughput = match case.as_str() {
        "carsales" => run::<CarSales>(&schema, mode, compression, iterations)?,
        "catrank" => run::<CatRank>(&schema, mode, compression, iterations)?,
        "eval" => run::<Eval>(&schema, mode, compression, iterations)?,
        _ => unreachable!("case was checked while loading its schema"),
    };
    println!("{throughput}");
    Ok(())
}

fn schema(bytes: &[u8]) -> Result<Arc<CompiledSchema>, Box<dyn Error>> {
    Ok(Arc::new(CompiledSchema::from_code_generator_request(
        bytes,
        LoadLimits::default(),
    )?))
}

fn usage() -> &'static str {
    "usage: capnp-native-benchmark CASE MODE no-reuse COMPRESSION ITERATIONS"
}
