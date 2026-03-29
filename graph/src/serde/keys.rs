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
// NodePropertyKey: [sub][ver][0x30][node_id:u64 BE][prop_key_id:u32 BE] = 15 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodePropertyKey {
    pub node_id: u64,
    pub prop_key_id: u32,
}

impl NodePropertyKey {
    const SIZE: usize = 15;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::NodeProperty, 0, Self::SIZE, |buf| {
            buf.put_u64(self.node_id);
            buf.put_u32(self.prop_key_id);
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::NodeProperty, "NodeProperty")?;
        let node_id = u64::from_be_bytes(data[3..11].try_into().unwrap());
        let prop_key_id = u32::from_be_bytes(data[11..15].try_into().unwrap());
        Ok(Self {
            node_id,
            prop_key_id,
        })
    }

    pub fn node_prefix(node_id: u64) -> BytesRange {
        prefix_range(RecordType::NodeProperty, |buf| buf.put_u64(node_id))
    }
}

// ---------------------------------------------------------------------------
// EdgePropertyKey: [sub][ver][0x40][edge_id:u64 BE][prop_key_id:u32 BE] = 15 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EdgePropertyKey {
    pub edge_id: u64,
    pub prop_key_id: u32,
}

impl EdgePropertyKey {
    const SIZE: usize = 15;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::EdgeProperty, 0, Self::SIZE, |buf| {
            buf.put_u64(self.edge_id);
            buf.put_u32(self.prop_key_id);
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::EdgeProperty, "EdgeProperty")?;
        let edge_id = u64::from_be_bytes(data[3..11].try_into().unwrap());
        let prop_key_id = u32::from_be_bytes(data[11..15].try_into().unwrap());
        Ok(Self {
            edge_id,
            prop_key_id,
        })
    }

    pub fn edge_prefix(edge_id: u64) -> BytesRange {
        prefix_range(RecordType::EdgeProperty, |buf| buf.put_u64(edge_id))
    }
}

// ---------------------------------------------------------------------------
// ForwardAdjKey: [sub][ver][0x50][src:u64 BE][type_id:u32 BE][dst:u64 BE][edge_id:u64 BE] = 31 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForwardAdjKey {
    pub src: u64,
    pub edge_type_id: u32,
    pub dst: u64,
    pub edge_id: u64,
}

impl ForwardAdjKey {
    const SIZE: usize = 31;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::ForwardAdj, 0, Self::SIZE, |buf| {
            buf.put_u64(self.src);
            buf.put_u32(self.edge_type_id);
            buf.put_u64(self.dst);
            buf.put_u64(self.edge_id);
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::ForwardAdj, "ForwardAdj")?;
        let src = u64::from_be_bytes(data[3..11].try_into().unwrap());
        let edge_type_id = u32::from_be_bytes(data[11..15].try_into().unwrap());
        let dst = u64::from_be_bytes(data[15..23].try_into().unwrap());
        let edge_id = u64::from_be_bytes(data[23..31].try_into().unwrap());
        Ok(Self {
            src,
            edge_type_id,
            dst,
            edge_id,
        })
    }

    pub fn src_prefix(src: u64) -> BytesRange {
        prefix_range(RecordType::ForwardAdj, |buf| buf.put_u64(src))
    }
}

// ---------------------------------------------------------------------------
// BackwardAdjKey: [sub][ver][0x60][dst:u64 BE][type_id:u32 BE][src:u64 BE][edge_id:u64 BE] = 31 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackwardAdjKey {
    pub dst: u64,
    pub edge_type_id: u32,
    pub src: u64,
    pub edge_id: u64,
}

impl BackwardAdjKey {
    const SIZE: usize = 31;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::BackwardAdj, 0, Self::SIZE, |buf| {
            buf.put_u64(self.dst);
            buf.put_u32(self.edge_type_id);
            buf.put_u64(self.src);
            buf.put_u64(self.edge_id);
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::BackwardAdj, "BackwardAdj")?;
        let dst = u64::from_be_bytes(data[3..11].try_into().unwrap());
        let edge_type_id = u32::from_be_bytes(data[11..15].try_into().unwrap());
        let src = u64::from_be_bytes(data[15..23].try_into().unwrap());
        let edge_id = u64::from_be_bytes(data[23..31].try_into().unwrap());
        Ok(Self {
            dst,
            edge_type_id,
            src,
            edge_id,
        })
    }

    pub fn dst_prefix(dst: u64) -> BytesRange {
        prefix_range(RecordType::BackwardAdj, |buf| buf.put_u64(dst))
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
// MetadataKey: [sub][ver][0xE0][sub_type:u8] = 4 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetadataKey {
    pub sub_type: MetadataSubType,
}

impl MetadataKey {
    const SIZE: usize = 4;

    pub fn encode(&self) -> Bytes {
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
    fn should_roundtrip_node_property_key() {
        let key = NodePropertyKey {
            node_id: 42,
            prop_key_id: 7,
        };
        let encoded = key.encode();
        assert_eq!(NodePropertyKey::decode(&encoded).unwrap(), key);
        assert_eq!(encoded.len(), NodePropertyKey::SIZE);
    }

    #[test]
    fn should_roundtrip_edge_property_key() {
        let key = EdgePropertyKey {
            edge_id: 99,
            prop_key_id: 3,
        };
        let encoded = key.encode();
        assert_eq!(EdgePropertyKey::decode(&encoded).unwrap(), key);
        assert_eq!(encoded.len(), EdgePropertyKey::SIZE);
    }

    #[test]
    fn should_roundtrip_forward_adj_key() {
        let key = ForwardAdjKey {
            src: 1,
            edge_type_id: 5,
            dst: 2,
            edge_id: 100,
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
            src: 1,
            edge_id: 100,
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

    // --- Ordering tests ---

    #[test]
    fn should_order_node_records_by_id() {
        let k1 = NodeRecordKey { node_id: 1 }.encode();
        let k2 = NodeRecordKey { node_id: 2 }.encode();
        assert!(k1 < k2, "smaller node_id should sort before larger");
    }

    #[test]
    fn should_order_forward_adj_by_src_type_dst_edge() {
        let k1 = ForwardAdjKey {
            src: 1,
            edge_type_id: 1,
            dst: 10,
            edge_id: 1,
        }
        .encode();
        let k2 = ForwardAdjKey {
            src: 1,
            edge_type_id: 1,
            dst: 20,
            edge_id: 2,
        }
        .encode();
        let k3 = ForwardAdjKey {
            src: 1,
            edge_type_id: 2,
            dst: 5,
            edge_id: 3,
        }
        .encode();
        let k4 = ForwardAdjKey {
            src: 2,
            edge_type_id: 1,
            dst: 1,
            edge_id: 4,
        }
        .encode();
        assert!(k1 < k2, "same src+type, dst 10 < dst 20");
        assert!(k2 < k3, "same src, type 1 < type 2");
        assert!(k3 < k4, "src 1 < src 2");

        // Multi-edges: same src+type+dst, different edge_id
        let k5 = ForwardAdjKey {
            src: 1,
            edge_type_id: 1,
            dst: 10,
            edge_id: 100,
        }
        .encode();
        assert!(k1 < k5, "same src+type+dst, edge_id 1 < edge_id 100");
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
        let nprop = NodePropertyKey {
            node_id: 0,
            prop_key_id: 0,
        }
        .encode();
        let eprop = EdgePropertyKey {
            edge_id: 0,
            prop_key_id: 0,
        }
        .encode();
        let fwd = ForwardAdjKey {
            src: 0,
            edge_type_id: 0,
            dst: 0,
            edge_id: 0,
        }
        .encode();
        let bwd = BackwardAdjKey {
            dst: 0,
            edge_type_id: 0,
            src: 0,
            edge_id: 0,
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
    fn should_forward_adj_src_prefix_contain_all_types_and_dsts() {
        let range = ForwardAdjKey::src_prefix(10);
        assert!(
            range.contains(
                &ForwardAdjKey {
                    src: 10,
                    edge_type_id: 1,
                    dst: 20,
                    edge_id: 1,
                }
                .encode()
            )
        );
        assert!(
            range.contains(
                &ForwardAdjKey {
                    src: 10,
                    edge_type_id: 99,
                    dst: 999,
                    edge_id: 42,
                }
                .encode()
            )
        );
        assert!(
            !range.contains(
                &ForwardAdjKey {
                    src: 11,
                    edge_type_id: 1,
                    dst: 1,
                    edge_id: 1,
                }
                .encode()
            )
        );
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

    #[test]
    fn should_order_node_property_by_node_then_prop_key_id() {
        let k1 = NodePropertyKey {
            node_id: 1,
            prop_key_id: 0,
        }
        .encode();
        let k2 = NodePropertyKey {
            node_id: 1,
            prop_key_id: 1,
        }
        .encode();
        let k3 = NodePropertyKey {
            node_id: 2,
            prop_key_id: 0,
        }
        .encode();
        assert!(k1 < k2, "same node, prop_key_id 0 < 1");
        assert!(k2 < k3, "node 1 < node 2");
    }
}
