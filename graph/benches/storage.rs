//! Benchmarks for graph storage operations.

use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use grafeo_common::types::Value;
use grafeo_core::graph::Direction;
use grafeo_core::graph::traits::{GraphStore, GraphStoreMut};

use common::StorageConfig;
use graph::db::GraphDb;
use graph::{Config, GraphStorage};

fn setup_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn setup_db(rt: &tokio::runtime::Runtime) -> Arc<GraphDb> {
    let config = Config {
        storage: StorageConfig::InMemory,
        ..Default::default()
    };
    rt.block_on(async { Arc::new(GraphDb::open_with_config(&config).await.unwrap()) })
}

fn store(db: &GraphDb) -> &GraphStorage {
    db.store()
}

fn bench_create_node(c: &mut Criterion) {
    let rt = setup_rt();
    let db = setup_db(&rt);
    let s = store(&db);

    c.bench_function("create_node_with_label", |b| {
        b.iter(|| {
            s.create_node(&["Person"]);
        });
    });
}

fn bench_create_node_with_props(c: &mut Criterion) {
    let rt = setup_rt();
    let db = setup_db(&rt);
    let s = store(&db);

    c.bench_function("create_node_with_props", |b| {
        let mut i = 0i64;
        b.iter(|| {
            let id = s.create_node(&["Person"]);
            s.set_node_property(id, "name", Value::String("Alice".into()));
            s.set_node_property(id, "age", Value::Int64(i));
            i += 1;
        });
    });
}

fn bench_create_edge(c: &mut Criterion) {
    let rt = setup_rt();
    let db = setup_db(&rt);
    let s = store(&db);

    let src = s.create_node(&["Person"]);
    let dst = s.create_node(&["Person"]);

    c.bench_function("create_edge", |b| {
        b.iter(|| {
            s.create_edge(src, dst, "KNOWS");
        });
    });
}

fn bench_get_node(c: &mut Criterion) {
    let rt = setup_rt();
    let db = setup_db(&rt);
    let s = store(&db);

    let id = s.create_node(&["Person"]);
    s.set_node_property(id, "name", Value::String("Alice".into()));
    s.set_node_property(id, "age", Value::Int64(30));

    c.bench_function("get_node", |b| {
        b.iter(|| {
            s.get_node(id);
        });
    });
}

fn bench_nodes_by_label(c: &mut Criterion) {
    let rt = setup_rt();
    let db = setup_db(&rt);
    let s = store(&db);

    for _ in 0..100 {
        s.create_node(&["Person"]);
    }

    c.bench_function("nodes_by_label_100", |b| {
        b.iter(|| {
            s.nodes_by_label("Person");
        });
    });
}

fn bench_find_by_property(c: &mut Criterion) {
    let rt = setup_rt();
    let db = setup_db(&rt);
    let s = store(&db);

    for i in 0..100 {
        let id = s.create_node(&["Person"]);
        s.set_node_property(id, "score", Value::Int64(i));
    }

    c.bench_function("find_by_property_100", |b| {
        b.iter(|| {
            s.find_nodes_by_property("score", &Value::Int64(50));
        });
    });
}

fn bench_neighbors(c: &mut Criterion) {
    let rt = setup_rt();
    let db = setup_db(&rt);
    let s = store(&db);

    let center = s.create_node(&["Hub"]);
    for _ in 0..100 {
        let n = s.create_node(&["Spoke"]);
        s.create_edge(center, n, "CONNECTS");
    }

    c.bench_function("neighbors_outgoing_100", |b| {
        b.iter(|| {
            s.neighbors(center, Direction::Outgoing);
        });
    });
}

criterion_group!(
    benches,
    bench_create_node,
    bench_create_node_with_props,
    bench_create_edge,
    bench_get_node,
    bench_nodes_by_label,
    bench_find_by_property,
    bench_neighbors,
);
criterion_main!(benches);
