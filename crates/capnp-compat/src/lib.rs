#![doc = "Optional Cap'n Proto text, JSON, and ecosystem adapters."]

mod byte_stream;

pub use byte_stream::{
    ByteSink, ByteStream, ByteStreamError, ByteStreamState, ByteSubstream, SubstreamCallback,
};
