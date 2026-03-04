# RFC 0001: Graph Database Storage

**Status**: Draft

**Authors**:

- [Steven Grond](https://github.com/StevenBtw)

## Summary

This RFC defines the storage model for a labeled property graph (LPG) database built on
[SlateDB](https://github.com/slatedb/slatedb). The graph database integrates
[Grafeo](https://github.com/grafeo-db/grafeo) (v0.5.13) as its query engine, implementing Grafeo's
`GraphStore` and `GraphStoreMut` traits over SlateDB's ordered key-value interface. The design maps
graph primitives: nodes, edges, labels, properties and adjacency, to SlateDB records using the
standard 2-byte key prefix, with additional index structures for label lookups, property searches,
and traversal. MVCC is provided through epoch-based versioning of entity records.

## Motivation

OpenData provides purpose-built databases for time series, logs, vectors, and key-value workloads,
all sharing SlateDB as a common storage engine. Graph workloads are a natural addition: they appear
in knowledge graphs, social networks, fraud detection, recommendation engines, and infrastructure
dependency mapping. A graph database built on SlateDB inherits the same operational simplicity
(single storage engine, unified tooling) while providing efficient traversal, pattern matching, and
multi-hop queries via Grafeo's query pipeline.

The storage design must support:

1. **Efficient traversal**: Following edges from a node to its neighbors is the fundamental graph
   operation. Adjacency must be stored in a layout that enables fast, direction-aware traversal
   without loading unrelated data.

2. **Point lookups**: Retrieving a node or edge by ID must be efficient via a direct key seek. The
   query engine issues point lookups during pattern matching and result materialization. Entity
   lookups are O(v) where v is the number of MVCC versions for that entity (typically 1-2, kept
   small by garbage collection).

3. **Label and property indexing**: GQL queries filter nodes by label (`MATCH (n:Person)`) and by
   property values (`WHERE n.age > 30`). Without indexes, these require full scans.

4. **MVCC for concurrent reads and writes**: The query engine expects snapshot isolation. Readers
   must see a consistent view while writers mutate the graph concurrently.

5. **Schema catalog persistence**: Grafeo maps label names, edge type names, and property key
   names to compact integer IDs for internal processing. This catalog must survive restarts.

6. **Compatibility with Grafeo's storage traits**: The implementation must satisfy the
   `GraphStore` (read) and `GraphStoreMut` (write) trait contracts so that Grafeo's query
   pipeline, optimizer, and execution engine work unmodified.

## Goals

- Define the record key/value encoding scheme for all graph entity types
- Map Grafeo's `GraphStore` and `GraphStoreMut` trait methods to SlateDB operations
- Define the catalog persistence format for labels, edge types, and property keys
- Define adjacency index layout for directional traversal
- Define label and property index layouts for filtered scans
- Define the MVCC epoch model for snapshot isolation
- Define merge operators for atomic counter updates and zone map maintenance

## Non-Goals (left for future RFCs)

- RDF triple store support (SPARQL, triple indexes)
- Query languages beyond GQL (Cypher, Gremlin, GraphQL, SQL/PGQ)
- Graph algorithms plugin (`algos` feature)
- Vector, text, and hybrid search indexes on graph properties
- GWP (gRPC) and Bolt (Neo4j) protocol support
- Write coordination and distributed deployment
- Compaction policies and garbage collection mechanics
- HTTP API design (covered in a future write/read API RFC)

## Dependencies

The graph engine introduces the following external crate dependencies:

### Grafeo Crates

| Crate            | Version | Role                                                                                                                                     |
|------------------|---------|------------------------------------------------------------------------------------------------------------------------------------------|
| `grafeo-core`    | 0.5.13  | Graph storage traits (`GraphStore`, `GraphStoreMut`), core types (`Node`, `Edge`, `NodeId`, `EdgeId`, `Value`, `PropertyKey`)            |
| `grafeo-common`  | 0.5.13  | Shared primitives (`NodeId`, `EdgeId`, `EpochId`, `Value` enum, MVCC types)                                                              |
| `grafeo-engine`  | 0.5.13  | Query engine: GQL parser, cost-based optimizer, push-based vectorized executor. GQL is the primary query interface and is always enabled |

Grafeo is published on crates.io. All three crates are required dependencies. The graph database
always includes the GQL query engine, there is no "storage-only" deployment mode. Additional query
languages (Cypher, SPARQL, etc.) may be added as optional features in the future, but GQL is the
default and mandatory interface.

### Other External Crates

| Crate          | Version | Role                                                                  |
|----------------|---------|-----------------------------------------------------------------------|
| `parking_lot`  | 0.12    | Faster RwLock/Mutex for catalog cache and sequence allocators         |
| `hashbrown`    | 0.14    | HashMap variant used by Grafeo types                                  |
| `arcstr`       | 1.2     | Atomic reference-counted strings (used for label/type/property names) |
| `smallvec`     | 1.13    | Stack-allocated vectors (used for node label lists)                   |

### Precedent

Other OpenData engines depend on external specialized crates: timeseries uses `promql-parser`
(PromQL query parsing) and `tsz` (time series compression), vector uses `usearch` (HNSW
similarity search). The graph engine follows the same pattern, using Grafeo for graph-specific
query parsing and execution. This could be internalized at a later moment, but would require a
significant amount of work to be done correctly (pruning, new integration tests, etc.).

## Design

### Architecture Overview

Each graph database instance corresponds to a single SlateDB instance. Graph entities (nodes,
edges), their properties, adjacency indexes, label indexes, property indexes, and catalog
dictionaries are all stored as key-value pairs in the LSM tree. Grafeo's query engine operates
on an in-memory `SlateGraphStore` adapter that translates trait method calls into SlateDB reads and
writes.

```ascii
┌──────────────────────────────────────────────────────────────────────┐
│                   OpenData Graph (per database)                      │
│                                                                      │
│   ┌──────────────────────────────────────────────────────────────┐   │
│   │                   Grafeo Query Engine                        │   │
│   │                                                              │   │
│   │   GQL Parser → Binder → Optimizer → Executor                 │   │
│   │                                                              │   │
│   │   Operates on GraphStore / GraphStoreMut trait objects       │   │
│   └──────────────────────────┬───────────────────────────────────┘   │
│                              │                                       │
│   ┌──────────────────────────▼───────────────────────────────────┐   │
│   │               SlateGraphStore Adapter                        │   │
│   │                                                              │   │
│   │   Implements GraphStore (reads) + GraphStoreMut (writes)     │   │
│   │   Translates trait calls → SlateDB get/put/delete/scan       │   │
│   │                                                              │   │
│   │   In-memory components:                                      │   │
│   │   ├─ Catalog cache (label/type/property-key dictionaries)    │   │
│   │   ├─ Statistics (cardinality estimates, degree histograms)   │   │
│   │   └─ Zone maps (property min/max for skip pruning)           │   │
│   └──────────────────────────┬───────────────────────────────────┘   │
│                              │                                       │
│   ┌──────────────────────────▼───────────────────────────────────┐   │
│   │                    Record Layout                             │   │
│   │                                                              │   │
│   │   NodeRecord    (0x10)  ─ Node existence + epoch             │   │
│   │   EdgeRecord    (0x20)  ─ Edge endpoints + type + epoch      │   │
│   │   NodeProperty  (0x30)  ─ Per-node property values           │   │
│   │   EdgeProperty  (0x40)  ─ Per-edge property values           │   │
│   │   ForwardAdj    (0x50)  ─ Outgoing adjacency index           │   │
│   │   BackwardAdj   (0x60)  ─ Incoming adjacency index           │   │
│   │   LabelIndex    (0x70)  ─ Label → node ID index              │   │
│   │   PropertyIndex (0x80)  ─ Property value → node ID index     │   │
│   │   Catalog       (0x90)  ─ Name ↔ ID dictionaries             │   │
│   │   Metadata      (0xE0)  ─ Counters, epoch, statistics        │   │
│   │   SeqBlock      (0xF0)  ─ ID allocation state                │   │
│   └──────────────────────────┬───────────────────────────────────┘   │
│                              │                                       │
│   Storage: SlateDB (LSM KV Store)                                    │
│   (all graph data as ordered key-value pairs)                        │
└──────────────────────────────────────────────────────────────────────┘
```

### Background on Grafeo's Storage Traits

This RFC focuses on the SlateDB record layout, but understanding how Grafeo's query engine
interacts with the storage layer motivates the design.

#### GraphStore (Read Path)

Grafeo's `GraphStore` trait defines the read interface that scan, expand, filter, project, and
shortest-path operators use. Key method groups:

- **Point lookups**: `get_node(id)`, `get_edge(id)`, retrieve entities by ID, returning `Node`
  (id, labels, properties) or `Edge` (id, src, dst, type, properties).
- **Versioned lookups**: `get_node_versioned(id, epoch, tx_id)`, retrieve a node visible to a
  specific transaction at a given epoch.
- **Property access**: `get_node_property(id, key)`, batch variants, fast-path access without
  loading the full entity.
- **Traversal**: `neighbors(node, direction)`, `edges_from(node, direction)`, follow adjacency
  in `Outgoing`, `Incoming`, or `Both` directions.
- **Scans**: `node_ids()`, `nodes_by_label(label)`, enumerate nodes, optionally filtered by label.
- **Filtered search**: `find_nodes_by_property(prop, value)`,
  `find_nodes_in_range(prop, min, max)`, use indexes when available.
- **Zone maps**: `node_property_might_match(prop, op, value)`, skip pruning for predicate
  evaluation.
- **Statistics**: `statistics()`, `estimate_label_cardinality(label)`,
  `estimate_avg_degree(edge_type, outgoing)`, feed the cost-based optimizer.

#### GraphStoreMut (Write Path)

The `GraphStoreMut` trait extends `GraphStore` with mutation methods:

- **Create**: `create_node(labels)`, `create_edge(src, dst, type)`, allocate IDs and persist
  records.
- **Delete**: `delete_node(id)`, `delete_edge(id)`, `delete_node_edges(node_id)`, mark entities
  as deleted in the current epoch.
- **Properties**: `set_node_property(id, key, value)`, `remove_node_property(id, key)`, mutate
  property maps.
- **Labels**: `add_label(node_id, label)`, `remove_label(node_id, label)`, modify node labels.
- **Batch**: `batch_create_edges(edges)`, create multiple edges in one operation.

#### Core Types

The following Grafeo types are serialized to/from SlateDB records:

| Type                 | Description                        | Size     |
|----------------------|------------------------------------|----------|
| `NodeId(u64)`        | Node identifier                    | 8 bytes  |
| `EdgeId(u64)`        | Edge identifier                    | 8 bytes  |
| `EpochId(u64)`       | MVCC epoch                         | 8 bytes  |
| `LabelId(u32)`       | Label dictionary ID                | 4 bytes  |
| `EdgeTypeId(u32)`    | Edge type dictionary ID            | 4 bytes  |
| `PropertyKeyId(u32)` | Property key dictionary ID         | 4 bytes  |
| `Value`              | Dynamic property value (see below) | variable |

The `Value` enum represents property values:

```text
Value variants:
├─ Null                         tag 0x00
├─ Bool(bool)                   tag 0x01
├─ Int64(i64)                   tag 0x02
├─ Float64(f64)                 tag 0x03
├─ String(ArcStr)               tag 0x04
├─ Bytes(Arc<[u8]>)             tag 0x05
├─ Timestamp(i64 micros)        tag 0x06
├─ Date(i32 days)               tag 0x07
├─ Time(i64 nanos, Option<i32>) tag 0x08
├─ Duration(months,days,nanos)  tag 0x09
├─ ZonedDatetime(...)           tag 0x0A
├─ List(Arc<[Value]>)           tag 0x0B
├─ Map(Arc<BTreeMap<...>>)      tag 0x0C
├─ Vector(Arc<[f32]>)           tag 0x0D
└─ Path { nodes, edges }        tag 0x0E
```

### Identifiers: External and Internal

Grafeo uses `NodeId(u64)` and `EdgeId(u64)` as internal identifiers. These are system-assigned,
monotonically increasing, and compact, enabling efficient key encoding and bitmap operations.

Users interact with these IDs through query results and parameters. Unlike the vector database,
graph entities do not have separate external/internal ID mappings, the `NodeId`/`EdgeId` values
returned by `create_node`/`create_edge` are the canonical identifiers.

### Block-Based ID Allocation

Node and edge IDs are allocated from monotonically increasing counters using block-based allocation
(reusing the common crate's `SequenceAllocator`). Two independent sequences are maintained: one for
node IDs and one for edge IDs.

**Allocation procedure:**

1. On initialization, read the `SeqBlock` records to get the last allocated ranges
2. Allocate new blocks starting after the previous ranges
3. During normal operation, assign IDs from the current blocks
4. When a block is exhausted, allocate a new block and persist the updated `SeqBlock`

**Recovery:**

On crash recovery, read the `SeqBlock` records and allocate fresh blocks starting after the
previous ranges. Unused IDs from pre-crash blocks are skipped, this creates gaps but preserves
monotonicity.

### Standard Key Prefix

All records use the standard 2-byte prefix per [RFC 0001](../../rfcs/0001-record-key-prefix.md):
a `u8` version byte and a `u8` record tag. The record tag encodes the record type in the high
4 bits, with the low 4 bits reserved (set to `0x0` for now).

```text
record_tag byte layout:
┌────────────┬────────────┐
│  bits 7-4  │  bits 3-0  │
│ record type│  reserved  │
│   (1-15)   │    (0)     │
└────────────┴────────────┘
```

### Common Encodings

This RFC uses the common encodings defined in [RFC 0004](../../rfcs/0004-common-encodings.md):

**Key encodings** (big-endian for lexicographic ordering):

- `TerminatedBytes`: Variable-length byte sequences with escape sequences and `0x00` terminator
- Sortable `i64`: XOR with `0x8000_0000_0000_0000`, big-endian (see `common::serde::sortable`).
  The XOR flips the sign bit so that negative values sort before positive values in unsigned
  lexicographic byte comparison.
- Sortable `f64`: IEEE 754 sign-bit flip encoding, big-endian

**Value encodings** (little-endian):

- `Utf8`: `len: u16` followed by UTF-8 payload
- `Array<T>`: `count: u16` followed by serialized elements

### Record Type Reference

| Tag    | Name               | Description                                             |
|--------|--------------------|---------------------------------------------------------|
| `0x10` | `NodeRecord`       | Node existence, label list, and MVCC epoch              |
| `0x20` | `EdgeRecord`       | Edge endpoints, type, and MVCC epoch                    |
| `0x30` | `NodeProperty`     | Property key-value pair for a node                      |
| `0x40` | `EdgeProperty`     | Property key-value pair for an edge                     |
| `0x50` | `ForwardAdj`       | Outgoing adjacency: src -> (edge_type, dst, edge_id)    |
| `0x60` | `BackwardAdj`      | Incoming adjacency: dst -> (edge_type, src, edge_id)    |
| `0x70` | `LabelIndex`       | Label -> node ID mapping for label scans                |
| `0x80` | `PropertyIndex`    | Sortable property value -> node ID for filtered search  |
| `0x90` | `CatalogLabel`     | Label name <-> LabelId dictionary                       |
| `0x91` | `CatalogEdgeType`  | Edge type name <-> EdgeTypeId dictionary                |
| `0x92` | `CatalogPropKey`   | Property key name <-> PropertyKeyId dictionary          |
| `0xB0` | `ZoneMap`          | Per-property min/max for skip pruning                   |
| `0xE0` | `Metadata`         | Global counters (node count, edge count, current epoch) |
| `0xE1` | `Statistics`       | Cardinality estimates, degree histograms                |
| `0xF0` | `SeqBlock`         | Sequence allocation state for node/edge ID generation   |

## Record Definitions & Schemas

### `NodeRecord` (`0x10`)

Stores the existence of a node, its labels, and the MVCC epoch at which it was created or last
modified. The epoch is part of the key to enable version chains, multiple versions of the same
node coexist in the LSM tree, and the reader selects the version visible at its snapshot epoch.

**Key Layout:**

```text
┌─────────┬─────────────┬──────────┬──────────┐
│ version │ record_tag  │ node_id  │  epoch   │
│ 1 byte  │   1 byte    │ 8 bytes  │ 8 bytes  │
└─────────┴─────────────┴──────────┴──────────┘
```

- `version` (u8): Key format version (currently `0x01`)
- `record_tag` (u8): `0x10`
- `node_id` (u64): Big-endian node identifier
- `epoch` (u64): Big-endian MVCC epoch (higher = newer)

**Value Schema:**

```text
┌────────────────────────────────────────────────────────┐
│                    NodeRecordValue                      │
├────────────────────────────────────────────────────────┤
│  flags:       u8                                       │
│  label_count: u16 (LE)                                 │
│  label_ids:   FixedElementArray<u32 LE>                │
│               (label_count elements)                   │
└────────────────────────────────────────────────────────┘
```

**Flags:**

| Bit | Name      | Description                              |
|-----|-----------|------------------------------------------|
| 0   | `DELETED` | Node was deleted at this epoch           |
| 1-7 | reserved  | Must be 0                                |

**Structure:**

- When `DELETED` is set, the node was deleted at this epoch. `label_count` is 0 and `label_ids`
  is empty. Readers seeing this flag treat the node as non-existent.
- `label_ids` are catalog-assigned `LabelId` values. Label names are resolved via the catalog.
- A node with no labels is valid (`label_count = 0`).
- The epoch in the key enables MVCC: to read at epoch E, scan for the node_id prefix and select
  the record with the highest epoch ≤ E that is not a deletion marker.

**Point Lookup:**

To retrieve a node by ID at epoch E:

1. Seek to `[0x01, 0x10, node_id_be, 0x00..0x00]`
2. Scan forward through versions until epoch > E
3. Return the last version with epoch ≤ E
4. If that version has `DELETED` flag, return `None`

### `EdgeRecord` (`0x20`)

Stores the existence of an edge, its source and destination nodes, edge type, and MVCC epoch.

**Key Layout:**

```text
┌─────────┬─────────────┬──────────┬──────────┐
│ version │ record_tag  │ edge_id  │  epoch   │
│ 1 byte  │   1 byte    │ 8 bytes  │ 8 bytes  │
└─────────┴─────────────┴──────────┴──────────┘
```

- `edge_id` (u64): Big-endian edge identifier
- `epoch` (u64): Big-endian MVCC epoch

**Value Schema:**

```text
┌────────────────────────────────────────────────────────┐
│                    EdgeRecordValue                      │
├────────────────────────────────────────────────────────┤
│  flags:        u8                                      │
│  src_node_id:  u64 (LE)                                │
│  dst_node_id:  u64 (LE)                                │
│  edge_type_id: u32 (LE)                                │
└────────────────────────────────────────────────────────┘
```

**Flags:** Same as `NodeRecord` (bit 0 = `DELETED`).

**Structure:**

- `src_node_id` and `dst_node_id` are the endpoints of the directed edge.
- `edge_type_id` is a catalog-assigned `EdgeTypeId`. The type name is resolved via the catalog.
- Point lookup follows the same MVCC pattern as `NodeRecord`.

### `NodeProperty` (`0x30`)

Stores a single property key-value pair for a node. Properties are stored as individual records
rather than as a single serialized map. This design enables:

- **Projection pushdown**: Reading only the requested properties without deserializing a full map.
- **Selective mutation**: Setting or removing a single property without read-modify-write on the
  full property set.
- **Batch property access**: The `get_nodes_properties_selective_batch` trait method benefits from
  targeted key lookups.

Properties use last-write-wins semantics, there is no epoch in the property key. The property
reflects the state after the most recent `set_node_property` or `remove_node_property` call.

**Key Layout:**

```text
┌─────────┬─────────────┬──────────┬─────────────────┐
│ version │ record_tag  │ node_id  │ property_key_id │
│ 1 byte  │   1 byte    │ 8 bytes  │    4 bytes      │
└─────────┴─────────────┴──────────┴─────────────────┘
```

- `node_id` (u64): Big-endian node identifier
- `property_key_id` (u32): Big-endian catalog-assigned `PropertyKeyId`

**Value Schema:**

```text
┌────────────────────────────────────────────────────────┐
│                  NodePropertyValue                      │
├────────────────────────────────────────────────────────┤
│  value: TaggedValue                                    │
└────────────────────────────────────────────────────────┘
```

Where `TaggedValue` is defined in the "Value Serialization" section below.

**Deletion:**

Removing a property is done by issuing a SlateDB tombstone for the key. After compaction, the
record disappears entirely.

**All Properties for a Node:**

To load all properties for a node (e.g., for `get_node`), perform a prefix scan:
`[0x01, 0x30, node_id_be]`, this returns all property records for that node, each keyed by
`property_key_id`. Resolve property names via the catalog.

### `EdgeProperty` (`0x40`)

Identical layout to `NodeProperty`, but for edges.

**Key Layout:**

```text
┌─────────┬─────────────┬──────────┬─────────────────┐
│ version │ record_tag  │ edge_id  │ property_key_id │
│ 1 byte  │   1 byte    │ 8 bytes  │    4 bytes      │
└─────────┴─────────────┴──────────┴─────────────────┘
```

**Value Schema:** Same as `NodePropertyValue`.

### `ForwardAdj` (`0x50`)

Stores outgoing adjacency: for a given source node, records each outgoing edge with its type and
destination. Adjacency is stored as **individual keys** rather than a single serialized list. This
avoids read-modify-write amplification for high-degree nodes, adding or removing an edge is a
single `put` or `delete` without touching other adjacency entries.

**Key Layout:**

```text
┌─────────┬─────────────┬──────────────┬───────────────┬──────────────┬──────────┐
│ version │ record_tag  │ src_node_id  │ edge_type_id  │ dst_node_id  │ edge_id  │
│ 1 byte  │   1 byte    │   8 bytes    │   4 bytes     │   8 bytes    │ 8 bytes  │
└─────────┴─────────────┴──────────────┴───────────────┴──────────────┴──────────┘
```

- `src_node_id` (u64): Big-endian source node identifier
- `edge_type_id` (u32): Big-endian edge type identifier
- `dst_node_id` (u64): Big-endian destination node identifier
- `edge_id` (u64): Big-endian edge identifier

**Value Schema:** Empty (all information is in the key).

**Structure:**

- The key is designed for efficient prefix scans:
  - `[prefix, src_node_id]`, all outgoing edges from a node (implements `neighbors(node, Outgoing)`)
  - `[prefix, src_node_id, edge_type_id]`, outgoing edges of a specific type (type-filtered
    traversal)
  - `[prefix, src_node_id, edge_type_id, dst_node_id]`, check if a specific edge exists
- Edge type is placed before destination to enable type-filtered traversal without loading all
  edge types.
- The `edge_id` suffix ensures uniqueness when multiple edges of the same type connect the same
  pair of nodes (multi-edges).

**Traversal:**

To implement `edges_from(node, Outgoing)`:

1. Prefix scan `[0x01, 0x50, node_id_be]`
2. Each key encodes `(edge_type_id, dst_node_id, edge_id)`
3. Return `Vec<(NodeId, EdgeId)>` pairs

To implement `out_degree(node)`:

1. Prefix scan `[0x01, 0x50, node_id_be]`
2. Count keys

### `BackwardAdj` (`0x60`)

Stores incoming adjacency: for a given destination node, records each incoming edge with its type
and source. This enables efficient `Incoming` and `Both` direction traversal.

**Key Layout:**

```text
┌─────────┬─────────────┬──────────────┬───────────────┬──────────────┬──────────┐
│ version │ record_tag  │ dst_node_id  │ edge_type_id  │ src_node_id  │ edge_id  │
│ 1 byte  │   1 byte    │   8 bytes    │   4 bytes     │   8 bytes    │ 8 bytes  │
└─────────┴─────────────┴──────────────┴───────────────┴──────────────┴──────────┘
```

Mirror of `ForwardAdj` with source and destination swapped.

**Value Schema:** Empty.

**Configuration:**

Backward adjacency is always maintained. While Grafeo allows disabling backward edges via
`Config::backward_edges`, the SlateDB implementation always writes backward adjacency entries.

**Cost:** Each backward adjacency key is 30 bytes (2-byte prefix + 8-byte dst + 4-byte type +
8-byte src + 8-byte edge_id) with an empty value. For a graph with N edges, this adds N keys
totaling ~30N bytes of key data. For example, a 100M-edge graph adds ~3 GB of backward adjacency
keys and doubles the write amplification per edge creation (one extra PUT per edge).

**Justification:** The benefit is that `Incoming` and `Both` direction traversal are first-class
operations without full-graph scans. Graph workloads frequently require bidirectional traversal
(e.g., "who follows this user?" or "what depends on this service?"), and without backward adjacency,
answering these queries requires scanning all ForwardAdj records. The per-edge storage overhead is
small relative to the total record set (each edge already produces an EdgeRecord + ForwardAdj +
potential property records).

### `LabelIndex` (`0x70`)

Maps a label to the set of nodes that carry it. Stored as individual keys (one per label-node
pair) to avoid read-modify-write on high-cardinality labels.

**Key Layout:**

```text
┌─────────┬─────────────┬───────────┬──────────┐
│ version │ record_tag  │ label_id  │ node_id  │
│ 1 byte  │   1 byte    │  4 bytes  │ 8 bytes  │
└─────────┴─────────────┴───────────┴──────────┘
```

- `label_id` (u32): Big-endian catalog-assigned `LabelId`
- `node_id` (u64): Big-endian node identifier

**Value Schema:** Empty.

**Label Scan:**

To implement `nodes_by_label(label)`:

1. Resolve `label` → `LabelId` via catalog
2. Prefix scan `[0x01, 0x70, label_id_be]`
3. Each key suffix is a `node_id`
4. Return `Vec<NodeId>`

**Cardinality Estimation:**

To implement `estimate_label_cardinality(label)`, the `Metadata` record stores per-label node
counts maintained by merge operators (see Metadata section).

### `PropertyIndex` (`0x80`)

An optional index mapping property values to node IDs. Enables `find_nodes_by_property`,
`find_nodes_by_properties`, and `find_nodes_in_range` without full scans.

Property indexes are created explicitly (via catalog index definitions). Only indexed properties
have `PropertyIndex` entries.

**Key Layout:**

```text
┌─────────┬─────────────┬─────────────────┬───────────────────┬──────────┐
│ version │ record_tag  │ property_key_id │    value_term     │ node_id  │
│ 1 byte  │   1 byte    │    4 bytes      │ SortableValue     │ 8 bytes  │
└─────────┴─────────────┴─────────────────┴───────────────────┴──────────┘
```

- `property_key_id` (u32): Big-endian `PropertyKeyId`
- `value_term`: Sortable encoding of the property value (see below)
- `node_id` (u64): Big-endian node identifier

**SortableValue Encoding:**

```text
┌──────────────────────────────────────────────────┐
│ tag: u8                                          │
│ payload:                                         │
│   Bool    (0x01) → u8 (0 or 1)                  │
│   Int64   (0x02) → sortable i64 (8 bytes BE)    │
│   Float64 (0x03) → sortable f64 (8 bytes BE)    │
│   String  (0x04) → TerminatedBytes              │
└──────────────────────────────────────────────────┘
```

The leading tag byte ensures values of different types sort into separate ranges. Within each type,
the encoding preserves numeric/lexicographic ordering.

**Value Schema:** Empty.

**Equality Lookup:**

To implement `find_nodes_by_property(property, value)`:

1. Resolve `property` → `PropertyKeyId` via catalog
2. Encode `value` as `SortableValue`
3. Prefix scan `[0x01, 0x80, property_key_id_be, sortable_value]`
4. Each key suffix is a `node_id`

**Range Scan:**

To implement `find_nodes_in_range(property, min, max, min_inclusive, max_inclusive)`:

1. Resolve `property` → `PropertyKeyId`
2. Encode `min` and `max` as `SortableValue`
3. Range scan from `[prefix, property_key_id, min_encoded]` to `[prefix, property_key_id, max_encoded]`
4. Apply inclusivity bounds

### Catalog Records (`0x90` – `0x92`)

The catalog maps human-readable names to compact integer IDs for labels, edge types, and property
keys. On startup, all catalog records are loaded into an in-memory cache. Writes update both the
cache and SlateDB atomically.

Three sub-types share the `0x9_` prefix using the low nibble for discrimination.

#### `CatalogLabel` (`0x90`)

**Key Layout:**

```text
┌─────────┬─────────────┬────────────────────┐
│ version │ record_tag  │    label_name      │
│ 1 byte  │   1 byte    │  TerminatedBytes   │
└─────────┴─────────────┴────────────────────┘
```

**Value Schema:**

```text
┌────────────────────────────────────────────────┐
│  label_id: u32 (LE)                            │
└────────────────────────────────────────────────┘
```

#### `CatalogEdgeType` (`0x91`)

**Key Layout:**

```text
┌─────────┬─────────────┬────────────────────┐
│ version │ record_tag  │  edge_type_name    │
│ 1 byte  │   1 byte    │  TerminatedBytes   │
└─────────┴─────────────┴────────────────────┘
```

**Value Schema:**

```text
┌────────────────────────────────────────────────┐
│  edge_type_id: u32 (LE)                        │
└────────────────────────────────────────────────┘
```

#### `CatalogPropKey` (`0x92`)

**Key Layout:**

```text
┌─────────┬─────────────┬────────────────────────┐
│ version │ record_tag  │  property_key_name     │
│ 1 byte  │   1 byte    │   TerminatedBytes      │
└─────────┴─────────────┴────────────────────────┘
```

**Value Schema:**

```text
┌────────────────────────────────────────────────┐
│  property_key_id: u32 (LE)                     │
└────────────────────────────────────────────────┘
```

**Catalog Loading:**

On startup, prefix scan `[0x01, 0x90]`, `[0x01, 0x91]`, and `[0x01, 0x92]` to populate the
in-memory bidirectional maps (name → ID and ID → name). The catalog is expected to be small
(thousands of entries at most), so full loading is practical.

**ID Assignment:**

New catalog entries use incrementing IDs starting from 0. The next available ID for each catalog
type is derived from the maximum ID seen during loading + 1. This assumes single-writer semantics:
only one process writes to the catalog at a time. Concurrent writers loading the catalog
independently could assign duplicate IDs. This is consistent with OpenData's current single-writer
model per database instance.

### `ZoneMap` (`0xB0`)

Stores per-property min/max values for skip pruning. The query engine calls
`node_property_might_match(property, op, value)` before scanning, if the zone map proves no
matches exist, the scan is skipped entirely.

**Key Layout:**

```text
┌─────────┬─────────────┬─────────────────┐
│ version │ record_tag  │ property_key_id │
│ 1 byte  │   1 byte    │    4 bytes      │
└─────────┴─────────────┴─────────────────┘
```

**Value Schema:**

```text
┌────────────────────────────────────────────────────────┐
│                     ZoneMapValue                        │
├────────────────────────────────────────────────────────┤
│  value_type: u8  (type tag from Value enum)            │
│  min:        TaggedValue                               │
│  max:        TaggedValue                               │
│  count:      u64 (LE), number of non-null values      │
└────────────────────────────────────────────────────────┘
```

**Merge Operator:**

Zone maps use SlateDB merge operators for atomic updates. When a property is set, a merge
operation extends the min/max range if the new value falls outside it and increments the count.
The merge function:

1. For each operand, decode `(value_type, min, max, count)`
2. Output: `min = min(all mins)`, `max = max(all maxes)`, `count = sum(all counts)`

This avoids read-modify-write on every property mutation.

### `Metadata` (`0xE0`)

Stores global counters and the current MVCC epoch. These are singleton records with fixed
well-known keys.

**Key Layout:**

```text
┌─────────┬─────────────┬──────────────┐
│ version │ record_tag  │ metadata_key │
│ 1 byte  │   1 byte    │    1 byte    │
└─────────┴─────────────┴──────────────┘
```

**Metadata Keys:**

| Key    | Name           | Value Type | Description                       |
|--------|----------------|------------|-----------------------------------|
| `0x01` | `NodeCount`    | u64 (LE)   | Total live (non-deleted) nodes    |
| `0x02` | `EdgeCount`    | u64 (LE)   | Total live (non-deleted) edges    |
| `0x03` | `Epoch`        | u64 (LE)   | Current global MVCC epoch         |
| `0x04` | `LabelCount`   | varies     | Per-label node counts (see below) |

**Merge Operator:**

`NodeCount` and `EdgeCount` use merge operators for atomic increment/decrement. Each mutation
(create or delete) issues a merge with a signed delta (`+1` or `-1`). The merge function sums
all deltas.

`Epoch` is updated via a simple `put` (not merge) when the epoch advances.

### `Statistics` (`0xE1`)

Stores pre-computed statistics for the cost-based optimizer. Updated periodically (not on every
mutation).

**Key Layout:**

```text
┌─────────┬─────────────┐
│ version │ record_tag  │
│ 1 byte  │   1 byte    │
└─────────┴─────────────┘
```

**Value Schema:**

```text
┌────────────────────────────────────────────────────────────────────┐
│                      StatisticsValue                               │
├────────────────────────────────────────────────────────────────────┤
│  version:          u32 (LE)                                        │
│  total_nodes:      u64 (LE)                                        │
│  total_edges:      u64 (LE)                                        │
│  label_stats:      Array<LabelStat>                                │
│  edge_type_stats:  Array<EdgeTypeStat>                             │
│                                                                    │
│  LabelStat                                                         │
│  ┌──────────────────────────────────────────────────────────────┐  │
│  │  label_id:   u32 (LE)                                        │  │
│  │  count:      u64 (LE)                                        │  │
│  └──────────────────────────────────────────────────────────────┘  │
│                                                                    │
│  EdgeTypeStat                                                      │
│  ┌──────────────────────────────────────────────────────────────┐  │
│  │  edge_type_id:  u32 (LE)                                     │  │
│  │  count:         u64 (LE)                                      │  │
│  │  avg_out_deg:   f64 (LE)                                      │  │
│  │  avg_in_deg:    f64 (LE)                                      │  │
│  └──────────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────┘
```

Statistics are recomputed periodically (e.g., every N commits) and persisted as a single record.
On startup, the statistics record is loaded and exposed via `GraphStore::statistics()`.

**Design assumption:** The statistics record stores all label and edge type statistics in a single
blob. This assumes a small to moderate schema (hundreds of labels and edge types, not millions).
Each refresh rewrites the entire record. If schema cardinality grows large, a future optimization
could split statistics into per-label and per-edge-type records (using the catalog ID as a key
suffix) to allow incremental updates.

### `SeqBlock` (`0xF0`)

Stores sequence allocation state for node and edge ID generation. Two `SeqBlock` records exist:
one for node IDs and one for edge IDs.

**Key Layout:**

```text
┌─────────┬─────────────┬──────────┐
│ version │ record_tag  │ seq_type │
│ 1 byte  │   1 byte    │  1 byte  │
└─────────┴─────────────┴──────────┘
```

- `seq_type` (u8): `0x01` for node IDs, `0x02` for edge IDs

**Value Schema:**

```text
┌────────────────────────────────────────────────────────┐
│                     SeqBlockValue                       │
├────────────────────────────────────────────────────────┤
│  base_sequence:  u64 (LE), start of allocated block   │
│  block_size:     u64 (LE), number of IDs in block     │
└────────────────────────────────────────────────────────┘
```

Follows the same allocation pattern as the common crate's `SequenceAllocator`.

### Value Serialization (`TaggedValue`)

Property values are serialized as a type tag followed by a type-specific payload. This format
is used in `NodeProperty`/`EdgeProperty` values and `ZoneMap` min/max fields.

```text
┌──────────────────────────────────────────────────────────────────────┐
│                          TaggedValue                                  │
├──────────────────────────────────────────────────────────────────────┤
│  tag: u8  (Value variant discriminant)                               │
│  payload: (depends on tag)                                           │
│                                                                      │
│  0x00 Null       → (no payload)                                      │
│  0x01 Bool       → u8 (0 = false, 1 = true)                         │
│  0x02 Int64      → i64 (LE)                                         │
│  0x03 Float64    → f64 (LE)                                         │
│  0x04 String     → Utf8                                              │
│  0x05 Bytes      → len: u32 (LE), payload: [u8; len]                │
│  0x06 Timestamp  → i64 (LE) microseconds since Unix epoch           │
│  0x07 Date       → i32 (LE) days since Unix epoch                   │
│  0x08 Time       → i64 (LE) nanoseconds since midnight,             │
│                    has_offset: u8, offset: i32 (LE) seconds          │
│  0x09 Duration   → months: i32 (LE), days: i32 (LE),               │
│                    nanos: i64 (LE)                                   │
│  0x0A ZonedDT    → micros: i64 (LE), offset_secs: i32 (LE)         │
│  0x0B List       → count: u32 (LE), elements: [TaggedValue; count]  │
│  0x0C Map        → count: u32 (LE),                                 │
│                    entries: [(Utf8 key, TaggedValue value); count]   │
│  0x0D Vector     → dims: u32 (LE), elements: [f32 LE; dims]        │
│  0x0E Path       → node_count: u32 (LE), nodes: [TaggedValue],     │
│                    edge_count: u32 (LE), edges: [TaggedValue]       │
└──────────────────────────────────────────────────────────────────────┘
```

### MVCC Model

The graph database uses epoch-based MVCC for snapshot isolation. Each write operation (create
node, delete edge, etc.) is associated with an epoch. Readers observe a consistent snapshot at
a specific epoch.

**Epoch Lifecycle:**

1. The global epoch starts at 1 and is stored in `Metadata(Epoch)`.
2. Each write transaction advances the epoch by 1 on commit.
3. Readers capture the current epoch at transaction start and use it for all reads.
4. Entity records (`NodeRecord`, `EdgeRecord`) include the epoch in their key.
5. Property records (`NodeProperty`, `EdgeProperty`) do **not** include the epoch, they use
   last-write-wins semantics. A property reflects the state after the most recent mutation.

**Visibility Rule:**

A node version at epoch E_v is visible to a reader at epoch E_r if:

- E_v ≤ E_r (the version was committed before or at the reader's snapshot)
- No deletion marker exists at any epoch E_d where E_v < E_d ≤ E_r

For point lookups, this means scanning the version chain (ordered by epoch descending) and
returning the first non-deleted version with epoch ≤ E_r.

**Properties and MVCC:**

Properties intentionally do not participate in MVCC versioning. This is a deliberate simplification
for the initial implementation:

- **Benefit**: Simpler storage (no version chains for properties), fewer keys, faster property
  access.
- **Tradeoff**: A reader at epoch E may see property values written after E. This is a **known
  correctness gap**, not merely a performance tradeoff. For example: a reader at epoch 5 could
  observe a property value written at epoch 6, causing `WHERE n.status = 'active'` to return
  incorrect results if the value was changed between epochs. Queries that depend on historical
  property consistency should not rely on epoch-based snapshots for property reads.
- **Mitigation**: In practice, most graph query workloads read the latest property values (e.g.,
  recommendation engines, dependency mapping). Analytical queries over historical property state
  are uncommon.
- **Future**: Full property MVCC can be added by including the epoch in property keys, following
  the same pattern as entity records. The current key layout is forward-compatible with this
  extension.

**Garbage Collection:**

Old versions accumulate in the LSM tree. A background garbage collector (details in a future RFC)
periodically removes versions older than the oldest active reader's epoch. Until GC runs,
old versions are harmless, they consume space but don't affect correctness.

### Write Path

Creating a node with labels and properties involves the following SlateDB operations:

```text
create_node_with_props(["Person", "Employee"], [("name", "Alice"), ("age", 30)]):

1. Allocate node_id from SequenceAllocator
2. Resolve/create label IDs: "Person" → LabelId(0), "Employee" → LabelId(1)
3. Resolve/create property key IDs: "name" → PropKeyId(0), "age" → PropKeyId(1)
4. Current epoch = E

WriteBatch:
  PUT [0x01, 0x10, node_id, E]           → NodeRecordValue { flags: 0, labels: [0, 1] }
  PUT [0x01, 0x30, node_id, PropKeyId(0)] → TaggedValue(String, "Alice")
  PUT [0x01, 0x30, node_id, PropKeyId(1)] → TaggedValue(Int64, 30)
  PUT [0x01, 0x70, LabelId(0), node_id]  → (empty)
  PUT [0x01, 0x70, LabelId(1), node_id]  → (empty)
  MERGE [0x01, 0xE0, 0x01]               → +1  (NodeCount)
  MERGE [0x01, 0xB0, PropKeyId(1)]       → ZoneMap extend(30)
```

Creating an edge:

```text
create_edge(src=NodeId(1), dst=NodeId(2), "KNOWS"):

1. Allocate edge_id from SequenceAllocator
2. Resolve/create edge type: "KNOWS" → EdgeTypeId(0)
3. Current epoch = E

WriteBatch:
  PUT [0x01, 0x20, edge_id, E]                                → EdgeRecordValue { src: 1, dst: 2, type: 0 }
  PUT [0x01, 0x50, NodeId(1), EdgeTypeId(0), NodeId(2), edge_id] → (empty)
  PUT [0x01, 0x60, NodeId(2), EdgeTypeId(0), NodeId(1), edge_id] → (empty)
  MERGE [0x01, 0xE0, 0x02]                                    → +1  (EdgeCount)
```

### Read Path: Assembling a Full Node

Grafeo's `get_node(id)` returns `Node { id, labels, properties }`. Assembling this from SlateDB:

```text
get_node(NodeId(42)) at epoch E:

1. Seek [0x01, 0x10, 42_be, 0x00..0x00]
2. Scan versions → find latest with epoch ≤ E, not DELETED
3. Decode NodeRecordValue → label_ids = [0, 1]
4. Resolve labels via catalog: LabelId(0) → "Person", LabelId(1) → "Employee"
5. Prefix scan [0x01, 0x30, 42_be] → all properties for node 42
6. Decode each TaggedValue, resolve property key names via catalog
7. Return Node { id: 42, labels: ["Person", "Employee"], properties: { "name": "Alice", "age": 30 } }
```

**Performance note:** Grafeo's `Node` struct includes a full `PropertyMap`, so `get_node` always
loads all properties via a prefix scan. This makes `get_node` more expensive than a simple point
lookup: it requires one seek for the NodeRecord plus one prefix scan for all NodeProperty records.
Grafeo's query engine avoids this cost during execution by using targeted property access methods
(`get_node_property`, `get_nodes_properties_selective_batch`) with projection pushdown, only
loading the properties referenced in the query's `RETURN` or `WHERE` clauses. The full `get_node`
path is primarily used for final result materialization when all properties are requested.

### Trait Method Mapping

The following table maps Grafeo `GraphStore` methods to SlateDB operations:

| GraphStore Method                                      | SlateDB Operation                                                                        |
|--------------------------------------------------------|------------------------------------------------------------------------------------------|
| `get_node(id)`                                         | Seek `NodeRecord` + prefix scan `NodeProperty`                                           |
| `get_edge(id)`                                         | Seek `EdgeRecord` + prefix scan `EdgeProperty`                                           |
| `get_node_versioned(id, epoch, tx)`                    | Seek `NodeRecord`, filter by epoch                                                       |
| `get_node_property(id, key)`                           | Point get `NodeProperty(id, key_id)`                                                     |
| `get_node_property_batch(ids, key)`                    | Multi-get `NodeProperty`                                                                 |
| `get_nodes_properties_selective_batch(ids, keys)`      | Multi-get `NodeProperty` for each (id, key)                                              |
| `neighbors(node, dir)`                                 | Prefix scan `ForwardAdj` and/or `BackwardAdj`                                            |
| `edges_from(node, dir)`                                | Prefix scan `ForwardAdj` and/or `BackwardAdj`                                            |
| `out_degree(node)`                                     | Count keys in `ForwardAdj` prefix scan                                                   |
| `in_degree(node)`                                      | Count keys in `BackwardAdj` prefix scan                                                  |
| `node_ids()`                                           | Full scan of `NodeRecord`, filter non-deleted (\*)                                       |
| `nodes_by_label(label)`                                | Prefix scan `LabelIndex(label_id)`                                                       |
| `node_count()`                                         | Read `Metadata(NodeCount)`                                                               |
| `edge_count()`                                         | Read `Metadata(EdgeCount)`                                                               |
| `find_nodes_by_property(p, v)`                         | Prefix scan `PropertyIndex(p_id, sortable_v)`                                            |
| `find_nodes_in_range(p, min, max)`                     | Range scan `PropertyIndex(p_id, min..max)`                                               |
| `node_property_might_match(p, op, v)`                  | Read `ZoneMap(p_id)`, check min/max                                                      |
| `statistics()`                                         | Return cached `Statistics` (loaded on startup)                                           |
| `estimate_label_cardinality(l)`                        | Lookup in cached `Statistics`                                                            |
| `current_epoch()`                                      | Return in-memory epoch counter                                                           |

| GraphStoreMut Method                                   | SlateDB Operation                                                                        |
|--------------------------------------------------------|------------------------------------------------------------------------------------------|
| `create_node(labels)`                                  | WriteBatch: `NodeRecord` + `LabelIndex` entries + merge `NodeCount`                      |
| `create_edge(src, dst, type)`                          | WriteBatch: `EdgeRecord` + `ForwardAdj` + `BackwardAdj` + merge `EdgeCount`              |
| `delete_node(id)`                                      | WriteBatch: `NodeRecord(DELETED)` + tombstone labels/properties/adj + merge `NodeCount`  |
| `delete_edge(id)`                                      | WriteBatch: `EdgeRecord(DELETED)` + tombstone adj entries + merge `EdgeCount`            |
| `set_node_property(id, k, v)`                          | Put `NodeProperty(id, k_id)` + merge `ZoneMap` + update `PropertyIndex`                  |
| `remove_node_property(id, k)`                          | Tombstone `NodeProperty(id, k_id)` + tombstone `PropertyIndex`                           |
| `add_label(node, l)`                                   | Put `LabelIndex(l_id, node)` + update `NodeRecord`                                       |
| `remove_label(node, l)`                                | Tombstone `LabelIndex(l_id, node)` + update `NodeRecord`                                 |
| `batch_create_edges(edges)`                            | Single WriteBatch with all edge records and adjacency entries                            |

(\*) `node_ids()` performs a full scan of all `NodeRecord` entries, which is expensive for large
graphs (O(N) where N is total node versions). The query engine avoids this path when possible by
using `nodes_by_label` or property-filtered searches. `node_ids()` is primarily used for
unfiltered `MATCH (n)` patterns without label or property predicates.

## Alternatives

### Bundled Property Map (Single Key per Entity)

An alternative stores all properties for an entity as a single serialized map:

```text
Key:   [0x01, 0x30, node_id]
Value: Map { "name": "Alice", "age": 30, "email": "alice@example.com" }
```

**Rejected because:**

1. **Read amplification**, Reading one property requires deserializing the entire map.
   `get_node_property(id, "name")` must load and decode all properties.
2. **Write amplification**, Setting one property requires read-modify-write of the entire map.
   For nodes with many properties or large values, this is expensive.
3. **Projection pushdown**, Grafeo's selective batch methods (`get_nodes_properties_selective_batch`)
   become less effective when the smallest unit of storage is the full property map.

The per-property approach trades key count for read/write efficiency on individual properties.

### Adjacency Lists as Serialized Arrays

An alternative stores all edges from a node as a single serialized array:

```text
Key:   [0x01, 0x50, src_node_id]
Value: [(edge_type_id, dst_node_id, edge_id), ...]
```

**Rejected because:**

1. **Read-modify-write**: Adding or removing one edge requires loading, deserializing, modifying,
   re-serializing, and writing the entire list. For high-degree nodes (thousands of edges), this
   is prohibitively expensive.
2. **No type-filtered prefix scans**: The per-key layout enables `[prefix, src, edge_type]` scans
   for type-filtered traversal. A single array requires filtering after deserialization.
3. **Concurrent writes**: Multiple concurrent edge creations on the same node conflict on the
   same key. Individual keys allow concurrent writes without conflicts.

### Epoch in Property Keys (Full Property MVCC)

An alternative includes the epoch in property keys for full MVCC:

```text
Key: [0x01, 0x30, node_id, property_key_id, epoch]
```

**Deferred because:**

1. **Complexity**: Version chains for properties multiply storage requirements and complicate
   garbage collection.
2. **Marginal benefit**: Most graph queries read the latest property values. Historical property
   queries are rare.
3. **Future-compatible**: The current design can be upgraded to full property MVCC by adding the
   epoch suffix, without changing the record tag allocation.

### Bitmap-Based Label Index

An alternative uses RoaringBitmaps per label (similar to the vector database's metadata index):

```text
Key:   [0x01, 0x70, label_id]
Value: RoaringTreemap of node IDs
```

**Deferred because:**

1. **Read-modify-write**: Every node creation/deletion requires loading and mutating the bitmap.
   Merge operators mitigate this but add complexity.
2. **Individual keys scale better for writes**: Graph workloads with high write throughput benefit
   from append-only individual keys.
3. **Can be added later**: A bitmap-based index can coexist with the per-key index as a
   compaction-time optimization.

## Open Questions

1. **Property index scope**: Should property indexes also cover edge properties, or only node
   properties? Grafeo's trait only exposes `find_nodes_by_property`, but edge property indexes
   could benefit future query patterns.

2. **Statistics refresh strategy**: How often should statistics be recomputed? Options include:
   every N commits, on explicit request, or adaptive (when cardinality drift exceeds a threshold).

3. **Multi-graph support**: Grafeo supports named graphs (independent graph partitions within a
   database). Should the storage layout include a graph namespace prefix in keys, or should each
   named graph map to a separate SlateDB instance?

4. ~~**Write batching granularity**~~: **Resolved.** Each mutation method (e.g.,
   `create_node_with_props`, `create_edge`) issues a single atomic `WriteBatch` containing the
   entity record, all associated index entries (label, adjacency, property index), and counter
   merge operations. This ensures that a node and its indexes are always consistent: there is no
   window where a node exists but its label index entry does not, or vice versa. This matches the
   pattern used by the vector and timeseries engines, where a single `storage.apply(ops)` call
   persists all records for one logical operation atomically.

## Future Considerations

Future RFCs will address:

- **HTTP API**: Read and write endpoints following the patterns established by timeseries and log.
- **GQL query endpoint**: Mapping GQL query text to Grafeo engine execution and streaming results.
- **Graph algorithms**: Integrating Grafeo's `algos` feature (PageRank, shortest path,
  community detection, etc.) as server-side procedures.
- **Change data capture**: Exposing graph mutations as a stream for downstream consumers.
- **Compaction and GC**: Policies for removing old MVCC versions and cleaning up tombstoned
  adjacency/index entries.
- **Write coordination**: Integration with the cross-project write coordination RFC for
  multi-writer scenarios.

## References

1. **Grafeo**, Graph database engine providing query parsing, optimization, and execution.
   [GitHub](https://github.com/GrafeoDB/grafeo)

2. **SlateDB**, Cloud-native LSM-tree storage engine built on object storage.
   [GitHub](https://github.com/slatedb/slatedb)

3. **GQL (ISO/IEC 39075:2024)**, The Graph Query Language standard.
   [ISO](https://www.iso.org/standard/76120.html)

4. **OpenData RFC 0001: Record Key Prefix**, Standard 2-byte key prefix format.
   [RFC](../../rfcs/0001-record-key-prefix.md)

5. **OpenData RFC 0004: Common Encodings**, Shared encoding primitives (TerminatedBytes, Utf8, etc.).
   [RFC](../../rfcs/0004-common-encodings.md)

## Updates

| Date       | Description   |
|------------|---------------|
| 2026-03-05 | Initial draft |
