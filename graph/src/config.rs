use common::StorageConfig;
use serde::{Deserialize, Serialize};

/// Graph model type.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub enum GraphModel {
    #[default]
    Lpg,
    Rdf,
}

/// Configuration for the graph database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Storage backend configuration.
    pub storage: StorageConfig,
    /// Graph model (LPG or RDF).
    #[serde(default)]
    pub graph_model: GraphModel,
    /// Whether to maintain backward adjacency indexes (incoming edges).
    #[serde(default = "default_true")]
    pub backward_edges: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            storage: StorageConfig::default(),
            graph_model: GraphModel::default(),
            backward_edges: true,
        }
    }
}

fn default_true() -> bool {
    true
}
