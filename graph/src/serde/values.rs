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
// EdgeRecordValue: src(8) + dst(8) + type_id(4) + prop_count(2) = 22 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EdgeRecordValue {
    pub src: u64,
    pub dst: u64,
    pub type_id: u32,
    pub prop_count: u16,
}

impl EdgeRecordValue {
    const SIZE: usize = 22;

    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(Self::SIZE);
        buf.put_u64_le(self.src);
        buf.put_u64_le(self.dst);
        buf.put_u32_le(self.type_id);
        buf.put_u16_le(self.prop_count);
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
        let prop_count = u16::from_le_bytes(data[20..22].try_into().unwrap());
        Ok(Self {
            src,
            dst,
            type_id,
            prop_count,
        })
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
            prop_count: 1,
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
}
