use arbor_authorizer::engine::AuthorizerEngine;
use arbor_bench::{build_scenario, BenchFixtures};
use arbor_index_snapshot::PolicySide;
use criterion::{BenchmarkGroup, BenchmarkId, Criterion, criterion_group, criterion_main, measurement::WallTime};

// ---------------------------------------------------------------------------
// Scale points
// ---------------------------------------------------------------------------

const SCALES: &[usize] = &[100_000, 1_000_000, 2_000_000];

// ---------------------------------------------------------------------------
// check() benchmark
// ---------------------------------------------------------------------------

fn bench_check(c: &mut Criterion) {
    let mut group = c.benchmark_group("check");

    for &n in SCALES {
        let (snapshot, fixtures) = build_scenario(n);
        let engine = AuthorizerEngine::from_snapshot(snapshot);

        bench_check_permitted(&mut group, n, &engine, &fixtures);
        bench_check_denied(&mut group, n, &engine, &fixtures);
    }

    group.finish();
}

fn bench_check_permitted(
    group: &mut BenchmarkGroup<WallTime>,
    n: usize,
    engine: &AuthorizerEngine,
    fixtures: &BenchFixtures,
) {
    group.bench_with_input(
        BenchmarkId::new("permitted", n),
        &n,
        |b, _| {
            b.iter(|| {
                engine
                    .check(
                        fixtures.permitted_principal,
                        fixtures.action,
                        fixtures.resource,
                    )
                    .expect("check failed")
            });
        },
    );
}

fn bench_check_denied(
    group: &mut BenchmarkGroup<WallTime>,
    n: usize,
    engine: &AuthorizerEngine,
    fixtures: &BenchFixtures,
) {
    group.bench_with_input(
        BenchmarkId::new("denied", n),
        &n,
        |b, _| {
            b.iter(|| {
                engine
                    .check(
                        fixtures.denied_principal,
                        fixtures.action,
                        fixtures.resource,
                    )
                    .expect("check failed")
            });
        },
    );
}

// ---------------------------------------------------------------------------
// list_entities() benchmark
// ---------------------------------------------------------------------------

fn bench_list_entities(c: &mut Criterion) {
    let mut group = c.benchmark_group("list_entities");

    for &n in SCALES {
        let (snapshot, fixtures) = build_scenario(n);
        let engine = AuthorizerEngine::from_snapshot(snapshot);

        bench_list_resources(&mut group, n, &engine, &fixtures);
        bench_list_principals(&mut group, n, &engine, &fixtures);
    }

    group.finish();
}

/// Principal fixed, enumerate permitted resources of type File.
fn bench_list_resources(
    group: &mut BenchmarkGroup<WallTime>,
    n: usize,
    engine: &AuthorizerEngine,
    fixtures: &BenchFixtures,
) {
    group.bench_with_input(
        BenchmarkId::new("list_resources", n),
        &n,
        |b, _| {
            b.iter(|| {
                engine
                    .list_entities(
                        fixtures.permitted_principal,
                        fixtures.action,
                        fixtures.file_type,
                        PolicySide::Resource,
                    )
                    .expect("list_entities failed")
            });
        },
    );
}

/// Resource fixed, enumerate principals that can access it.
fn bench_list_principals(
    group: &mut BenchmarkGroup<WallTime>,
    n: usize,
    engine: &AuthorizerEngine,
    fixtures: &BenchFixtures,
) {
    group.bench_with_input(
        BenchmarkId::new("list_principals", n),
        &n,
        |b, _| {
            b.iter(|| {
                engine
                    .list_entities(
                        fixtures.resource,
                        fixtures.action,
                        fixtures.file_type,
                        PolicySide::Principal,
                    )
                    .expect("list_entities failed")
            });
        },
    );
}

// ---------------------------------------------------------------------------
// Criterion entry points
// ---------------------------------------------------------------------------

criterion_group!(benches, bench_check, bench_list_entities);
criterion_main!(benches);
