use arcstr::ArcStr;
use bytes::Bytes;
use common::storage::Storage;

use crate::error::{Error, Result};
use crate::serde::CatalogKind;
use crate::serde::keys::{CatalogByIdKey, CatalogByNameKey};

/// Bidirectional dictionary mapping IDs to names and back.
///
/// Used for labels, edge types, and property keys. The catalog is loaded
/// from storage on startup and kept in sync by persisting new entries
/// as they are created.
#[derive(Debug)]
pub(crate) struct Catalog {
    labels: BiMap,
    edge_types: BiMap,
    prop_keys: BiMap,
}

/// A simple bidirectional map: u32 ID <-> ArcStr name.
#[derive(Debug, Default)]
struct BiMap {
    by_id: Vec<ArcStr>,
    by_name: hashbrown::HashMap<ArcStr, u32>,
}

impl BiMap {
    fn get_name(&self, id: u32) -> Option<&ArcStr> {
        self.by_id.get(id as usize)
    }

    fn get_id(&self, name: &str) -> Option<u32> {
        self.by_name.get(name).copied()
    }

    fn insert(&mut self, id: u32, name: ArcStr) {
        // Ensure by_id is large enough
        while self.by_id.len() <= id as usize {
            self.by_id.push(ArcStr::default());
        }
        self.by_id[id as usize] = name.clone();
        self.by_name.insert(name, id);
    }

    fn next_id(&self) -> u32 {
        self.by_id.len() as u32
    }

    fn len(&self) -> usize {
        self.by_name.len()
    }
}

impl Catalog {
    /// Loads the catalog from storage by scanning all catalog key prefixes.
    pub async fn load(storage: &dyn Storage) -> Result<Self> {
        let labels = load_bimap(storage, CatalogKind::LabelById).await?;

        let edge_types = load_bimap(storage, CatalogKind::EdgeTypeById).await?;

        let prop_keys = load_bimap(storage, CatalogKind::PropertyKeyById).await?;

        Ok(Self {
            labels,
            edge_types,
            prop_keys,
        })
    }

    // --- Labels ---

    pub fn get_label_name(&self, id: u32) -> Option<&ArcStr> {
        self.labels.get_name(id)
    }

    pub fn get_label_id(&self, name: &str) -> Option<u32> {
        self.labels.get_id(name)
    }

    pub fn label_count(&self) -> usize {
        self.labels.len()
    }

    pub fn get_or_create_label(&mut self, name: &str) -> (u32, Vec<common::storage::RecordOp>) {
        get_or_create(
            &mut self.labels,
            CatalogKind::LabelById,
            CatalogKind::LabelByName,
            name,
        )
    }

    // --- Edge Types ---

    pub fn get_edge_type_name(&self, id: u32) -> Option<&ArcStr> {
        self.edge_types.get_name(id)
    }

    pub fn edge_type_count(&self) -> usize {
        self.edge_types.len()
    }

    pub fn get_or_create_edge_type(&mut self, name: &str) -> (u32, Vec<common::storage::RecordOp>) {
        get_or_create(
            &mut self.edge_types,
            CatalogKind::EdgeTypeById,
            CatalogKind::EdgeTypeByName,
            name,
        )
    }

    // --- Property Keys ---

    pub fn get_prop_key_name(&self, id: u32) -> Option<&ArcStr> {
        self.prop_keys.get_name(id)
    }

    pub fn get_prop_key_id(&self, name: &str) -> Option<u32> {
        self.prop_keys.get_id(name)
    }

    pub fn get_or_create_prop_key(&mut self, name: &str) -> (u32, Vec<common::storage::RecordOp>) {
        get_or_create(
            &mut self.prop_keys,
            CatalogKind::PropertyKeyById,
            CatalogKind::PropertyKeyByName,
            name,
        )
    }
}

/// Gets or creates an entry in a BiMap, returning (id, records_to_persist).
fn get_or_create(
    bimap: &mut BiMap,
    by_id_kind: CatalogKind,
    by_name_kind: CatalogKind,
    name: &str,
) -> (u32, Vec<common::storage::RecordOp>) {
    if let Some(id) = bimap.get_id(name) {
        return (id, Vec::new());
    }
    let id = bimap.next_id();
    let arc_name = ArcStr::from(name);
    bimap.insert(id, arc_name.clone());
    (id, catalog_put_ops(by_id_kind, by_name_kind, id, &arc_name))
}

/// Loads a BiMap from catalog-by-id entries in storage.
async fn load_bimap(storage: &dyn Storage, kind: CatalogKind) -> Result<BiMap> {
    let range = CatalogByIdKey::kind_prefix(kind);
    let records = storage.scan(range).await?;

    let mut bimap = BiMap::default();
    for record in records {
        let catalog_key = CatalogByIdKey::decode(&record.key)?;
        let name = ArcStr::from(
            std::str::from_utf8(&record.value)
                .map_err(|e| Error::Encoding(format!("invalid UTF-8 in catalog value: {e}")))?,
        );
        bimap.insert(catalog_key.id, name);
    }
    Ok(bimap)
}

/// Creates Put records for both by-id and by-name catalog entries.
fn catalog_put_ops(
    by_id_kind: CatalogKind,
    by_name_kind: CatalogKind,
    id: u32,
    name: &ArcStr,
) -> Vec<common::storage::RecordOp> {
    use common::storage::{PutRecordOp, Record, RecordOp};

    let by_id_key = CatalogByIdKey {
        kind: by_id_kind,
        id,
    }
    .encode();
    let by_name_key = CatalogByNameKey {
        kind: by_name_kind,
        name: Bytes::copy_from_slice(name.as_bytes()),
    }
    .encode();

    vec![
        RecordOp::Put(PutRecordOp::from(Record::new(
            by_id_key,
            Bytes::copy_from_slice(name.as_bytes()),
        ))),
        RecordOp::Put(PutRecordOp::from(Record::new(
            by_name_key,
            Bytes::copy_from_slice(&id.to_le_bytes()),
        ))),
    ]
}
