use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use regex::Regex;
use serde::Serialize;
use serde_json::json;
use yq::v1::eval::{Context, VariableProvider};
use yq::v1::expr::{Atom, Cons, Expression};

struct CountingAllocator;

static ALLOCATION_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

// SAFETY: this allocator delegates every operation to the process System
// allocator and only records monotonic counters around successful requests.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: forwarding the exact layout received from GlobalAlloc.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: forwarding the pointer/layout pair supplied by GlobalAlloc.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Clone)]
struct FixtureStatus {
    text: String,
    visibility: &'static str,
}

struct Provider(FixtureStatus);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PageScenario {
    query: &'static str,
    matches: usize,
    scanned_rows: usize,
    page_p95_ms: f64,
    allocations_p50: u64,
    allocated_bytes_p50: u64,
}

impl VariableProvider for Provider {
    fn get(&self, symbol: &str) -> Option<Expression> {
        match symbol {
            "text" | "content" => Some(Expression::Atom(Atom::String(self.0.text.clone()))),
            "visibility" => Some(Expression::Atom(Atom::String(
                self.0.visibility.to_string(),
            ))),
            _ => None,
        }
    }
}

fn main() {
    let output = env::args()
        .nth(1)
        .unwrap_or_else(|| "build/yq-benchmark.json".to_string());
    let query = "where (and (= visibility \"public\") (regex text \"benchmark needle\"))";
    let fixtures = (0..10_000)
        .map(|index| FixtureStatus {
            text: if index % 97 == 0 {
                format!("benchmark needle {index}")
            } else {
                format!("ordinary timeline text {index}")
            },
            visibility: if index % 11 == 0 { "private" } else { "public" },
        })
        .collect::<Vec<_>>();

    let mut compile_samples = Vec::new();
    for _ in 0..50 {
        let started = Instant::now();
        let parsed = yq::v1::parser::parse(query).expect("parse fixed YQ fixture");
        std::hint::black_box(parsed.expression());
        compile_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
    }

    let parsed = yq::v1::parser::parse(query).expect("parse fixed YQ fixture");
    let expression = parsed.expression().clone();
    let mut evaluation_samples = Vec::new();
    let mut evaluation_allocation_samples = Vec::new();
    let mut evaluation_byte_samples = Vec::new();
    let mut matches = 0;
    for _ in 0..15 {
        let allocation_start = ALLOCATION_COUNT.load(Ordering::Relaxed);
        let bytes_start = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let mut context = Context::new();
        register_regex(&mut context);
        let started = Instant::now();
        matches = 0;
        for fixture in &fixtures {
            context.set_variable_provider(Box::new(Provider(fixture.clone())));
            if context
                .evaluate(&expression)
                .map(|value| !value.is_nil())
                .unwrap_or(false)
            {
                matches += 1;
            }
        }
        evaluation_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        evaluation_allocation_samples.push(
            ALLOCATION_COUNT
                .load(Ordering::Relaxed)
                .saturating_sub(allocation_start),
        );
        evaluation_byte_samples.push(
            ALLOCATED_BYTES
                .load(Ordering::Relaxed)
                .saturating_sub(bytes_start),
        );
    }

    let baseline_allocation_start = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let baseline_bytes_start = ALLOCATED_BYTES.load(Ordering::Relaxed);
    let baseline_started = Instant::now();
    let mut baseline_matches = 0;
    for fixture in &fixtures {
        let mut context = Context::new();
        register_regex(&mut context);
        context.set_variable_provider(Box::new(Provider(fixture.clone())));
        if context
            .evaluate(&expression)
            .map(|value| !value.is_nil())
            .unwrap_or(false)
        {
            baseline_matches += 1;
        }
    }
    let baseline_duration_ms = baseline_started.elapsed().as_secs_f64() * 1_000.0;
    let baseline_allocations = ALLOCATION_COUNT
        .load(Ordering::Relaxed)
        .saturating_sub(baseline_allocation_start);
    let baseline_bytes = ALLOCATED_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(baseline_bytes_start);

    let mut page_scenarios = vec![
        benchmark_page_scenario(
            &fixtures,
            "where (and (= visibility \"public\") (regex text \"benchmark needle\"))",
        ),
        benchmark_page_scenario(&fixtures, "where (regex text \"timeline\")"),
    ];
    page_scenarios.sort_by_key(|scenario| scenario.scanned_rows);
    let low_selectivity = &page_scenarios[0];
    let high_selectivity = &page_scenarios[1];

    let compile_p50 = percentile(&mut compile_samples, 0.50);
    let compile_p95 = percentile(&mut compile_samples, 0.95);
    let evaluation_p50 = percentile(&mut evaluation_samples, 0.50);
    let evaluation_p95 = percentile(&mut evaluation_samples, 0.95);
    evaluation_allocation_samples.sort_unstable();
    evaluation_byte_samples.sort_unstable();
    let evaluation_allocations_p50 =
        evaluation_allocation_samples[evaluation_allocation_samples.len() / 2];
    let evaluation_bytes_p50 = evaluation_byte_samples[evaluation_byte_samples.len() / 2];
    assert_eq!(matches, baseline_matches);
    let report = json!({
        "schemaVersion": 1,
        "fixtureId": "awayuki-yq-v1-10000",
        "environment": {
            "platform": env::consts::OS,
            "arch": env::consts::ARCH,
            "runtime": format!("rust {}", env!("CARGO_PKG_VERSION")),
        },
        "dataset": {
            "statuses": fixtures.len(),
            "synthetic": true,
            "seed": "awayuki-yq-v1",
            "query": query,
            "matches": matches,
        },
        "metrics": {
            "yq.compileP50Ms": metric(compile_p50, 5.0, "enforce", 0.25),
            "yq.compileP95Ms": metric(compile_p95, 10.0, "enforce", 0.25),
            "yq.evaluate10kP50Ms": metric(evaluation_p50, 250.0, "enforce", 5.0),
            "yq.evaluate10kP95Ms": metric(evaluation_p95, 400.0, "enforce", 5.0),
            "yq.evaluate10kAllocations": count_metric(
                evaluation_allocations_p50,
                baseline_allocations,
                "allocations"
            ),
            "yq.evaluate10kAllocatedBytes": count_metric(
                evaluation_bytes_p50,
                baseline_bytes,
                "bytes"
            ),
            "yq.lowSelectivityPageP95Ms": metric(
                low_selectivity.page_p95_ms,
                50.0,
                "enforce",
                1.0
            ),
            "yq.highSelectivityPageP95Ms": metric(
                high_selectivity.page_p95_ms,
                50.0,
                "enforce",
                1.0
            ),
        },
        "comparison": {
            "perStatusContextBaseline": {
                "durationMs": baseline_duration_ms,
                "allocations": baseline_allocations,
                "allocatedBytes": baseline_bytes,
            },
            "querySessionReuse": {
                "allocationsP50": evaluation_allocations_p50,
                "allocatedBytesP50": evaluation_bytes_p50,
            },
            "pageScenarios": &page_scenarios,
        }
    });
    if let Some(parent) = std::path::Path::new(&output).parent() {
        fs::create_dir_all(parent).expect("create benchmark output directory");
    }
    fs::write(
        &output,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&report).expect("serialize metrics")
        ),
    )
    .expect("write YQ benchmark metrics");
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("serialize metrics")
    );
    if compile_p95 > 10.0
        || evaluation_p95 > 400.0
        || evaluation_allocations_p50 >= baseline_allocations
        || evaluation_bytes_p50 >= baseline_bytes
        || low_selectivity.page_p95_ms > 50.0
        || high_selectivity.page_p95_ms > 50.0
    {
        std::process::exit(1);
    }
}

fn benchmark_page_scenario(fixtures: &[FixtureStatus], query: &'static str) -> PageScenario {
    let parsed = yq::v1::parser::parse(query).expect("parse page scenario");
    let expression = parsed.expression().clone();
    let mut durations = Vec::new();
    let mut allocation_samples = Vec::new();
    let mut byte_samples = Vec::new();
    let mut final_matches = 0;
    let mut final_scanned = 0;
    for _ in 0..15 {
        let allocation_start = ALLOCATION_COUNT.load(Ordering::Relaxed);
        let bytes_start = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let mut context = Context::new();
        register_regex(&mut context);
        let started = Instant::now();
        let mut matches = 0;
        let mut scanned = 0;
        for fixture in fixtures {
            scanned += 1;
            context.set_variable_provider(Box::new(Provider(fixture.clone())));
            if context
                .evaluate(&expression)
                .map(|value| !value.is_nil())
                .unwrap_or(false)
            {
                matches += 1;
                if matches == 40 {
                    break;
                }
            }
        }
        durations.push(started.elapsed().as_secs_f64() * 1_000.0);
        allocation_samples.push(
            ALLOCATION_COUNT
                .load(Ordering::Relaxed)
                .saturating_sub(allocation_start),
        );
        byte_samples.push(
            ALLOCATED_BYTES
                .load(Ordering::Relaxed)
                .saturating_sub(bytes_start),
        );
        final_matches = matches;
        final_scanned = scanned;
    }
    allocation_samples.sort_unstable();
    byte_samples.sort_unstable();
    PageScenario {
        query,
        matches: final_matches,
        scanned_rows: final_scanned,
        page_p95_ms: percentile(&mut durations, 0.95),
        allocations_p50: allocation_samples[allocation_samples.len() / 2],
        allocated_bytes_p50: byte_samples[byte_samples.len() / 2],
    }
}

fn count_metric(value: u64, maximum: u64, unit: &str) -> serde_json::Value {
    json!({
        "value": value,
        "unit": unit,
        "absolute": { "max": maximum, "passed": value < maximum },
        "regression": {
            "mode": "enforce",
            "direction": "lower",
            "maxRatio": 1.5,
            "noiseFloor": 1,
        },
    })
}

fn register_regex(context: &mut Context) {
    let cache = Arc::new(Mutex::new(HashMap::<String, Option<Regex>>::new()));
    context.register_function("regex", move |context, _symbol, cdr| {
        let mut arguments = cdr.iter();
        let haystack = context.evaluate(arguments.next().ok_or_else(wrong_number_of_arguments)?)?;
        let pattern = context.evaluate(arguments.next().ok_or_else(wrong_number_of_arguments)?)?;
        match (haystack, pattern) {
            (
                Expression::Atom(Atom::String(haystack)) | Expression::Atom(Atom::Symbol(haystack)),
                Expression::Atom(Atom::String(pattern)) | Expression::Atom(Atom::Symbol(pattern)),
            ) => {
                let Ok(mut regexes) = cache.lock() else {
                    return Ok(Expression::nil());
                };
                let regex = regexes
                    .entry(pattern.clone())
                    .or_insert_with(|| Regex::new(&pattern).ok());
                Ok(
                    if regex
                        .as_ref()
                        .is_some_and(|value| value.is_match(&haystack))
                    {
                        Expression::t()
                    } else {
                        Expression::nil()
                    },
                )
            }
            _ => Ok(Expression::nil()),
        }
    });
}

fn wrong_number_of_arguments() -> Expression {
    Expression::Cons(Cons::from(
        Atom::symbol("wrong-number-of-arguments").into(),
        Expression::nil(),
    ))
}

fn percentile(samples: &mut [f64], percentile: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    let index = ((samples.len() as f64 * percentile).ceil() as usize)
        .saturating_sub(1)
        .min(samples.len().saturating_sub(1));
    (samples[index] * 1_000.0).round() / 1_000.0
}

fn metric(value: f64, maximum: f64, mode: &str, noise_floor: f64) -> serde_json::Value {
    json!({
        "value": value,
        "unit": "ms",
        "absolute": { "max": maximum, "passed": value <= maximum },
        "regression": {
            "mode": mode,
            "direction": "lower",
            "maxRatio": 1.5,
            "noiseFloor": noise_floor,
        },
    })
}
