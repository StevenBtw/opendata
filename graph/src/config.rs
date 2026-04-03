use common::StorageConfig;
use serde::{Deserialize, Serialize};

/// Graph model type.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub enum GraphModel {
    #[default]
    Lpg,
    Rdf,
}

/// Storage layout strategy for properties and adjacency data.
///
/// Controls how node/edge properties and adjacency entries are stored
/// in the underlying KV store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StorageLayout {
    /// Each property and adjacency entry is a separate KV row (current design).
    #[default]
    Individual,
    /// Properties and adjacency packed into single keys, updated via merge operator.
    /// Gives fast reads (single lookup) and fast writes (no read-before-write).
    Merged,
}

/// Configuration for the graph database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Storage backend configuration.
    pub storage: StorageConfig,
    /// Graph model (LPG or RDF).
    #[serde(default)]
    pub graph_model: GraphModel,
    /// Storage layout strategy (Individual or Merged).
    #[serde(default)]
    pub storage_layout: StorageLayout,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            storage: StorageConfig::default(),
            graph_model: GraphModel::default(),
            storage_layout: StorageLayout::default(),
        }
    }
}
