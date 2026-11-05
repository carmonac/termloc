//! A [`StoreClient`] that keeps embeddings on this machine instead of on a server.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Context as _, anyhow};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::store_client::{IntermediateNode, StoreClient};
use super::{
    CodebaseContextConfig, ContentHash, EmbeddingConfig, Error, Fragment, NodeHash, RepoMetadata,
};

/// How many fragments a search returns.
const MAX_RELEVANT_FRAGMENTS: usize = 20;
/// Fragments per embedding request. Local models are slow, and a smaller request fails less work.
const EMBEDDING_REQUEST_SIZE: usize = 32;
const EMBEDDING_CADENCE: Duration = Duration::from_secs(300);

/// Turns text into vectors with a user-chosen model.
#[async_trait]
pub trait Embedder: Send + Sync + 'static {
    /// The model in use, if one is configured.
    fn model(&self) -> Option<String>;

    /// Returns one vector per text, in order.
    async fn embed(&self, model: &str, texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>>;
}

#[derive(Serialize, Deserialize)]
enum Record {
    Node { hash: String, children: Vec<String> },
    Embedding { hash: String, vector: Vec<f32> },
}

/// Everything known for one embedding model. Vectors from different models aren't comparable, so
/// each model has its own log.
struct ModelStore {
    model: String,
    nodes: HashMap<NodeHash, Vec<NodeHash>>,
    /// Normalized, so that a dot product is the cosine similarity.
    vectors: HashMap<NodeHash, Vec<f32>>,
    log: BufWriter<File>,
}

impl ModelStore {
    fn open(directory: &Path, model: &str) -> anyhow::Result<Self> {
        fs::create_dir_all(directory)
            .with_context(|| format!("could not create {}", directory.display()))?;
        let path = log_path(directory, model);
        let mut nodes = HashMap::new();
        let mut vectors = HashMap::new();
        if let Ok(file) = File::open(&path) {
            let mut reader = BufReader::new(file);
            let mut valid_length = 0;
            while let Ok(record) = bincode::deserialize_from::<_, Record>(&mut reader) {
                valid_length = reader.stream_position()?;
                match record {
                    Record::Node { hash, children } => {
                        let children = children
                            .iter()
                            .map(|child| NodeHash::from_str(child))
                            .collect::<Result<_, _>>()?;
                        nodes.insert(NodeHash::from_str(&hash)?, children);
                    }
                    Record::Embedding { hash, vector } => {
                        vectors.insert(NodeHash::from_str(&hash)?, vector);
                    }
                }
            }
            // A crash can leave a partial last record, which would hide anything appended later.
            OpenOptions::new()
                .write(true)
                .open(&path)?
                .set_len(valid_length)?;
        }
        let log = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            model: model.to_owned(),
            nodes,
            vectors,
            log: BufWriter::new(log),
        })
    }

    fn append(&mut self, record: &Record) -> anyhow::Result<()> {
        bincode::serialize_into(&mut self.log, record).context("could not write embedding")
    }

    fn contains(&self, hash: &NodeHash) -> bool {
        self.nodes.contains_key(hash) || self.vectors.contains_key(hash)
    }

    /// The fragments under `root` with the highest similarity to `query`, best first.
    fn nearest(&self, root: &NodeHash, query: &[f32]) -> Vec<NodeHash> {
        let mut pending = vec![root];
        let mut seen = HashSet::new();
        let mut scored = Vec::new();
        while let Some(hash) = pending.pop() {
            if !seen.insert(hash) {
                continue;
            }
            if let Some(children) = self.nodes.get(hash) {
                pending.extend(children);
            } else if let Some(vector) = self.vectors.get(hash) {
                let score: f32 = vector.iter().zip(query).map(|(a, b)| a * b).sum();
                scored.push((score, hash));
            }
        }
        scored.sort_by(|(a, _), (b, _)| b.total_cmp(a));
        scored
            .into_iter()
            .take(MAX_RELEVANT_FRAGMENTS)
            .map(|(_, hash)| hash.clone())
            .collect()
    }
}

fn log_path(directory: &Path, model: &str) -> PathBuf {
    let name: String = model
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    directory.join(format!("{name}.log"))
}

fn normalized(mut vector: Vec<f32>) -> Vec<f32> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        vector.iter_mut().for_each(|value| *value /= norm);
    }
    vector
}

/// Stores Merkle nodes and embeddings in append-only logs in a directory.
pub struct LocalStore {
    embedder: Arc<dyn Embedder>,
    directory: PathBuf,
    store: Mutex<Option<ModelStore>>,
}

impl LocalStore {
    pub fn new(embedder: Arc<dyn Embedder>, directory: PathBuf) -> Self {
        Self {
            embedder,
            directory,
            store: Mutex::new(None),
        }
    }

    fn model(&self) -> Result<String, Error> {
        self.embedder
            .model()
            .ok_or_else(|| anyhow!("no embedding model is selected").into())
    }

    /// Runs `f` on the store for `model`, loading it if another model was in use.
    fn with_store<T>(
        &self,
        model: &str,
        f: impl FnOnce(&mut ModelStore) -> anyhow::Result<T>,
    ) -> Result<T, Error> {
        let mut guard: MutexGuard<'_, Option<ModelStore>> =
            self.store.lock().unwrap_or_else(|err| err.into_inner());
        if guard.as_ref().is_none_or(|store| store.model != model) {
            *guard = Some(ModelStore::open(&self.directory, model)?);
        }
        let store = guard.as_mut().expect("store was just opened");
        let result = f(store)?;
        store.log.flush().map_err(anyhow::Error::from)?;
        Ok(result)
    }
}

#[async_trait]
impl StoreClient for LocalStore {
    async fn update_intermediate_nodes(
        &self,
        _embedding_config: EmbeddingConfig,
        nodes: Vec<IntermediateNode>,
    ) -> Result<HashMap<NodeHash, bool>, Error> {
        let model = self.model()?;
        self.with_store(&model, |store| {
            let mut results = HashMap::with_capacity(nodes.len());
            for node in nodes {
                store.append(&Record::Node {
                    hash: node.hash.to_string(),
                    children: node.children.iter().map(ToString::to_string).collect(),
                })?;
                results.insert(node.hash.clone(), true);
                store.nodes.insert(node.hash, node.children);
            }
            Ok(results)
        })
    }

    async fn generate_embeddings(
        &self,
        _embedding_config: EmbeddingConfig,
        fragments: Vec<Fragment>,
        _root_hash: NodeHash,
        _repo_metadata: RepoMetadata,
    ) -> Result<HashMap<ContentHash, bool>, Error> {
        let model = self.model()?;
        let mut results = HashMap::with_capacity(fragments.len());
        for chunk in fragments.chunks(EMBEDDING_REQUEST_SIZE) {
            let texts = chunk.iter().map(|f| f.content.clone()).collect();
            let vectors = self.embedder.embed(&model, texts).await?;
            if vectors.len() != chunk.len() {
                return Err(anyhow!(
                    "expected {} embeddings from {model}, got {}",
                    chunk.len(),
                    vectors.len()
                )
                .into());
            }
            self.with_store(&model, |store| {
                for (fragment, vector) in chunk.iter().zip(vectors) {
                    let vector = normalized(vector);
                    let hash = NodeHash::from(fragment.content_hash());
                    store.append(&Record::Embedding {
                        hash: hash.to_string(),
                        vector: vector.clone(),
                    })?;
                    store.vectors.insert(hash, vector);
                    results.insert(fragment.content_hash().clone(), true);
                }
                Ok(())
            })?;
        }
        Ok(results)
    }

    async fn populate_merkle_tree_cache(
        &self,
        _embedding_config: EmbeddingConfig,
        _root_hash: NodeHash,
        _repo_metadata: RepoMetadata,
    ) -> Result<bool, Error> {
        Ok(true)
    }

    async fn sync_merkle_tree(
        &self,
        nodes: Vec<NodeHash>,
        _embedding_config: EmbeddingConfig,
    ) -> Result<HashSet<NodeHash>, Error> {
        let model = self.model()?;
        self.with_store(&model, |store| {
            Ok(nodes
                .into_iter()
                .filter(|hash| !store.contains(hash))
                .collect())
        })
    }

    /// Fragments already arrive ordered by similarity.
    async fn rerank_fragments(
        &self,
        _query: String,
        fragments: Vec<Fragment>,
    ) -> Result<Vec<Fragment>, Error> {
        Ok(fragments)
    }

    async fn get_relevant_fragments(
        &self,
        _embedding_config: EmbeddingConfig,
        query: String,
        root_hash: NodeHash,
        _repo_metadata: RepoMetadata,
    ) -> Result<Vec<ContentHash>, Error> {
        let model = self.model()?;
        let query = self
            .embedder
            .embed(&model, vec![query])
            .await?
            .pop()
            .map(normalized)
            .ok_or_else(|| anyhow!("{model} returned no embedding for the query"))?;
        self.with_store(&model, |store| {
            if !store.contains(&root_hash) {
                return Err(anyhow!("this codebase has not been indexed with {model}"));
            }
            Ok(store
                .nearest(&root_hash, &query)
                .iter()
                .map(|hash| ContentHash::new(hash.as_ref().clone()))
                .collect())
        })
    }

    async fn codebase_context_config(&self) -> Result<CodebaseContextConfig, Error> {
        Ok(CodebaseContextConfig {
            embedding_config: EmbeddingConfig::default(),
            embedding_cadence: EMBEDDING_CADENCE,
        })
    }
}

#[cfg(test)]
#[path = "local_store_tests.rs"]
mod tests;
