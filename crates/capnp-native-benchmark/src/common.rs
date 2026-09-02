use std::error::Error;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use capnp_io::{FrameLimits, FrameRead, encode_frame, pack, parse_frame, unpack};
use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};
use capnp_schema::CompiledSchema;

const MAX_WORDS: u32 = 128 * 1024;
const MAX_BYTES: usize = MAX_WORDS as usize * 8 + 64 * 1024;

pub type BenchResult<T> = Result<T, Box<dyn Error>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Object,
    Bytes,
}

impl FromStr for Mode {
    type Err = ArgumentError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "object" => Ok(Self::Object),
            "bytes" => Ok(Self::Bytes),
            _ => Err(ArgumentError("mode must be object or bytes")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Compression {
    None,
    Packed,
}

impl FromStr for Compression {
    type Err = ArgumentError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "none" => Ok(Self::None),
            "packed" => Ok(Self::Packed),
            _ => Err(ArgumentError("compression must be none or packed")),
        }
    }
}

#[derive(Debug)]
pub struct ArgumentError(pub &'static str);

impl fmt::Display for ArgumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for ArgumentError {}

pub trait Case {
    type Expectation;

    fn build_request(
        schema: &CompiledSchema,
        arena: &mut ExclusiveArena,
        random: &mut FastRand,
    ) -> BenchResult<Self::Expectation>;

    fn handle_request(
        schema: &Arc<CompiledSchema>,
        request: Arc<OwnedMessage>,
    ) -> BenchResult<ExclusiveArena>;

    fn check_response(
        schema: &Arc<CompiledSchema>,
        response: Arc<OwnedMessage>,
        expected: Self::Expectation,
    ) -> BenchResult<bool>;
}

pub fn run<C: Case>(
    schema: &Arc<CompiledSchema>,
    mode: Mode,
    compression: Compression,
    iterations: u64,
) -> BenchResult<u64> {
    if std::env::var_os("CAPNP_BENCH_PROFILE").is_some() {
        return run_profiled::<C>(schema, mode, compression, iterations);
    }
    if mode == Mode::Object && compression != Compression::None {
        return Err(ArgumentError("object mode requires compression=none").into());
    }
    let mut random = FastRand::default();
    let mut throughput = 0_u64;
    for _ in 0..iterations {
        let mut request_arena = new_arena()?;
        let expected = C::build_request(schema, &mut request_arena, &mut random)?;
        let request = match mode {
            Mode::Object => owned(request_arena)?,
            Mode::Bytes => {
                let bytes = encode(&request_arena, compression)?;
                throughput = throughput
                    .checked_add(u64::try_from(bytes.len())?)
                    .ok_or(ArgumentError("throughput overflow"))?;
                decode(&bytes, compression)?
            }
        };

        let response_arena = C::handle_request(schema, request)?;
        let response = match mode {
            Mode::Object => owned(response_arena)?,
            Mode::Bytes => {
                let bytes = encode(&response_arena, compression)?;
                throughput = throughput
                    .checked_add(u64::try_from(bytes.len())?)
                    .ok_or(ArgumentError("throughput overflow"))?;
                decode(&bytes, compression)?
            }
        };
        if !C::check_response(schema, response, expected)? {
            return Err(ArgumentError("response did not match the request expectation").into());
        }
    }
    Ok(throughput)
}

#[derive(Default)]
struct PhaseTimes {
    build_request: Duration,
    encode_request: Duration,
    decode_request: Duration,
    handle_request: Duration,
    encode_response: Duration,
    decode_response: Duration,
    check_response: Duration,
}

fn run_profiled<C: Case>(
    schema: &Arc<CompiledSchema>,
    mode: Mode,
    compression: Compression,
    iterations: u64,
) -> BenchResult<u64> {
    if iterations == 0 {
        return Err(ArgumentError("profiled runs require at least one iteration").into());
    }
    if mode == Mode::Object && compression != Compression::None {
        return Err(ArgumentError("object mode requires compression=none").into());
    }
    let mut random = FastRand::default();
    let mut throughput = 0_u64;
    let mut phases = PhaseTimes::default();
    for _ in 0..iterations {
        let mut request_arena = new_arena()?;
        let started = Instant::now();
        let expected = C::build_request(schema, &mut request_arena, &mut random)?;
        phases.build_request += started.elapsed();
        let request = match mode {
            Mode::Object => owned(request_arena)?,
            Mode::Bytes => {
                let started = Instant::now();
                let bytes = encode(&request_arena, compression)?;
                phases.encode_request += started.elapsed();
                throughput = throughput
                    .checked_add(u64::try_from(bytes.len())?)
                    .ok_or(ArgumentError("throughput overflow"))?;
                let started = Instant::now();
                let message = decode(&bytes, compression)?;
                phases.decode_request += started.elapsed();
                message
            }
        };

        let started = Instant::now();
        let response_arena = C::handle_request(schema, request)?;
        phases.handle_request += started.elapsed();
        let response = match mode {
            Mode::Object => owned(response_arena)?,
            Mode::Bytes => {
                let started = Instant::now();
                let bytes = encode(&response_arena, compression)?;
                phases.encode_response += started.elapsed();
                throughput = throughput
                    .checked_add(u64::try_from(bytes.len())?)
                    .ok_or(ArgumentError("throughput overflow"))?;
                let started = Instant::now();
                let message = decode(&bytes, compression)?;
                phases.decode_response += started.elapsed();
                message
            }
        };
        let started = Instant::now();
        let matches = C::check_response(schema, response, expected)?;
        phases.check_response += started.elapsed();
        if !matches {
            return Err(ArgumentError("response did not match the request expectation").into());
        }
    }
    print_phases(&phases, iterations);
    Ok(throughput)
}

fn print_phases(phases: &PhaseTimes, iterations: u64) {
    eprintln!("phase\ttotal_ns\tns_per_iteration");
    for (name, elapsed) in [
        ("build_request", phases.build_request),
        ("encode_request", phases.encode_request),
        ("decode_request", phases.decode_request),
        ("handle_request", phases.handle_request),
        ("encode_response", phases.encode_response),
        ("decode_response", phases.decode_response),
        ("check_response", phases.check_response),
    ] {
        eprintln!(
            "{name}\t{}\t{}",
            elapsed.as_nanos(),
            elapsed.as_nanos() / u128::from(iterations)
        );
    }
}

pub fn new_arena() -> BenchResult<ExclusiveArena> {
    Ok(ExclusiveArena::new(1024, MAX_WORDS)?)
}

fn owned(arena: ExclusiveArena) -> BenchResult<Arc<OwnedMessage>> {
    Ok(OwnedMessage::new(arena.into_segments(), reader_limits())?)
}

fn encode(arena: &ExclusiveArena, compression: Compression) -> BenchResult<Vec<u8>> {
    let segments = arena.segments().collect::<Vec<_>>();
    let frame = encode_frame(&segments, frame_limits())?;
    match compression {
        Compression::None => Ok(frame),
        Compression::Packed => Ok(pack(&frame, MAX_BYTES)?),
    }
}

fn decode(bytes: &[u8], compression: Compression) -> BenchResult<Arc<OwnedMessage>> {
    let unpacked;
    let frame_bytes = match compression {
        Compression::None => bytes,
        Compression::Packed => {
            unpacked = unpack(bytes, MAX_BYTES)?;
            &unpacked
        }
    };
    let frame = match parse_frame(frame_bytes, frame_limits())? {
        FrameRead::Message {
            frame,
            remaining: [],
        } => frame,
        FrameRead::Message { .. } => return Err(ArgumentError("frame has trailing bytes").into()),
        FrameRead::EndOfInput => return Err(ArgumentError("frame is empty").into()),
    };
    Ok(OwnedMessage::new(
        frame
            .segments()
            .iter()
            .map(|segment| Box::<[u8]>::from(segment.bytes()))
            .collect::<Vec<_>>(),
        reader_limits(),
    )?)
}

const fn reader_limits() -> ReaderLimits {
    ReaderLimits {
        traversal_words: 1_u64 << 40,
        nesting_levels: 64,
    }
}

const fn frame_limits() -> FrameLimits {
    FrameLimits {
        max_segments: 1,
        max_total_words: MAX_WORDS as u64,
    }
}

#[derive(Clone, Copy)]
pub struct FastRand {
    x: u32,
    y: u32,
    z: u32,
    w: u32,
}

impl Default for FastRand {
    fn default() -> Self {
        Self {
            x: 0x1d2a_cd47,
            y: 0x58ca_3e14,
            z: 0xf563_f232,
            w: 0x0bc7_6199,
        }
    }
}

impl FastRand {
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let temporary = self.x ^ (self.x << 11);
        self.x = self.y;
        self.y = self.z;
        self.z = self.w;
        self.w = self.w ^ (self.w >> 19) ^ temporary ^ (temporary >> 8);
        self.w
    }

    #[inline]
    pub fn next_less_than(&mut self, range: u32) -> u32 {
        self.next_u32() % range
    }

    #[inline]
    pub fn next_bool(&mut self) -> bool {
        self.next_less_than(2) == 1
    }

    #[inline]
    pub fn next_double(&mut self, range: f64) -> f64 {
        f64::from(self.next_u32()) * range / f64::from(u32::MAX)
    }
}

#[inline]
pub fn safe_divide(left: i32, right: i32) -> i32 {
    if right == 0 || (left == i32::MIN && right == -1) {
        i32::MAX
    } else {
        left / right
    }
}

#[inline]
pub fn safe_modulus(left: i32, right: i32) -> i32 {
    if right == 0 || (left == i32::MIN && right == -1) {
        i32::MAX
    } else {
        left % right
    }
}

pub const WORDS: [&str; 13] = [
    "foo ", "bar ", "baz ", "qux ", "quux ", "corge ", "grault ", "garply ", "waldo ", "fred ",
    "plugh ", "xyzzy ", "thud ",
];
