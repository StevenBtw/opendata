use std::sync::atomic::Ordering;

use bytes::Bytes;
use grafeo_common::types::{EdgeId, EpochId, NodeId, PropertyKey, TransactionId, Value};
use grafeo_core::graph::traits::{GraphStore, GraphStoreMut};

use super::SlateGraphStore;
use crate::serde::MetadataSubType;
use crate::serde::keys::*;
use crate::serde::values::{self, EdgeRecordValue, NodeRecordValue};
use common::storage::{MergeRecordOp, PutRecordOp, Record, RecordOp};

fn put_record(key: Bytes, value: Bytes) -> RecordOp {
    RecordOp::Put(PutRecordOp::from(Record::new(key, value)))
}

fn counter_merge(sub_type: MetadataSubType, delta: i64) -> RecordOp {
    RecordOp::Merge(MergeRecordOp::from(Record::new(
        MetadataKey { sub_type }.encode(),
        super::encode_i64_le(delta),
    )))
}

impl GraphStoreMut for SlateGraphStore {
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

        let _ = self.exec(async { self.storage.apply(ops).await });

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

        let fwd_key = ForwardAdjKey {
            src: src.0,
            edge_type_id: type_id,
            dst: dst.0,
            edge_id,
        };
        ops.push(put_record(fwd_key.encode(), Bytes::new()));

        let bwd_key = BackwardAdjKey {
            dst: dst.0,
            edge_type_id: type_id,
            src: src.0,
            edge_id,
        };
        ops.push(put_record(bwd_key.encode(), Bytes::new()));

        ops.push(counter_merge(MetadataSubType::EdgeCount, 1));

        let _ = self.exec(async { self.storage.apply(ops).await });

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

                ops.push(put_record(
                    ForwardAdjKey {
                        src: src.0,
                        edge_type_id: type_id,
                        dst: dst.0,
                        edge_id,
                    }
                    .encode(),
                    Bytes::new(),
                ));

                ops.push(put_record(
                    BackwardAdjKey {
                        dst: dst.0,
                        edge_type_id: type_id,
                        src: src.0,
                        edge_id,
                    }
                    .encode(),
                    Bytes::new(),
                ));

                edge_ids.push(EdgeId(edge_id));
            }
        }

        ops.push(counter_merge(
            MetadataSubType::EdgeCount,
            edges.len() as i64,
        ));

        let _ = self.exec(async { self.storage.apply(ops).await });
        self.edge_count
            .fetch_add(edges.len() as i64, Ordering::Relaxed);
        edge_ids
    }

    fn delete_node(&self, id: NodeId) -> bool {
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

                // Clean up any remaining edges (crash safety: if delete_node_edges
                // was interrupted, this ensures no orphaned adjacency entries remain).
                let mut edges_deleted: i64 = 0;

                let fwd_records = txn.scan(ForwardAdjKey::src_prefix(id.0)).await?;
                for r in &fwd_records {
                    if let Ok(fwd) = ForwardAdjKey::decode(&r.key) {
                        // Delete edge record
                        txn.delete(EdgeRecordKey { edge_id: fwd.edge_id }.encode())?;
                        // Delete backward adjacency
                        let bwd = BackwardAdjKey {
                            dst: fwd.dst,
                            edge_type_id: fwd.edge_type_id,
                            src: id.0,
                            edge_id: fwd.edge_id,
                        };
                        txn.delete(bwd.encode())?;
                        // Delete edge properties
                        let eprops = txn.scan(EdgePropertyKey::edge_prefix(fwd.edge_id)).await?;
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
                        // Delete edge record
                        txn.delete(EdgeRecordKey { edge_id: bwd.edge_id }.encode())?;
                        // Delete forward adjacency
                        let fwd = ForwardAdjKey {
                            src: bwd.src,
                            edge_type_id: bwd.edge_type_id,
                            dst: id.0,
                            edge_id: bwd.edge_id,
                        };
                        txn.delete(fwd.encode())?;
                        // Delete edge properties
                        let eprops = txn.scan(EdgePropertyKey::edge_prefix(bwd.edge_id)).await?;
                        for ep in &eprops {
                            txn.delete(ep.key.clone())?;
                        }
                        edges_deleted += 1;
                    }
                    txn.delete(r.key.clone())?;
                }

                // Delete node properties and their PropertyIndex entries
                let prop_records = txn
                    .scan(NodePropertyKey::node_prefix(id.0))
                    .await?;
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
        // Delete outgoing edges
        if let Ok(records) = self.exec(async {
            self.storage
                .scan(ForwardAdjKey::src_prefix(node_id.0))
                .await
        }) {
            for record in &records {
                if let Ok(key) = ForwardAdjKey::decode(&record.key) {
                    self.delete_edge(EdgeId(key.edge_id));
                }
            }
        }

        // Delete incoming edges
        if let Ok(records) = self.exec(async {
            self.storage
                .scan(BackwardAdjKey::dst_prefix(node_id.0))
                .await
        }) {
            for record in &records {
                if let Ok(key) = BackwardAdjKey::decode(&record.key) {
                    self.delete_edge(EdgeId(key.edge_id));
                }
            }
        }
    }

    fn delete_edge(&self, id: EdgeId) -> bool {
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
                let prop_records = txn
                    .scan(EdgePropertyKey::edge_prefix(id.0))
                    .await?;
                for r in &prop_records {
                    txn.delete(r.key.clone())?;
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
        let Ok(value_bytes) = values::encode_value(&value) else {
            return;
        };

        let mut catalog = self.catalog.write();
        let (prop_key_id, catalog_ops) = catalog.get_or_create_prop_key(key);
        let mut ops: Vec<RecordOp> = catalog_ops;

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

        if let Some(sortable) = values::encode_sortable_value(&value) {
            let idx_key = PropertyIndexKey {
                prop_id: prop_key_id,
                sortable_value: sortable,
                node_id: id.0,
            };
            ops.push(put_record(idx_key.encode(), Bytes::new()));
        }

        let _ = self.exec(async { self.storage.apply(ops).await });
    }

    fn set_edge_property(&self, id: EdgeId, key: &str, value: Value) {
        let Ok(value_bytes) = values::encode_value(&value) else {
            return;
        };

        let mut catalog = self.catalog.write();
        let (prop_key_id, catalog_ops) = catalog.get_or_create_prop_key(key);
        let mut ops: Vec<RecordOp> = catalog_ops;

        let prop_key = EdgePropertyKey {
            edge_id: id.0,
            prop_key_id,
        };
        ops.push(put_record(prop_key.encode(), value_bytes));

        let _ = self.exec(async { self.storage.apply(ops).await });
    }

    fn remove_node_property(&self, id: NodeId, key: &str) -> Option<Value> {
        let existing = self.get_node_property(id, &PropertyKey::new(key));

        let catalog = self.catalog.read();
        let prop_key_id = catalog.get_prop_key_id(key)?;

        let prop_key = NodePropertyKey {
            node_id: id.0,
            prop_key_id,
        };

        let mut ops = vec![RecordOp::Delete(prop_key.encode())];

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

        drop(catalog);
        let _ = self.exec(async { self.storage.apply(ops).await });
        existing
    }

    fn remove_edge_property(&self, id: EdgeId, key: &str) -> Option<Value> {
        let existing = self.get_edge_property(id, &PropertyKey::new(key));

        let catalog = self.catalog.read();
        let prop_key_id = match catalog.get_prop_key_id(key) {
            Some(id) => id,
            None => return existing,
        };

        let prop_key = EdgePropertyKey {
            edge_id: id.0,
            prop_key_id,
        };

        drop(catalog);
        let _ = self.exec(async {
            self.storage
                .apply(vec![RecordOp::Delete(prop_key.encode())])
                .await
        });

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
