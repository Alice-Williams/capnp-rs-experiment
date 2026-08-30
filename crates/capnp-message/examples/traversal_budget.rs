use std::hint::black_box;
use std::time::{Duration, Instant};

#[cfg(target_has_atomic = "64")]
use capnp_message::SharedTraversalBudget;
use capnp_message::{LocalTraversalBudget, TraversalBudget};

const DEFAULT_CHARGES: u64 = 10_000_000;

fn timed(mut operation: impl FnMut()) -> Duration {
    let started = Instant::now();
    operation();
    started.elapsed()
}

fn main() {
    let charges = std::env::args()
        .nth(1)
        .map(|value| value.parse().expect("charge count must be an integer"))
        .unwrap_or(DEFAULT_CHARGES);

    let local = LocalTraversalBudget::new(charges);
    let local_elapsed = timed(|| {
        for _ in 0..charges {
            black_box(&local)
                .try_charge(1)
                .expect("local limit is exact");
        }
    });
    println!(
        "local:  {charges} charges in {local_elapsed:?} ({:.2} ns/charge)",
        local_elapsed.as_nanos() as f64 / charges as f64
    );

    #[cfg(target_has_atomic = "64")]
    {
        let shared = SharedTraversalBudget::new(charges);
        let shared_elapsed = timed(|| {
            for _ in 0..charges {
                black_box(&shared)
                    .try_charge(1)
                    .expect("shared limit is exact");
            }
        });
        println!(
            "shared: {charges} charges in {shared_elapsed:?} ({:.2} ns/charge, {:.2}x local)",
            shared_elapsed.as_nanos() as f64 / charges as f64,
            shared_elapsed.as_secs_f64() / local_elapsed.as_secs_f64()
        );
    }
}
