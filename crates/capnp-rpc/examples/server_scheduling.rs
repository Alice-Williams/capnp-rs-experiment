use std::error::Error;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};
use capnp_rpc::{
    Concurrent, ExecutorService, Keyed, LocalService, MessageFuture, RpcError, Serial,
    ThreadPoolExecutor,
};

struct CpuService {
    rounds: u64,
    began: Instant,
    completions: Arc<Mutex<Vec<(u64, u16, Duration)>>>,
}

impl LocalService for CpuService {
    fn dispatch(
        self: Arc<Self>,
        job_id: u64,
        key: u16,
        params: Arc<OwnedMessage>,
    ) -> MessageFuture {
        Box::pin(async move {
            let mut value = job_id ^ u64::from(key);
            for round in 0..self.rounds {
                value = value
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(round ^ 1_442_695_040_888_963_407);
            }
            std::hint::black_box(value);
            self.completions
                .lock()
                .map_err(|_| RpcError::MissingResponse)?
                .push((job_id, key, self.began.elapsed()));
            Ok(params)
        })
    }
}

struct ThreadWake(std::thread::Thread);

impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}

fn params() -> Result<Arc<OwnedMessage>, Box<dyn Error>> {
    let mut arena = ExclusiveArena::new(1, 8)?;
    arena.init_root_struct(0, 0)?;
    Ok(OwnedMessage::new(
        arena.into_segments(),
        ReaderLimits::default(),
    )?)
}

fn percentile(sorted: &[Duration], numerator: usize, denominator: usize) -> Duration {
    let rank = sorted
        .len()
        .saturating_mul(numerator)
        .div_ceil(denominator)
        .saturating_sub(1)
        .min(sorted.len().saturating_sub(1));
    sorted[rank]
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let policy = arguments
        .next()
        .ok_or("usage: server_scheduling POLICY WORKERS JOBS ROUNDS")?;
    let workers = arguments
        .next()
        .ok_or("missing workers")?
        .parse::<usize>()?;
    let jobs = arguments.next().ok_or("missing jobs")?.parse::<usize>()?;
    let rounds = arguments.next().ok_or("missing rounds")?.parse::<u64>()?;
    if arguments.next().is_some() || jobs == 0 {
        return Err("invalid scheduling benchmark arguments".into());
    }

    let began = Instant::now();
    let completions = Arc::new(Mutex::new(Vec::with_capacity(jobs)));
    let base = Arc::new(CpuService {
        rounds,
        began,
        completions: Arc::clone(&completions),
    });
    let executor = ThreadPoolExecutor::new(workers, jobs)?;
    let executed: Arc<dyn LocalService> = Arc::new(ExecutorService::new(base, executor));
    let service: Arc<dyn LocalService> = match policy.as_str() {
        "concurrent" => Arc::new(Concurrent::new(executed)),
        "serial" => Arc::new(Serial::new(executed)),
        "keyed" => Arc::new(Keyed::new(executed, |_job, key, _params: &OwnedMessage| {
            key
        })),
        _ => return Err(format!("unknown policy {policy}").into()),
    };
    let params = params()?;
    let mut responses = Vec::with_capacity(jobs);
    for job in 0..jobs {
        let job_id = u64::try_from(job)?;
        let key = u16::try_from(job % 4)?;
        responses.push(Arc::clone(&service).dispatch(job_id, key, Arc::clone(&params)));
    }
    std::thread::scope(|scope| -> Result<(), Box<dyn Error>> {
        let handles = responses
            .into_iter()
            .map(|response| scope.spawn(move || block_on(response)))
            .collect::<Vec<_>>();
        for handle in handles {
            match handle.join() {
                Ok(result) => {
                    result?;
                }
                Err(_) => return Err("benchmark response thread panicked".into()),
            }
        }
        Ok(())
    })?;
    let elapsed = began.elapsed();
    let mut completions = completions.lock().map_err(|_| "completion lock")?.clone();
    completions.sort_by_key(|(_, _, latency)| *latency);
    if completions.len() != jobs {
        return Err("missing benchmark completion".into());
    }
    let mut latencies = completions
        .iter()
        .map(|(_, _, latency)| *latency)
        .collect::<Vec<_>>();
    latencies.sort_unstable();
    let p50 = percentile(&latencies, 50, 100);
    let p99 = percentile(&latencies, 99, 100);
    let mut maximum_key_run = 0_usize;
    let mut current_key = None;
    let mut current_run = 0_usize;
    for (_, key, _) in completions {
        if current_key == Some(key) {
            current_run += 1;
        } else {
            current_key = Some(key);
            current_run = 1;
        }
        maximum_key_run = maximum_key_run.max(current_run);
    }
    let throughput = jobs as f64 / elapsed.as_secs_f64();
    println!(
        "{policy}\t{workers}\t{jobs}\t{rounds}\t{}\t{throughput:.3}\t{}\t{}\t{maximum_key_run}",
        elapsed.as_nanos(),
        p50.as_micros(),
        p99.as_micros(),
    );
    Ok(())
}
