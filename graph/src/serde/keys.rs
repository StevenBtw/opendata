use bytes::{BufMut, Bytes, BytesMut};
use common::BytesRange;
use common::serde::DeserializeError;
use common::serde::key_prefix::{KeyPrefix, RecordTag};
use common::serde::terminated_bytes;
use std::ops::Bound;

use super::{CatalogKind, KEY_VERSION, MetadataSubType, RecordType, SequenceKind};

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
    let prefix = KeyPrefix::from_bytes_versioned(data, KEY_VERSION)?;
    if prefix.tag().record_type() != expected as u8 {
        return Err(DeserializeError {
            message: format!("expected {name} tag, got {}", prefix.tag().record_type()),
        });
    }
    Ok(prefix)
}

/// Encodes a key with the standard [version][tag][...fields] layout.
fn encode_key(
    record_type: RecordType,
    reserved: u8,
    capacity: usize,
    f: impl FnOnce(&mut BytesMut),
) -> Bytes {
    let tag = RecordTag::new(record_type as u8, reserved);
    let mut buf = BytesMut::with_capacity(capacity);
    KeyPrefix::new(KEY_VERSION, tag).write_to(&mut buf);
    f(&mut buf);
    buf.freeze()
}

/// Creates a prefix scan range for [version][tag][...id_fields].
fn prefix_range(record_type: RecordType, f: impl FnOnce(&mut BytesMut)) -> BytesRange {
    let tag = RecordTag::new(record_type as u8, 0);
    let mut start = BytesMut::with_capacity(18);
    KeyPrefix::new(KEY_VERSION, tag).write_to(&mut start);
    f(&mut start);
    BytesRange::prefix(start.freeze())
}

// ---------------------------------------------------------------------------
// NodeRecordKey: [ver][0x10][node_id:u64 BE][epoch:u64 BE] = 18 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodeRecordKey {
    pub node_id: u64,
    pub epoch: u64,
}

impl NodeRecordKey {
    const SIZE: usize = 18;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::NodeRecord, 0, Self::SIZE, |buf| {
            buf.put_u64(self.node_id);
            buf.put_u64(self.epoch);
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::NodeRecord, "NodeRecord")?;
        let node_id = u64::from_be_bytes(data[2..10].try_into().unwrap());
        let epoch = u64::from_be_bytes(data[10..18].try_into().unwrap());
        Ok(Self { node_id, epoch })
    }

    pub fn node_prefix(node_id: u64) -> BytesRange {
        prefix_range(RecordType::NodeRecord, |buf| buf.put_u64(node_id))
    }

    pub fn all_nodes_range() -> BytesRange {
        record_type_range(RecordType::NodeRecord)
    }
}

// ---------------------------------------------------------------------------
// EdgeRecordKey: [ver][0x20][edge_id:u64 BE][epoch:u64 BE] = 18 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EdgeRecordKey {
    pub edge_id: u64,
    pub epoch: u64,
}

impl EdgeRecordKey {
    const SIZE: usize = 18;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::EdgeRecord, 0, Self::SIZE, |buf| {
            buf.put_u64(self.edge_id);
            buf.put_u64(self.epoch);
        })
    }

    #[cfg(test)]
    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::EdgeRecord, "EdgeRecord")?;
        let edge_id = u64::from_be_bytes(data[2..10].try_into().unwrap());
        let epoch = u64::from_be_bytes(data[10..18].try_into().unwrap());
        Ok(Self { edge_id, epoch })
    }

    pub fn edge_prefix(edge_id: u64) -> BytesRange {
        prefix_range(RecordType::EdgeRecord, |buf| buf.put_u64(edge_id))
    }
}

// ---------------------------------------------------------------------------
// NodePropertyKey: [ver][0x30][node_id:u64 BE][prop_key:terminated] = 10+ bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodePropertyKey {
    pub node_id: u64,
    pub prop_key: Bytes,
}

impl NodePropertyKey {
    pub fn encode(&self) -> Bytes {
        encode_key(
            RecordType::NodeProperty,
            0,
            10 + self.prop_key.len() + 2,
            |buf| {
                buf.put_u64(self.node_id);
                terminated_bytes::serialize(&self.prop_key, buf);
            },
        )
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, 11, RecordType::NodeProperty, "NodeProperty")?;
        let node_id = u64::from_be_bytes(data[2..10].try_into().unwrap());
        let mut remaining = &data[10..];
        let prop_key = terminated_bytes::deserialize(&mut remaining)?;
        Ok(Self { node_id, prop_key })
    }

    pub fn node_prefix(node_id: u64) -> BytesRange {
        prefix_range(RecordType::NodeProperty, |buf| buf.put_u64(node_id))
    }
}

// ---------------------------------------------------------------------------
// EdgePropertyKey: [ver][0x40][edge_id:u64 BE][prop_key:terminated] = 10+ bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EdgePropertyKey {
    pub edge_id: u64,
    pub prop_key: Bytes,
}

impl EdgePropertyKey {
    pub fn encode(&self) -> Bytes {
        encode_key(
            RecordType::EdgeProperty,
            0,
            10 + self.prop_key.len() + 2,
            |buf| {
                buf.put_u64(self.edge_id);
                terminated_bytes::serialize(&self.prop_key, buf);
            },
        )
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, 11, RecordType::EdgeProperty, "EdgeProperty")?;
        let edge_id = u64::from_be_bytes(data[2..10].try_into().unwrap());
        let mut remaining = &data[10..];
        let prop_key = terminated_bytes::deserialize(&mut remaining)?;
        Ok(Self { edge_id, prop_key })
    }

    pub fn edge_prefix(edge_id: u64) -> BytesRange {
        prefix_range(RecordType::EdgeProperty, |buf| buf.put_u64(edge_id))
    }
}

// ---------------------------------------------------------------------------
// ForwardAdjKey: [ver][0x50][src:u64 BE][type_id:u32 BE][dst:u64 BE] = 22 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForwardAdjKey {
    pub src: u64,
    pub edge_type_id: u32,
    pub dst: u64,
}

impl ForwardAdjKey {
    const SIZE: usize = 22;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::ForwardAdj, 0, Self::SIZE, |buf| {
            buf.put_u64(self.src);
            buf.put_u32(self.edge_type_id);
            buf.put_u64(self.dst);
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::ForwardAdj, "ForwardAdj")?;
        let src = u64::from_be_bytes(data[2..10].try_into().unwrap());
        let edge_type_id = u32::from_be_bytes(data[10..14].try_into().unwrap());
        let dst = u64::from_be_bytes(data[14..22].try_into().unwrap());
        Ok(Self {
            src,
            edge_type_id,
            dst,
        })
    }

    pub fn src_prefix(src: u64) -> BytesRange {
        prefix_range(RecordType::ForwardAdj, |buf| buf.put_u64(src))
    }
}

// ---------------------------------------------------------------------------
// BackwardAdjKey: [ver][0x60][dst:u64 BE][type_id:u32 BE][src:u64 BE] = 22 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackwardAdjKey {
    pub dst: u64,
    pub edge_type_id: u32,
    pub src: u64,
}

impl BackwardAdjKey {
    const SIZE: usize = 22;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::BackwardAdj, 0, Self::SIZE, |buf| {
            buf.put_u64(self.dst);
            buf.put_u32(self.edge_type_id);
            buf.put_u64(self.src);
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::BackwardAdj, "BackwardAdj")?;
        let dst = u64::from_be_bytes(data[2..10].try_into().unwrap());
        let edge_type_id = u32::from_be_bytes(data[10..14].try_into().unwrap());
        let src = u64::from_be_bytes(data[14..22].try_into().unwrap());
        Ok(Self {
            dst,
            edge_type_id,
            src,
        })
    }

    pub fn dst_prefix(dst: u64) -> BytesRange {
        prefix_range(RecordType::BackwardAdj, |buf| buf.put_u64(dst))
    }
}

// ---------------------------------------------------------------------------
// LabelIndexKey: [ver][0x70][label_id:u32 BE][node_id:u64 BE] = 14 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LabelIndexKey {
    pub label_id: u32,
    pub node_id: u64,
}

impl LabelIndexKey {
    const SIZE: usize = 14;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::LabelIndex, 0, Self::SIZE, |buf| {
            buf.put_u32(self.label_id);
            buf.put_u64(self.node_id);
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::LabelIndex, "LabelIndex")?;
        let label_id = u32::from_be_bytes(data[2..6].try_into().unwrap());
        let node_id = u64::from_be_bytes(data[6..14].try_into().unwrap());
        Ok(Self { label_id, node_id })
    }

    pub fn label_prefix(label_id: u32) -> BytesRange {
        prefix_range(RecordType::LabelIndex, |buf| buf.put_u32(label_id))
    }
}

// ---------------------------------------------------------------------------
// PropertyIndexKey: [ver][0x80][prop_id:u32 BE][sortable_value:var][node_id:u64 BE]
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
            14 + self.sortable_value.len(),
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
                let mut buf = BytesMut::with_capacity(6 + min_val.len());
                KeyPrefix::new(KEY_VERSION, tag).write_to(&mut buf);
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
                let mut buf = BytesMut::with_capacity(6);
                KeyPrefix::new(KEY_VERSION, tag).write_to(&mut buf);
                buf.put_u32(prop_id);
                Bound::Included(buf.freeze())
            }
        };

        let end = match max {
            Some(max_val) => {
                let mut buf = BytesMut::with_capacity(6 + max_val.len());
                KeyPrefix::new(KEY_VERSION, tag).write_to(&mut buf);
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
                let mut buf = BytesMut::with_capacity(2);
                KeyPrefix::new(KEY_VERSION, next_tag).write_to(&mut buf);
                Bound::Excluded(buf.freeze())
            }
        };

        BytesRange::new(start, end)
    }
}

// ---------------------------------------------------------------------------
// CatalogKey: [ver][0x9x][id:u32 BE] or [ver][0x9x][name:terminated]
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogByIdKey {
    pub kind: CatalogKind,
    pub id: u32,
}

impl CatalogByIdKey {
    const SIZE: usize = 6;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::Catalog, self.kind as u8, Self::SIZE, |buf| {
            buf.put_u32(self.id);
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        let prefix = decode_prefix(data, Self::SIZE, RecordType::Catalog, "CatalogById")?;
        let kind = CatalogKind::try_from(prefix.tag().reserved())?;
        let id = u32::from_be_bytes(data[2..6].try_into().unwrap());
        Ok(Self { kind, id })
    }

    pub fn kind_prefix(kind: CatalogKind) -> BytesRange {
        let tag = RecordTag::new(RecordType::Catalog as u8, kind as u8);
        BytesRange::prefix(KeyPrefix::new(KEY_VERSION, tag).to_bytes())
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
            2 + self.name.len() + 2,
            |buf| {
                terminated_bytes::serialize(&self.name, buf);
            },
        )
    }

    #[cfg(test)]
    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        let prefix = decode_prefix(data, 3, RecordType::Catalog, "CatalogByName")?;
        let kind = CatalogKind::try_from(prefix.tag().reserved())?;
        let mut remaining = &data[2..];
        let name = terminated_bytes::deserialize(&mut remaining)?;
        Ok(Self { kind, name })
    }
}

// ---------------------------------------------------------------------------
// MetadataKey: [ver][0xE0][sub_type:u8] = 3 bytes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetadataKey {
    pub sub_type: MetadataSubType,
}

impl MetadataKey {
    const SIZE: usize = 3;

    pub fn encode(&self) -> Bytes {
        encode_key(RecordType::Metadata, 0, Self::SIZE, |buf| {
            buf.put_u8(self.sub_type as u8);
        })
    }

    #[cfg(test)]
    pub fn decode(data: &[u8]) -> Result<Self, DeserializeError> {
        decode_prefix(data, Self::SIZE, RecordType::Metadata, "Metadata")?;
        let sub_type = MetadataSubType::try_from(data[2])?;
        Ok(Self { sub_type })
    }
}

// ---------------------------------------------------------------------------
// SequenceKey: [ver][0xFx] = 2 bytes (used by SequenceAllocator)
// ---------------------------------------------------------------------------

pub(crate) struct SequenceKey;

impl SequenceKey {
    pub fn encode(kind: SequenceKind) -> Bytes {
        let tag = RecordTag::new(RecordType::Sequence as u8, kind as u8);
        KeyPrefix::new(KEY_VERSION, tag).to_bytes()
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Creates a `BytesRange` covering all keys of a given record type.
fn record_type_range(record_type: RecordType) -> BytesRange {
    let tag_start = RecordTag::new(record_type as u8, 0);
    let start = KeyPrefix::new(KEY_VERSION, tag_start).to_bytes();

    let rt = record_type as u8;
    if rt < 15 {
        let tag_end = RecordTag::new(rt + 1, 0);
        let end = KeyPrefix::new(KEY_VERSION, tag_end).to_bytes();
        BytesRange::new(Bound::Included(start), Bound::Excluded(end))
    } else {
        let end = Bytes::from_static(&[KEY_VERSION + 1]);
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
        let key = NodeRecordKey {
            node_id: 42,
            epoch: 7,
        };
        let encoded = key.encode();
        assert_eq!(NodeRecordKey::decode(&encoded).unwrap(), key);
        assert_eq!(encoded.len(), NodeRecordKey::SIZE);
    }

    #[test]
    fn should_roundtrip_edge_record_key() {
        let key = EdgeRecordKey {
            edge_id: 100,
            epoch: 3,
        };
        let encoded = key.encode();
        assert_eq!(EdgeRecordKey::decode(&encoded).unwrap(), key);
        assert_eq!(encoded.len(), EdgeRecordKey::SIZE);
    }

    #[test]
    fn should_roundtrip_node_property_key() {
        let key = NodePropertyKey {
            node_id: 42,
            prop_key: Bytes::from("name"),
        };
        let encoded = key.encode();
        assert_eq!(NodePropertyKey::decode(&encoded).unwrap(), key);
    }

    #[test]
    fn should_roundtrip_edge_property_key() {
        let key = EdgePropertyKey {
            edge_id: 99,
            prop_key: Bytes::from("weight"),
        };
        let encoded = key.encode();
        assert_eq!(EdgePropertyKey::decode(&encoded).unwrap(), key);
    }

    #[test]
    fn should_roundtrip_forward_adj_key() {
        let key = ForwardAdjKey {
            src: 1,
            edge_type_id: 5,
            dst: 2,
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
    fn should_order_node_records_by_id_then_epoch() {
        let k1 = NodeRecordKey {
            node_id: 1,
            epoch: 5,
        }
        .encode();
        let k2 = NodeRecordKey {
            node_id: 1,
            epoch: 10,
        }
        .encode();
        let k3 = NodeRecordKey {
            node_id: 2,
            epoch: 1,
        }
        .encode();
        assert!(k1 < k2, "same node, earlier epoch should sort first");
        assert!(k2 < k3, "smaller node_id should sort before larger");
    }

    #[test]
    fn should_order_forward_adj_by_src_type_dst() {
        let k1 = ForwardAdjKey {
            src: 1,
            edge_type_id: 1,
            dst: 10,
        }
        .encode();
        let k2 = ForwardAdjKey {
            src: 1,
            edge_type_id: 1,
            dst: 20,
        }
        .encode();
        let k3 = ForwardAdjKey {
            src: 1,
            edge_type_id: 2,
            dst: 5,
        }
        .encode();
        let k4 = ForwardAdjKey {
            src: 2,
            edge_type_id: 1,
            dst: 1,
        }
        .encode();
        assert!(k1 < k2, "same src+type, dst 10 < dst 20");
        assert!(k2 < k3, "same src, type 1 < type 2");
        assert!(k3 < k4, "src 1 < src 2");
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
        let node = NodeRecordKey {
            node_id: 0,
            epoch: 0,
        }
        .encode();
        let edge = EdgeRecordKey {
            edge_id: 0,
            epoch: 0,
        }
        .encode();
        let nprop = NodePropertyKey {
            node_id: 0,
            prop_key: Bytes::from("a"),
        }
        .encode();
        let eprop = EdgePropertyKey {
            edge_id: 0,
            prop_key: Bytes::from("a"),
        }
        .encode();
        let fwd = ForwardAdjKey {
            src: 0,
            edge_type_id: 0,
            dst: 0,
        }
        .encode();
        let bwd = BackwardAdjKey {
            dst: 0,
            edge_type_id: 0,
            src: 0,
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
    fn should_node_prefix_contain_all_epochs() {
        let range = NodeRecordKey::node_prefix(42);
        assert!(
            range.contains(
                &NodeRecordKey {
                    node_id: 42,
                    epoch: 0
                }
                .encode()
            )
        );
        assert!(
            range.contains(
                &NodeRecordKey {
                    node_id: 42,
                    epoch: u64::MAX
                }
                .encode()
            )
        );
        assert!(
            !range.contains(
                &NodeRecordKey {
                    node_id: 43,
                    epoch: 0
                }
                .encode()
            )
        );
    }

    #[test]
    fn should_forward_adj_src_prefix_contain_all_types_and_dsts() {
        let range = ForwardAdjKey::src_prefix(10);
        assert!(
            range.contains(
                &ForwardAdjKey {
                    src: 10,
                    edge_type_id: 1,
                    dst: 20
                }
                .encode()
            )
        );
        assert!(
            range.contains(
                &ForwardAdjKey {
                    src: 10,
                    edge_type_id: 99,
                    dst: 999
                }
                .encode()
            )
        );
        assert!(
            !range.contains(
                &ForwardAdjKey {
                    src: 11,
                    edge_type_id: 1,
                    dst: 1
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
    fn should_property_key_handle_special_chars() {
        let key = NodePropertyKey {
            node_id: 1,
            prop_key: Bytes::from_static(&[0x00, 0x01, 0xFF]),
        };
        let encoded = key.encode();
        assert_eq!(NodePropertyKey::decode(&encoded).unwrap(), key);
    }
}
