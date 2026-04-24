use bytes::Bytes;
use common::storage::MergeOperator;

use crate::serde::RecordType;
use crate::serde::values::{
    ADJ_MERGE_ADD, ADJ_MERGE_REMOVE, PackedAdj, PackedProps, PROP_MERGE_REMOVE,
    PROP_MERGE_SET,
};

/// Merge operator for graph storage records.
///
/// Dispatches by record type:
/// - **NodeProperty / EdgeProperty** (tags 0x30, 0x40): property merge operands
///   (set/remove) applied sequentially to a packed properties blob.
/// - **ForwardAdj / BackwardAdj** (tags 0x50, 0x60): adjacency merge operands
///   (add/remove) applied sequentially to a packed adjacency blob.
/// - **Metadata** (tag 0xE0): additive i64 counter merge.
/// - Everything else: last-write-wins.
pub struct GraphMergeOperator;

impl MergeOperator for GraphMergeOperator {
    fn merge_batch(&self, key: &Bytes, existing_value: Option<Bytes>, operands: &[Bytes]) -> Bytes {
        // Need at least 3 bytes for key prefix (subsystem, version, tag)
        if key.len() < 3 {
            // Fallback: last-write-wins
            return operands.last().cloned().unwrap_or_default();
        }

        let record_type = (key[2] & 0xF0) >> 4;

        match record_type {
            rt if rt == RecordType::NodeProperty as u8
                || rt == RecordType::EdgeProperty as u8 =>
            {
                merge_properties(existing_value, operands)
            }
            rt if rt == RecordType::ForwardAdj as u8
                || rt == RecordType::BackwardAdj as u8 =>
            {
                merge_adjacency(existing_value, operands)
            }
            rt if rt == RecordType::Metadata as u8 => {
                // Counters: fold all operands additively
                let mut result = existing_value;
                for operand in operands {
                    result = Some(merge_i64_counter(result, operand.clone()));
                }
                result.unwrap_or_default()
            }
            _ => {
                // Last-write-wins for everything else
                operands.last().cloned().unwrap_or_default()
            }
        }
    }
}

/// Merges property operands into a packed properties blob.
fn merge_properties(existing: Option<Bytes>, operands: &[Bytes]) -> Bytes {
    let mut props = match &existing {
        Some(data) => PackedProps::decode(data).unwrap_or_default(),
        None => PackedProps::default(),
    };

    for op in operands {
        if op.is_empty() {
            continue;
        }
        match op[0] {
            PROP_MERGE_SET if op.len() >= 5 => {
                let prop_key_id = u32::from_le_bytes(op[1..5].try_into().unwrap());
                let value_bytes = Bytes::copy_from_slice(&op[5..]);
                props.set(prop_key_id, value_bytes);
            }
            PROP_MERGE_REMOVE if op.len() >= 5 => {
                let prop_key_id = u32::from_le_bytes(op[1..5].try_into().unwrap());
                props.remove(prop_key_id);
            }
            tag => {
                tracing::warn!(tag, len = op.len(), "malformed property merge operand");
            }
        }
    }

    props.encode()
}

/// Merges adjacency operands into a packed adjacency blob.
fn merge_adjacency(existing: Option<Bytes>, operands: &[Bytes]) -> Bytes {
    let mut adj = match &existing {
        Some(data) => PackedAdj::decode(data).unwrap_or_default(),
        None => PackedAdj::default(),
    };

    for op in operands {
        if op.len() < 17 {
            tracing::warn!(len = op.len(), "adjacency merge operand too short (need 17)");
            continue;
        }
        let peer_id = u64::from_le_bytes(op[1..9].try_into().unwrap());
        let edge_id = u64::from_le_bytes(op[9..17].try_into().unwrap());
        match op[0] {
            ADJ_MERGE_ADD => adj.add(peer_id, edge_id),
            ADJ_MERGE_REMOVE => adj.remove(peer_id, edge_id),
            tag => {
                tracing::warn!(tag, "unknown adjacency merge operand tag");
            }
        }
    }

    adj.encode()
}

/// Merges two i64 counters by addition.
///
/// Both existing and new values are interpreted as little-endian i64.
/// Returns the sum as little-endian bytes.
fn merge_i64_counter(existing: Option<Bytes>, new: Bytes) -> Bytes {
    let existing_val = existing
        .as_ref()
        .and_then(|b| {
            if b.len() >= 8 {
                Some(i64::from_le_bytes(b[..8].try_into().unwrap()))
            } else {
                None
            }
        })
        .unwrap_or(0);

    let new_val = if new.len() >= 8 {
        i64::from_le_bytes(new[..8].try_into().unwrap())
    } else {
        0
    };

    Bytes::copy_from_slice(&(existing_val + new_val).to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serde::MetadataSubType;
    use crate::serde::keys::{BackwardAdjKey, ForwardAdjKey, NodePropsKey};
    use crate::serde::values::{
        encode_adj_add_operand, encode_adj_remove_operand, encode_prop_remove_operand,
        encode_prop_set_operand,
    };
    use grafeo_common::types::Value;

    fn merge_counter(existing: Option<i64>, delta: i64) -> i64 {
        let op = GraphMergeOperator;
        let key = crate::serde::keys::MetadataKey {
            sub_type: MetadataSubType::NodeCount,
        }
        .encode();
        let existing = existing.map(|v| Bytes::copy_from_slice(&v.to_le_bytes()));
        let delta = Bytes::copy_from_slice(&delta.to_le_bytes());
        let result = op.merge_batch(&key, existing, &[delta]);
        i64::from_le_bytes(result[..8].try_into().unwrap())
    }

    #[test]
    fn should_merge_counters_additively() {
        assert_eq!(merge_counter(Some(10), 5), 15);
    }

    #[test]
    fn should_merge_counter_with_no_existing() {
        assert_eq!(merge_counter(None, 7), 7);
    }

    #[test]
    fn should_merge_negative_counter_delta() {
        assert_eq!(merge_counter(Some(10), -3), 7);
    }

    #[test]
    fn should_merge_multiple_counter_operands() {
        let op = GraphMergeOperator;
        let key = crate::serde::keys::MetadataKey {
            sub_type: MetadataSubType::NodeCount,
        }
        .encode();
        let o1 = Bytes::copy_from_slice(&3i64.to_le_bytes());
        let o2 = Bytes::copy_from_slice(&5i64.to_le_bytes());
        let result = op.merge_batch(&key, Some(Bytes::copy_from_slice(&10i64.to_le_bytes())), &[o1, o2]);
        let val = i64::from_le_bytes(result[..8].try_into().unwrap());
        assert_eq!(val, 18);
    }

    #[test]
    fn should_last_write_wins_for_non_metadata() {
        let op = GraphMergeOperator;
        let key = Bytes::from_static(&[0x05, 0x01, 0x10, 0x00]);
        let new = Bytes::from("new");
        let result = op.merge_batch(&key, Some(Bytes::from("old")), &[new.clone()]);
        assert_eq!(result, new);
    }

    // --- Property merge tests ---

    #[test]
    fn should_merge_property_set_from_empty() {
        let op = GraphMergeOperator;
        let key = NodePropsKey { node_id: 1 }.encode();
        let operand = encode_prop_set_operand(10, &Value::Int64(42)).unwrap();

        let result = op.merge_batch(&key, None, &[operand]);
        let props = PackedProps::decode(&result).unwrap();
        assert_eq!(props.properties.len(), 1);
        assert_eq!(props.properties[0].0, 10);
    }

    #[test]
    fn should_merge_property_set_overwrite() {
        let op = GraphMergeOperator;
        let key = NodePropsKey { node_id: 1 }.encode();

        // Set initial
        let o1 = encode_prop_set_operand(10, &Value::Int64(42)).unwrap();
        let result = op.merge_batch(&key, None, &[o1]);

        // Overwrite
        let o2 = encode_prop_set_operand(10, &Value::Int64(99)).unwrap();
        let result = op.merge_batch(&key, Some(result), &[o2]);
        let props = PackedProps::decode(&result).unwrap();
        assert_eq!(props.properties.len(), 1);

        let val = crate::serde::values::decode_value(&props.properties[0].1).unwrap();
        assert_eq!(val, Value::Int64(99));
    }

    #[test]
    fn should_merge_property_set_then_remove() {
        let op = GraphMergeOperator;
        let key = NodePropsKey { node_id: 1 }.encode();

        let o1 = encode_prop_set_operand(10, &Value::Int64(42)).unwrap();
        let o2 = encode_prop_set_operand(20, &Value::String("hello".into())).unwrap();
        let result = op.merge_batch(&key, None, &[o1, o2]);

        let o3 = encode_prop_remove_operand(10);
        let result = op.merge_batch(&key, Some(result), &[o3]);
        let props = PackedProps::decode(&result).unwrap();
        assert_eq!(props.properties.len(), 1);
        assert_eq!(props.properties[0].0, 20);
    }

    #[test]
    fn should_merge_multiple_property_operands_in_one_batch() {
        let op = GraphMergeOperator;
        let key = NodePropsKey { node_id: 1 }.encode();

        let o1 = encode_prop_set_operand(1, &Value::Int64(10)).unwrap();
        let o2 = encode_prop_set_operand(2, &Value::Int64(20)).unwrap();
        let o3 = encode_prop_set_operand(1, &Value::Int64(30)).unwrap(); // overwrite
        let o4 = encode_prop_remove_operand(2);

        let result = op.merge_batch(&key, None, &[o1, o2, o3, o4]);
        let props = PackedProps::decode(&result).unwrap();
        assert_eq!(props.properties.len(), 1);
        assert_eq!(props.properties[0].0, 1);
        let val = crate::serde::values::decode_value(&props.properties[0].1).unwrap();
        assert_eq!(val, Value::Int64(30));
    }

    // --- Adjacency merge tests ---

    #[test]
    fn should_merge_adj_add_from_empty() {
        let op = GraphMergeOperator;
        let key = ForwardAdjKey {
            src: 1,
            edge_type_id: 5,
        }
        .encode();
        let operand = encode_adj_add_operand(10, 100);

        let result = op.merge_batch(&key, None, &[operand]);
        let adj = PackedAdj::decode(&result).unwrap();
        assert_eq!(adj.entries, vec![(10, 100)]);
    }

    #[test]
    fn should_merge_adj_add_multiple() {
        let op = GraphMergeOperator;
        let key = ForwardAdjKey {
            src: 1,
            edge_type_id: 5,
        }
        .encode();

        let o1 = encode_adj_add_operand(10, 100);
        let o2 = encode_adj_add_operand(20, 200);
        let result = op.merge_batch(&key, None, &[o1, o2]);
        let adj = PackedAdj::decode(&result).unwrap();
        assert_eq!(adj.entries, vec![(10, 100), (20, 200)]);
    }

    #[test]
    fn should_merge_adj_add_then_remove() {
        let op = GraphMergeOperator;
        let key = BackwardAdjKey {
            dst: 2,
            edge_type_id: 5,
        }
        .encode();

        let o1 = encode_adj_add_operand(10, 100);
        let o2 = encode_adj_add_operand(20, 200);
        let result = op.merge_batch(&key, None, &[o1, o2]);

        let o3 = encode_adj_remove_operand(10, 100);
        let result = op.merge_batch(&key, Some(result), &[o3]);
        let adj = PackedAdj::decode(&result).unwrap();
        assert_eq!(adj.entries, vec![(20, 200)]);
    }

    #[test]
    fn should_merge_adj_add_idempotent() {
        let op = GraphMergeOperator;
        let key = ForwardAdjKey {
            src: 1,
            edge_type_id: 5,
        }
        .encode();

        let o1 = encode_adj_add_operand(10, 100);
        let o2 = encode_adj_add_operand(10, 100); // duplicate
        let result = op.merge_batch(&key, None, &[o1, o2]);
        let adj = PackedAdj::decode(&result).unwrap();
        assert_eq!(adj.entries.len(), 1);
    }

    #[test]
    fn should_ignore_malformed_operands() {
        let op = GraphMergeOperator;
        let key = NodePropsKey { node_id: 1 }.encode();

        // Too short operand
        let malformed = Bytes::from_static(&[0x01, 0x02]);
        let good = encode_prop_set_operand(10, &Value::Int64(42)).unwrap();
        let result = op.merge_batch(&key, None, &[malformed, good]);
        let props = PackedProps::decode(&result).unwrap();
        assert_eq!(props.properties.len(), 1);
    }
}
