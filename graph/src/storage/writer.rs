use std::sync::atomic::Ordering;

use bytes::Bytes;
use grafeo_common::types::{EdgeId, EpochId, NodeId, PropertyKey, TransactionId, Value};
use grafeo_core::graph::traits::{GraphStore, GraphStoreMut};

use super::GraphStorage;
use crate::config::StorageLayout;
use crate::serde::MetadataSubType;
use crate::serde::keys::*;
use crate::serde::values::{self, EdgeRecordValue, MergedAdjValue, MergedPropsValue, NodeRecordValue};
use common::storage::{MergeRecordOp, PutRecordOp, Record, RecordOp};

fn put_record(key: Bytes, value: Bytes) -> RecordOp {
    RecordOp::Put(PutRecordOp::from(Record::new(key, value)))
}

fn merge_record(key: Bytes, value: Bytes) -> RecordOp {
    RecordOp::Merge(MergeRecordOp::from(Record::new(key, value)))
}

fn counter_merge(sub_type: MetadataSubType, delta: i64) -> RecordOp {
    RecordOp::Merge(MergeRecordOp::from(Record::new(
        MetadataKey { sub_type }.encode(),
        super::encode_i64_le(delta),
    )))
}

/// Pushes forward + backward adjacency ops for a single edge.
fn push_adj_ops(
    ops: &mut Vec<RecordOp>,
    layout: StorageLayout,
    src: u64,
    dst: u64,
    type_id: u32,
    edge_id: u64,
) {
    match layout {
        StorageLayout::Individual => {
            ops.push(put_record(
                ForwardAdjKey { src, edge_type_id: type_id, dst, edge_id }.encode(),
                Bytes::new(),
            ));
            ops.push(put_record(
                BackwardAdjKey { dst, edge_type_id: type_id, src, edge_id }.encode(),
                Bytes::new(),
            ));
        }
        StorageLayout::Merged => {
            ops.push(merge_record(
                MergedForwardAdjKey { src, edge_type_id: type_id }.encode(),
                values::encode_adj_add_operand(dst, edge_id),
            ));
            ops.push(merge_record(
                MergedBackwardAdjKey { dst, edge_type_id: type_id }.encode(),
                values::encode_adj_add_operand(src, edge_id),
            ));
        }
    }
}

impl GraphStoreMut for GraphStorage {
    fn create_node(&self, labels: &[&str]) -> NodeId {
        let (node_id, seq_record) = {
            let mut seq = self.node_seq.lock().unwrap();
            seq.allocate_one()
        };

        let mut ops: Vec<RecordOp> = Vec::new();

        if let Some(record) = seq_record {
            ops.push(RecordOp::Put(PutRecordOp::from(record)));
        }

        let mut label_ids = Vec::with_capacity(labels.len());
        {
            let mut catalog = self.catalog.write();
            for label in labels {
                let (label_id, catalog_ops) = catalog.get_or_create_label(label);
                ops.extend(catalog_ops);
                label_ids.push(label_id);

                let label_key = LabelIndexKey { label_id, node_id };
                ops.push(put_record(label_key.encode(), Bytes::new()));
            }
        }

        let node_key = NodeRecordKey { node_id };
        let node_val = NodeRecordValue { label_ids };
        ops.push(put_record(node_key.encode(), node_val.encode()));

        ops.push(counter_merge(MetadataSubType::NodeCount, 1));

        if let Err(e) = self.exec(async { self.storage.apply(ops).await }) {
            tracing::warn!(error = %e, "storage apply failed");
        }

        self.node_count.fetch_add(1, Ordering::Relaxed);
        NodeId(node_id)
    }

    fn create_node_versioned(
        &self,
        labels: &[&str],
        _epoch: EpochId,
        _transaction_id: TransactionId,
    ) -> NodeId {
        self.create_node(labels)
    }

    fn create_edge(&self, src: NodeId, dst: NodeId, edge_type: &str) -> EdgeId {
        let (edge_id, seq_record) = {
            let mut seq = self.edge_seq.lock().unwrap();
            seq.allocate_one()
        };

        let mut ops: Vec<RecordOp> = Vec::new();

        if let Some(record) = seq_record {
            ops.push(RecordOp::Put(PutRecordOp::from(record)));
        }

        let type_id = {
            let mut catalog = self.catalog.write();
            let (type_id, catalog_ops) = catalog.get_or_create_edge_type(edge_type);
            ops.extend(catalog_ops);
            type_id
        };

        let edge_key = EdgeRecordKey { edge_id };
        let edge_val = EdgeRecordValue {
            src: src.0,
            dst: dst.0,
            type_id,
            prop_count: 0,
        };
        ops.push(put_record(edge_key.encode(), edge_val.encode()));

        push_adj_ops(&mut ops, self.storage_layout, src.0, dst.0, type_id, edge_id);

        ops.push(counter_merge(MetadataSubType::EdgeCount, 1));

        if let Err(e) = self.exec(async { self.storage.apply(ops).await }) {
            tracing::warn!(error = %e, "storage apply failed");
        }

        self.edge_count.fetch_add(1, Ordering::Relaxed);
        EdgeId(edge_id)
    }

    fn create_edge_versioned(
        &self,
        src: NodeId,
        dst: NodeId,
        edge_type: &str,
        _epoch: EpochId,
        _transaction_id: TransactionId,
    ) -> EdgeId {
        self.create_edge(src, dst, edge_type)
    }

    fn batch_create_edges(&self, edges: &[(NodeId, NodeId, &str)]) -> Vec<EdgeId> {
        if edges.is_empty() {
            return Vec::new();
        }

        let mut ops: Vec<RecordOp> = Vec::new();
        let mut edge_ids = Vec::with_capacity(edges.len());

        let mut adj_entries: Vec<(u64, u32, u64, u64)> = Vec::with_capacity(edges.len());

        {
            let mut seq = self.edge_seq.lock().unwrap();
            let mut catalog = self.catalog.write();

            for (src, dst, edge_type) in edges {
                let (edge_id, seq_record) = seq.allocate_one();
                if let Some(record) = seq_record {
                    ops.push(RecordOp::Put(PutRecordOp::from(record)));
                }

                let (type_id, catalog_ops) = catalog.get_or_create_edge_type(edge_type);
                ops.extend(catalog_ops);

                let edge_val = EdgeRecordValue {
                    src: src.0,
                    dst: dst.0,
                    type_id,
                    prop_count: 0,
                };
                ops.push(put_record(
                    EdgeRecordKey { edge_id }.encode(),
                    edge_val.encode(),
                ));

                adj_entries.push((src.0, type_id, dst.0, edge_id));
                edge_ids.push(EdgeId(edge_id));
            }
        }

        for &(src, type_id, dst, edge_id) in &adj_entries {
            push_adj_ops(&mut ops, self.storage_layout, src, dst, type_id, edge_id);
        }

        ops.push(counter_merge(
            MetadataSubType::EdgeCount,
            edges.len() as i64,
        ));

        if let Err(e) = self.exec(async { self.storage.apply(ops).await }) {
            tracing::warn!(error = %e, "storage apply failed");
        }
        self.edge_count
            .fetch_add(edges.len() as i64, Ordering::Relaxed);
        edge_ids
    }

    fn delete_node(&self, id: NodeId) -> bool {
        let storage_layout = self.storage_layout;
        let result = self.exec_txn(|storage| {
            let node_key = NodeRecordKey { node_id: id.0 }.encode();
            Box::pin(async move {
                let txn = storage.begin_transaction().await?;

                // Check existence
                let record = txn.get(node_key.clone()).await?;
                let Some(record) = record else {
                    return Ok((false, 0i64));
                };

                // Delete the node record
                txn.delete(node_key)?;

                let mut edges_deleted: i64 = 0;

                match storage_layout {
                    StorageLayout::Individual => {
                        let fwd_records = txn.scan(ForwardAdjKey::src_prefix(id.0)).await?;
                        for r in &fwd_records {
                            if let Ok(fwd) = ForwardAdjKey::decode(&r.key) {
                                txn.delete(EdgeRecordKey { edge_id: fwd.edge_id }.encode())?;
                                let bwd = BackwardAdjKey {
                                    dst: fwd.dst,
                                    edge_type_id: fwd.edge_type_id,
                                    src: id.0,
                                    edge_id: fwd.edge_id,
                                };
                                txn.delete(bwd.encode())?;
                                // Delete individual edge properties
                                let eprops =
                                    txn.scan(EdgePropertyKey::edge_prefix(fwd.edge_id)).await?;
                                for ep in &eprops {
                                    txn.delete(ep.key.clone())?;
                                }
                                edges_deleted += 1;
                            }
                            txn.delete(r.key.clone())?;
                        }

                        let bwd_records = txn.scan(BackwardAdjKey::dst_prefix(id.0)).await?;
                        for r in &bwd_records {
                            if let Ok(bwd) = BackwardAdjKey::decode(&r.key) {
                                txn.delete(EdgeRecordKey { edge_id: bwd.edge_id }.encode())?;
                                let fwd = ForwardAdjKey {
                                    src: bwd.src,
                                    edge_type_id: bwd.edge_type_id,
                                    dst: id.0,
                                    edge_id: bwd.edge_id,
                                };
                                txn.delete(fwd.encode())?;
                                let eprops =
                                    txn.scan(EdgePropertyKey::edge_prefix(bwd.edge_id)).await?;
                                for ep in &eprops {
                                    txn.delete(ep.key.clone())?;
                                }
                                edges_deleted += 1;
                            }
                            txn.delete(r.key.clone())?;
                        }

                        // Delete node properties and PropertyIndex entries
                        let prop_records =
                            txn.scan(NodePropertyKey::node_prefix(id.0)).await?;
                        for r in &prop_records {
                            if let Ok(prop_key) = NodePropertyKey::decode(&r.key) {
                                if let Ok(value) = values::decode_value(&r.value) {
                                    if let Some(sortable) = values::encode_sortable_value(&value) {
                                        let idx_key = PropertyIndexKey {
                                            prop_id: prop_key.prop_key_id,
                                            sortable_value: sortable,
                                            node_id: id.0,
                                        };
                                        txn.delete(idx_key.encode())?;
                                    }
                                }
                            }
                            txn.delete(r.key.clone())?;
                        }
                    }
                    StorageLayout::Merged => {
                        // Track processed edge_ids to avoid double-cleanup on self-loops
                        let mut seen_edges = std::collections::HashSet::new();

                        // Scan all merged forward adj keys for this node
                        let fwd_records =
                            txn.scan(MergedForwardAdjKey::src_prefix(id.0)).await?;
                        for r in &fwd_records {
                            if let Ok(adj_val) = MergedAdjValue::decode(&r.value) {
                                if let Ok(fwd_key) = MergedForwardAdjKey::decode(&r.key) {
                                    for &(dst, edge_id) in &adj_val.entries {
                                        if !seen_edges.insert(edge_id) {
                                            continue;
                                        }
                                        txn.delete(EdgeRecordKey { edge_id }.encode())?;
                                        txn.merge(
                                            MergedBackwardAdjKey {
                                                dst,
                                                edge_type_id: fwd_key.edge_type_id,
                                            }
                                            .encode(),
                                            values::encode_adj_remove_operand(id.0, edge_id),
                                        )?;
                                        txn.delete(
                                            MergedEdgePropsKey { edge_id }.encode(),
                                        )?;
                                        edges_deleted += 1;
                                    }
                                }
                            }
                            txn.delete(r.key.clone())?;
                        }

                        // Scan all merged backward adj keys for this node
                        let bwd_records =
                            txn.scan(MergedBackwardAdjKey::dst_prefix(id.0)).await?;
                        for r in &bwd_records {
                            if let Ok(adj_val) = MergedAdjValue::decode(&r.value) {
                                if let Ok(bwd_key) = MergedBackwardAdjKey::decode(&r.key) {
                                    for &(src, edge_id) in &adj_val.entries {
                                        if !seen_edges.insert(edge_id) {
                                            continue;
                                        }
                                        txn.delete(EdgeRecordKey { edge_id }.encode())?;
                                        txn.merge(
                                            MergedForwardAdjKey {
                                                src,
                                                edge_type_id: bwd_key.edge_type_id,
                                            }
                                            .encode(),
                                            values::encode_adj_remove_operand(id.0, edge_id),
                                        )?;
                                        txn.delete(
                                            MergedEdgePropsKey { edge_id }.encode(),
                                        )?;
                                        edges_deleted += 1;
                                    }
                                }
                            }
                            txn.delete(r.key.clone())?;
                        }

                        // Delete merged node properties and PropertyIndex entries
                        let merged_key = MergedNodePropsKey { node_id: id.0 }.encode();
                        if let Some(packed_record) = txn.get(merged_key.clone()).await? {
                            if let Ok(props) = MergedPropsValue::decode(&packed_record.value) {
                                for (prop_key_id, val_bytes) in &props.properties {
                                    if let Ok(value) = values::decode_value(val_bytes) {
                                        if let Some(sortable) =
                                            values::encode_sortable_value(&value)
                                        {
                                            let idx_key = PropertyIndexKey {
                                                prop_id: *prop_key_id,
                                                sortable_value: sortable,
                                                node_id: id.0,
                                            };
                                            txn.delete(idx_key.encode())?;
                                        }
                                    }
                                }
                            }
                            txn.delete(merged_key)?;
                        }
                    }
                }

                // Delete label index entries using labels from the node record
                if let Ok(val) = NodeRecordValue::decode(&record.value) {
                    for label_id in &val.label_ids {
                        let label_key = LabelIndexKey {
                            label_id: *label_id,
                            node_id: id.0,
                        };
                        txn.delete(label_key.encode())?;
                    }
                }

                // Counter decrements
                if edges_deleted > 0 {
                    txn.merge(
                        MetadataKey {
                            sub_type: MetadataSubType::EdgeCount,
                        }
                        .encode(),
                        super::encode_i64_le(-edges_deleted),
                    )?;
                }
                txn.merge(
                    MetadataKey {
                        sub_type: MetadataSubType::NodeCount,
                    }
                    .encode(),
                    super::encode_i64_le(-1),
                )?;

                txn.commit().await?;
                Ok((true, edges_deleted))
            })
        });

        match result {
            Ok((true, edges_deleted)) => {
                self.node_count.fetch_sub(1, Ordering::Relaxed);
                if edges_deleted > 0 {
                    self.edge_count.fetch_sub(edges_deleted, Ordering::Relaxed);
                }
                true
            }
            _ => false,
        }
    }

    fn delete_node_versioned(
        &self,
        id: NodeId,
        _epoch: EpochId,
        _transaction_id: TransactionId,
    ) -> bool {
        self.delete_node(id)
    }

    fn delete_node_edges(&self, node_id: NodeId) {
        let storage_layout = self.storage_layout;
        let id = node_id;
        let result = self.exec_txn(|storage| {
            let node_key = NodeRecordKey { node_id: id.0 }.encode();
            Box::pin(async move {
                let txn = storage.begin_transaction().await?;

                // Verify the node exists
                if txn.get(node_key).await?.is_none() {
                    return Ok(0i64);
                }

                // Track processed edge_ids to avoid double-cleanup on self-loops
                let mut seen_edges = std::collections::HashSet::new();
                let mut edges_deleted: i64 = 0;

                match storage_layout {
                    StorageLayout::Individual => {
                        let fwd_records =
                            txn.scan(ForwardAdjKey::src_prefix(id.0)).await?;
                        for r in &fwd_records {
                            if let Ok(fwd) = ForwardAdjKey::decode(&r.key) {
                                if !seen_edges.insert(fwd.edge_id) {
                                    continue;
                                }
                                txn.delete(EdgeRecordKey { edge_id: fwd.edge_id }.encode())?;
                                let bwd = BackwardAdjKey {
                                    dst: fwd.dst,
                                    edge_type_id: fwd.edge_type_id,
                                    src: id.0,
                                    edge_id: fwd.edge_id,
                                };
                                txn.delete(bwd.encode())?;
                                let eprops =
                                    txn.scan(EdgePropertyKey::edge_prefix(fwd.edge_id)).await?;
                                for ep in &eprops {
                                    txn.delete(ep.key.clone())?;
                                }
                                edges_deleted += 1;
                            }
                            txn.delete(r.key.clone())?;
                        }

                        let bwd_records =
                            txn.scan(BackwardAdjKey::dst_prefix(id.0)).await?;
                        for r in &bwd_records {
                            if let Ok(bwd) = BackwardAdjKey::decode(&r.key) {
                                if !seen_edges.insert(bwd.edge_id) {
                                    // Self-loop already processed in forward pass;
                                    // still delete the backward adj key itself.
                                    txn.delete(r.key.clone())?;
                                    continue;
                                }
                                txn.delete(EdgeRecordKey { edge_id: bwd.edge_id }.encode())?;
                                let fwd = ForwardAdjKey {
                                    src: bwd.src,
                                    edge_type_id: bwd.edge_type_id,
                                    dst: id.0,
                                    edge_id: bwd.edge_id,
                                };
                                txn.delete(fwd.encode())?;
                                let eprops =
                                    txn.scan(EdgePropertyKey::edge_prefix(bwd.edge_id)).await?;
                                for ep in &eprops {
                                    txn.delete(ep.key.clone())?;
                                }
                                edges_deleted += 1;
                            }
                            txn.delete(r.key.clone())?;
                        }
                    }
                    StorageLayout::Merged => {
                        let fwd_records =
                            txn.scan(MergedForwardAdjKey::src_prefix(id.0)).await?;
                        for r in &fwd_records {
                            if let Ok(adj_val) = MergedAdjValue::decode(&r.value) {
                                if let Ok(fwd_key) = MergedForwardAdjKey::decode(&r.key) {
                                    for &(dst, edge_id) in &adj_val.entries {
                                        if !seen_edges.insert(edge_id) {
                                            continue;
                                        }
                                        txn.delete(
                                            EdgeRecordKey { edge_id }.encode(),
                                        )?;
                                        txn.merge(
                                            MergedBackwardAdjKey {
                                                dst,
                                                edge_type_id: fwd_key.edge_type_id,
                                            }
                                            .encode(),
                                            values::encode_adj_remove_operand(id.0, edge_id),
                                        )?;
                                        txn.delete(
                                            MergedEdgePropsKey { edge_id }.encode(),
                                        )?;
                                        edges_deleted += 1;
                                    }
                                }
                            }
                            txn.delete(r.key.clone())?;
                        }

                        let bwd_records =
                            txn.scan(MergedBackwardAdjKey::dst_prefix(id.0)).await?;
                        for r in &bwd_records {
                            if let Ok(adj_val) = MergedAdjValue::decode(&r.value) {
                                if let Ok(bwd_key) = MergedBackwardAdjKey::decode(&r.key) {
                                    for &(src, edge_id) in &adj_val.entries {
                                        if !seen_edges.insert(edge_id) {
                                            continue;
                                        }
                                        txn.delete(
                                            EdgeRecordKey { edge_id }.encode(),
                                        )?;
                                        txn.merge(
                                            MergedForwardAdjKey {
                                                src,
                                                edge_type_id: bwd_key.edge_type_id,
                                            }
                                            .encode(),
                                            values::encode_adj_remove_operand(id.0, edge_id),
                                        )?;
                                        txn.delete(
                                            MergedEdgePropsKey { edge_id }.encode(),
                                        )?;
                                        edges_deleted += 1;
                                    }
                                }
                            }
                            txn.delete(r.key.clone())?;
                        }
                    }
                }

                if edges_deleted > 0 {
                    txn.merge(
                        MetadataKey {
                            sub_type: MetadataSubType::EdgeCount,
                        }
                        .encode(),
                        super::encode_i64_le(-edges_deleted),
                    )?;
                }

                txn.commit().await?;
                Ok(edges_deleted)
            })
        });

        match result {
            Ok(edges_deleted) if edges_deleted > 0 => {
                self.edge_count.fetch_sub(edges_deleted, Ordering::Relaxed);
            }
            Err(e) => {
                tracing::warn!(node_id = id.0, error = %e, "delete_node_edges failed");
            }
            _ => {}
        }
    }

    fn delete_edge(&self, id: EdgeId) -> bool {
        let storage_layout = self.storage_layout;
        let result = self.exec_txn(|storage| {
            let edge_key = EdgeRecordKey { edge_id: id.0 }.encode();
            Box::pin(async move {
                let txn = storage.begin_transaction().await?;

                // Get edge record to find src/dst/type for adjacency cleanup
                let record = txn.get(edge_key.clone()).await?;
                let Some(record) = record else {
                    return Ok(false);
                };
                let edge_val = EdgeRecordValue::decode(&record.value)
                    .map_err(|e| common::storage::StorageError::Internal(e.to_string()))?;

                // Delete edge record
                txn.delete(edge_key)?;

                // Delete adjacency indexes
                match storage_layout {
                    StorageLayout::Individual => {
                        let fwd = ForwardAdjKey {
                            src: edge_val.src,
                            edge_type_id: edge_val.type_id,
                            dst: edge_val.dst,
                            edge_id: id.0,
                        };
                        txn.delete(fwd.encode())?;

                        let bwd = BackwardAdjKey {
                            dst: edge_val.dst,
                            edge_type_id: edge_val.type_id,
                            src: edge_val.src,
                            edge_id: id.0,
                        };
                        txn.delete(bwd.encode())?;

                        // Delete edge properties
                        let prop_records =
                            txn.scan(EdgePropertyKey::edge_prefix(id.0)).await?;
                        for r in &prop_records {
                            txn.delete(r.key.clone())?;
                        }
                    }
                    StorageLayout::Merged => {
                        // Merge remove from forward adj
                        txn.merge(
                            MergedForwardAdjKey {
                                src: edge_val.src,
                                edge_type_id: edge_val.type_id,
                            }
                            .encode(),
                            values::encode_adj_remove_operand(edge_val.dst, id.0),
                        )?;

                        // Merge remove from backward adj
                        txn.merge(
                            MergedBackwardAdjKey {
                                dst: edge_val.dst,
                                edge_type_id: edge_val.type_id,
                            }
                            .encode(),
                            values::encode_adj_remove_operand(edge_val.src, id.0),
                        )?;

                        // Delete merged edge properties
                        txn.delete(MergedEdgePropsKey { edge_id: id.0 }.encode())?;
                    }
                }

                // Counter decrement
                txn.merge(
                    MetadataKey {
                        sub_type: MetadataSubType::EdgeCount,
                    }
                    .encode(),
                    super::encode_i64_le(-1),
                )?;

                txn.commit().await?;
                Ok(true)
            })
        });

        match result {
            Ok(true) => {
                self.edge_count.fetch_sub(1, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    fn delete_edge_versioned(
        &self,
        id: EdgeId,
        _epoch: EpochId,
        _transaction_id: TransactionId,
    ) -> bool {
        self.delete_edge(id)
    }

    fn set_node_property(&self, id: NodeId, key: &str, value: Value) {
        let mut catalog = self.catalog.write();
        let (prop_key_id, catalog_ops) = catalog.get_or_create_prop_key(key);
        let mut ops: Vec<RecordOp> = catalog_ops;
        drop(catalog);

        match self.storage_layout {
            StorageLayout::Individual => {
                let Ok(value_bytes) = values::encode_value(&value) else {
                    return;
                };

                let prop_key = NodePropertyKey {
                    node_id: id.0,
                    prop_key_id,
                };

                // Delete stale PropertyIndex entry if overwriting an existing indexed value
                if let Ok(Some(old_record)) =
                    self.exec(async { self.storage.get(prop_key.encode()).await })
                {
                    if let Ok(old_value) = values::decode_value(&old_record.value) {
                        if let Some(old_sortable) = values::encode_sortable_value(&old_value) {
                            let old_idx = PropertyIndexKey {
                                prop_id: prop_key_id,
                                sortable_value: old_sortable,
                                node_id: id.0,
                            };
                            ops.push(RecordOp::Delete(old_idx.encode()));
                        }
                    }
                }

                ops.push(put_record(prop_key.encode(), value_bytes));
            }
            StorageLayout::Merged => {
                let Ok(operand) = values::encode_prop_set_operand(prop_key_id, &value) else {
                    return;
                };

                // Blind write: don't read the old merged blob to clean up stale
                // PropertyIndex entries. Reading triggers O(N) merge-operand
                // resolution in SlateDB, which is pathologically slow under
                // repeated updates. Instead, stale index entries are filtered
                // at read time in find_nodes_by_property / find_nodes_in_range.
                ops.push(merge_record(
                    MergedNodePropsKey { node_id: id.0 }.encode(),
                    operand,
                ));
            }
        }

        // PropertyIndex entry (common to both layouts)
        if let Some(sortable) = values::encode_sortable_value(&value) {
            let idx_key = PropertyIndexKey {
                prop_id: prop_key_id,
                sortable_value: sortable,
                node_id: id.0,
            };
            ops.push(put_record(idx_key.encode(), Bytes::new()));
        }

        if let Err(e) = self.exec(async { self.storage.apply(ops).await }) {
            tracing::warn!(error = %e, "storage apply failed");
        }
    }

    fn set_edge_property(&self, id: EdgeId, key: &str, value: Value) {
        let mut catalog = self.catalog.write();
        let (prop_key_id, catalog_ops) = catalog.get_or_create_prop_key(key);
        let mut ops: Vec<RecordOp> = catalog_ops;
        drop(catalog);

        match self.storage_layout {
            StorageLayout::Individual => {
                let Ok(value_bytes) = values::encode_value(&value) else {
                    return;
                };

                let prop_key = EdgePropertyKey {
                    edge_id: id.0,
                    prop_key_id,
                };
                ops.push(put_record(prop_key.encode(), value_bytes));
            }
            StorageLayout::Merged => {
                let Ok(operand) = values::encode_prop_set_operand(prop_key_id, &value) else {
                    return;
                };
                ops.push(merge_record(
                    MergedEdgePropsKey { edge_id: id.0 }.encode(),
                    operand,
                ));
            }
        }

        if let Err(e) = self.exec(async { self.storage.apply(ops).await }) {
            tracing::warn!(error = %e, "storage apply failed");
        }
    }

    fn remove_node_property(&self, id: NodeId, key: &str) -> Option<Value> {
        let existing = self.get_node_property(id, &PropertyKey::new(key));

        let catalog = self.catalog.read();
        let prop_key_id = catalog.get_prop_key_id(key)?;
        drop(catalog);

        let mut ops: Vec<RecordOp> = Vec::new();

        match self.storage_layout {
            StorageLayout::Individual => {
                let prop_key = NodePropertyKey {
                    node_id: id.0,
                    prop_key_id,
                };
                ops.push(RecordOp::Delete(prop_key.encode()));
            }
            StorageLayout::Merged => {
                ops.push(merge_record(
                    MergedNodePropsKey { node_id: id.0 }.encode(),
                    values::encode_prop_remove_operand(prop_key_id),
                ));
            }
        }

        if let Some(ref value) = existing
            && let Some(sortable) = values::encode_sortable_value(value)
        {
            let idx_key = PropertyIndexKey {
                prop_id: prop_key_id,
                sortable_value: sortable,
                node_id: id.0,
            };
            ops.push(RecordOp::Delete(idx_key.encode()));
        }

        if let Err(e) = self.exec(async { self.storage.apply(ops).await }) {
            tracing::warn!(error = %e, "storage apply failed");
        }
        existing
    }

    fn remove_edge_property(&self, id: EdgeId, key: &str) -> Option<Value> {
        let existing = self.get_edge_property(id, &PropertyKey::new(key));

        let catalog = self.catalog.read();
        let prop_key_id = match catalog.get_prop_key_id(key) {
            Some(id) => id,
            None => return existing,
        };
        drop(catalog);

        let ops = match self.storage_layout {
            StorageLayout::Individual => {
                let prop_key = EdgePropertyKey {
                    edge_id: id.0,
                    prop_key_id,
                };
                vec![RecordOp::Delete(prop_key.encode())]
            }
            StorageLayout::Merged => {
                vec![merge_record(
                    MergedEdgePropsKey { edge_id: id.0 }.encode(),
                    values::encode_prop_remove_operand(prop_key_id),
                )]
            }
        };

        if let Err(e) = self.exec(async { self.storage.apply(ops).await }) {
            tracing::warn!(error = %e, "storage apply failed");
        }
        existing
    }

    fn add_label(&self, node_id: NodeId, label: &str) -> bool {
        // Resolve or create label ID (needs catalog write lock)
        let (label_id, catalog_ops) = {
            let mut catalog = self.catalog.write();
            catalog.get_or_create_label(label)
        };

        let result = self.exec_txn(|storage| {
            let nk = NodeRecordKey {
                node_id: node_id.0,
            }
            .encode();
            let catalog_ops = catalog_ops.clone();
            Box::pin(async move {
                let txn = storage.begin_transaction().await?;

                // Read current node record
                let record = txn.get(nk.clone()).await?;
                let Some(record) = record else {
                    return Ok(false);
                };
                let mut node_val = NodeRecordValue::decode(&record.value)
                    .map_err(|e| common::storage::StorageError::Internal(e.to_string()))?;

                if node_val.label_ids.contains(&label_id) {
                    return Ok(false);
                }

                node_val.label_ids.push(label_id);

                // Persist catalog entries if new
                for op in catalog_ops {
                    match op {
                        RecordOp::Put(p) => txn.put(p.record.key, p.record.value)?,
                        RecordOp::Merge(m) => txn.merge(m.record.key, m.record.value)?,
                        RecordOp::Delete(k) => txn.delete(k)?,
                    }
                }

                // Update label index
                let label_key = LabelIndexKey {
                    label_id,
                    node_id: node_id.0,
                };
                txn.put(label_key.encode(), Bytes::new())?;

                // Rewrite node record with updated labels
                txn.put(nk, node_val.encode())?;

                txn.commit().await?;
                Ok(true)
            })
        });

        result.unwrap_or(false)
    }

    fn remove_label(&self, node_id: NodeId, label: &str) -> bool {
        let label_id = {
            let catalog = self.catalog.read();
            match catalog.get_label_id(label) {
                Some(id) => id,
                None => return false,
            }
        };

        let result = self.exec_txn(|storage| {
            let nk = NodeRecordKey {
                node_id: node_id.0,
            }
            .encode();
            Box::pin(async move {
                let txn = storage.begin_transaction().await?;

                let record = txn.get(nk.clone()).await?;
                let Some(record) = record else {
                    return Ok(false);
                };
                let mut node_val = NodeRecordValue::decode(&record.value)
                    .map_err(|e| common::storage::StorageError::Internal(e.to_string()))?;

                if !node_val.label_ids.contains(&label_id) {
                    return Ok(false);
                }

                node_val.label_ids.retain(|&id| id != label_id);

                // Remove label index entry
                let label_key = LabelIndexKey {
                    label_id,
                    node_id: node_id.0,
                };
                txn.delete(label_key.encode())?;

                // Rewrite node record
                txn.put(nk, node_val.encode())?;

                txn.commit().await?;
                Ok(true)
            })
        });

        result.unwrap_or(false)
    }
}
