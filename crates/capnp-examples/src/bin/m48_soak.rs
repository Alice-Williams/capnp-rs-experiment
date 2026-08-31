use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use capnp_examples::{ExampleResult, address_book_example, calculator_example, platform_example};

const DEFAULT_SESSIONS: u64 = 100_000;
const DEFAULT_SECONDS: u64 = 48 * 60 * 60;
const DEFAULT_WARMUP_SESSIONS: u64 = 100;
const DEFAULT_RSS_GROWTH_KIB: u64 = 64 * 1024;
const DEFAULT_SEED: u64 = 0x6d34_385f_736f_616b;
const ORDERS: [[u8; 3]; 6] = [
    [0, 1, 2],
    [0, 2, 1],
    [1, 0, 2],
    [1, 2, 0],
    [2, 0, 1],
    [2, 1, 0],
];

fn main() -> ExampleResult<()> {
    let settings = Settings::parse()?;
    write_status(
        settings.result_file.as_deref(),
        &format!(
            "status=RUNNING\nseed={}\nminimum_sessions={}\nduration_seconds={}\nwarmup_sessions={}\nmax_rss_growth_kib={}\nelapsed_seconds=0\nsessions=0\n",
            settings.seed,
            settings.minimum_sessions,
            settings.duration.as_secs(),
            settings.warmup_sessions,
            settings.max_rss_growth_kib,
        ),
    )?;

    let mut random = Random::new(settings.seed);
    for _ in 0..settings.warmup_sessions {
        run_session(&mut random)?;
    }

    let baseline_rss_kib = resident_kib();
    let mut maximum_rss_kib = baseline_rss_kib;
    let mut sessions = 0_u64;
    let started = Instant::now();
    let mut next_report = started + Duration::from_secs(60);
    while sessions < settings.minimum_sessions || started.elapsed() < settings.duration {
        run_session(&mut random)?;
        sessions = sessions.saturating_add(1);
        if sessions % 64 == 0 {
            maximum_rss_kib = maximum_rss_kib.max(resident_kib());
        }
        if Instant::now() >= next_report {
            let elapsed_seconds = started.elapsed().as_secs();
            let rss_kib = resident_kib();
            eprintln!(
                "m48-soak-progress sessions={sessions} elapsed_seconds={elapsed_seconds} rss_kib={rss_kib}"
            );
            write_status(
                settings.result_file.as_deref(),
                &format!(
                    "status=RUNNING\nseed={}\nminimum_sessions={}\nduration_seconds={}\nwarmup_sessions={}\nmax_rss_growth_kib={}\nelapsed_seconds={elapsed_seconds}\nsessions={sessions}\nrss_kib={rss_kib}\n",
                    settings.seed,
                    settings.minimum_sessions,
                    settings.duration.as_secs(),
                    settings.warmup_sessions,
                    settings.max_rss_growth_kib,
                ),
            )?;
            next_report += Duration::from_secs(60);
        }
    }

    let elapsed_seconds = started.elapsed().as_secs();
    let final_rss_kib = resident_kib();
    maximum_rss_kib = maximum_rss_kib.max(final_rss_kib);
    let allowed_rss_kib = baseline_rss_kib.saturating_add(settings.max_rss_growth_kib);
    if baseline_rss_kib != 0 && maximum_rss_kib > allowed_rss_kib {
        return Err(io::Error::other(format!(
            "resident memory grew from {baseline_rss_kib} KiB to a maximum of {maximum_rss_kib} KiB (limit {allowed_rss_kib} KiB)"
        ))
        .into());
    }

    let summary = format!(
        "m48-full-platform-soak-ok sessions={sessions} warmup_sessions={} seed={} elapsed_seconds={elapsed_seconds} baseline_rss_kib={baseline_rss_kib} maximum_rss_kib={maximum_rss_kib} final_rss_kib={final_rss_kib}",
        settings.warmup_sessions, settings.seed
    );
    write_status(
        settings.result_file.as_deref(),
        &format!(
            "status=PASS\n{summary}\ngate=PASS: at least {} sessions and {} wall-clock seconds; each session exercised address-book persistence, calculator capabilities, streaming cancellation, authenticated handoff, distributed equality, and persistent restart\n",
            settings.minimum_sessions,
            settings.duration.as_secs()
        ),
    )?;
    println!("{summary}");
    Ok(())
}

struct Settings {
    minimum_sessions: u64,
    duration: Duration,
    warmup_sessions: u64,
    max_rss_growth_kib: u64,
    seed: u64,
    result_file: Option<PathBuf>,
}

impl Settings {
    fn parse() -> ExampleResult<Self> {
        let mut settings = Self {
            minimum_sessions: DEFAULT_SESSIONS,
            duration: Duration::from_secs(DEFAULT_SECONDS),
            warmup_sessions: DEFAULT_WARMUP_SESSIONS,
            max_rss_growth_kib: DEFAULT_RSS_GROWTH_KIB,
            seed: DEFAULT_SEED,
            result_file: None,
        };
        let mut arguments = env::args().skip(1);
        while let Some(argument) = arguments.next() {
            let value = arguments
                .next()
                .ok_or_else(|| io::Error::other(format!("missing value after {argument}")))?;
            match argument.as_str() {
                "--minimum-sessions" => settings.minimum_sessions = parse_u64(&argument, &value)?,
                "--duration-seconds" => {
                    settings.duration = Duration::from_secs(parse_u64(&argument, &value)?)
                }
                "--warmup-sessions" => settings.warmup_sessions = parse_u64(&argument, &value)?,
                "--max-rss-growth-kib" => {
                    settings.max_rss_growth_kib = parse_u64(&argument, &value)?
                }
                "--seed" => settings.seed = parse_u64(&argument, &value)?,
                "--result-file" => settings.result_file = Some(PathBuf::from(value)),
                _ => return Err(io::Error::other(format!("unknown argument: {argument}")).into()),
            }
        }
        Ok(settings)
    }
}

fn parse_u64(argument: &str, value: &str) -> ExampleResult<u64> {
    value.parse().map_err(|error| {
        io::Error::other(format!("invalid unsigned integer for {argument}: {error}")).into()
    })
}

fn write_status(path: Option<&Path>, contents: &str) -> ExampleResult<()> {
    if let Some(path) = path {
        std::fs::write(path, contents)?;
    }
    Ok(())
}

fn run_session(random: &mut Random) -> ExampleResult<()> {
    let order = ORDERS[(random.next_u64() % ORDERS.len() as u64) as usize];
    for scenario in order {
        match scenario {
            0 => verify_address_book()?,
            1 => verify_calculator()?,
            2 => verify_platform()?,
            _ => return Err(io::Error::other("invalid scenario order").into()),
        }
    }
    Ok(())
}

fn verify_address_book() -> ExampleResult<()> {
    let result = address_book_example::run()?;
    let expected = [
        "123|Alice|alice@example.com|555-1212:Mobile|school=MIT",
        "456|Bob|bob@example.com|555-4567:Home,555-7654:Work|unemployed",
    ];
    if result.standard_summary != expected || result.packed_summary != expected {
        return Err(io::Error::other("address-book persistence changed during soak").into());
    }
    if result.packed.len() >= result.standard.len() {
        return Err(io::Error::other("packed address book was not smaller than standard").into());
    }
    Ok(())
}

fn verify_calculator() -> ExampleResult<()> {
    let result = calculator_example::run()?;
    if result.operator_result != 42.0
        || result.callback_result != 42.0
        || result.defined_function_result != 42.0
        || result.concurrent_results != [11.0, 31.0]
        || result.callback_calls != 2
    {
        return Err(io::Error::other("calculator capability result changed during soak").into());
    }
    Ok(())
}

fn verify_platform() -> ExampleResult<()> {
    let result = platform_example::run()?;
    if result.streamed_bytes != b"ordered stream discarded"
        || result.clean_ends != 1
        || result.cancellations != 1
        || !result.direct_handoff
        || !result.distributed_equality
        || result.restored_object != 7
        || result.original_connection != 44
        || result.restored_connection != 900
    {
        return Err(io::Error::other("platform lifecycle result changed during soak").into());
    }
    Ok(())
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
