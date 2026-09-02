use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};
use capnp_rpc::{
    ActorLimits, CapDescriptor, ConnectionDriver, DriverDispatch, DuplexTransport, EnvelopeLimits,
    HandlerResult, HostedCapability, IncomingRequest, OutgoingCapability, ProtocolLimits,
    ReturnPayload,
};
use capnp_rpc_core::memory_transport_pair;
use capnp_schema::DynamicAnyPointer;

const PING_INTERFACE_ID: u64 = 0xedeceb51a9a148d1;
const PING_METHOD_ID: u16 = 0;
const PROFILE_NAMES: [&str; 9] = [
    "request_build",
    "submit_call",
    "client_request",
    "server_dispatch",
    "server_handle",
    "server_return",
    "client_return",
    "result_check",
    "server_finish",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let iterations = std::env::args()
        .nth(1)
        .ok_or("usage: native_rpc ITERATIONS")?
        .parse::<u64>()?;

    let envelope_limits = EnvelopeLimits::default();
    let (client_transport, server_transport) = memory_transport_pair(envelope_limits);
    let (client_handle, mut client) = ConnectionDriver::new(
        client_transport,
        ActorLimits::default(),
        ProtocolLimits::default(),
        envelope_limits,
    );
    let (_server_handle, mut server) = ConnectionDriver::new(
        server_transport,
        ActorLimits::default(),
        ProtocolLimits::default(),
        envelope_limits,
    );

    let mut bootstrap = client_handle.bootstrap()?;
    expect_pending(&mut client)?;
    let bootstrap_dispatch = expect_dispatch(&mut server)?;
    if !matches!(bootstrap_dispatch.request, IncomingRequest::Bootstrap) {
        return Err("server received a call before bootstrap".into());
    }
    bootstrap_dispatch.completion.complete_with_capabilities(
        capability_result(0)?,
        vec![OutgoingCapability::Hosted(HostedCapability::new()?)],
    )?;
    expect_pending(&mut server)?;
    expect_pending(&mut client)?;
    let bootstrap_payload = expect_results(&mut bootstrap)?;
    let import_id = match bootstrap_payload
        .cap_table
        .first()
        .map(CapDescriptor::descriptor)
    {
        Some(CapDescriptor::SenderHosted(import_id)) => *import_id,
        _ => return Err("bootstrap did not return a hosted capability".into()),
    };
    expect_pending(&mut server)?;

    let mut checksum = 0_u64;
    let mut profile = PhaseProfile::from_environment();
    for index in 0..iterations {
        profile.begin();
        let params = data_message(index)?;
        profile.mark(0);
        let mut response = client_handle.call_imported(
            import_id,
            PING_INTERFACE_ID,
            PING_METHOD_ID,
            params,
            Vec::new(),
        )?;
        profile.mark(1);
        expect_pending(&mut client)?;
        profile.mark(2);
        let dispatch = expect_dispatch(&mut server)?;
        profile.mark(3);
        let IncomingRequest::Call {
            interface_id,
            method_id,
            params,
            ..
        } = dispatch.request
        else {
            return Err("unexpected repeated bootstrap".into());
        };
        if interface_id != PING_INTERFACE_ID || method_id != PING_METHOD_ID {
            return Err("unexpected ping method identity".into());
        }
        let request_value = read_value(&params.content)?;
        dispatch
            .completion
            .complete(HandlerResult::Results(data_message(request_value + 1)?))?;
        profile.mark(4);
        expect_pending(&mut server)?;
        profile.mark(5);
        expect_pending(&mut client)?;
        profile.mark(6);
        let results = expect_results(&mut response)?;
        checksum ^= read_value(&results.content)?;
        profile.mark(7);
        expect_pending(&mut server)?;
        profile.mark(8);
    }

    if checksum != expected_checksum(iterations) {
        return Err("ping checksum mismatch".into());
    }
    profile.report(iterations);
    println!("{checksum}");
    Ok(())
}

fn drive<T: DuplexTransport>(
    driver: &mut ConnectionDriver<T>,
) -> Poll<Result<Option<DriverDispatch>, capnp_rpc::DriverError<T::Error>>> {
    let mut context = Context::from_waker(Waker::noop());
    driver.poll_next_dispatch(&mut context)
}

fn expect_pending<T>(driver: &mut ConnectionDriver<T>) -> Result<(), Box<dyn std::error::Error>>
where
    T: DuplexTransport,
    T::Error: std::error::Error + 'static,
{
    match drive(driver) {
        Poll::Pending => Ok(()),
        Poll::Ready(Ok(Some(_))) => Err("unexpected application dispatch".into()),
        Poll::Ready(Ok(None)) => Err("connection closed unexpectedly".into()),
        Poll::Ready(Err(error)) => Err(Box::new(error)),
    }
}

fn expect_dispatch<T>(
    driver: &mut ConnectionDriver<T>,
) -> Result<DriverDispatch, Box<dyn std::error::Error>>
where
    T: DuplexTransport,
    T::Error: std::error::Error + 'static,
{
    match drive(driver) {
        Poll::Ready(Ok(Some(dispatch))) => Ok(dispatch),
        Poll::Pending => Err("expected an application dispatch".into()),
        Poll::Ready(Ok(None)) => Err("connection closed unexpectedly".into()),
        Poll::Ready(Err(error)) => Err(Box::new(error)),
    }
}

fn expect_results(
    response: &mut capnp_rpc::QuestionFuture,
) -> Result<capnp_rpc::Payload, Box<dyn std::error::Error>> {
    let mut context = Context::from_waker(Waker::noop());
    match Pin::new(response).poll(&mut context) {
        Poll::Ready(Ok(ReturnPayload::Results(payload))) => Ok(payload),
        Poll::Ready(Ok(_)) => Err("RPC returned a non-results payload".into()),
        Poll::Ready(Err(error)) => Err(Box::new(error)),
        Poll::Pending => Err("RPC response remained pending after both drivers ran".into()),
    }
}

fn capability_result(index: u32) -> Result<Arc<OwnedMessage>, Box<dyn std::error::Error>> {
    let mut arena = ExclusiveArena::new(2, 16)?;
    arena.init_root_struct(0, 1)?.set_capability(0, index)?;
    Ok(OwnedMessage::new(
        arena.into_segments(),
        ReaderLimits::default(),
    )?)
}

fn data_message(value: u64) -> Result<Arc<OwnedMessage>, Box<dyn std::error::Error>> {
    let mut arena = ExclusiveArena::new(2, 16)?;
    arena.init_root_struct(1, 0)?.set_u64(0, value, 0)?;
    Ok(OwnedMessage::new(
        arena.into_segments(),
        ReaderLimits::default(),
    )?)
}

fn read_value(content: &DynamicAnyPointer) -> Result<u64, Box<dyn std::error::Error>> {
    let DynamicAnyPointer::Struct(root) = content else {
        return Err("ping payload root was not a struct".into());
    };
    root.with_reader(|reader| {
        let data = reader
            .data_section()
            .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;
        data.read_u64(0, 0)
            .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
    })?
}

fn expected_checksum(iterations: u64) -> u64 {
    (1..=iterations).fold(0, std::ops::BitXor::bitxor)
}

struct PhaseProfile {
    enabled: bool,
    last: Option<Instant>,
    totals: [Duration; PROFILE_NAMES.len()],
}

impl PhaseProfile {
    fn from_environment() -> Self {
        Self {
            enabled: std::env::var_os("CAPNP_BENCH_PROFILE").is_some(),
            last: None,
            totals: [Duration::ZERO; PROFILE_NAMES.len()],
        }
    }

    fn begin(&mut self) {
        if self.enabled {
            self.last = Some(Instant::now());
        }
    }

    fn mark(&mut self, phase: usize) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.last.replace(now) {
            self.totals[phase] += now.duration_since(last);
        }
    }

    fn report(&self, iterations: u64) {
        if !self.enabled {
            return;
        }
        eprint!("profile iterations={iterations}");
        for (name, total) in PROFILE_NAMES.iter().zip(self.totals) {
            eprint!(" {name}_ns={}", total.as_nanos());
        }
        eprintln!();
    }
}
