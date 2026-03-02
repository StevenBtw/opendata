pub(crate) mod keys;
pub(crate) mod values;

/// Key format version for graph records.
pub(crate) const KEY_VERSION: u8 = 0x01;

/// Record types for graph storage keys.
///
/// Each type occupies the high 4 bits of the record tag byte.
/// The low 4 bits are reserved for sub-type disambiguation
/// (e.g., catalog kind, RDF triple permutation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum RecordType {
    /// Node entity record with MVCC epoch.
    NodeRecord = 1, // tag 0x10
    /// Edge entity record with MVCC epoch.
    EdgeRecord = 2, // tag 0x20
    /// Node property (key -> value).
    NodeProperty = 3, // tag 0x30
    /// Edge property (key -> value).
    EdgeProperty = 4, // tag 0x40
    /// Forward adjacency index (src -> dst via edge).
    ForwardAdj = 5, // tag 0x50
    /// Backward adjacency index (dst -> src via edge).
    BackwardAdj = 6, // tag 0x60
    /// Label index (label_id -> node_id).
    LabelIndex = 7, // tag 0x70
    /// Property value index for range queries.
    PropertyIndex = 8, // tag 0x80
    /// Catalog entries (id -> name, name -> id).
    /// Sub-types via reserved bits: 0=label, 1=edge_type, 2=property_key.
    Catalog = 9, // tag 0x90-0x9F
    /// Metadata counters and epoch tracking.
    Metadata = 14, // tag 0xE0
    /// Sequence allocator blocks.
    Sequence = 15, // tag 0xF0
}

/// Catalog sub-kinds stored in the reserved bits of the record tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum CatalogKind {
    LabelById = 0,
    LabelByName = 1,
    EdgeTypeById = 2,
    EdgeTypeByName = 3,
    PropertyKeyById = 4,
    PropertyKeyByName = 5,
}

impl TryFrom<u8> for CatalogKind {
    type Error = common::serde::DeserializeError;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(Self::LabelById),
            1 => Ok(Self::LabelByName),
            2 => Ok(Self::EdgeTypeById),
            3 => Ok(Self::EdgeTypeByName),
            4 => Ok(Self::PropertyKeyById),
            5 => Ok(Self::PropertyKeyByName),
            other => Err(common::serde::DeserializeError {
                message: format!("unknown catalog kind: {other}"),
            }),
        }
    }
}

/// Metadata sub-types stored after the key prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum MetadataSubType {
    NodeCount = 0,
    EdgeCount = 1,
    CurrentEpoch = 2,
}

impl TryFrom<u8> for MetadataSubType {
    type Error = common::serde::DeserializeError;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(Self::NodeCount),
            1 => Ok(Self::EdgeCount),
            2 => Ok(Self::CurrentEpoch),
            other => Err(common::serde::DeserializeError {
                message: format!("unknown metadata sub-type: {other}"),
            }),
        }
    }
}

/// Sequence allocator sub-types in the reserved bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum SequenceKind {
    /// Node ID sequence allocator.
    NodeId = 0,
    /// Edge ID sequence allocator.
    EdgeId = 1,
}
