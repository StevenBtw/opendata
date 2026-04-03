//! Integration tests for the `Merged` storage layout.
//!
//! These tests mirror the core scenarios from `storage.rs` but run against
//! `StorageLayout::Merged`, which uses merge operands for properties and
//! adjacency instead of individual KV rows.

use std::sync::Arc;

use common::StorageConfig;
use grafeo_common::types::{PropertyKey, Value};
use grafeo_core::graph::Direction;
use grafeo_core::graph::traits::{GraphStore, GraphStoreMut};
use graph::db::GraphDb;
use graph::{Config, GraphStorage, StorageLayout};

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

async fn setup() -> Arc<GraphDb> {
    let config = Config {
        storage: StorageConfig::InMemory,
        storage_layout: StorageLayout::Merged,
        ..Default::default()
    };
    Arc::new(GraphDb::open_with_config(&config).await.unwrap())
}

fn store(db: &GraphDb) -> &GraphStorage {
    db.store()
}

// ═══════════════════════════════════════════════════════════════════════
// Node lifecycle
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn merged_node_create_and_get() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);
    let node = s.get_node(id).expect("created node should be retrievable");
    assert_eq!(node.id, id);
    assert!(node.labels.iter().any(|l| l.as_str() == "Person"));
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_node_delete() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);
    assert!(s.delete_node(id));
    assert!(s.get_node(id).is_none());
    assert!(!s.delete_node(id), "second delete returns false");
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_node_count() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&["A"]);
    let _b = s.create_node(&["B"]);
    assert_eq!(s.node_count(), 2);

    s.delete_node(a);
    assert_eq!(s.node_count(), 1);
}

// ═══════════════════════════════════════════════════════════════════════
// Properties (via merge operands)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn merged_node_property_set_get() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);
    s.set_node_property(id, "name", Value::String("Alice".into()));
    s.set_node_property(id, "age", Value::Int64(30));

    assert_eq!(
        s.get_node_property(id, &PropertyKey::new("name")),
        Some(Value::String("Alice".into()))
    );
    assert_eq!(
        s.get_node_property(id, &PropertyKey::new("age")),
        Some(Value::Int64(30))
    );
    assert_eq!(
        s.get_node_property(id, &PropertyKey::new("nonexistent")),
        None
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_node_property_overwrite() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&[]);
    s.set_node_property(id, "score", Value::Int64(10));
    s.set_node_property(id, "score", Value::Int64(20));

    assert_eq!(
        s.get_node_property(id, &PropertyKey::new("score")),
        Some(Value::Int64(20))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_node_property_remove() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&[]);
    s.set_node_property(id, "temp", Value::Bool(true));
    let removed = s.remove_node_property(id, "temp");
    assert_eq!(removed, Some(Value::Bool(true)));
    assert_eq!(
        s.get_node_property(id, &PropertyKey::new("temp")),
        None
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_node_get_includes_properties() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);
    s.set_node_property(id, "name", Value::String("Bob".into()));
    s.set_node_property(id, "age", Value::Int64(25));

    let node = s.get_node(id).unwrap();
    assert_eq!(
        node.properties.get(&PropertyKey::new("name")),
        Some(&Value::String("Bob".into()))
    );
    assert_eq!(
        node.properties.get(&PropertyKey::new("age")),
        Some(&Value::Int64(25))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_node_properties_batch() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    s.set_node_property(a, "x", Value::Int64(1));
    s.set_node_property(b, "x", Value::Int64(2));

    let results = s.get_node_property_batch(&[a, b], &PropertyKey::new("x"));
    assert_eq!(results, vec![Some(Value::Int64(1)), Some(Value::Int64(2))]);
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_selective_property_batch() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&[]);
    s.set_node_property(id, "a", Value::Int64(1));
    s.set_node_property(id, "b", Value::Int64(2));
    s.set_node_property(id, "c", Value::Int64(3));

    let results = s.get_nodes_properties_selective_batch(
        &[id],
        &[PropertyKey::new("a"), PropertyKey::new("c")],
    );
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get(&PropertyKey::new("a")), Some(&Value::Int64(1)));
    assert_eq!(results[0].get(&PropertyKey::new("c")), Some(&Value::Int64(3)));
    assert_eq!(results[0].get(&PropertyKey::new("b")), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_edge_property_set_get_remove() {
    let db = setup().await;
    let s = store(&db);

    let src = s.create_node(&[]);
    let dst = s.create_node(&[]);
    let eid = s.create_edge(src, dst, "KNOWS");

    s.set_edge_property(eid, "since", Value::Int64(2020));
    assert_eq!(
        s.get_edge_property(eid, &PropertyKey::new("since")),
        Some(Value::Int64(2020))
    );

    let removed = s.remove_edge_property(eid, "since");
    assert_eq!(removed, Some(Value::Int64(2020)));
    assert_eq!(
        s.get_edge_property(eid, &PropertyKey::new("since")),
        None
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_edge_get_includes_properties() {
    let db = setup().await;
    let s = store(&db);

    let src = s.create_node(&[]);
    let dst = s.create_node(&[]);
    let eid = s.create_edge(src, dst, "WORKS_AT");
    s.set_edge_property(eid, "role", Value::String("Engineer".into()));

    let edge = s.get_edge(eid).unwrap();
    assert_eq!(
        edge.properties.get(&PropertyKey::new("role")),
        Some(&Value::String("Engineer".into()))
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Property index (maintained in both layouts)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn merged_find_nodes_by_property() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&["Person"]);
    let b = s.create_node(&["Person"]);
    s.set_node_property(a, "name", Value::String("Alice".into()));
    s.set_node_property(b, "name", Value::String("Bob".into()));

    let results = s.find_nodes_by_property("name", &Value::String("Alice".into()));
    assert_eq!(results, vec![a]);
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_property_index_updated_on_overwrite() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&[]);
    s.set_node_property(id, "score", Value::Int64(10));
    s.set_node_property(id, "score", Value::Int64(20));

    // Old value should not be findable
    let old = s.find_nodes_by_property("score", &Value::Int64(10));
    assert!(old.is_empty(), "old property value should be removed from index");

    // New value should be findable
    let new = s.find_nodes_by_property("score", &Value::Int64(20));
    assert_eq!(new, vec![id]);
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_find_nodes_in_range() {
    let db = setup().await;
    let s = store(&db);

    for i in 0..10 {
        let id = s.create_node(&[]);
        s.set_node_property(id, "val", Value::Int64(i));
    }

    let results = s.find_nodes_in_range(
        "val",
        Some(&Value::Int64(3)),
        Some(&Value::Int64(7)),
        true,
        false,
    );
    assert_eq!(results.len(), 4, "range [3,7) should match 3,4,5,6");
}

// ═══════════════════════════════════════════════════════════════════════
// Edge lifecycle & adjacency (via merge operands)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn merged_edge_create_and_get() {
    let db = setup().await;
    let s = store(&db);

    let src = s.create_node(&["Person"]);
    let dst = s.create_node(&["Person"]);
    let eid = s.create_edge(src, dst, "KNOWS");

    let edge = s.get_edge(eid).unwrap();
    assert_eq!(edge.src, src);
    assert_eq!(edge.dst, dst);
    assert_eq!(edge.edge_type.as_str(), "KNOWS");
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_edge_delete() {
    let db = setup().await;
    let s = store(&db);

    let src = s.create_node(&[]);
    let dst = s.create_node(&[]);
    let eid = s.create_edge(src, dst, "KNOWS");

    assert!(s.delete_edge(eid));
    assert!(s.get_edge(eid).is_none());
    assert_eq!(s.edge_count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_neighbors_outgoing() {
    let db = setup().await;
    let s = store(&db);

    let hub = s.create_node(&["Hub"]);
    let mut spokes = Vec::new();
    for _ in 0..5 {
        let n = s.create_node(&["Spoke"]);
        s.create_edge(hub, n, "CONNECTS");
        spokes.push(n);
    }

    let neighbors = s.neighbors(hub, Direction::Outgoing);
    assert_eq!(neighbors.len(), 5);
    for spoke in &spokes {
        assert!(neighbors.contains(spoke));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_neighbors_incoming() {
    let db = setup().await;
    let s = store(&db);

    let hub = s.create_node(&["Hub"]);
    let src = s.create_node(&["Src"]);
    s.create_edge(src, hub, "POINTS_TO");

    let incoming = s.neighbors(hub, Direction::Incoming);
    assert_eq!(incoming, vec![src]);
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_neighbors_both() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let c = s.create_node(&[]);
    s.create_edge(a, b, "X");
    s.create_edge(c, b, "Y");

    let both = s.neighbors(b, Direction::Both);
    assert_eq!(both.len(), 2);
    assert!(both.contains(&a));
    assert!(both.contains(&c));
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_out_degree_in_degree() {
    let db = setup().await;
    let s = store(&db);

    let hub = s.create_node(&[]);
    for _ in 0..10 {
        let n = s.create_node(&[]);
        s.create_edge(hub, n, "E");
    }

    assert_eq!(s.out_degree(hub), 10);
    assert_eq!(s.in_degree(hub), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_edges_from() {
    let db = setup().await;
    let s = store(&db);

    let src = s.create_node(&[]);
    let d1 = s.create_node(&[]);
    let d2 = s.create_node(&[]);
    let e1 = s.create_edge(src, d1, "A");
    let e2 = s.create_edge(src, d2, "B");

    let edges = s.edges_from(src, Direction::Outgoing);
    assert_eq!(edges.len(), 2);
    assert!(edges.contains(&(d1, e1)));
    assert!(edges.contains(&(d2, e2)));
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_batch_create_edges() {
    let db = setup().await;
    let s = store(&db);

    let nodes: Vec<_> = (0..5).map(|_| s.create_node(&[])).collect();
    let edges_input: Vec<_> = nodes[1..]
        .iter()
        .map(|dst| (nodes[0], *dst, "LINK"))
        .collect();
    let edge_ids = s.batch_create_edges(&edges_input);
    assert_eq!(edge_ids.len(), 4);
    assert_eq!(s.out_degree(nodes[0]), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_delete_node_cleans_edges() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&["A"]);
    let b = s.create_node(&["B"]);
    let c = s.create_node(&["C"]);
    let e1 = s.create_edge(a, b, "X");
    let e2 = s.create_edge(c, a, "Y");

    s.delete_node(a);

    assert!(s.get_edge(e1).is_none(), "outgoing edge should be deleted");
    assert!(s.get_edge(e2).is_none(), "incoming edge should be deleted");
    assert_eq!(s.edge_count(), 0);
    // b and c should have no remaining adjacency to a
    assert!(s.neighbors(b, Direction::Both).is_empty());
    assert!(s.neighbors(c, Direction::Both).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_delete_node_cleans_properties_and_index() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);
    s.set_node_property(id, "email", Value::String("test@example.com".into()));

    s.delete_node(id);

    let results = s.find_nodes_by_property("email", &Value::String("test@example.com".into()));
    assert!(results.is_empty(), "property index should be cleaned up on delete");
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_adjacency_cleaned_on_edge_delete() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let eid = s.create_edge(a, b, "KNOWS");

    assert_eq!(s.neighbors(a, Direction::Outgoing), vec![b]);
    assert_eq!(s.neighbors(b, Direction::Incoming), vec![a]);

    s.delete_edge(eid);

    assert!(s.neighbors(a, Direction::Outgoing).is_empty());
    assert!(s.neighbors(b, Direction::Incoming).is_empty());
}

// ═══════════════════════════════════════════════════════════════════════
// Labels (same codepath for both layouts, but verify in Merged context)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn merged_label_add_and_remove() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);
    s.add_label(id, "Employee");

    let node = s.get_node(id).unwrap();
    assert_eq!(node.labels.len(), 2);

    s.remove_label(id, "Person");
    let node = s.get_node(id).unwrap();
    assert_eq!(node.labels.len(), 1);
    assert!(node.labels.iter().any(|l| l.as_str() == "Employee"));
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_nodes_by_label() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&["Person"]);
    let _b = s.create_node(&["Company"]);
    let c = s.create_node(&["Person"]);

    let persons = s.nodes_by_label("Person");
    assert_eq!(persons.len(), 2);
    assert!(persons.contains(&a));
    assert!(persons.contains(&c));
}

// ═══════════════════════════════════════════════════════════════════════
// Mixed property types
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn merged_mixed_property_types() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&[]);
    s.set_node_property(id, "flag", Value::Bool(true));
    s.set_node_property(id, "count", Value::Int64(42));
    s.set_node_property(id, "ratio", Value::Float64(3.14));
    s.set_node_property(id, "name", Value::String("test".into()));
    s.set_node_property(id, "empty", Value::Null);

    assert_eq!(s.get_node_property(id, &PropertyKey::new("flag")), Some(Value::Bool(true)));
    assert_eq!(s.get_node_property(id, &PropertyKey::new("count")), Some(Value::Int64(42)));
    assert_eq!(s.get_node_property(id, &PropertyKey::new("ratio")), Some(Value::Float64(3.14)));
    assert_eq!(s.get_node_property(id, &PropertyKey::new("name")), Some(Value::String("test".into())));
    assert_eq!(s.get_node_property(id, &PropertyKey::new("empty")), Some(Value::Null));
}

// ═══════════════════════════════════════════════════════════════════════
// Edge type lookup
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn merged_edge_type_lookup() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let eid = s.create_edge(a, b, "MANAGES");

    assert_eq!(s.edge_type(eid).unwrap().as_str(), "MANAGES");
}

// ═══════════════════════════════════════════════════════════════════════
// Multi-edge support
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn merged_multi_edges_same_type() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let e1 = s.create_edge(a, b, "KNOWS");
    let e2 = s.create_edge(a, b, "KNOWS");

    assert_ne!(e1, e2);
    assert_eq!(s.out_degree(a), 2);

    let edges = s.edges_from(a, Direction::Outgoing);
    assert_eq!(edges.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_delete_node_with_self_loop() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let _self_edge = s.create_edge(a, a, "SELF");
    assert_eq!(s.edge_count(), 1);

    s.delete_node(a);

    assert_eq!(s.node_count(), 0);
    assert_eq!(s.edge_count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_delete_node_edges_cleans_up() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let c = s.create_node(&[]);

    let e1 = s.create_edge(a, b, "X");
    let e2 = s.create_edge(c, a, "Y");

    s.delete_node_edges(a);

    assert!(s.get_edge(e1).is_none(), "outgoing edge should be deleted");
    assert!(s.get_edge(e2).is_none(), "incoming edge should be deleted");
    assert!(s.get_node(a).is_some(), "node itself should still exist");
    assert_eq!(s.edge_count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_delete_node_edges_self_loop() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let _e1 = s.create_edge(a, a, "SELF");
    let _e2 = s.create_edge(a, b, "OTHER");
    assert_eq!(s.edge_count(), 2);

    s.delete_node_edges(a);

    assert!(s.get_node(a).is_some(), "node itself should still exist");
    assert_eq!(s.edge_count(), 0, "self-loop should be counted once");
}
