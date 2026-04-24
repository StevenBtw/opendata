use bytes::{BufMut, Bytes, BytesMut};
use common::serde::sortable::{encode_f64_sortable, encode_i64_sortable};
use common::serde::terminated_bytes;
use grafeo_common::types::Value;

// ---------------------------------------------------------------------------
// NodeRecordValue: FixedElementArray<u32 LE> (count derived from value_length / 4)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodeRecordValue {
    pub label_ids: Vec<u32>,
}

impl NodeRecordValue {
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(self.label_ids.len() * 4);
        for &id in &self.label_ids {
            buf.put_u32_le(id);
        }
        buf.freeze()
    }

    pub fn decode(data: &[u8]) -> Result<Self, crate::Error> {
        if data.len() % 4 != 0 {
            return Err(crate::Error::Encoding(format!(
                "NodeRecordValue length {} is not a multiple of 4",
                data.len()
            )));
        }
        let label_count = data.len() / 4;
        let mut label_ids = Vec::with_capacity(label_count);
        for i in 0..label_count {
            let offset = i * 4;
            label_ids.push(u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()));
        }
        Ok(Self { label_ids })
    }
}

// ---------------------------------------------------------------------------
// EdgeRecordValue: src(8) + dst(8) + type_id(4) = 20 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EdgeRecordValue {
    pub src: u64,
    pub dst: u64,
    pub type_id: u32,
}

impl EdgeRecordValue {
    const SIZE: usize = 20;

    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(Self::SIZE);
        buf.put_u64_le(self.src);
        buf.put_u64_le(self.dst);
        buf.put_u32_le(self.type_id);
        buf.freeze()
    }

    pub fn decode(data: &[u8]) -> Result<Self, crate::Error> {
        if data.len() < Self::SIZE {
            return Err(crate::Error::Encoding(format!(
                "EdgeRecordValue too short: need {}, got {}",
                Self::SIZE,
                data.len()
            )));
        }
        let src = u64::from_le_bytes(data[0..8].try_into().unwrap());
        let dst = u64::from_le_bytes(data[8..16].try_into().unwrap());
        let type_id = u32::from_le_bytes(data[16..20].try_into().unwrap());
        Ok(Self { src, dst, type_id })
    }
}

// ---------------------------------------------------------------------------
// Property value encoding (delegated to grafeo-common bincode)
// ---------------------------------------------------------------------------

/// Encodes a Grafeo Value to bytes for storage.
pub(crate) fn encode_value(value: &Value) -> Result<Bytes, crate::Error> {
    let bytes = value
        .serialize()
        .map_err(|e| crate::Error::Encoding(format!("failed to serialize value: {e}")))?;
    Ok(Bytes::from(bytes))
}

/// Decodes a Grafeo Value from stored bytes.
pub(crate) fn decode_value(data: &[u8]) -> Result<Value, crate::Error> {
    Value::deserialize(data)
        .map_err(|e| crate::Error::Encoding(format!("failed to deserialize value: {e}")))
}

// ---------------------------------------------------------------------------
// Merge operand constants and encoding
// ---------------------------------------------------------------------------

/// Actions for property merge operands.
pub(crate) const PROP_MERGE_SET: u8 = 0x01;
pub(crate) const PROP_MERGE_REMOVE: u8 = 0x02;

/// Actions for adjacency merge operands.
pub(crate) const ADJ_MERGE_ADD: u8 = 0x01;
pub(crate) const ADJ_MERGE_REMOVE: u8 = 0x02;

/// Encodes a "set property" merge operand: [0x01][prop_key_id:u32 LE][value_bytes...]
pub(crate) fn encode_prop_set_operand(prop_key_id: u32, value: &Value) -> Result<Bytes, crate::Error> {
    let val_bytes = encode_value(value)?;
    let mut buf = BytesMut::with_capacity(5 + val_bytes.len());
    buf.put_u8(PROP_MERGE_SET);
    buf.put_u32_le(prop_key_id);
    buf.extend_from_slice(&val_bytes);
    Ok(buf.freeze())
}

/// Encodes a "remove property" merge operand: [0x02][prop_key_id:u32 LE]
pub(crate) fn encode_prop_remove_operand(prop_key_id: u32) -> Bytes {
    let mut buf = BytesMut::with_capacity(5);
    buf.put_u8(PROP_MERGE_REMOVE);
    buf.put_u32_le(prop_key_id);
    buf.freeze()
}

/// Encodes an "add edge" adjacency operand: [0x01][peer_id:u64 LE][edge_id:u64 LE]
pub(crate) fn encode_adj_add_operand(peer_id: u64, edge_id: u64) -> Bytes {
    let mut buf = BytesMut::with_capacity(17);
    buf.put_u8(ADJ_MERGE_ADD);
    buf.put_u64_le(peer_id);
    buf.put_u64_le(edge_id);
    buf.freeze()
}

/// Encodes a "remove edge" adjacency operand: [0x02][peer_id:u64 LE][edge_id:u64 LE]
pub(crate) fn encode_adj_remove_operand(peer_id: u64, edge_id: u64) -> Bytes {
    let mut buf = BytesMut::with_capacity(17);
    buf.put_u8(ADJ_MERGE_REMOVE);
    buf.put_u64_le(peer_id);
    buf.put_u64_le(edge_id);
    buf.freeze()
}

// ---------------------------------------------------------------------------
// PackedProps: [count:u32 LE]([prop_key_id:u32 LE][val_len:u32 LE][val_bytes])*
// ---------------------------------------------------------------------------

/// Packed properties value stored at NodePropsKey or EdgePropsKey.
///
/// Stores all properties of an entity in a single value, with each property
/// identified by its catalog prop_key_id and raw serialized value bytes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PackedProps {
    pub properties: Vec<(u32, Bytes)>, // (prop_key_id, serialized value bytes)
}

impl PackedProps {
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(4 + self.properties.len() * 12);
        buf.put_u32_le(self.properties.len() as u32);
        for (prop_key_id, value_bytes) in &self.properties {
            buf.put_u32_le(*prop_key_id);
            buf.put_u32_le(value_bytes.len() as u32);
            buf.extend_from_slice(value_bytes);
        }
        buf.freeze()
    }

    pub fn decode(data: &[u8]) -> Result<Self, crate::Error> {
        if data.len() < 4 {
            return Err(crate::Error::Encoding(
                "PackedProps too short for count".to_string(),
            ));
        }
        let count = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
        let mut offset = 4;
        let mut properties = Vec::with_capacity(count);
        for _ in 0..count {
            if offset + 8 > data.len() {
                return Err(crate::Error::Encoding(
                    "PackedProps truncated at prop header".to_string(),
                ));
            }
            let prop_key_id = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            let value_len =
                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            if offset + value_len > data.len() {
                return Err(crate::Error::Encoding(
                    "PackedProps truncated at value bytes".to_string(),
                ));
            }
            let value_bytes = Bytes::copy_from_slice(&data[offset..offset + value_len]);
            offset += value_len;
            properties.push((prop_key_id, value_bytes));
        }
        Ok(Self { properties })
    }

    /// Gets the raw value bytes for a property key, if present.
    pub fn get(&self, prop_key_id: u32) -> Option<&Bytes> {
        self.properties
            .iter()
            .find(|(k, _)| *k == prop_key_id)
            .map(|(_, v)| v)
    }

    /// Sets a property value (replaces if existing, appends if new).
    pub fn set(&mut self, prop_key_id: u32, value: Bytes) {
        if let Some(pos) = self.properties.iter().position(|(k, _)| *k == prop_key_id) {
            self.properties[pos].1 = value;
        } else {
            self.properties.push((prop_key_id, value));
        }
    }

    /// Removes a property by key id.
    pub fn remove(&mut self, prop_key_id: u32) {
        self.properties.retain(|(k, _)| *k != prop_key_id);
    }
}

// ---------------------------------------------------------------------------
// PackedAdj: [count:u32 LE]([peer_id:u64 LE][edge_id:u64 LE])*
// ---------------------------------------------------------------------------

/// Packed adjacency value stored at ForwardAdjKey or BackwardAdjKey.
///
/// Stores all adjacency entries for a (node, edge_type) pair in a single value.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PackedAdj {
    pub entries: Vec<(u64, u64)>, // (peer_node_id, edge_id)
}

impl PackedAdj {
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(4 + self.entries.len() * 16);
        buf.put_u32_le(self.entries.len() as u32);
        for &(peer_id, edge_id) in &self.entries {
            buf.put_u64_le(peer_id);
            buf.put_u64_le(edge_id);
        }
        buf.freeze()
    }

    pub fn decode(data: &[u8]) -> Result<Self, crate::Error> {
        if data.len() < 4 {
            return Err(crate::Error::Encoding(
                "PackedAdj too short for count".to_string(),
            ));
        }
        let count = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
        let expected_len = 4 + count * 16;
        if data.len() < expected_len {
            return Err(crate::Error::Encoding(format!(
                "PackedAdj too short: need {expected_len}, got {}",
                data.len()
            )));
        }
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let offset = 4 + i * 16;
            let peer_id = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
            let edge_id = u64::from_le_bytes(data[offset + 8..offset + 16].try_into().unwrap());
            entries.push((peer_id, edge_id));
        }
        Ok(Self { entries })
    }

    /// Adds an adjacency entry if not already present.
    pub fn add(&mut self, peer_id: u64, edge_id: u64) {
        if !self.entries.iter().any(|&(p, e)| p == peer_id && e == edge_id) {
            self.entries.push((peer_id, edge_id));
        }
    }

    /// Removes a matching (peer_id, edge_id) entry.
    pub fn remove(&mut self, peer_id: u64, edge_id: u64) {
        self.entries.retain(|&(p, e)| !(p == peer_id && e == edge_id));
    }
}

// ---------------------------------------------------------------------------
// Sortable value encoding (for PropertyIndex keys)
// ---------------------------------------------------------------------------

/// Encodes a value for use in PropertyIndex keys, preserving sort order.
///
/// Returns `None` for types that cannot be meaningfully sorted (Null, List, Map, etc.).
pub(crate) fn encode_sortable_value(value: &Value) -> Option<Bytes> {
    match value {
        Value::Bool(b) => Some(Bytes::from_static(if *b { &[1] } else { &[0] })),
        Value::Int64(n) => Some(Bytes::copy_from_slice(
            &encode_i64_sortable(*n).to_be_bytes(),
        )),
        Value::Float64(f) => Some(Bytes::copy_from_slice(
            &encode_f64_sortable(*f).to_be_bytes(),
        )),
        Value::String(s) => Some(terminated_bytes::serialize_to_bytes(s.as_bytes())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_roundtrip_node_record_value() {
        let val = NodeRecordValue {
            label_ids: vec![0, 1, 5],
        };
        let encoded = val.encode();
        let decoded = NodeRecordValue::decode(&encoded).unwrap();
        assert_eq!(decoded, val);
    }

    #[test]
    fn should_roundtrip_node_record_value_no_labels() {
        let val = NodeRecordValue {
            label_ids: vec![],
        };
        let encoded = val.encode();
        let decoded = NodeRecordValue::decode(&encoded).unwrap();
        assert_eq!(decoded, val);
    }

    #[test]
    fn should_roundtrip_edge_record_value() {
        let val = EdgeRecordValue {
            src: 10,
            dst: 20,
            type_id: 3,
        };

        // when
        let encoded = val.encode();
        let decoded = EdgeRecordValue::decode(&encoded).unwrap();

        // then
        assert_eq!(decoded, val);
    }

    #[test]
    fn should_roundtrip_grafeo_value() {
        // given
        let values = vec![
            Value::Null,
            Value::Bool(true),
            Value::Int64(42),
            Value::Float64(1.23),
            Value::String("hello".into()),
        ];

        for value in values {
            // when
            let encoded = encode_value(&value).unwrap();
            let decoded = decode_value(&encoded).unwrap();

            // then
            assert_eq!(decoded, value, "roundtrip failed for {value:?}");
        }
    }

    #[test]
    fn should_encode_sortable_int64_preserving_order() {
        // given
        let values: Vec<i64> = vec![-1000, -1, 0, 1, 1000];

        // when
        let encoded: Vec<Bytes> = values
            .iter()
            .map(|v| encode_sortable_value(&Value::Int64(*v)).unwrap())
            .collect();

        // then: lexicographic order should match numeric order
        for window in encoded.windows(2) {
            assert!(
                window[0] < window[1],
                "sortable ordering violated for int64"
            );
        }
    }

    #[test]
    fn should_encode_sortable_float64_preserving_order() {
        // given
        let values: Vec<f64> = vec![-100.0, -1.0, 0.0, 1.0, 100.0];

        // when
        let encoded: Vec<Bytes> = values
            .iter()
            .map(|v| encode_sortable_value(&Value::Float64(*v)).unwrap())
            .collect();

        // then: lexicographic order should match numeric order
        for window in encoded.windows(2) {
            assert!(
                window[0] < window[1],
                "sortable ordering violated for float64"
            );
        }
    }

    #[test]
    fn should_encode_sortable_string_preserving_order() {
        // given
        let values = ["apple", "banana", "cherry"];

        // when
        let encoded: Vec<Bytes> = values
            .iter()
            .map(|v| encode_sortable_value(&Value::String((*v).into())).unwrap())
            .collect();

        // then
        for window in encoded.windows(2) {
            assert!(
                window[0] < window[1],
                "sortable ordering violated for string"
            );
        }
    }

    #[test]
    fn should_return_none_for_unsortable_types() {
        assert!(encode_sortable_value(&Value::Null).is_none());
    }

    // --- PackedProps tests ---

    #[test]
    fn should_roundtrip_packed_props() {
        let v1 = encode_value(&Value::String("hello".into())).unwrap();
        let v2 = encode_value(&Value::Int64(42)).unwrap();
        let val = PackedProps {
            properties: vec![(1, v1.clone()), (2, v2.clone())],
        };
        let encoded = val.encode();
        let decoded = PackedProps::decode(&encoded).unwrap();
        assert_eq!(decoded, val);
    }

    #[test]
    fn should_roundtrip_packed_props_empty() {
        let val = PackedProps::default();
        let encoded = val.encode();
        let decoded = PackedProps::decode(&encoded).unwrap();
        assert_eq!(decoded, val);
    }

    #[test]
    fn should_packed_props_set_and_get() {
        let mut val = PackedProps::default();
        let v1 = Bytes::from_static(&[1, 2, 3]);
        let v2 = Bytes::from_static(&[4, 5, 6]);

        val.set(10, v1.clone());
        assert_eq!(val.get(10), Some(&v1));
        assert_eq!(val.get(99), None);

        // Overwrite
        val.set(10, v2.clone());
        assert_eq!(val.get(10), Some(&v2));
        assert_eq!(val.properties.len(), 1);
    }

    #[test]
    fn should_packed_props_remove() {
        let mut val = PackedProps::default();
        val.set(1, Bytes::from_static(&[1]));
        val.set(2, Bytes::from_static(&[2]));
        val.remove(1);
        assert_eq!(val.get(1), None);
        assert_eq!(val.properties.len(), 1);
    }

    #[test]
    fn should_packed_props_reject_truncated() {
        assert!(PackedProps::decode(&[0, 0]).is_err());
        assert!(PackedProps::decode(&[1, 0, 0, 0]).is_err());
    }

    // --- PackedAdj tests ---

    #[test]
    fn should_roundtrip_packed_adj() {
        let val = PackedAdj {
            entries: vec![(10, 100), (20, 200), (30, 300)],
        };
        let encoded = val.encode();
        let decoded = PackedAdj::decode(&encoded).unwrap();
        assert_eq!(decoded, val);
    }

    #[test]
    fn should_roundtrip_packed_adj_empty() {
        let val = PackedAdj::default();
        let encoded = val.encode();
        let decoded = PackedAdj::decode(&encoded).unwrap();
        assert_eq!(decoded, val);
    }

    #[test]
    fn should_packed_adj_add_and_remove() {
        let mut val = PackedAdj::default();
        val.add(10, 100);
        val.add(20, 200);
        assert_eq!(val.entries.len(), 2);

        // Duplicate add should be idempotent
        val.add(10, 100);
        assert_eq!(val.entries.len(), 2);

        val.remove(10, 100);
        assert_eq!(val.entries.len(), 1);
        assert_eq!(val.entries[0], (20, 200));
    }

    #[test]
    fn should_packed_adj_reject_truncated() {
        assert!(PackedAdj::decode(&[0, 0]).is_err());
        // Claims 1 entry but insufficient data
        assert!(PackedAdj::decode(&[1, 0, 0, 0]).is_err());
    }

    // --- Merge operand encoding tests ---

    #[test]
    fn should_encode_prop_set_operand() {
        let operand = encode_prop_set_operand(42, &Value::Int64(99)).unwrap();
        assert_eq!(operand[0], PROP_MERGE_SET);
        let prop_key_id = u32::from_le_bytes(operand[1..5].try_into().unwrap());
        assert_eq!(prop_key_id, 42);
        // Rest is the encoded value
        let val = decode_value(&operand[5..]).unwrap();
        assert_eq!(val, Value::Int64(99));
    }

    #[test]
    fn should_encode_prop_remove_operand() {
        let operand = encode_prop_remove_operand(42);
        assert_eq!(operand[0], PROP_MERGE_REMOVE);
        let prop_key_id = u32::from_le_bytes(operand[1..5].try_into().unwrap());
        assert_eq!(prop_key_id, 42);
        assert_eq!(operand.len(), 5);
    }

    #[test]
    fn should_encode_adj_add_operand() {
        let operand = encode_adj_add_operand(10, 100);
        assert_eq!(operand[0], ADJ_MERGE_ADD);
        let peer_id = u64::from_le_bytes(operand[1..9].try_into().unwrap());
        let edge_id = u64::from_le_bytes(operand[9..17].try_into().unwrap());
        assert_eq!(peer_id, 10);
        assert_eq!(edge_id, 100);
    }

    #[test]
    fn should_encode_adj_remove_operand() {
        let operand = encode_adj_remove_operand(10, 100);
        assert_eq!(operand[0], ADJ_MERGE_REMOVE);
        let peer_id = u64::from_le_bytes(operand[1..9].try_into().unwrap());
        let edge_id = u64::from_le_bytes(operand[9..17].try_into().unwrap());
        assert_eq!(peer_id, 10);
        assert_eq!(edge_id, 100);
    }
}
