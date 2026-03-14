use std::sync::Arc;
use std::sync::atomic::Ordering;

use arcstr::ArcStr;
use grafeo_common::types::{EdgeId, EpochId, NodeId, PropertyKey, TransactionId, Value};
use grafeo_common::utils::hash::FxHashMap;
use grafeo_core::graph::Direction;
use grafeo_core::graph::lpg::CompareOp;
use grafeo_core::graph::lpg::{Edge, Node};
use grafeo_core::graph::traits::GraphStore;
use grafeo_core::statistics::Statistics;
use smallvec::SmallVec;

use super::SlateGraphStore;
use crate::serde::keys::*;
use crate::serde::values::{self, EdgeRecordValue, NodeRecordValue};

impl GraphStore for SlateGraphStore {
    fn get_node(&self, id: NodeId) -> Option<Node> {
        let key = NodeRecordKey { node_id: id.0 }.encode();
        self.exec(async { self.storage.get(key).await })
            .ok()?
            .as_ref()?;
        self.build_node(id).ok().flatten()
    }

    fn get_edge(&self, id: EdgeId) -> Option<Edge> {
        let key = EdgeRecordKey { edge_id: id.0 }.encode();
        let record = self
            .exec(async { self.storage.get(key).await })
            .ok()??;
        let val = EdgeRecordValue::decode(&record.value).ok()?;
        self.build_edge(id, &val).ok()
    }

    fn get_node_at_epoch(&self, id: NodeId, _epoch: EpochId) -> Option<Node> {
        self.get_node(id)
    }

    fn get_edge_at_epoch(&self, id: EdgeId, _epoch: EpochId) -> Option<Edge> {
        self.get_edge(id)
    }

    fn get_node_versioned(
        &self,
        id: NodeId,
        _epoch: EpochId,
        _transaction_id: TransactionId,
    ) -> Option<Node> {
        self.get_node(id)
    }

    fn get_edge_versioned(
        &self,
        id: EdgeId,
        _epoch: EpochId,
        _transaction_id: TransactionId,
    ) -> Option<Edge> {
        self.get_edge(id)
    }

    fn get_node_property(&self, id: NodeId, key: &PropertyKey) -> Option<Value> {
        let prop_key_id = self.catalog.read().get_prop_key_id(key.as_str())?;
        let prop_key = NodePropertyKey {
            node_id: id.0,
            prop_key_id,
        };
        self.exec(async { self.storage.get(prop_key.encode()).await })
            .ok()
            .flatten()
            .and_then(|record| values::decode_value(&record.value).ok())
    }

    fn get_edge_property(&self, id: EdgeId, key: &PropertyKey) -> Option<Value> {
        let prop_key_id = self.catalog.read().get_prop_key_id(key.as_str())?;
        let prop_key = EdgePropertyKey {
            edge_id: id.0,
            prop_key_id,
        };
        self.exec(async { self.storage.get(prop_key.encode()).await })
            .ok()
            .flatten()
            .and_then(|record| values::decode_value(&record.value).ok())
    }

    fn get_node_property_batch(&self, ids: &[NodeId], key: &PropertyKey) -> Vec<Option<Value>> {
        ids.iter()
            .map(|id| self.get_node_property(*id, key))
            .collect()
    }

    fn get_nodes_properties_batch(&self, ids: &[NodeId]) -> Vec<FxHashMap<PropertyKey, Value>> {
        ids.iter()
            .map(|id| self.load_node_properties(id.0).unwrap_or_default())
            .collect()
    }

    fn get_nodes_properties_selective_batch(
        &self,
        ids: &[NodeId],
        keys: &[PropertyKey],
    ) -> Vec<FxHashMap<PropertyKey, Value>> {
        ids.iter()
            .map(|id| {
                let mut map = FxHashMap::default();
                for key in keys {
                    if let Some(val) = self.get_node_property(*id, key) {
                        map.insert(key.clone(), val);
                    }
                }
                map
            })
            .collect()
    }

    fn get_edges_properties_selective_batch(
        &self,
        ids: &[EdgeId],
        keys: &[PropertyKey],
    ) -> Vec<FxHashMap<PropertyKey, Value>> {
        ids.iter()
            .map(|id| {
                let mut map = FxHashMap::default();
                for key in keys {
                    if let Some(val) = self.get_edge_property(*id, key) {
                        map.insert(key.clone(), val);
                    }
                }
                map
            })
            .collect()
    }

    fn neighbors(&self, node: NodeId, direction: Direction) -> Vec<NodeId> {
        let mut result = Vec::new();

        if matches!(direction, Direction::Outgoing | Direction::Both)
            && let Ok(records) =
                self.exec(async { self.storage.scan(ForwardAdjKey::src_prefix(node.0)).await })
        {
            for record in &records {
                if let Ok(key) = ForwardAdjKey::decode(&record.key) {
                    result.push(NodeId(key.dst));
                }
            }
        }

        if matches!(direction, Direction::Incoming | Direction::Both)
            && let Ok(records) =
                self.exec(async { self.storage.scan(BackwardAdjKey::dst_prefix(node.0)).await })
        {
            for record in &records {
                if let Ok(key) = BackwardAdjKey::decode(&record.key) {
                    result.push(NodeId(key.src));
                }
            }
        }

        result
    }

    fn edges_from(&self, node: NodeId, direction: Direction) -> Vec<(NodeId, EdgeId)> {
        let mut result = Vec::new();

        if matches!(direction, Direction::Outgoing | Direction::Both)
            && let Ok(records) =
                self.exec(async { self.storage.scan(ForwardAdjKey::src_prefix(node.0)).await })
        {
            for record in &records {
                if let Ok(key) = ForwardAdjKey::decode(&record.key) {
                    result.push((NodeId(key.dst), EdgeId(key.edge_id)));
                }
            }
        }

        if matches!(direction, Direction::Incoming | Direction::Both)
            && let Ok(records) =
                self.exec(async { self.storage.scan(BackwardAdjKey::dst_prefix(node.0)).await })
        {
            for record in &records {
                if let Ok(key) = BackwardAdjKey::decode(&record.key) {
                    result.push((NodeId(key.src), EdgeId(key.edge_id)));
                }
            }
        }

        result
    }

    fn out_degree(&self, node: NodeId) -> usize {
        self.exec(async { self.storage.scan(ForwardAdjKey::src_prefix(node.0)).await })
            .map(|records| records.len())
            .unwrap_or(0)
    }

    fn in_degree(&self, node: NodeId) -> usize {
        self.exec(async { self.storage.scan(BackwardAdjKey::dst_prefix(node.0)).await })
            .map(|records| records.len())
            .unwrap_or(0)
    }

    fn has_backward_adjacency(&self) -> bool {
        true
    }

    fn node_ids(&self) -> Vec<NodeId> {
        let Ok(records) =
            self.exec(async { self.storage.scan(NodeRecordKey::all_nodes_range()).await })
        else {
            return Vec::new();
        };

        records
            .iter()
            .filter_map(|r| NodeRecordKey::decode(&r.key).ok())
            .map(|k| NodeId(k.node_id))
            .collect()
    }

    fn nodes_by_label(&self, label: &str) -> Vec<NodeId> {
        let label_id = {
            let catalog = self.catalog.read();
            match catalog.get_label_id(label) {
                Some(id) => id,
                None => return Vec::new(),
            }
        };

        let Ok(records) = self.exec(async {
            self.storage
                .scan(LabelIndexKey::label_prefix(label_id))
                .await
        }) else {
            return Vec::new();
        };

        records
            .iter()
            .filter_map(|r| LabelIndexKey::decode(&r.key).ok())
            .map(|k| NodeId(k.node_id))
            .collect()
    }

    fn node_count(&self) -> usize {
        self.node_count.load(Ordering::Relaxed).max(0) as usize
    }

    fn edge_count(&self) -> usize {
        self.edge_count.load(Ordering::Relaxed).max(0) as usize
    }

    fn edge_type(&self, id: EdgeId) -> Option<ArcStr> {
        let key = EdgeRecordKey { edge_id: id.0 }.encode();
        let record = self
            .exec(async { self.storage.get(key).await })
            .ok()??;
        let val = EdgeRecordValue::decode(&record.value).ok()?;
        let catalog = self.catalog.read();
        catalog.get_edge_type_name(val.type_id).cloned()
    }

    fn find_nodes_by_property(&self, property: &str, value: &Value) -> Vec<NodeId> {
        let sortable = match values::encode_sortable_value(value) {
            Some(s) => s,
            None => return Vec::new(),
        };

        let prop_id = {
            let catalog = self.catalog.read();
            match catalog.get_prop_key_id(property) {
                Some(id) => id,
                None => return Vec::new(),
            }
        };

        let range = PropertyIndexKey::prop_value_prefix(prop_id, &sortable);
        self.node_ids_from_index_scan(range)
    }

    fn find_nodes_by_properties(&self, conditions: &[(&str, Value)]) -> Vec<NodeId> {
        if conditions.is_empty() {
            return Vec::new();
        }
        let mut result = self.find_nodes_by_property(conditions[0].0, &conditions[0].1);
        for (prop, val) in &conditions[1..] {
            let candidates: std::collections::HashSet<NodeId> =
                self.find_nodes_by_property(prop, val).into_iter().collect();
            result.retain(|id| candidates.contains(id));
        }
        result
    }

    fn find_nodes_in_range(
        &self,
        property: &str,
        min: Option<&Value>,
        max: Option<&Value>,
        min_inclusive: bool,
        max_inclusive: bool,
    ) -> Vec<NodeId> {
        let prop_id = {
            let catalog = self.catalog.read();
            match catalog.get_prop_key_id(property) {
                Some(id) => id,
                None => return Vec::new(),
            }
        };

        let min_bytes = min.and_then(values::encode_sortable_value);
        let max_bytes = max.and_then(values::encode_sortable_value);

        let range = PropertyIndexKey::prop_value_range(
            prop_id,
            min_bytes.as_deref(),
            max_bytes.as_deref(),
            min_inclusive,
            max_inclusive,
        );

        self.node_ids_from_index_scan(range)
    }

    fn node_property_might_match(
        &self,
        _property: &PropertyKey,
        _op: CompareOp,
        _value: &Value,
    ) -> bool {
        true
    }

    fn edge_property_might_match(
        &self,
        _property: &PropertyKey,
        _op: CompareOp,
        _value: &Value,
    ) -> bool {
        true
    }

    fn statistics(&self) -> Arc<Statistics> {
        let mut stats = Statistics::new();
        stats.total_nodes = self.node_count() as u64;
        stats.total_edges = self.edge_count() as u64;
        Arc::new(stats)
    }

    fn estimate_label_cardinality(&self, label: &str) -> f64 {
        self.nodes_by_label(label).len() as f64
    }

    fn estimate_avg_degree(&self, _edge_type: &str, _outgoing: bool) -> f64 {
        let nc = self.node_count() as f64;
        let ec = self.edge_count() as f64;
        if nc > 0.0 { ec / nc } else { 0.0 }
    }

    fn current_epoch(&self) -> EpochId {
        EpochId(0)
    }
}

// --- Private helper methods ---

impl SlateGraphStore {
    /// Builds a full Node from storage (properties + labels).
    fn build_node(&self, id: NodeId) -> crate::Result<Option<Node>> {
        let properties = self.load_node_properties(id.0)?;
        let labels = self.load_node_labels(id.0)?;

        let property_map = grafeo_common::types::PropertyMap::from_iter(properties);

        Ok(Some(Node {
            id,
            labels: SmallVec::from_vec(labels),
            properties: property_map,
        }))
    }

    /// Builds a full Edge from an EdgeRecordValue.
    fn build_edge(&self, id: EdgeId, val: &EdgeRecordValue) -> crate::Result<Edge> {
        let properties = self.load_edge_properties(id.0)?;

        let edge_type = {
            let catalog = self.catalog.read();
            catalog
                .get_edge_type_name(val.type_id)
                .cloned()
                .unwrap_or_else(|| ArcStr::from("UNKNOWN"))
        };

        let property_map = grafeo_common::types::PropertyMap::from_iter(properties);

        Ok(Edge {
            id,
            src: NodeId(val.src),
            dst: NodeId(val.dst),
            edge_type,
            properties: property_map,
        })
    }

    fn load_node_properties(&self, node_id: u64) -> crate::Result<FxHashMap<PropertyKey, Value>> {
        let records = self.exec(async {
            self.storage
                .scan(NodePropertyKey::node_prefix(node_id))
                .await
        })?;
        let catalog = self.catalog.read();
        let mut props = FxHashMap::default();
        for record in &records {
            if let Ok(key) = NodePropertyKey::decode(&record.key)
                && let Some(name) = catalog.get_prop_key_name(key.prop_key_id)
                && let Ok(val) = values::decode_value(&record.value)
            {
                props.insert(PropertyKey::new(name.as_str()), val);
            }
        }
        Ok(props)
    }

    fn load_edge_properties(&self, edge_id: u64) -> crate::Result<FxHashMap<PropertyKey, Value>> {
        let records = self.exec(async {
            self.storage
                .scan(EdgePropertyKey::edge_prefix(edge_id))
                .await
        })?;
        let catalog = self.catalog.read();
        let mut props = FxHashMap::default();
        for record in &records {
            if let Ok(key) = EdgePropertyKey::decode(&record.key)
                && let Some(name) = catalog.get_prop_key_name(key.prop_key_id)
                && let Ok(val) = values::decode_value(&record.value)
            {
                props.insert(PropertyKey::new(name.as_str()), val);
            }
        }
        Ok(props)
    }

    /// Extracts NodeIds from the last 8 bytes of keys in an index scan.
    fn node_ids_from_index_scan(&self, range: common::BytesRange) -> Vec<NodeId> {
        let Ok(records) = self.exec(async { self.storage.scan(range).await }) else {
            return Vec::new();
        };
        records
            .iter()
            .filter(|r| r.key.len() >= 8)
            .map(|r| {
                NodeId(u64::from_be_bytes(
                    r.key[r.key.len() - 8..].try_into().unwrap(),
                ))
            })
            .collect()
    }

    /// Loads labels for a node from the NodeRecord value.
    fn load_node_labels(&self, node_id: u64) -> crate::Result<Vec<ArcStr>> {
        let key = NodeRecordKey { node_id }.encode();
        let Some(record) = self.exec(async { self.storage.get(key).await })? else {
            return Ok(Vec::new());
        };
        let val = NodeRecordValue::decode(&record.value)?;

        let catalog = self.catalog.read();
        let labels = val
            .label_ids
            .iter()
            .filter_map(|&id| catalog.get_label_name(id).cloned())
            .collect();
        Ok(labels)
    }
}
