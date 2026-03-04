//! Layer 2 & 3 integration tests for the SlateGraphStore.
//!
//! These tests exercise the `GraphStore` and `GraphStoreMut` trait methods
//! directly against a real (in-memory) SlateDB backend, bypassing the GQL
//! query engine. They validate the RFC 0006 storage contract.

use std::sync::Arc;

use common::{StorageConfig, StorageRuntime, StorageSemantics};
use grafeo_common::types::{EdgeId, EpochId, NodeId, PropertyKey, TxId, Value};
use grafeo_core::graph::Direction;
use grafeo_core::graph::traits::{GraphStore, GraphStoreMut};
use graph::db::GraphDb;
use graph::{Config, SlateGraphStore};

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

async fn setup() -> Arc<GraphDb> {
    let config = Config {
        storage: StorageConfig::InMemory,
        ..Default::default()
    };
    Arc::new(GraphDb::open_with_config(&config).await.unwrap())
}

/// Convenience: get a &SlateGraphStore that impls both traits.
fn store(db: &GraphDb) -> &SlateGraphStore {
    db.store()
}

// ═══════════════════════════════════════════════════════════════════════
// Layer 2: Storage Trait Tests
// ═══════════════════════════════════════════════════════════════════════

// ─── 2.1 Node lifecycle ──────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn node_create_and_get() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);

    let node = s.get_node(id);
    assert!(node.is_some(), "created node should be retrievable");

    let node = node.unwrap();
    assert_eq!(node.id, id);
    assert!(
        node.labels.iter().any(|l| l.as_str() == "Person"),
        "node should carry the Person label"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn node_create_no_labels() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&[]);
    let node = s.get_node(id).expect("labelless node should exist");
    assert!(node.labels.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn node_create_multiple_labels() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person", "Employee", "Manager"]);
    let node = s.get_node(id).unwrap();

    let label_names: Vec<&str> = node.labels.iter().map(|l| l.as_str()).collect();
    assert!(label_names.contains(&"Person"));
    assert!(label_names.contains(&"Employee"));
    assert!(label_names.contains(&"Manager"));
}

#[tokio::test(flavor = "multi_thread")]
async fn node_get_nonexistent() {
    let db = setup().await;
    let s = store(&db);

    assert!(s.get_node(NodeId(999_999)).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn node_delete() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);
    assert!(s.get_node(id).is_some());

    let deleted = s.delete_node(id);
    assert!(deleted, "delete should return true for existing node");
    assert!(
        s.get_node(id).is_none(),
        "deleted node should not be retrievable"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn node_delete_nonexistent() {
    let db = setup().await;
    let s = store(&db);

    assert!(!s.delete_node(NodeId(999_999)));
}

#[tokio::test(flavor = "multi_thread")]
async fn node_delete_idempotent() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);
    assert!(s.delete_node(id));
    assert!(!s.delete_node(id), "second delete should return false");
}

#[tokio::test(flavor = "multi_thread")]
async fn node_ids_lists_all_live_nodes() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&["A"]);
    let b = s.create_node(&["B"]);
    let c = s.create_node(&["C"]);
    s.delete_node(b);

    let ids = s.node_ids();
    assert!(ids.contains(&a));
    assert!(!ids.contains(&b), "deleted node should not appear");
    assert!(ids.contains(&c));
}

// ─── 2.2 Edge lifecycle ──────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn edge_create_and_get() {
    let db = setup().await;
    let s = store(&db);

    let src = s.create_node(&["Person"]);
    let dst = s.create_node(&["Person"]);
    let eid = s.create_edge(src, dst, "KNOWS");

    let edge = s.get_edge(eid);
    assert!(edge.is_some(), "created edge should be retrievable");

    let edge = edge.unwrap();
    assert_eq!(edge.id, eid);
    assert_eq!(edge.src, src);
    assert_eq!(edge.dst, dst);
    assert_eq!(edge.edge_type.as_str(), "KNOWS");
}

#[tokio::test(flavor = "multi_thread")]
async fn edge_get_nonexistent() {
    let db = setup().await;
    let s = store(&db);
    assert!(s.get_edge(EdgeId(999_999)).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn edge_delete() {
    let db = setup().await;
    let s = store(&db);

    let src = s.create_node(&[]);
    let dst = s.create_node(&[]);
    let eid = s.create_edge(src, dst, "KNOWS");

    assert!(s.delete_edge(eid));
    assert!(s.get_edge(eid).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn edge_delete_nonexistent() {
    let db = setup().await;
    let s = store(&db);
    assert!(!s.delete_edge(EdgeId(999_999)));
}

#[tokio::test(flavor = "multi_thread")]
async fn edge_type_lookup() {
    let db = setup().await;
    let s = store(&db);

    let src = s.create_node(&[]);
    let dst = s.create_node(&[]);
    let eid = s.create_edge(src, dst, "FOLLOWS");

    assert_eq!(s.edge_type(eid).unwrap().as_str(), "FOLLOWS");
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_node_edges_cleans_up() {
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
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_create_edges() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let c = s.create_node(&[]);

    let edges = s.batch_create_edges(&[(a, b, "X"), (a, c, "Y"), (b, c, "Z")]);
    assert_eq!(edges.len(), 3);

    for eid in &edges {
        assert!(s.get_edge(*eid).is_some());
    }
}

// ─── 2.3 Properties ─────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn node_property_set_get() {
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
}

#[tokio::test(flavor = "multi_thread")]
async fn node_property_overwrite() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&[]);
    s.set_node_property(id, "x", Value::Int64(1));
    s.set_node_property(id, "x", Value::Int64(2));

    assert_eq!(
        s.get_node_property(id, &PropertyKey::new("x")),
        Some(Value::Int64(2))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn node_property_remove() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&[]);
    s.set_node_property(id, "x", Value::Int64(42));

    let removed = s.remove_node_property(id, "x");
    assert_eq!(removed, Some(Value::Int64(42)));
    assert!(s.get_node_property(id, &PropertyKey::new("x")).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn node_property_remove_nonexistent() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&[]);
    assert!(s.remove_node_property(id, "nope").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn node_properties_batch() {
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
async fn node_get_includes_properties() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);
    s.set_node_property(id, "name", Value::String("Alice".into()));

    let node = s.get_node(id).unwrap();
    assert_eq!(
        node.properties.get(&PropertyKey::new("name")),
        Some(&Value::String("Alice".into()))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn edge_property_set_get_remove() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let eid = s.create_edge(a, b, "KNOWS");

    s.set_edge_property(eid, "since", Value::Int64(2020));
    assert_eq!(
        s.get_edge_property(eid, &PropertyKey::new("since")),
        Some(Value::Int64(2020))
    );

    let removed = s.remove_edge_property(eid, "since");
    assert_eq!(removed, Some(Value::Int64(2020)));
    assert!(s.get_edge_property(eid, &PropertyKey::new("since")).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn edge_get_includes_properties() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let eid = s.create_edge(a, b, "KNOWS");
    s.set_edge_property(eid, "weight", Value::Float64(0.95));

    let edge = s.get_edge(eid).unwrap();
    assert_eq!(
        edge.properties.get(&PropertyKey::new("weight")),
        Some(&Value::Float64(0.95))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mixed_property_types() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&[]);
    let cases: Vec<(&str, Value)> = vec![
        ("null_val", Value::Null),
        ("bool_val", Value::Bool(true)),
        ("int_val", Value::Int64(-42)),
        ("float_val", Value::Float64(3.14)),
        ("string_val", Value::String("hello".into())),
    ];

    for (key, val) in &cases {
        s.set_node_property(id, key, val.clone());
    }
    for (key, val) in &cases {
        assert_eq!(
            s.get_node_property(id, &PropertyKey::new(*key)),
            Some(val.clone()),
            "roundtrip failed for {key}"
        );
    }
}

// ─── 2.4 Labels ──────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn label_add_and_remove() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);

    // Add a new label
    assert!(s.add_label(id, "Employee"), "adding new label should succeed");
    let node = s.get_node(id).unwrap();
    let labels: Vec<&str> = node.labels.iter().map(|l| l.as_str()).collect();
    assert!(labels.contains(&"Person"));
    assert!(labels.contains(&"Employee"));

    // Remove original label
    assert!(s.remove_label(id, "Person"));
    let node = s.get_node(id).unwrap();
    let labels: Vec<&str> = node.labels.iter().map(|l| l.as_str()).collect();
    assert!(!labels.contains(&"Person"), "Person label should be removed");
    assert!(labels.contains(&"Employee"));
}

#[tokio::test(flavor = "multi_thread")]
async fn label_add_duplicate() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);
    assert!(
        !s.add_label(id, "Person"),
        "adding duplicate label should return false"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn label_remove_nonexistent() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);
    assert!(!s.remove_label(id, "Employee"));
}

#[tokio::test(flavor = "multi_thread")]
async fn nodes_by_label_scan() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&["Person"]);
    let _b = s.create_node(&["City"]);
    let c = s.create_node(&["Person", "Employee"]);

    let persons = s.nodes_by_label("Person");
    assert!(persons.contains(&a));
    assert!(persons.contains(&c));
    assert_eq!(persons.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn nodes_by_label_empty() {
    let db = setup().await;
    let s = store(&db);

    assert!(s.nodes_by_label("NonExistent").is_empty());
}

// ─── 2.5 Adjacency traversal ────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn neighbors_outgoing() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let c = s.create_node(&[]);
    s.create_edge(a, b, "X");
    s.create_edge(a, c, "Y");

    let out = s.neighbors(a, Direction::Outgoing);
    assert_eq!(out.len(), 2);
    assert!(out.contains(&b));
    assert!(out.contains(&c));
}

#[tokio::test(flavor = "multi_thread")]
async fn neighbors_incoming() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let c = s.create_node(&[]);
    s.create_edge(b, a, "X");
    s.create_edge(c, a, "Y");

    let inc = s.neighbors(a, Direction::Incoming);
    assert_eq!(inc.len(), 2);
    assert!(inc.contains(&b));
    assert!(inc.contains(&c));
}

#[tokio::test(flavor = "multi_thread")]
async fn neighbors_both() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let c = s.create_node(&[]);
    s.create_edge(a, b, "X");
    s.create_edge(c, a, "Y");

    let both = s.neighbors(a, Direction::Both);
    assert!(both.contains(&b), "outgoing neighbor should be included");
    assert!(both.contains(&c), "incoming neighbor should be included");
}

#[tokio::test(flavor = "multi_thread")]
async fn edges_from_outgoing() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let eid = s.create_edge(a, b, "KNOWS");

    let edges = s.edges_from(a, Direction::Outgoing);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].0, b);
    assert_eq!(edges[0].1, eid);
}

#[tokio::test(flavor = "multi_thread")]
async fn out_degree_and_in_degree() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let c = s.create_node(&[]);
    s.create_edge(a, b, "X");
    s.create_edge(a, c, "Y");
    s.create_edge(c, a, "Z");

    assert_eq!(s.out_degree(a), 2);
    assert_eq!(s.in_degree(a), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn adjacency_cleaned_on_edge_delete() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let eid = s.create_edge(a, b, "X");

    assert_eq!(s.out_degree(a), 1);
    s.delete_edge(eid);
    assert_eq!(s.out_degree(a), 0);
    assert_eq!(s.in_degree(b), 0);
}

// ─── 2.6 Property index ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn find_nodes_by_property_equality() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&["Person"]);
    let b = s.create_node(&["Person"]);
    let c = s.create_node(&["Person"]);
    s.set_node_property(a, "age", Value::Int64(30));
    s.set_node_property(b, "age", Value::Int64(25));
    s.set_node_property(c, "age", Value::Int64(30));

    let found = s.find_nodes_by_property("age", &Value::Int64(30));
    assert_eq!(found.len(), 2);
    assert!(found.contains(&a));
    assert!(found.contains(&c));
}

#[tokio::test(flavor = "multi_thread")]
async fn find_nodes_by_property_string() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    s.set_node_property(a, "name", Value::String("Alice".into()));
    s.set_node_property(b, "name", Value::String("Bob".into()));

    let found = s.find_nodes_by_property("name", &Value::String("Alice".into()));
    assert_eq!(found, vec![a]);
}

#[tokio::test(flavor = "multi_thread")]
async fn find_nodes_by_multiple_properties() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let c = s.create_node(&[]);
    s.set_node_property(a, "city", Value::String("NYC".into()));
    s.set_node_property(a, "age", Value::Int64(30));
    s.set_node_property(b, "city", Value::String("NYC".into()));
    s.set_node_property(b, "age", Value::Int64(25));
    s.set_node_property(c, "city", Value::String("LA".into()));
    s.set_node_property(c, "age", Value::Int64(30));

    let found = s.find_nodes_by_properties(&[
        ("city", Value::String("NYC".into())),
        ("age", Value::Int64(30)),
    ]);
    assert_eq!(found, vec![a]);
}

#[tokio::test(flavor = "multi_thread")]
async fn find_nodes_in_range() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let c = s.create_node(&[]);
    s.set_node_property(a, "score", Value::Int64(10));
    s.set_node_property(b, "score", Value::Int64(50));
    s.set_node_property(c, "score", Value::Int64(90));

    // Range: 20 <= score <= 80
    let found = s.find_nodes_in_range(
        "score",
        Some(&Value::Int64(20)),
        Some(&Value::Int64(80)),
        true,
        true,
    );
    assert_eq!(found, vec![b]);
}

#[tokio::test(flavor = "multi_thread")]
async fn property_index_updated_on_remove() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&[]);
    s.set_node_property(id, "x", Value::Int64(42));

    assert_eq!(
        s.find_nodes_by_property("x", &Value::Int64(42)),
        vec![id]
    );

    s.remove_node_property(id, "x");

    assert!(
        s.find_nodes_by_property("x", &Value::Int64(42)).is_empty(),
        "property index should be cleaned up on remove"
    );
}

// ─── 2.7 Counters & statistics ───────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn node_count_tracks_creates_and_deletes() {
    let db = setup().await;
    let s = store(&db);

    assert_eq!(s.node_count(), 0);

    let a = s.create_node(&[]);
    let _b = s.create_node(&[]);
    assert_eq!(s.node_count(), 2);

    s.delete_node(a);
    assert_eq!(s.node_count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn edge_count_tracks_creates_and_deletes() {
    let db = setup().await;
    let s = store(&db);

    assert_eq!(s.edge_count(), 0);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let e1 = s.create_edge(a, b, "X");
    let _e2 = s.create_edge(b, a, "Y");
    assert_eq!(s.edge_count(), 2);

    s.delete_edge(e1);
    assert_eq!(s.edge_count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn statistics_reflect_counts() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    s.create_edge(a, b, "X");

    let stats = s.statistics();
    assert_eq!(stats.total_nodes, 2);
    assert_eq!(stats.total_edges, 1);
}

// ═══════════════════════════════════════════════════════════════════════
// Layer 3: Behavioral Invariants
// ═══════════════════════════════════════════════════════════════════════

// ─── 3.1 Multi-edge correctness ──────────────────────────────────────
// RFC: edge_id in adjacency key ensures multiple edges between the same
// node pair with the same type are preserved as distinct entries.

#[tokio::test(flavor = "multi_thread")]
async fn multi_edge_same_type_preserved() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);

    // Create two edges of the same type between the same nodes
    let e1 = s.create_edge(a, b, "TRANSFERRED_TO");
    let e2 = s.create_edge(a, b, "TRANSFERRED_TO");

    assert_ne!(e1, e2, "should get distinct edge IDs");

    // Both edges should be retrievable by ID
    assert!(s.get_edge(e1).is_some(), "first edge should exist");
    assert!(s.get_edge(e2).is_some(), "second edge should exist");

    // Adjacency traversal should return BOTH edges
    let edges = s.edges_from(a, Direction::Outgoing);
    let edge_ids: Vec<EdgeId> = edges.iter().map(|(_, eid)| *eid).collect();
    assert!(
        edge_ids.contains(&e1),
        "adjacency should contain first edge"
    );
    assert!(
        edge_ids.contains(&e2),
        "adjacency should contain second edge"
    );
    assert_eq!(
        edges.len(),
        2,
        "adjacency should list both multi-edges, got {}",
        edges.len()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_edge_out_degree() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);

    s.create_edge(a, b, "SENT");
    s.create_edge(a, b, "SENT");
    s.create_edge(a, b, "SENT");

    assert_eq!(
        s.out_degree(a),
        3,
        "out_degree should count all multi-edges"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_edge_neighbors_dedup() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);

    s.create_edge(a, b, "KNOWS");
    s.create_edge(a, b, "KNOWS");

    // neighbors() returns NodeIds — b should appear (possibly duplicated
    // depending on implementation, but at least not lost)
    let nbrs = s.neighbors(a, Direction::Outgoing);
    assert!(
        !nbrs.is_empty(),
        "neighbors should include b even with multi-edges"
    );
}

// ─── 3.2 MVCC epoch visibility ──────────────────────────────────────
// RFC: get_node_versioned(id, epoch, tx) should return the node version
// visible at the given epoch. A node deleted at epoch E2 should still
// be visible at epoch E1 < E2.

#[tokio::test(flavor = "multi_thread")]
async fn mvcc_versioned_read_sees_earlier_epoch() {
    let db = setup().await;
    let s = store(&db);

    // Create node at epoch 1
    let id = s.create_node_versioned(&["Person"], EpochId(1), TxId(0));
    assert!(
        s.get_node_versioned(id, EpochId(1), TxId(0)).is_some(),
        "node should be visible at creation epoch"
    );

    // Delete at epoch 2
    s.delete_node_versioned(id, EpochId(2), TxId(0));

    // At epoch 1, the node should still be visible (pre-deletion)
    assert!(
        s.get_node_versioned(id, EpochId(1), TxId(0)).is_some(),
        "node should be visible at epoch before deletion"
    );

    // At epoch 2, the node should be gone
    assert!(
        s.get_node_versioned(id, EpochId(2), TxId(0)).is_none(),
        "node should not be visible at deletion epoch"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mvcc_versioned_read_future_epoch_not_visible() {
    let db = setup().await;
    let s = store(&db);

    // Create node at epoch 5
    let id = s.create_node_versioned(&["Thing"], EpochId(5), TxId(0));

    // At epoch 3, the node should NOT be visible (created in the future)
    assert!(
        s.get_node_versioned(id, EpochId(3), TxId(0)).is_none(),
        "node created at epoch 5 should not be visible at epoch 3"
    );

    // At epoch 5, it should be visible
    assert!(
        s.get_node_versioned(id, EpochId(5), TxId(0)).is_some(),
        "node should be visible at its creation epoch"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mvcc_edge_versioned_read() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node_versioned(&[], EpochId(1), TxId(0));
    let b = s.create_node_versioned(&[], EpochId(1), TxId(0));
    let eid = s.create_edge_versioned(a, b, "KNOWS", EpochId(2), TxId(0));

    // Edge created at epoch 2 should not be visible at epoch 1
    assert!(
        s.get_edge_versioned(eid, EpochId(1), TxId(0)).is_none(),
        "edge should not be visible before creation epoch"
    );

    assert!(
        s.get_edge_versioned(eid, EpochId(2), TxId(0)).is_some(),
        "edge should be visible at creation epoch"
    );
}

// ─── 3.3 Counter accuracy after mixed operations ────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn counter_accuracy_after_mixed_ops() {
    let db = setup().await;
    let s = store(&db);

    let mut node_ids = Vec::new();
    for _ in 0..10 {
        node_ids.push(s.create_node(&["X"]));
    }

    // Delete 4 nodes
    for id in &node_ids[0..4] {
        s.delete_node(*id);
    }

    assert_eq!(s.node_count(), 6, "10 created - 4 deleted = 6");

    // Create edges between remaining nodes
    let remaining = &node_ids[4..];
    let mut edge_ids = Vec::new();
    for i in 0..remaining.len() - 1 {
        edge_ids.push(s.create_edge(remaining[i], remaining[i + 1], "NEXT"));
    }
    assert_eq!(s.edge_count(), 5);

    // Delete 2 edges
    s.delete_edge(edge_ids[0]);
    s.delete_edge(edge_ids[1]);
    assert_eq!(s.edge_count(), 3);
}

// ─── 3.4 Catalog persistence across reopen ──────────────────────────
// The catalog should survive database close and reopen.

#[tokio::test(flavor = "multi_thread")]
async fn catalog_survives_reopen() {
    use graph::storage::merge_operator::GraphMergeOperator;

    // Create storage that outlives the first GraphDb instance
    let storage = common::create_storage(
        &StorageConfig::InMemory,
        StorageRuntime::new(),
        StorageSemantics::new().with_merge_operator(Arc::new(GraphMergeOperator)),
    )
    .await
    .unwrap();

    let config = Config::default();

    // First session: create data
    {
        let db = GraphDb::open(storage.clone(), &config).await.unwrap();
        let s = store(&db);

        let a = s.create_node(&["Person"]);
        let b = s.create_node(&["City"]);
        s.create_edge(a, b, "LIVES_IN");
        s.set_node_property(a, "name", Value::String("Alice".into()));
    }

    // Second session: verify data survives
    {
        let db = GraphDb::open(storage.clone(), &config).await.unwrap();
        let s = store(&db);

        // Counters should be restored from metadata merge
        assert_eq!(s.node_count(), 2, "node count should survive reopen");
        assert_eq!(s.edge_count(), 1, "edge count should survive reopen");

        // Label scan should still work (catalog loaded from storage)
        let persons = s.nodes_by_label("Person");
        assert_eq!(persons.len(), 1, "label index should survive reopen");

        // Data should be readable
        let person_id = persons[0];
        let node = s.get_node(person_id);
        assert!(node.is_some(), "node should survive reopen");

        let name = s.get_node_property(person_id, &PropertyKey::new("name"));
        assert_eq!(
            name,
            Some(Value::String("Alice".into())),
            "property should survive reopen"
        );
    }
}

// ─── 3.5 Write atomicity ────────────────────────────────────────────
// Creating a node with labels should make both the node and its label
// index entries visible atomically.

#[tokio::test(flavor = "multi_thread")]
async fn node_and_label_index_consistent() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);

    // Both node lookup and label scan should find it
    assert!(s.get_node(id).is_some());
    assert!(s.nodes_by_label("Person").contains(&id));
}

#[tokio::test(flavor = "multi_thread")]
async fn edge_and_adjacency_consistent() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    let eid = s.create_edge(a, b, "KNOWS");

    // Edge record and adjacency should agree
    assert!(s.get_edge(eid).is_some());
    assert_eq!(s.out_degree(a), 1);
    assert_eq!(s.in_degree(b), 1);
    assert!(s.neighbors(a, Direction::Outgoing).contains(&b));
    assert!(s.neighbors(b, Direction::Incoming).contains(&a));
}

// ─── 3.6 Selective property batch ───────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn selective_property_batch() {
    let db = setup().await;
    let s = store(&db);

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    s.set_node_property(a, "name", Value::String("Alice".into()));
    s.set_node_property(a, "age", Value::Int64(30));
    s.set_node_property(a, "city", Value::String("NYC".into()));
    s.set_node_property(b, "name", Value::String("Bob".into()));
    s.set_node_property(b, "age", Value::Int64(25));

    // Only request name and age (not city)
    let results = s.get_nodes_properties_selective_batch(
        &[a, b],
        &[PropertyKey::new("name"), PropertyKey::new("age")],
    );

    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0].get(&PropertyKey::new("name")),
        Some(&Value::String("Alice".into()))
    );
    assert_eq!(
        results[0].get(&PropertyKey::new("age")),
        Some(&Value::Int64(30))
    );
    assert!(
        results[0].get(&PropertyKey::new("city")).is_none(),
        "city should not be included in selective batch"
    );
}

// ─── 3.7 Backward adjacency always maintained ──────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn backward_adjacency_always_on() {
    let db = setup().await;
    let s = store(&db);

    assert!(
        s.has_backward_adjacency(),
        "default config should enable backward adjacency"
    );

    let a = s.create_node(&[]);
    let b = s.create_node(&[]);
    s.create_edge(a, b, "X");

    assert_eq!(s.in_degree(b), 1);
    assert!(s.neighbors(b, Direction::Incoming).contains(&a));
}

// ─── 3.8 Delete node cleans up label index ──────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn delete_node_cleans_label_index() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&["Person"]);
    assert_eq!(s.nodes_by_label("Person").len(), 1);

    s.delete_node(id);
    assert!(
        s.nodes_by_label("Person").is_empty(),
        "label index should be cleaned on node delete"
    );
}

// ─── 3.9 Delete node cleans up properties ───────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn delete_node_cleans_properties() {
    let db = setup().await;
    let s = store(&db);

    let id = s.create_node(&[]);
    s.set_node_property(id, "x", Value::Int64(1));
    s.set_node_property(id, "y", Value::Int64(2));

    s.delete_node(id);

    // Properties should be gone (no orphan data)
    assert!(s.get_node_property(id, &PropertyKey::new("x")).is_none());
    assert!(s.get_node_property(id, &PropertyKey::new("y")).is_none());
}
