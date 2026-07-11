use std::collections::HashMap;
use std::env;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use regex::Regex;
use serde_json::json;
use yq::v1::eval::{Context, VariableProvider};
use yq::v1::expr::{Atom, Cons, Expression};

#[derive(Clone)]
struct FixtureStatus {
    text: String,
    visibility: &'static str,
}

struct Provider(FixtureStatus);

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
    let mut matches = 0;
    for _ in 0..15 {
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
    }

    let compile_p50 = percentile(&mut compile_samples, 0.50);
    let compile_p95 = percentile(&mut compile_samples, 0.95);
    let evaluation_p50 = percentile(&mut evaluation_samples, 0.50);
    let evaluation_p95 = percentile(&mut evaluation_samples, 0.95);
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
        },
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
    if compile_p95 > 10.0 || evaluation_p95 > 400.0 {
        std::process::exit(1);
    }
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
