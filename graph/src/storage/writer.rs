use std::sync::atomic::Ordering;

use bytes::Bytes;
use grafeo_common::types::{EdgeId, EpochId, NodeId, PropertyKey, TxId, Value};
use grafeo_core::graph::traits::{GraphStore, GraphStoreMut};

use super::SlateGraphStore;
use crate::serde::MetadataSubType;
use crate::serde::keys::*;
use crate::serde::values::{self, EdgeRecordValue, FLAG_DELETED, NodeRecordValue};
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
        self.create_node_versioned(
            labels,
            EpochId(self.current_epoch.load(Ordering::Relaxed)),
            TxId(0),
        )
    }

    fn create_node_versioned(&self, labels: &[&str], epoch: EpochId, _tx_id: TxId) -> NodeId {
        let (node_id, seq_record) = {
            let mut seq = self.node_seq.lock().unwrap();
            seq.allocate_one()
        };

        let mut ops: Vec<RecordOp> = Vec::new();

        // Persist sequence block if needed
        if let Some(record) = seq_record {
            ops.push(RecordOp::Put(PutRecordOp::from(record)));
        }

        // Resolve label IDs and write label index entries
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

        // Write node record with inline label IDs
        let node_key = NodeRecordKey {
            node_id,
            epoch: epoch.0,
        };
        let node_val = NodeRecordValue {
            flags: 0,
            label_ids,
        };
        ops.push(put_record(node_key.encode(), node_val.encode()));

        ops.push(counter_merge(MetadataSubType::NodeCount, 1));

        // Apply atomically
        let _ = self.exec(async { self.storage.apply(ops).await });

        self.node_count.fetch_add(1, Ordering::Relaxed);
        NodeId(node_id)
    }

    fn create_edge(&self, src: NodeId, dst: NodeId, edge_type: &str) -> EdgeId {
        self.create_edge_versioned(
            src,
            dst,
            edge_type,
            EpochId(self.current_epoch.load(Ordering::Relaxed)),
            TxId(0),
        )
    }

    fn create_edge_versioned(
        &self,
        src: NodeId,
        dst: NodeId,
        edge_type: &str,
        epoch: EpochId,
        _tx_id: TxId,
    ) -> EdgeId {
        let (edge_id, seq_record) = {
            let mut seq = self.edge_seq.lock().unwrap();
            seq.allocate_one()
        };

        let mut ops: Vec<RecordOp> = Vec::new();

        // Persist sequence block if needed
        if let Some(record) = seq_record {
            ops.push(RecordOp::Put(PutRecordOp::from(record)));
        }

        // Get or create edge type in catalog
        let type_id = {
            let mut catalog = self.catalog.write();
            let (type_id, catalog_ops) = catalog.get_or_create_edge_type(edge_type);
            ops.extend(catalog_ops);
            type_id
        };

        // Write edge record
        let edge_key = EdgeRecordKey {
            edge_id,
            epoch: epoch.0,
        };
        let edge_val = EdgeRecordValue {
            src: src.0,
            dst: dst.0,
            type_id,
            flags: 0,
            prop_count: 0,
        };
        ops.push(put_record(edge_key.encode(), edge_val.encode()));

        // Write forward adjacency (edge_id in key, empty value)
        let fwd_key = ForwardAdjKey {
            src: src.0,
            edge_type_id: type_id,
            dst: dst.0,
            edge_id,
        };
        ops.push(put_record(fwd_key.encode(), Bytes::new()));

        // Write backward adjacency if enabled
        if self.backward_edges {
            let bwd_key = BackwardAdjKey {
                dst: dst.0,
                edge_type_id: type_id,
                src: src.0,
                edge_id,
            };
            ops.push(put_record(bwd_key.encode(), Bytes::new()));
        }

        ops.push(counter_merge(MetadataSubType::EdgeCount, 1));

        let _ = self.exec(async { self.storage.apply(ops).await });

        self.edge_count.fetch_add(1, Ordering::Relaxed);
        EdgeId(edge_id)
    }

    fn batch_create_edges(&self, edges: &[(NodeId, NodeId, &str)]) -> Vec<EdgeId> {
        edges
            .iter()
            .map(|(src, dst, edge_type)| self.create_edge(*src, *dst, edge_type))
            .collect()
    }

    fn delete_node(&self, id: NodeId) -> bool {
        self.delete_node_versioned(
            id,
            EpochId(self.current_epoch.load(Ordering::Relaxed)),
            TxId(0),
        )
    }

    fn delete_node_versioned(&self, id: NodeId, epoch: EpochId, _tx_id: TxId) -> bool {
        // Check if node exists
        let exists = self
            .exec(async {
                let records = self.storage.scan(NodeRecordKey::node_prefix(id.0)).await?;
                Ok(records)
            })
            .ok()
            .and_then(|records| {
                let record = records.last()?;
                let val = NodeRecordValue::decode(&record.value).ok()?;
                if val.is_deleted() { None } else { Some(true) }
            });

        if exists.is_none() {
            return false;
        }

        let mut ops: Vec<RecordOp> = Vec::new();

        // Write deleted node record
        let node_key = NodeRecordKey {
            node_id: id.0,
            epoch: epoch.0,
        };
        let node_val = NodeRecordValue {
            flags: FLAG_DELETED,
            label_ids: Vec::new(),
        };
        ops.push(put_record(node_key.encode(), node_val.encode()));

        // Delete properties
        if let Ok(records) =
            self.exec(async { self.storage.scan(NodePropertyKey::node_prefix(id.0)).await })
        {
            for record in &records {
                ops.push(RecordOp::Delete(record.key.clone()));
            }
        }

        // Delete label index entries
        {
            let catalog = self.catalog.read();
            for label_id in 0..catalog.label_count() as u32 {
                let label_key = LabelIndexKey {
                    label_id,
                    node_id: id.0,
                };
                ops.push(RecordOp::Delete(label_key.encode()));
            }
        }

        ops.push(counter_merge(MetadataSubType::NodeCount, -1));

        let _ = self.exec(async { self.storage.apply(ops).await });
        self.node_count.fetch_sub(1, Ordering::Relaxed);
        true
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
        if self.backward_edges
            && let Ok(records) = self.exec(async {
                self.storage
                    .scan(BackwardAdjKey::dst_prefix(node_id.0))
                    .await
            })
        {
            for record in &records {
                if let Ok(key) = BackwardAdjKey::decode(&record.key) {
                    self.delete_edge(EdgeId(key.edge_id));
                }
            }
        }
    }

    fn delete_edge(&self, id: EdgeId) -> bool {
        self.delete_edge_versioned(
            id,
            EpochId(self.current_epoch.load(Ordering::Relaxed)),
            TxId(0),
        )
    }

    fn delete_edge_versioned(&self, id: EdgeId, epoch: EpochId, _tx_id: TxId) -> bool {
        // Get edge record to find src/dst/type for adjacency cleanup
        let edge_val = self
            .exec(async {
                let records = self.storage.scan(EdgeRecordKey::edge_prefix(id.0)).await?;
                Ok(records)
            })
            .ok()
            .and_then(|records| {
                let record = records.last()?;
                EdgeRecordValue::decode(&record.value).ok()
            });

        let edge_val = match edge_val {
            Some(v) if !v.is_deleted() => v,
            _ => return false,
        };

        let mut ops: Vec<RecordOp> = Vec::new();

        // Write deleted edge record
        let edge_key = EdgeRecordKey {
            edge_id: id.0,
            epoch: epoch.0,
        };
        let deleted_val = EdgeRecordValue {
            flags: FLAG_DELETED,
            ..edge_val.clone()
        };
        ops.push(put_record(edge_key.encode(), deleted_val.encode()));

        // Delete adjacency indexes
        let fwd = ForwardAdjKey {
            src: edge_val.src,
            edge_type_id: edge_val.type_id,
            dst: edge_val.dst,
            edge_id: id.0,
        };
        ops.push(RecordOp::Delete(fwd.encode()));
        if self.backward_edges {
            let bwd = BackwardAdjKey {
                dst: edge_val.dst,
                edge_type_id: edge_val.type_id,
                src: edge_val.src,
                edge_id: id.0,
            };
            ops.push(RecordOp::Delete(bwd.encode()));
        }

        // Delete edge properties
        if let Ok(records) =
            self.exec(async { self.storage.scan(EdgePropertyKey::edge_prefix(id.0)).await })
        {
            for record in &records {
                ops.push(RecordOp::Delete(record.key.clone()));
            }
        }

        ops.push(counter_merge(MetadataSubType::EdgeCount, -1));

        let _ = self.exec(async { self.storage.apply(ops).await });
        self.edge_count.fetch_sub(1, Ordering::Relaxed);
        true
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
        ops.push(put_record(prop_key.encode(), value_bytes));

        // Update property index if the value is sortable
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
        // Read existing value first
        let existing = self.get_node_property(id, &PropertyKey::new(key));

        let catalog = self.catalog.read();
        let prop_key_id = catalog.get_prop_key_id(key)?;

        let prop_key = NodePropertyKey {
            node_id: id.0,
            prop_key_id,
        };

        let mut ops = vec![RecordOp::Delete(prop_key.encode())];

        // Remove from property index if we had a sortable value
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
        // Read current node record to get existing label_ids
        let current = self
            .exec(async {
                let records = self
                    .storage
                    .scan(NodeRecordKey::node_prefix(node_id.0))
                    .await?;
                Ok(records)
            })
            .ok()
            .and_then(|records| {
                let record = records.last()?;
                let key = NodeRecordKey::decode(&record.key).ok()?;
                let val = NodeRecordValue::decode(&record.value).ok()?;
                if val.is_deleted() {
                    None
                } else {
                    Some((key.epoch, val))
                }
            });

        let (epoch, mut node_val) = match current {
            Some(v) => v,
            None => return false,
        };

        let mut catalog = self.catalog.write();
        let (label_id, catalog_ops) = catalog.get_or_create_label(label);

        // Check if label already exists
        if node_val.label_ids.contains(&label_id) {
            return false;
        }

        node_val.label_ids.push(label_id);

        let mut ops = catalog_ops;

        // Update label index
        let label_key = LabelIndexKey {
            label_id,
            node_id: node_id.0,
        };
        ops.push(put_record(label_key.encode(), Bytes::new()));

        // Rewrite node record with updated labels
        let node_key = NodeRecordKey {
            node_id: node_id.0,
            epoch,
        };
        ops.push(put_record(node_key.encode(), node_val.encode()));

        let _ = self.exec(async { self.storage.apply(ops).await });
        true
    }

    fn remove_label(&self, node_id: NodeId, label: &str) -> bool {
        let catalog = self.catalog.read();
        let label_id = match catalog.get_label_id(label) {
            Some(id) => id,
            None => return false,
        };
        drop(catalog);

        // Read current node record
        let current = self
            .exec(async {
                let records = self
                    .storage
                    .scan(NodeRecordKey::node_prefix(node_id.0))
                    .await?;
                Ok(records)
            })
            .ok()
            .and_then(|records| {
                let record = records.last()?;
                let key = NodeRecordKey::decode(&record.key).ok()?;
                let val = NodeRecordValue::decode(&record.value).ok()?;
                if val.is_deleted() {
                    None
                } else {
                    Some((key.epoch, val))
                }
            });

        let (epoch, mut node_val) = match current {
            Some(v) => v,
            None => return false,
        };

        if !node_val.label_ids.contains(&label_id) {
            return false;
        }

        node_val.label_ids.retain(|&id| id != label_id);

        let mut ops = Vec::new();

        // Remove label index entry
        let label_key = LabelIndexKey {
            label_id,
            node_id: node_id.0,
        };
        ops.push(RecordOp::Delete(label_key.encode()));

        // Rewrite node record
        let node_key = NodeRecordKey {
            node_id: node_id.0,
            epoch,
        };
        ops.push(put_record(node_key.encode(), node_val.encode()));

        let _ = self.exec(async { self.storage.apply(ops).await });
        true
    }
}
