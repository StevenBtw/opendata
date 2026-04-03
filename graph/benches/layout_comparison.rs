//! Benchmarks comparing Individual vs Merged storage layouts across backends.
//!
//! Tests property operations, adjacency traversals, and mixed workloads for each
//! (layout x backend) combination: Individual/InMemory, Individual/SlateDB,
//! Merged/InMemory, Merged/SlateDB.

use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use grafeo_common::types::Value;
use grafeo_core::graph::Direction;
use grafeo_core::graph::traits::{GraphStore, GraphStoreMut};

use common::StorageConfig;
use common::storage::config::{ObjectStoreConfig, SlateDbStorageConfig};
use graph::db::GraphDb;
use graph::{Config, GraphStorage, StorageLayout};

fn setup_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn setup_db(rt: &tokio::runtime::Runtime, layout: StorageLayout, use_slatedb: bool) -> Arc<GraphDb> {
    let storage = if use_slatedb {
        StorageConfig::SlateDb(SlateDbStorageConfig {
            path: format!(
                "bench-{:?}-{}",
                layout,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ),
            object_store: ObjectStoreConfig::InMemory,
            settings_path: None,
            block_cache: None,
        })
    } else {
        StorageConfig::InMemory
    };
    let config = Config {
        storage,
        storage_layout: layout,
        ..Default::default()
    };
    rt.block_on(async { Arc::new(GraphDb::open_with_config(&config).await.unwrap()) })
}

fn store(db: &GraphDb) -> &GraphStorage {
    db.store()
}

fn set_10_props(s: &GraphStorage, id: grafeo_common::types::NodeId, base: i64) {
    s.set_node_property(id, "name", Value::String("Alice".into()));
    s.set_node_property(id, "age", Value::Int64(30 + base));
    s.set_node_property(id, "email", Value::String("alice@example.com".into()));
    s.set_node_property(id, "score", Value::Float64(95.5));
    s.set_node_property(id, "active", Value::Bool(true));
    s.set_node_property(id, "city", Value::String("Amsterdam".into()));
    s.set_node_property(id, "country", Value::String("NL".into()));
    s.set_node_property(id, "level", Value::Int64(5));
    s.set_node_property(id, "balance", Value::Float64(1234.56));
    s.set_node_property(id, "verified", Value::Bool(false));
}

/// All (name, layout, use_slatedb) combinations.
fn variants() -> Vec<(&'static str, StorageLayout, bool)> {
    vec![
        ("individual/inmemory", StorageLayout::Individual, false),
        ("individual/slatedb", StorageLayout::Individual, true),
        ("merged/inmemory", StorageLayout::Merged, false),
        ("merged/slatedb", StorageLayout::Merged, true),
    ]
}

// ---------------------------------------------------------------------------
// Property operations
// ---------------------------------------------------------------------------

fn bench_create_node_10_props(c: &mut Criterion) {
    let rt = setup_rt();
    let mut group = c.benchmark_group("create_node_10_props");

    for (name, layout, slatedb) in variants() {
        let db = setup_db(&rt, layout, slatedb);
        let s = store(&db);
        let mut i = 0i64;
        group.bench_function(BenchmarkId::new("layout", name), |b| {
            b.iter(|| {
                let id = s.create_node(&["Person"]);
                set_10_props(s, id, i);
                i += 1;
            });
        });
    }
    group.finish();
}

fn bench_get_node_10_props(c: &mut Criterion) {
    let rt = setup_rt();
    let mut group = c.benchmark_group("get_node_10_props");

    for (name, layout, slatedb) in variants() {
        let db = setup_db(&rt, layout, slatedb);
        let s = store(&db);
        let id = s.create_node(&["Person"]);
        set_10_props(s, id, 0);

        group.bench_function(BenchmarkId::new("layout", name), |b| {
            b.iter(|| {
                s.get_node(id);
            });
        });
    }
    group.finish();
}

fn bench_update_single_property(c: &mut Criterion) {
    let rt = setup_rt();
    let mut group = c.benchmark_group("update_single_property");

    for (name, layout, slatedb) in variants() {
        let db = setup_db(&rt, layout, slatedb);
        let s = store(&db);
        let id = s.create_node(&["Person"]);
        set_10_props(s, id, 0);

        let mut counter = 0i64;
        group.bench_function(BenchmarkId::new("layout", name), |b| {
            b.iter(|| {
                s.set_node_property(id, "score", Value::Float64(counter as f64));
                counter += 1;
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Adjacency operations
// ---------------------------------------------------------------------------

fn bench_create_100_edges(c: &mut Criterion) {
    let rt = setup_rt();
    let mut group = c.benchmark_group("create_100_edges");

    for (name, layout, slatedb) in variants() {
        group.bench_function(BenchmarkId::new("layout", name), |b| {
            b.iter(|| {
                let db = setup_db(&rt, layout, slatedb);
                let s = store(&db);
                let hub = s.create_node(&["Hub"]);
                for _ in 0..100 {
                    let target = s.create_node(&["Target"]);
                    s.create_edge(hub, target, "CONNECTS");
                }
            });
        });
    }
    group.finish();
}

fn bench_neighbors_outgoing_100(c: &mut Criterion) {
    let rt = setup_rt();
    let mut group = c.benchmark_group("neighbors_outgoing_100");

    for (name, layout, slatedb) in variants() {
        let db = setup_db(&rt, layout, slatedb);
        let s = store(&db);
        let hub = s.create_node(&["Hub"]);
        for _ in 0..100 {
            let n = s.create_node(&["Spoke"]);
            s.create_edge(hub, n, "CONNECTS");
        }

        group.bench_function(BenchmarkId::new("layout", name), |b| {
            b.iter(|| {
                s.neighbors(hub, Direction::Outgoing);
            });
        });
    }
    group.finish();
}

fn bench_neighbors_outgoing_1000(c: &mut Criterion) {
    let rt = setup_rt();
    let mut group = c.benchmark_group("neighbors_outgoing_1000");

    for (name, layout, slatedb) in variants() {
        let db = setup_db(&rt, layout, slatedb);
        let s = store(&db);
        let hub = s.create_node(&["Hub"]);
        for _ in 0..1000 {
            let n = s.create_node(&["Spoke"]);
            s.create_edge(hub, n, "CONNECTS");
        }

        group.bench_function(BenchmarkId::new("layout", name), |b| {
            b.iter(|| {
                s.neighbors(hub, Direction::Outgoing);
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Mixed workload
// ---------------------------------------------------------------------------

fn bench_bulk_insert_100_nodes_with_edges(c: &mut Criterion) {
    let rt = setup_rt();
    let mut group = c.benchmark_group("bulk_insert_100_nodes_with_edges");
    group.sample_size(10);

    for (name, layout, slatedb) in variants() {
        group.bench_function(BenchmarkId::new("layout", name), |b| {
            let mut base = 0i64;
            b.iter(|| {
                let db = setup_db(&rt, layout, slatedb);
                let s = store(&db);

                let mut nodes = Vec::with_capacity(100);
                for i in 0..100 {
                    let id = s.create_node(&["Person"]);
                    s.set_node_property(id, "name", Value::String(format!("user_{}", base + i).into()));
                    s.set_node_property(id, "age", Value::Int64(20 + (base + i) % 60));
                    s.set_node_property(id, "score", Value::Float64((base + i) as f64 * 0.1));
                    s.set_node_property(id, "active", Value::Bool(i % 2 == 0));
                    s.set_node_property(id, "city", Value::String("Amsterdam".into()));
                    nodes.push(id);
                }

                // Create 3 edges per node
                for i in 0..100 {
                    for j in 1..=3 {
                        let dst_idx = (i + j) % 100;
                        s.create_edge(nodes[i], nodes[dst_idx], "KNOWS");
                    }
                }

                base += 100;
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_create_node_10_props,
    bench_get_node_10_props,
    bench_update_single_property,
    bench_create_100_edges,
    bench_neighbors_outgoing_100,
    bench_neighbors_outgoing_1000,
    bench_bulk_insert_100_nodes_with_edges,
);
criterion_main!(benches);
