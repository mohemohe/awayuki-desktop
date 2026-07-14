use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

const TASKS: usize = 16;
const QUERIES_PER_TASK: usize = 40;
const RUNTIME_READER_CONNECTION_CAP: u32 = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReaderSample {
    connections: u32,
    acquire_p95_ms: f64,
    query_p95_ms: f64,
    throughput_queries_per_second: f64,
    peak_rss_delta_bytes: u64,
    queries: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReaderReport {
    schema_version: u32,
    fixture_id: String,
    cpu_parallelism: u32,
    runtime_connection_cap: u32,
    selected_connection_cap: u32,
    samples: Vec<ReaderSample>,
}

#[tokio::main]
async fn main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|argument| argument == "--child")
    {
        let database = PathBuf::from(arguments.get(1).expect("child database path"));
        let connections = arguments
            .get(2)
            .expect("child connection count")
            .parse::<u32>()
            .expect("numeric connection count");
        let sample = measure(&database, connections)
            .await
            .expect("measure reader pool");
        println!(
            "{}",
            serde_json::to_string(&sample).expect("serialize sample")
        );
        return;
    }

    let database = PathBuf::from(
        arguments
            .first()
            .map(String::as_str)
            .unwrap_or("build/benchmark-420000.db"),
    );
    let output = PathBuf::from(
        arguments
            .get(1)
            .map(String::as_str)
            .unwrap_or("build/reader-pool-benchmark.json"),
    );
    let cpu_parallelism = std::thread::available_parallelism()
        .map(|count| count.get() as u32)
        .unwrap_or(4);
    let connection_counts = BTreeSet::from([2, 4, cpu_parallelism]);
    let executable = std::env::current_exe().expect("benchmark executable path");
    let mut samples = Vec::new();
    for connections in connection_counts {
        let result = Command::new(&executable)
            .arg("--child")
            .arg(&database)
            .arg(connections.to_string())
            .output()
            .expect("run isolated reader benchmark");
        if !result.status.success() {
            panic!(
                "reader benchmark child failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        samples.push(
            serde_json::from_slice::<ReaderSample>(&result.stdout)
                .expect("parse child reader benchmark"),
        );
    }
    let selected_connection_cap = select_connection_cap(&samples);
    let report = ReaderReport {
        schema_version: 1,
        fixture_id: database
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("reader-pool")
            .to_string(),
        cpu_parallelism,
        runtime_connection_cap: RUNTIME_READER_CONNECTION_CAP,
        selected_connection_cap,
        samples,
    };
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).expect("create benchmark output directory");
    }
    let json = serde_json::to_string_pretty(&report).expect("serialize reader report");
    std::fs::write(&output, format!("{json}\n")).expect("write reader report");
    println!("{json}");
}

async fn measure(database: &Path, connections: u32) -> Result<ReaderSample, sqlx::Error> {
    let rss_before = peak_rss_bytes();
    let options = SqliteConnectOptions::new()
        .filename(database)
        .read_only(true)
        .busy_timeout(std::time::Duration::from_secs(5))
        .pragma("foreign_keys", "ON");
    let pool = SqlitePoolOptions::new()
        .max_connections(connections)
        .connect_with(options)
        .await?;
    let started_at = Instant::now();
    let mut tasks = Vec::with_capacity(TASKS);
    for task_index in 0..TASKS {
        let pool = pool.clone();
        tasks.push(tokio::spawn(async move {
            let mut acquire_samples = Vec::with_capacity(QUERIES_PER_TASK);
            let mut query_samples = Vec::with_capacity(QUERIES_PER_TASK);
            for query_index in 0..QUERIES_PER_TASK {
                let acquire_started = Instant::now();
                let mut connection = pool.acquire().await?;
                acquire_samples.push(acquire_started.elapsed().as_secs_f64() * 1_000.0);
                let query_started = Instant::now();
                let offset = ((task_index * QUERIES_PER_TASK + query_index) * 17 % 10_000) as i64;
                let _: Vec<String> = sqlx::query_scalar(
                    "SELECT id FROM statuses ORDER BY created_at DESC, id DESC LIMIT 40 OFFSET ?",
                )
                .bind(offset)
                .fetch_all(&mut *connection)
                .await?;
                query_samples.push(query_started.elapsed().as_secs_f64() * 1_000.0);
            }
            Ok::<_, sqlx::Error>((acquire_samples, query_samples))
        }));
    }
    let mut acquire_samples = Vec::new();
    let mut query_samples = Vec::new();
    for task in tasks {
        let (acquire, query) = task.await.expect("reader task join")?;
        acquire_samples.extend(acquire);
        query_samples.extend(query);
    }
    let duration = started_at.elapsed().as_secs_f64();
    pool.close().await;
    let queries = acquire_samples.len();
    Ok(ReaderSample {
        connections,
        acquire_p95_ms: percentile(&mut acquire_samples, 0.95),
        query_p95_ms: percentile(&mut query_samples, 0.95),
        throughput_queries_per_second: queries as f64 / duration.max(f64::EPSILON),
        peak_rss_delta_bytes: peak_rss_bytes().saturating_sub(rss_before),
        queries,
    })
}

fn select_connection_cap(samples: &[ReaderSample]) -> u32 {
    let best_query_p95 = samples
        .iter()
        .map(|sample| sample.query_p95_ms)
        .fold(f64::INFINITY, f64::min);
    samples
        .iter()
        .filter(|sample| sample.query_p95_ms <= best_query_p95 * 1.10 + 0.05)
        .min_by(|left, right| {
            left.peak_rss_delta_bytes
                .cmp(&right.peak_rss_delta_bytes)
                .then_with(|| left.connections.cmp(&right.connections))
        })
        .map(|sample| sample.connections)
        .unwrap_or(4)
}

fn percentile(samples: &mut [f64], ratio: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    let index = (samples.len() as f64 * ratio).ceil() as usize;
    samples[index.saturating_sub(1).min(samples.len().saturating_sub(1))]
}

#[cfg(unix)]
fn peak_rss_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `getrusage` initializes the provided `rusage` on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return 0;
    }
    // SAFETY: the successful call above initialized the value.
    let max_rss = unsafe { usage.assume_init() }.ru_maxrss.max(0) as u64;
    if cfg!(target_os = "macos") {
        max_rss
    } else {
        max_rss.saturating_mul(1024)
    }
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> u64 {
    0
}
