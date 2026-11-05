use std::sync::RwLock;

use string_offset::ByteOffset;

use super::*;

const WORDS: [&str; 3] = ["auth", "render", "parser"];

/// Embeds a text as the count of each word in `WORDS`.
struct FakeEmbedder {
    model: RwLock<Option<String>>,
}

impl FakeEmbedder {
    fn with_model(model: &str) -> Arc<Self> {
        Arc::new(Self {
            model: RwLock::new(Some(model.to_owned())),
        })
    }

    fn set_model(&self, model: Option<&str>) {
        *self.model.write().unwrap() = model.map(str::to_owned);
    }
}

#[async_trait]
impl Embedder for FakeEmbedder {
    fn model(&self) -> Option<String> {
        self.model.read().unwrap().clone()
    }

    async fn embed(&self, _model: &str, texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| {
                WORDS
                    .iter()
                    .map(|word| text.matches(word).count() as f32)
                    .collect()
            })
            .collect())
    }
}

fn hash(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}

fn node_hash(byte: u8) -> NodeHash {
    hash(byte).parse().unwrap()
}

fn fragment(byte: u8, content: &str) -> Fragment {
    Fragment::from_byte_range(
        content.to_owned(),
        hash(byte).parse().unwrap(),
        PathBuf::from("/repo/file.rs"),
        ByteOffset::from(0)..ByteOffset::from(content.len()),
    )
}

fn repo_metadata() -> RepoMetadata {
    RepoMetadata { path: None }
}

fn temp_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("local_store_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    directory
}

/// Indexes a root node `1` with fragments `10` and `11`, and a root node `2` with fragment `12`.
async fn index_two_roots(store: &LocalStore) {
    store
        .generate_embeddings(
            EmbeddingConfig::default(),
            vec![
                fragment(10, "auth auth login"),
                fragment(11, "render render"),
                fragment(12, "parser"),
            ],
            node_hash(1),
            repo_metadata(),
        )
        .await
        .unwrap();
    store
        .update_intermediate_nodes(
            EmbeddingConfig::default(),
            vec![
                IntermediateNode {
                    hash: node_hash(1),
                    children: vec![node_hash(10), node_hash(11)],
                },
                IntermediateNode {
                    hash: node_hash(2),
                    children: vec![node_hash(12)],
                },
            ],
        )
        .await
        .unwrap();
}

async fn search(store: &LocalStore, query: &str, root: u8) -> Result<Vec<String>, Error> {
    let hashes = store
        .get_relevant_fragments(
            EmbeddingConfig::default(),
            query.to_owned(),
            node_hash(root),
            repo_metadata(),
        )
        .await?;
    Ok(hashes.iter().map(ToString::to_string).collect())
}

#[tokio::test]
async fn ranks_fragments_by_similarity() {
    let store = LocalStore::new(FakeEmbedder::with_model("m"), temp_directory("ranks"));
    index_two_roots(&store).await;

    let results = search(&store, "render", 1).await.unwrap();

    assert_eq!(results, vec![hash(11), hash(10)]);
}

#[tokio::test]
async fn search_only_covers_the_requested_root() {
    let store = LocalStore::new(FakeEmbedder::with_model("m"), temp_directory("roots"));
    index_two_roots(&store).await;

    let results = search(&store, "parser", 2).await.unwrap();

    assert_eq!(results, vec![hash(12)]);
}

#[tokio::test]
async fn sync_reports_only_unknown_nodes() {
    let store = LocalStore::new(FakeEmbedder::with_model("m"), temp_directory("sync"));
    index_two_roots(&store).await;

    let missing = store
        .sync_merkle_tree(
            vec![node_hash(1), node_hash(10), node_hash(99)],
            EmbeddingConfig::default(),
        )
        .await
        .unwrap();

    assert_eq!(missing, HashSet::from([node_hash(99)]));
}

#[tokio::test]
async fn index_survives_a_restart() {
    let directory = temp_directory("restart");
    let store = LocalStore::new(FakeEmbedder::with_model("m"), directory.clone());
    index_two_roots(&store).await;
    drop(store);

    let store = LocalStore::new(FakeEmbedder::with_model("m"), directory);

    assert_eq!(
        search(&store, "render", 1).await.unwrap(),
        vec![hash(11), hash(10)]
    );
}

#[tokio::test]
async fn a_truncated_log_keeps_earlier_records() {
    let directory = temp_directory("truncated");
    let store = LocalStore::new(FakeEmbedder::with_model("m"), directory.clone());
    index_two_roots(&store).await;
    drop(store);
    let path = log_path(&directory, "m");
    let length = fs::metadata(&path).unwrap().len();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(length - 3)
        .unwrap();

    let store = LocalStore::new(FakeEmbedder::with_model("m"), directory);

    let missing = store
        .sync_merkle_tree(vec![node_hash(1), node_hash(2)], EmbeddingConfig::default())
        .await
        .unwrap();
    assert_eq!(missing, HashSet::from([node_hash(2)]));
}

#[tokio::test]
async fn records_written_after_a_truncated_log_are_kept() {
    let directory = temp_directory("after_truncated");
    let store = LocalStore::new(FakeEmbedder::with_model("m"), directory.clone());
    index_two_roots(&store).await;
    drop(store);
    let path = log_path(&directory, "m");
    let length = fs::metadata(&path).unwrap().len();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(length - 3)
        .unwrap();
    let store = LocalStore::new(FakeEmbedder::with_model("m"), directory.clone());
    store
        .update_intermediate_nodes(
            EmbeddingConfig::default(),
            vec![IntermediateNode {
                hash: node_hash(3),
                children: vec![],
            }],
        )
        .await
        .unwrap();
    drop(store);

    let store = LocalStore::new(FakeEmbedder::with_model("m"), directory);

    let missing = store
        .sync_merkle_tree(vec![node_hash(3)], EmbeddingConfig::default())
        .await
        .unwrap();
    assert!(missing.is_empty());
}

#[tokio::test]
async fn each_model_has_its_own_index() {
    let embedder = FakeEmbedder::with_model("first");
    let store = LocalStore::new(embedder.clone(), temp_directory("models"));
    index_two_roots(&store).await;

    embedder.set_model(Some("second"));

    assert!(search(&store, "render", 1).await.is_err());
    let missing = store
        .sync_merkle_tree(vec![node_hash(1)], EmbeddingConfig::default())
        .await
        .unwrap();
    assert_eq!(missing, HashSet::from([node_hash(1)]));
}

#[tokio::test]
async fn fails_without_a_model() {
    let embedder = FakeEmbedder::with_model("m");
    embedder.set_model(None);
    let store = LocalStore::new(embedder, temp_directory("no_model"));

    assert!(search(&store, "render", 1).await.is_err());
}
