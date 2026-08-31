use std::error::Error;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};
use capnp_rpc_core::{
    ActorEffect, ActorLimits, CompletionToken, ConnectionActor, ConnectionHandle, HandlerResult,
    ProtocolLimits, QuestionFuture, ReturnPayload,
};

const DEFAULT_SESSIONS: u64 = 100_000;
const DEFAULT_SECONDS: u64 = 24 * 60 * 60;
const RSS_ALLOWANCE_KIB: u64 = 64 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    let mut minimum_sessions = DEFAULT_SESSIONS;
    let mut duration = Duration::from_secs(DEFAULT_SECONDS);
    let mut seed = 0x6d34_305f_736f_616b_u64;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let value = arguments
            .next()
            .ok_or_else(|| format!("missing value after {argument}"))?;
        match argument.as_str() {
            "--minimum-sessions" => minimum_sessions = value.parse()?,
            "--duration-seconds" => duration = Duration::from_secs(value.parse()?),
            "--seed" => seed = value.parse()?,
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }

    let started = Instant::now();
    let mut random = Random::new(seed);
    let warmup_sessions = minimum_sessions.clamp(1_000, 10_000);
    for _ in 0..warmup_sessions {
        run_session(&mut random)?;
    }
    let baseline_rss_kib = resident_kib();
    let mut maximum_rss_kib = baseline_rss_kib;
    let mut sessions = 0_u64;
    let mut next_report = Instant::now() + Duration::from_secs(60);

    while sessions < minimum_sessions || started.elapsed() < duration {
        run_session(&mut random)?;
        sessions = sessions.saturating_add(1);
        if sessions % 10_000 == 0 {
            maximum_rss_kib = maximum_rss_kib.max(resident_kib());
        }
        if Instant::now() >= next_report {
            eprintln!(
                "m40-soak-progress sessions={sessions} elapsed_seconds={} rss_kib={}",
                started.elapsed().as_secs(),
                resident_kib()
            );
            next_report += Duration::from_secs(60);
        }
    }

    let final_rss_kib = resident_kib();
    maximum_rss_kib = maximum_rss_kib.max(final_rss_kib);
    if baseline_rss_kib != 0 && final_rss_kib > baseline_rss_kib.saturating_add(RSS_ALLOWANCE_KIB) {
        return Err(format!(
            "resident memory grew from {baseline_rss_kib} KiB to {final_rss_kib} KiB"
        )
        .into());
    }

    println!(
        "m40-level1-soak-ok sessions={sessions} warmup_sessions={warmup_sessions} seed={seed} elapsed_seconds={} baseline_rss_kib={baseline_rss_kib} maximum_rss_kib={maximum_rss_kib} final_rss_kib={final_rss_kib}",
        started.elapsed().as_secs()
    );
    Ok(())
}

fn run_session(random: &mut Random) -> Result<(), Box<dyn Error>> {
    let limits = ActorLimits {
        mailbox_capacity: 16,
        max_questions: 8,
        max_answers: 8,
        max_incoming_call_bytes: 4096,
        max_imports: 8,
        max_exports: 8,
        max_embargoes: 8,
        max_embargoed_calls: 8,
    };
    let (left_handle, mut left) = ConnectionActor::new(limits, ProtocolLimits::default());
    let (right_handle, mut right) = ConnectionActor::new(limits, ProtocolLimits::default());

    let mut root = left_handle.bootstrap()?;
    let target = root.target();
    transfer_one_send(&mut left, &right_handle)?;
    let root_completion = take_dispatch(&mut right)?;

    let mut call = left_handle.call(
        &target,
        0xfeed_face_cafe_beef,
        u16::try_from(random.next_u64() & 0xffff)?,
        message(random.next_u64())?,
    )?;
    transfer_one_send(&mut left, &right_handle)?;
    let call_completion = take_dispatch(&mut right)?;

    match random.next_u64() % 5 {
        0 => complete_successfully(
            &mut left,
            &left_handle,
            &mut right,
            &right_handle,
            &mut root,
            &mut call,
            root_completion,
            call_completion,
            random,
        )?,
        1 => cancel_call(
            &mut left,
            &left_handle,
            &mut right,
            &right_handle,
            root,
            call,
            root_completion,
            call_completion,
            false,
            random,
        )?,
        2 => cancel_call(
            &mut left,
            &left_handle,
            &mut right,
            &right_handle,
            root,
            call,
            root_completion,
            call_completion,
            true,
            random,
        )?,
        3 => {
            root.cancel()?;
            call.cancel()?;
            drain_sends(&mut left, &right_handle)?;
            drain_effects(&mut right)?;
            let _ = root_completion.complete(HandlerResult::Canceled);
            let _ = call_completion.complete(HandlerResult::Canceled);
        }
        _ => {
            drop(root);
            drop(call);
            let _ = root_completion;
            let _ = call_completion;
        }
    }

    terminate(&left_handle, &mut left)?;
    terminate(&right_handle, &mut right)?;
    assert_empty(&left)?;
    assert_empty(&right)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn complete_successfully(
    left: &mut ConnectionActor,
    left_handle: &ConnectionHandle,
    right: &mut ConnectionActor,
    right_handle: &ConnectionHandle,
    root: &mut QuestionFuture,
    call: &mut QuestionFuture,
    root_completion: CompletionToken,
    call_completion: CompletionToken,
    random: &mut Random,
) -> Result<(), Box<dyn Error>> {
    call_completion.complete(HandlerResult::Results(message(random.next_u64())?))?;
    transfer_one_send(right, left_handle)?;
    transfer_one_send(left, right_handle)?;
    expect_results(call)?;

    root_completion.complete(HandlerResult::Results(message(random.next_u64())?))?;
    transfer_one_send(right, left_handle)?;
    transfer_one_send(left, right_handle)?;
    expect_results(root)?;
    drain_effects(right)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cancel_call(
    left: &mut ConnectionActor,
    left_handle: &ConnectionHandle,
    right: &mut ConnectionActor,
    right_handle: &ConnectionHandle,
    root: QuestionFuture,
    call: QuestionFuture,
    root_completion: CompletionToken,
    call_completion: CompletionToken,
    opt_out: bool,
    random: &mut Random,
) -> Result<(), Box<dyn Error>> {
    let cancellation = call_completion.cancellation();
    if opt_out && !call_completion.disallow_cancellation() {
        return Err(failure("cancellation opt-out lost before Finish"));
    }
    call.cancel()?;
    transfer_one_send(left, right_handle)?;
    drain_effects(right)?;
    if opt_out {
        if cancellation.is_canceled() {
            return Err(failure("opted-out handler was canceled"));
        }
        call_completion.complete(HandlerResult::Results(message(random.next_u64())?))?;
    } else {
        if !cancellation.is_canceled() {
            return Err(failure("cancelable handler did not observe Finish"));
        }
        let _ = call_completion.complete(HandlerResult::Canceled);
    }
    drain_effects(right)?;

    root_completion.complete(HandlerResult::Results(message(random.next_u64())?))?;
    transfer_one_send(right, left_handle)?;
    transfer_one_send(left, right_handle)?;
    let mut root = root;
    expect_results(&mut root)?;
    drain_effects(right)?;
    Ok(())
}

fn transfer_one_send(
    actor: &mut ConnectionActor,
    peer: &ConnectionHandle,
) -> Result<(), Box<dyn Error>> {
    match next(actor) {
        Poll::Ready(Some(ActorEffect::Send(message))) => peer.receive(message).map_err(Into::into),
        _ => Err(failure("expected an actor send effect")),
    }
}

fn take_dispatch(actor: &mut ConnectionActor) -> Result<CompletionToken, Box<dyn Error>> {
    match next(actor) {
        Poll::Ready(Some(ActorEffect::Dispatch { completion, .. })) => Ok(completion),
        _ => Err(failure("expected a remote dispatch effect")),
    }
}

fn drain_sends(actor: &mut ConnectionActor, peer: &ConnectionHandle) -> Result<(), Box<dyn Error>> {
    for _ in 0..32 {
        match next(actor) {
            Poll::Ready(Some(ActorEffect::Send(message))) => peer.receive(message)?,
            Poll::Pending | Poll::Ready(None) => return Ok(()),
            Poll::Ready(Some(ActorEffect::CloseTransport)) => {}
            Poll::Ready(Some(ActorEffect::Dispatch { .. } | ActorEffect::DispatchLocal { .. })) => {
                return Err(failure("unexpected dispatch while draining sends"));
            }
        }
    }
    Err(failure("send drain exceeded its transition bound"))
}

fn drain_effects(actor: &mut ConnectionActor) -> Result<(), Box<dyn Error>> {
    for _ in 0..32 {
        match next(actor) {
            Poll::Pending | Poll::Ready(None) => return Ok(()),
            Poll::Ready(Some(ActorEffect::CloseTransport | ActorEffect::Send(_))) => {}
            Poll::Ready(Some(ActorEffect::Dispatch { .. } | ActorEffect::DispatchLocal { .. })) => {
                return Err(failure("unexpected dispatch while draining actor"));
            }
        }
    }
    Err(failure("actor drain exceeded its transition bound"))
}

fn terminate(handle: &ConnectionHandle, actor: &mut ConnectionActor) -> Result<(), Box<dyn Error>> {
    let _ = handle.shutdown();
    for _ in 0..64 {
        match next(actor) {
            Poll::Ready(None) => return Ok(()),
            Poll::Ready(Some(_)) | Poll::Pending => {}
        }
    }
    Err(failure(
        "connection did not terminate within 64 transitions",
    ))
}

fn assert_empty(actor: &ConnectionActor) -> Result<(), Box<dyn Error>> {
    let stats = actor.stats();
    if stats.active_questions != 0
        || stats.active_answers != 0
        || stats.incoming_call_bytes != 0
        || stats.active_imports != 0
        || stats.active_exports != 0
        || stats.import_references != 0
        || stats.export_references != 0
        || stats.active_embargoes != 0
        || stats.queued_embargo_calls != 0
    {
        return Err(format!("connection retained live state after disconnect: {stats:?}").into());
    }
    Ok(())
}

fn expect_results(future: &mut QuestionFuture) -> Result<(), Box<dyn Error>> {
    let mut context = Context::from_waker(Waker::noop());
    match Pin::new(future).poll(&mut context) {
        Poll::Ready(Ok(ReturnPayload::Results(_))) => Ok(()),
        _ => Err(failure("question did not complete with results")),
    }
}

fn next(actor: &mut ConnectionActor) -> Poll<Option<ActorEffect>> {
    let mut context = Context::from_waker(Waker::noop());
    actor.poll_next_effect(&mut context)
}

fn message(value: u64) -> Result<Arc<OwnedMessage>, Box<dyn Error>> {
    let mut arena = ExclusiveArena::new(2, 16)?;
    arena.init_root_struct(1, 0)?.set_u64(0, value, 0)?;
    Ok(OwnedMessage::new(
        arena.into_segments(),
        ReaderLimits::default(),
    )?)
}

fn resident_kib() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    status
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(0)
}

fn failure(message: &str) -> Box<dyn Error> {
    io::Error::other(message).into()
}

struct Random(u64);

impl Random {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }
}
