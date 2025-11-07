use anyhow::{anyhow, bail, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_rusqlite::{Connection, Error as TokioSqlError};

const OPENAI_EMBEDDING_ENDPOINT: &str = "https://api.openai.com/v1/embeddings";

#[derive(Clone)]
pub struct RagRetriever {
    store: ChunkStore,
    pinecone: PineconeClient,
    embedder: EmbeddingClient,
    top_k: usize,
    min_score: f32,
}

#[derive(Clone, Debug)]
pub struct ContextChunk {
    pub id: String,
    pub source: String,
    pub topic: String,
    pub body: String,
    pub score: f32,
}

impl RagRetriever {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        db_path: PathBuf,
        pinecone_host: String,
        pinecone_key: String,
        pinecone_namespace: Option<String>,
        embedding_key: String,
        embedding_model: String,
        top_k: usize,
        min_score: f32,
    ) -> Result<Self> {
        let store = ChunkStore::open(db_path).await?;
        let client = Client::builder().build()?;
        let pinecone = PineconeClient::new(
            client.clone(),
            pinecone_host,
            pinecone_key,
            pinecone_namespace,
        );
        let embedder = EmbeddingClient::new(client, embedding_key, embedding_model)?;
        Ok(Self {
            store,
            pinecone,
            embedder,
            top_k,
            min_score,
        })
    }

    pub async fn retrieve(&self, question: &str) -> Result<Vec<ContextChunk>> {
        let embedding = self.embedder.embed(question).await?;
        let matches = self.pinecone.query(&embedding, self.top_k).await?;
        if matches.is_empty() {
            return Ok(Vec::new());
        }
        let mut filtered: Vec<_> = matches
            .into_iter()
            .filter(|hit| hit.score.unwrap_or_default() >= self.min_score)
            .collect();
        if filtered.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<String> = filtered.iter().map(|hit| hit.id.clone()).collect();
        let mut chunks = self.store.fetch_chunks(&ids).await?;
        let mut chunk_map: HashMap<String, ContextChunk> = chunks
            .drain(..)
            .map(|chunk| (chunk.id.clone(), chunk))
            .collect();

        let mut ordered = Vec::new();
        for hit in filtered.drain(..) {
            if let Some(mut chunk) = chunk_map.remove(&hit.id) {
                chunk.score = hit.score.unwrap_or_default();
                ordered.push(chunk);
            }
        }
        Ok(ordered)
    }
}

#[derive(Clone)]
struct ChunkStore {
    connection: Connection,
}

impl ChunkStore {
    async fn open(path: PathBuf) -> Result<Self> {
        if !Path::new(&path).exists() {
            bail!("SQLite RAG bundle missing at {:?}", path);
        }
        let connection = Connection::open(path).await?;
        Ok(Self { connection })
    }

    async fn fetch_chunks(&self, ids: &[String]) -> Result<Vec<ContextChunk>> {
        let ids = ids.to_vec();
        let chunks = self
            .connection
            .call(
                move |conn: &mut rusqlite::Connection| -> Result<Vec<ContextChunk>, TokioSqlError> {
                    let mut chunks = Vec::new();
                    for id in ids {
                        let mut stmt = conn.prepare(
                            "SELECT id, source, topic, body FROM rag_chunks WHERE id = ?1 LIMIT 1",
                        )?;
                        let mut rows = stmt.query([&id])?;
                        if let Some(row) = rows.next()? {
                            chunks.push(ContextChunk {
                                id: row.get(0)?,
                                source: row.get(1)?,
                                topic: row.get(2)?,
                                body: row.get(3)?,
                                score: 0.0,
                            });
                        }
                    }
                    Ok(chunks)
                },
            )
            .await?;
        Ok(chunks)
    }
}

#[derive(Clone)]
struct PineconeClient {
    client: Client,
    host: String,
    api_key: String,
    namespace: Option<String>,
}

impl PineconeClient {
    fn new(client: Client, host: String, api_key: String, namespace: Option<String>) -> Self {
        Self {
            client,
            host: host.trim_end_matches('/').to_string(),
            api_key,
            namespace,
        }
    }

    async fn query(&self, vector: &[f32], top_k: usize) -> Result<Vec<PineconeMatch>> {
        let mut payload = json!({
            "vector": vector,
            "topK": top_k as u32,
            "includeMetadata": false,
            "includeValues": false,
        });
        if let Some(namespace) = &self.namespace {
            payload.as_object_mut().expect("payload json").insert(
                "namespace".to_string(),
                serde_json::Value::String(namespace.clone()),
            );
        }
        let response = self
            .client
            .post(format!("{}/query", self.host))
            .header("Api-Key", &self.api_key)
            .json(&payload)
            .send()
            .await
            .context("Failed to query Pinecone")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("Pinecone query failed ({status}): {body}");
        }

        let body: PineconeQueryResponse = response.json().await?;
        Ok(body.matches.unwrap_or_default())
    }
}

#[derive(Deserialize)]
struct PineconeQueryResponse {
    matches: Option<Vec<PineconeMatch>>,
}

#[derive(Deserialize)]
struct PineconeMatch {
    id: String,
    score: Option<f32>,
}

#[derive(Clone)]
struct EmbeddingClient {
    client: Client,
    api_key: Arc<String>,
    model: String,
}

impl EmbeddingClient {
    fn new(client: Client, api_key: String, model: String) -> Result<Self> {
        Ok(Self {
            client,
            api_key: Arc::new(api_key),
            model,
        })
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let payload = serde_json::json!({
            "model": self.model,
            "input": text,
        });
        let response = self
            .client
            .post(OPENAI_EMBEDDING_ENDPOINT)
            .bearer_auth(self.api_key.as_str())
            .json(&payload)
            .send()
            .await
            .context("Failed to query OpenAI embeddings")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("OpenAI embedding error ({status}): {body}");
        }

        let body: EmbeddingResponse = response.json().await?;
        let embedding = body
            .data
            .into_iter()
            .next()
            .map(|item| item.embedding)
            .ok_or_else(|| anyhow!("OpenAI returned an empty embedding list"))?;
        Ok(embedding)
    }
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}
