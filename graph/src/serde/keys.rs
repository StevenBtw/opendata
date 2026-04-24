use bytes::{BufMut, Bytes, BytesMut};
use common::BytesRange;
use common::serde::DeserializeError;
use common::serde::key_prefix::KeyPrefix;
use common::serde::record_tag::RecordTag;
use std::ops::Bound;

use super::{CatalogKind, KEY_VERSION, MetadataSubType, RecordType, SequenceKind};

/// Subsystem identifier for graph storage keys.
const SUBSYSTEM: u8 = 0x05;

// ---------------------------------------------------------------------------
// Shared encode/decode helpers
// ---------------------------------------------------------------------------

/// Validates key prefix: checks minimum length, version, and record type.
fn decode_prefix(
    data: &[u8],
    min_len: usize,
    expected: RecordType,
    name: &str,
) -> Result<KeyPrefix, DeserializeError> {
    if data.len() < min_len {
        return Err(DeserializeError {
            message: format!("{name} too short: need {min_len}, got {}", data.len()),
        });
    }
    let prefix = KeyPrefix::from_bytes_with_validation(data, SUBSYSTEM, KEY_VERSION)?;
    let tag = RecordTag::from_byte(prefix.tag())?;
    if tag.record_type() != expected as u8 {
        return Err(DeserializeError {
            message: format!("expected {name} tag, got {}", tag.record_type()),
        });
    }
    Ok(prefix)
}

/// Encodes a key with the standard [subsystem][version][tag][...fields] layout.
fn encode_key(
    record_type: RecordType,
    reserved: u8,
    capacity: usize,
    f: impl FnOnce(&mut BytesMut),
) -> Bytes {
    let tag = RecordTag::new(record_type as u8, reserved);
    let mut buf = BytesMut::with_capacity(capacity);
    KeyPrefix::new(SUBSYSTEM, KEY_VERSION, tag.as_byte()).write_to(&mut buf);
    f(&mut buf);
    buf.freeze()
}

/// Creates a prefix scan range for [subsystem][version][tag][...id_fields].
fn prefix_range(record_type: RecordType, f: impl FnOnce(&mut BytesMut)) -> BytesRange {
    let tag = RecordTag::new(record_type as u8, 0);
    let mut start = BytesMut::with_capacity(19);
    KeyPrefix::new(SUBSYSTEM, KEY_VERSION, tag.as_byte()).write_to(&mut start);
    f(&mut start);
    BytesRange::prefix(start.freeze())
}

// ---------------------------------------------------------------------------
// NodeRecordKey: [sub][ver][0x10][node_id:u64 BE] = 11 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodeRecordKey {
    pub node_id: u64,
}

impl NodeRecordKey {
    const SIZE: usize = 11;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::NodeRecord, 0, Self::SIZE, |buf| {
            buf.put_u64(self.node_id);
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::NodeRecord, "NodeRecord")?;
        let node_id = u64::from_be_bytes(data[3..11].try_into().unwrap());
        Ok(Self { node_id })
    }

    pub fn all_nodes_range() -> BytesRange {
        record_type_range(RecordType::NodeRecord)
    }
}

// ---------------------------------------------------------------------------
// EdgeRecordKey: [sub][ver][0x20][edge_id:u64 BE] = 11 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EdgeRecordKey {
    pub edge_id: u64,
}

impl EdgeRecordKey {
    const SIZE: usize = 11;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::EdgeRecord, 0, Self::SIZE, |buf| {
            buf.put_u64(self.edge_id);
        })
    }

    #[cfg(test)]
    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::EdgeRecord, "EdgeRecord")?;
        let edge_id = u64::from_be_bytes(data[3..11].try_into().unwrap());
        Ok(Self { edge_id })
    }
}

// ---------------------------------------------------------------------------
// LabelIndexKey: [sub][ver][0x70][label_id:u32 BE][node_id:u64 BE] = 15 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LabelIndexKey {
    pub label_id: u32,
    pub node_id: u64,
}

impl LabelIndexKey {
    const SIZE: usize = 15;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::LabelIndex, 0, Self::SIZE, |buf| {
            buf.put_u32(self.label_id);
            buf.put_u64(self.node_id);
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::LabelIndex, "LabelIndex")?;
        let label_id = u32::from_be_bytes(data[3..7].try_into().unwrap());
        let node_id = u64::from_be_bytes(data[7..15].try_into().unwrap());
        Ok(Self { label_id, node_id })
    }

    pub fn label_prefix(label_id: u32) -> BytesRange {
        prefix_range(RecordType::LabelIndex, |buf| buf.put_u32(label_id))
    }
}

// ---------------------------------------------------------------------------
// PropertyIndexKey: [sub][ver][0x80][prop_id:u32 BE][sortable_value:var][node_id:u64 BE]
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PropertyIndexKey {
    pub prop_id: u32,
    pub sortable_value: Bytes,
    pub node_id: u64,
}

impl PropertyIndexKey {
    pub fn encode(&self) -> Bytes {
        encode_key(
            RecordType::PropertyIndex,
            0,
            15 + self.sortable_value.len(),
            |buf| {
                buf.put_u32(self.prop_id);
                buf.extend_from_slice(&self.sortable_value);
                buf.put_u64(self.node_id);
            },
        )
    }

    pub fn prop_value_prefix(prop_id: u32, sortable_value: &[u8]) -> BytesRange {
        prefix_range(RecordType::PropertyIndex, |buf| {
            buf.put_u32(prop_id);
            buf.extend_from_slice(sortable_value);
        })
    }

    pub fn prop_value_range(
        prop_id: u32,
        min: Option<&[u8]>,
        max: Option<&[u8]>,
        min_inclusive: bool,
        max_inclusive: bool,
    ) -> BytesRange {
        let tag = RecordTag::new(RecordType::PropertyIndex as u8, 0);

        let start = match min {
            Some(min_val) => {
                let mut buf = BytesMut::with_capacity(7 + min_val.len());
                KeyPrefix::new(SUBSYSTEM, KEY_VERSION, tag.as_byte()).write_to(&mut buf);
                buf.put_u32(prop_id);
                buf.extend_from_slice(min_val);
                if min_inclusive {
                    Bound::Included(buf.freeze())
                } else {
                    buf.put_u64(u64::MAX);
                    Bound::Excluded(buf.freeze())
                }
            }
            None => {
                let mut buf = BytesMut::with_capacity(7);
                KeyPrefix::new(SUBSYSTEM, KEY_VERSION, tag.as_byte()).write_to(&mut buf);
                buf.put_u32(prop_id);
                Bound::Included(buf.freeze())
            }
        };

        let end = match max {
            Some(max_val) => {
                let mut buf = BytesMut::with_capacity(7 + max_val.len());
                KeyPrefix::new(SUBSYSTEM, KEY_VERSION, tag.as_byte()).write_to(&mut buf);
                buf.put_u32(prop_id);
                buf.extend_from_slice(max_val);
                if max_inclusive {
                    buf.put_u64(u64::MAX);
                    Bound::Included(buf.freeze())
                } else {
                    Bound::Excluded(buf.freeze())
                }
            }
            None => {
                let next_tag = RecordTag::new(RecordType::PropertyIndex as u8 + 1, 0);
                let mut buf = BytesMut::with_capacity(3);
                KeyPrefix::new(SUBSYSTEM, KEY_VERSION, next_tag.as_byte()).write_to(&mut buf);
                Bound::Excluded(buf.freeze())
            }
        };

        BytesRange::new(start, end)
    }
}

// ---------------------------------------------------------------------------
// CatalogKey: [sub][ver][0x9x][id:u32 BE] or [sub][ver][0x9x][name:terminated]
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogByIdKey {
    pub kind: CatalogKind,
    pub id: u32,
}

impl CatalogByIdKey {
    const SIZE: usize = 7;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::Catalog, self.kind as u8, Self::SIZE, |buf| {
            buf.put_u32(self.id);
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        let prefix = decode_prefix(data, Self::SIZE, RecordType::Catalog, "CatalogById")?;
        let tag = RecordTag::from_byte(prefix.tag())?;
        let kind = CatalogKind::try_from(tag.reserved())?;
        let id = u32::from_be_bytes(data[3..7].try_into().unwrap());
        Ok(Self { kind, id })
    }

    pub fn kind_prefix(kind: CatalogKind) -> BytesRange {
        let tag = RecordTag::new(RecordType::Catalog as u8, kind as u8);
        BytesRange::prefix(KeyPrefix::new(SUBSYSTEM, KEY_VERSION, tag.as_byte()).to_bytes())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogByNameKey {
    pub kind: CatalogKind,
    pub name: Bytes,
}

impl CatalogByNameKey {
    pub fn encode(&self) -> Bytes {
        encode_key(
            RecordType::Catalog,
            self.kind as u8,
            3 + self.name.len(),
            |buf| {
                buf.extend_from_slice(&self.name);
            },
        )
    }

    #[cfg(test)]
    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        let prefix = decode_prefix(data, 4, RecordType::Catalog, "CatalogByName")?;
        let tag = RecordTag::from_byte(prefix.tag())?;
        let kind = CatalogKind::try_from(tag.reserved())?;
        let name = Bytes::copy_from_slice(&data[3..]);
        Ok(Self { kind, name })
    }
}

// ---------------------------------------------------------------------------
// NodePropsKey: [sub][ver][0x30][node_id:u64 BE] = 11 bytes
// All node properties are packed into a single value updated via the merge op.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodePropsKey {
    pub node_id: u64,
}

impl NodePropsKey {
    const SIZE: usize = 11;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::NodeProperty, 0, Self::SIZE, |buf| {
            buf.put_u64(self.node_id);
        })
    }

    #[cfg(test)]
    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::NodeProperty, "NodeProps")?;
        let node_id = u64::from_be_bytes(data[3..11].try_into().unwrap());
        Ok(Self { node_id })
    }
}

// ---------------------------------------------------------------------------
// EdgePropsKey: [sub][ver][0x40][edge_id:u64 BE] = 11 bytes
// All edge properties are packed into a single value updated via the merge op.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EdgePropsKey {
    pub edge_id: u64,
}

impl EdgePropsKey {
    const SIZE: usize = 11;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::EdgeProperty, 0, Self::SIZE, |buf| {
            buf.put_u64(self.edge_id);
        })
    }

    #[cfg(test)]
    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::EdgeProperty, "EdgeProps")?;
        let edge_id = u64::from_be_bytes(data[3..11].try_into().unwrap());
        Ok(Self { edge_id })
    }
}

// ---------------------------------------------------------------------------
// ForwardAdjKey: [sub][ver][0x50][src:u64 BE][type_id:u32 BE] = 15 bytes
// All forward adjacency entries per (src, type) are packed into one value via merge op.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForwardAdjKey {
    pub src: u64,
    pub edge_type_id: u32,
}

impl ForwardAdjKey {
    const SIZE: usize = 15;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::ForwardAdj, 0, Self::SIZE, |buf| {
            buf.put_u64(self.src);
            buf.put_u32(self.edge_type_id);
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::ForwardAdj, "ForwardAdj")?;
        let src = u64::from_be_bytes(data[3..11].try_into().unwrap());
        let edge_type_id = u32::from_be_bytes(data[11..15].try_into().unwrap());
        Ok(Self { src, edge_type_id })
    }

    /// Prefix range covering all edge types for a given src node.
    pub fn src_prefix(src: u64) -> BytesRange {
        prefix_range(RecordType::ForwardAdj, |buf| buf.put_u64(src))
    }
}

// ---------------------------------------------------------------------------
// BackwardAdjKey: [sub][ver][0x60][dst:u64 BE][type_id:u32 BE] = 15 bytes
// All backward adjacency entries per (dst, type) are packed into one value via merge op.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackwardAdjKey {
    pub dst: u64,
    pub edge_type_id: u32,
}

impl BackwardAdjKey {
    const SIZE: usize = 15;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::BackwardAdj, 0, Self::SIZE, |buf| {
            buf.put_u64(self.dst);
            buf.put_u32(self.edge_type_id);
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::BackwardAdj, "BackwardAdj")?;
        let dst = u64::from_be_bytes(data[3..11].try_into().unwrap());
        let edge_type_id = u32::from_be_bytes(data[11..15].try_into().unwrap());
        Ok(Self { dst, edge_type_id })
    }

    /// Prefix range covering all edge types for a given dst node.
    pub fn dst_prefix(dst: u64) -> BytesRange {
        prefix_range(RecordType::BackwardAdj, |buf| buf.put_u64(dst))
    }
}

// ---------------------------------------------------------------------------
// MetadataKey: [sub][ver][0xE0][sub_type:u8] = 4 bytes
// Aggregate counters only (NodeCount, EdgeCount). Per-type counters use
// `KeyedMetadataKey` with a trailing u32 id.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetadataKey {
    pub sub_type: MetadataSubType,
}

impl MetadataKey {
    const SIZE: usize = 4;

    pub fn encode(&self) -> Bytes {
        debug_assert!(
            !self.sub_type.is_keyed(),
            "use KeyedMetadataKey for per-type counters"
        );
        encode_key(RecordType::Metadata, 0, Self::SIZE, |buf| {
            buf.put_u8(self.sub_type as u8);
        })
    }

    #[cfg(test)]
    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::Metadata, "Metadata")?;
        let sub_type = MetadataSubType::try_from(data[3])?;
        Ok(Self { sub_type })
    }
}

// ---------------------------------------------------------------------------
// KeyedMetadataKey: [sub][ver][0xE0][sub_type:u8][id:u32 BE] = 8 bytes
// Per-label / per-edge-type counters. Gives the optimizer per-type cardinality
// and degree estimates without scanning LabelIndex or adjacency.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyedMetadataKey {
    pub sub_type: MetadataSubType,
    pub id: u32,
}

impl KeyedMetadataKey {
    const SIZE: usize = 8;

    pub fn encode(&self) -> Bytes {
        debug_assert!(
            self.sub_type.is_keyed(),
            "KeyedMetadataKey only valid for per-type sub_types"
        );
        encode_key(RecordType::Metadata, 0, Self::SIZE, |buf| {
            buf.put_u8(self.sub_type as u8);
            buf.put_u32(self.id);
        })
    }

    #[cfg(test)]
    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::Metadata, "KeyedMetadata")?;
        let sub_type = MetadataSubType::try_from(data[3])?;
        let id = u32::from_be_bytes(data[4..8].try_into().unwrap());
        Ok(Self { sub_type, id })
    }
}

// ---------------------------------------------------------------------------
// SequenceKey: [sub][ver][0xFx] = 3 bytes (used by SequenceAllocator)
// ---------------------------------------------------------------------------

pub(crate) struct SequenceKey;

impl SequenceKey {
    pub fn encode(kind: SequenceKind) -> Bytes {
        let tag = RecordTag::new(RecordType::Sequence as u8, kind as u8);
        KeyPrefix::new(SUBSYSTEM, KEY_VERSION, tag.as_byte()).to_bytes()
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Creates a `BytesRange` covering all keys of a given record type.
fn record_type_range(record_type: RecordType) -> BytesRange {
    let tag_start = RecordTag::new(record_type as u8, 0);
    let start = KeyPrefix::new(SUBSYSTEM, KEY_VERSION, tag_start.as_byte()).to_bytes();

    let rt = record_type as u8;
    if rt < 15 {
        let tag_end = RecordTag::new(rt + 1, 0);
        let end = KeyPrefix::new(SUBSYSTEM, KEY_VERSION, tag_end.as_byte()).to_bytes();
        BytesRange::new(Bound::Included(start), Bound::Excluded(end))
    } else {
        let mut buf = BytesMut::with_capacity(2);
        buf.put_u8(SUBSYSTEM);
        buf.put_u8(KEY_VERSION + 1);
        let end = buf.freeze();
        BytesRange::new(Bound::Included(start), Bound::Excluded(end))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Roundtrip tests ---

    #[test]
    fn should_roundtrip_node_record_key() {
        let key = NodeRecordKey { node_id: 42 };
        let encoded = key.encode();
        assert_eq!(NodeRecordKey::decode(&encoded).unwrap(), key);
        assert_eq!(encoded.len(), NodeRecordKey::SIZE);
    }

    #[test]
    fn should_roundtrip_edge_record_key() {
        let key = EdgeRecordKey { edge_id: 100 };
        let encoded = key.encode();
        assert_eq!(EdgeRecordKey::decode(&encoded).unwrap(), key);
        assert_eq!(encoded.len(), EdgeRecordKey::SIZE);
    }

    #[test]
    fn should_roundtrip_node_props_key() {
        let key = NodePropsKey { node_id: 42 };
        let encoded = key.encode();
        assert_eq!(NodePropsKey::decode(&encoded).unwrap(), key);
        assert_eq!(encoded.len(), NodePropsKey::SIZE);
    }

    #[test]
    fn should_roundtrip_edge_props_key() {
        let key = EdgePropsKey { edge_id: 99 };
        let encoded = key.encode();
        assert_eq!(EdgePropsKey::decode(&encoded).unwrap(), key);
        assert_eq!(encoded.len(), EdgePropsKey::SIZE);
    }

    #[test]
    fn should_roundtrip_forward_adj_key() {
        let key = ForwardAdjKey {
            src: 1,
            edge_type_id: 5,
        };
        let encoded = key.encode();
        assert_eq!(ForwardAdjKey::decode(&encoded).unwrap(), key);
        assert_eq!(encoded.len(), ForwardAdjKey::SIZE);
    }

    #[test]
    fn should_roundtrip_backward_adj_key() {
        let key = BackwardAdjKey {
            dst: 2,
            edge_type_id: 5,
        };
        let encoded = key.encode();
        assert_eq!(BackwardAdjKey::decode(&encoded).unwrap(), key);
        assert_eq!(encoded.len(), BackwardAdjKey::SIZE);
    }

    #[test]
    fn should_roundtrip_label_index_key() {
        let key = LabelIndexKey {
            label_id: 3,
            node_id: 42,
        };
        let encoded = key.encode();
        assert_eq!(LabelIndexKey::decode(&encoded).unwrap(), key);
        assert_eq!(encoded.len(), LabelIndexKey::SIZE);
    }

    #[test]
    fn should_roundtrip_catalog_by_id_key() {
        let key = CatalogByIdKey {
            kind: CatalogKind::LabelById,
            id: 42,
        };
        let encoded = key.encode();
        assert_eq!(CatalogByIdKey::decode(&encoded).unwrap(), key);
        assert_eq!(encoded.len(), CatalogByIdKey::SIZE);
    }

    #[test]
    fn should_roundtrip_catalog_by_name_key() {
        let key = CatalogByNameKey {
            kind: CatalogKind::EdgeTypeByName,
            name: Bytes::from("KNOWS"),
        };
        let encoded = key.encode();
        assert_eq!(CatalogByNameKey::decode(&encoded).unwrap(), key);
    }

    #[test]
    fn should_roundtrip_metadata_key() {
        let key = MetadataKey {
            sub_type: MetadataSubType::NodeCount,
        };
        let encoded = key.encode();
        assert_eq!(MetadataKey::decode(&encoded).unwrap(), key);
        assert_eq!(encoded.len(), MetadataKey::SIZE);
    }

    #[test]
    fn should_roundtrip_keyed_metadata_key() {
        let key = KeyedMetadataKey {
            sub_type: MetadataSubType::LabelNodeCount,
            id: 0x0A0B0C0D,
        };
        let encoded = key.encode();
        assert_eq!(KeyedMetadataKey::decode(&encoded).unwrap(), key);
        assert_eq!(encoded.len(), KeyedMetadataKey::SIZE);
        assert_eq!(&encoded[4..8], &[0x0A, 0x0B, 0x0C, 0x0D], "id must be big-endian");
    }

    #[test]
    fn should_order_keyed_metadata_by_sub_type_then_id() {
        let k_label_1 = KeyedMetadataKey {
            sub_type: MetadataSubType::LabelNodeCount,
            id: 1,
        }
        .encode();
        let k_label_2 = KeyedMetadataKey {
            sub_type: MetadataSubType::LabelNodeCount,
            id: 2,
        }
        .encode();
        let k_type_1 = KeyedMetadataKey {
            sub_type: MetadataSubType::EdgeTypeCount,
            id: 1,
        }
        .encode();
        assert!(k_label_1 < k_label_2, "same sub_type, id 1 < id 2");
        assert!(k_label_2 < k_type_1, "LabelNodeCount sub_type < EdgeTypeCount");
    }

    // --- Byte-order validation tests ---
    // Guard against regressions: assert exact byte patterns in encoded keys
    // to ensure big-endian encoding is preserved (put_u64/put_u32 are BE).

    #[test]
    fn should_encode_node_record_key_big_endian() {
        let key = NodeRecordKey {
            node_id: 0x0102030405060708,
        };
        let encoded = key.encode();
        assert_eq!(
            &encoded[3..11],
            &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
            "node_id must be big-endian"
        );
    }

    #[test]
    fn should_encode_node_props_key_big_endian() {
        let key = NodePropsKey {
            node_id: 0x0102030405060708,
        };
        let encoded = key.encode();
        assert_eq!(
            &encoded[3..11],
            &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
            "node_id must be big-endian"
        );
    }

    #[test]
    fn should_encode_forward_adj_key_big_endian() {
        let key = ForwardAdjKey {
            src: 0x0102030405060708,
            edge_type_id: 0x0A0B0C0D,
        };
        let encoded = key.encode();
        assert_eq!(&encoded[3..11], &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08], "src BE");
        assert_eq!(&encoded[11..15], &[0x0A, 0x0B, 0x0C, 0x0D], "edge_type_id BE");
    }

    #[test]
    fn should_encode_label_index_key_big_endian() {
        let key = LabelIndexKey {
            label_id: 0x0A0B0C0D,
            node_id: 0x0102030405060708,
        };
        let encoded = key.encode();
        assert_eq!(&encoded[3..7], &[0x0A, 0x0B, 0x0C, 0x0D], "label_id BE");
        assert_eq!(&encoded[7..15], &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08], "node_id BE");
    }

    #[test]
    fn should_encode_catalog_by_id_key_big_endian() {
        let key = CatalogByIdKey {
            kind: CatalogKind::LabelById,
            id: 0x0A0B0C0D,
        };
        let encoded = key.encode();
        assert_eq!(&encoded[3..7], &[0x0A, 0x0B, 0x0C, 0x0D], "id BE");
    }

    // --- Ordering tests ---

    #[test]
    fn should_order_node_records_by_id() {
        let k1 = NodeRecordKey { node_id: 1 }.encode();
        let k2 = NodeRecordKey { node_id: 2 }.encode();
        assert!(k1 < k2, "smaller node_id should sort before larger");
    }

    #[test]
    fn should_order_forward_adj_by_src_then_type() {
        let k1 = ForwardAdjKey {
            src: 1,
            edge_type_id: 1,
        }
        .encode();
        let k2 = ForwardAdjKey {
            src: 1,
            edge_type_id: 2,
        }
        .encode();
        let k3 = ForwardAdjKey {
            src: 2,
            edge_type_id: 1,
        }
        .encode();
        assert!(k1 < k2, "same src, type 1 < type 2");
        assert!(k2 < k3, "src 1 < src 2");
    }

    #[test]
    fn should_order_label_index_by_label_then_node() {
        let k1 = LabelIndexKey {
            label_id: 1,
            node_id: 100,
        }
        .encode();
        let k2 = LabelIndexKey {
            label_id: 1,
            node_id: 200,
        }
        .encode();
        let k3 = LabelIndexKey {
            label_id: 2,
            node_id: 50,
        }
        .encode();
        assert!(k1 < k2, "same label, node 100 < node 200");
        assert!(k2 < k3, "label 1 < label 2");
    }

    #[test]
    fn should_separate_record_types_lexicographically() {
        let node = NodeRecordKey { node_id: 0 }.encode();
        let edge = EdgeRecordKey { edge_id: 0 }.encode();
        let nprop = NodePropsKey { node_id: 0 }.encode();
        let eprop = EdgePropsKey { edge_id: 0 }.encode();
        let fwd = ForwardAdjKey {
            src: 0,
            edge_type_id: 0,
        }
        .encode();
        let bwd = BackwardAdjKey {
            dst: 0,
            edge_type_id: 0,
        }
        .encode();
        let label = LabelIndexKey {
            label_id: 0,
            node_id: 0,
        }
        .encode();
        let meta = MetadataKey {
            sub_type: MetadataSubType::NodeCount,
        }
        .encode();

        assert!(node < edge);
        assert!(edge < nprop);
        assert!(nprop < eprop);
        assert!(eprop < fwd);
        assert!(fwd < bwd);
        assert!(bwd < label);
        assert!(label < meta);
    }

    // --- Prefix containment tests ---

    #[test]
    fn should_forward_adj_src_prefix_contain_all_types() {
        let range = ForwardAdjKey::src_prefix(10);
        assert!(range.contains(
            &ForwardAdjKey {
                src: 10,
                edge_type_id: 1,
            }
            .encode()
        ));
        assert!(range.contains(
            &ForwardAdjKey {
                src: 10,
                edge_type_id: 99,
            }
            .encode()
        ));
        assert!(!range.contains(
            &ForwardAdjKey {
                src: 11,
                edge_type_id: 1,
            }
            .encode()
        ));
    }

    #[test]
    fn should_backward_adj_dst_prefix_contain_all_types() {
        let range = BackwardAdjKey::dst_prefix(10);
        assert!(range.contains(
            &BackwardAdjKey {
                dst: 10,
                edge_type_id: 1,
            }
            .encode()
        ));
        assert!(!range.contains(
            &BackwardAdjKey {
                dst: 11,
                edge_type_id: 1,
            }
            .encode()
        ));
    }

    #[test]
    fn should_label_prefix_contain_all_nodes() {
        let range = LabelIndexKey::label_prefix(5);
        assert!(
            range.contains(
                &LabelIndexKey {
                    label_id: 5,
                    node_id: 1
                }
                .encode()
            )
        );
        assert!(
            range.contains(
                &LabelIndexKey {
                    label_id: 5,
                    node_id: u64::MAX
                }
                .encode()
            )
        );
        assert!(
            !range.contains(
                &LabelIndexKey {
                    label_id: 6,
                    node_id: 1
                }
                .encode()
            )
        );
    }

}
