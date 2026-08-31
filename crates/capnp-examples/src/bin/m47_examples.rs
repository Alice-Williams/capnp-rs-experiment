use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::PathBuf;

use capnp_examples::{address_book_example, calculator_example, platform_example};

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut arguments = env::args_os().skip(1);
    let frame_path = match arguments.next() {
        Some(flag) if flag == "--addressbook-frame" => {
            Some(PathBuf::from(arguments.next().ok_or_else(|| {
                io::Error::other("--addressbook-frame requires a path")
            })?))
        }
        Some(_) => {
            return Err(io::Error::other("usage: m47_examples [--addressbook-frame PATH]").into());
        }
        None => None,
    };
    if arguments.next().is_some() {
        return Err(io::Error::other("unexpected trailing argument").into());
    }

    let address_book = address_book_example::run()?;
    if let Some(path) = frame_path {
        fs::write(path, &address_book.standard)?;
    }
    for person in &address_book.standard_summary {
        println!("address-book: {person}");
    }

    let calculator = calculator_example::run()?;
    println!(
        "calculator: operator={} callback={} defined={} concurrent={:?} callback-calls={}",
        calculator.operator_result,
        calculator.callback_result,
        calculator.defined_function_result,
        calculator.concurrent_results,
        calculator.callback_calls
    );

    let platform = platform_example::run()?;
    println!(
        "platform: stream={} ends={} cancellations={} handoff={} restart={}->{} object={}",
        String::from_utf8_lossy(&platform.streamed_bytes),
        platform.clean_ends,
        platform.cancellations,
        platform.direct_handoff,
        platform.original_connection,
        platform.restored_connection,
        platform.restored_object
    );
    Ok(())
}
