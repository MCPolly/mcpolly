use crate::db::DbPool;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zerocopy::IntoBytes;

pub struct OllamaConfig {
    pub url: String,
    pub model: String,
}

impl OllamaConfig {
    pub fn from_env() -> Self {
        Self {
            url: std::env::var("OLLAMA_URL")
                .unwrap_or_else(|_| "http://localhost:11434".to_string()),
            model: std::env::var("OLLAMA_EMBEDDING_MODEL")
                .unwrap_or_else(|_| "all-minilm".to_string()),
        }
    }
}

pub struct Chunk {
    pub heading: String,
    pub content: String,
    pub index: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub heading: String,
    pub content: String,
    pub source_name: String,
    pub source_type: String,
    pub distance: f64,
}

#[derive(Serialize)]
struct EmbedRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

/// Split markdown content on `## ` headings. Chunks under 20 chars merge with the next.
pub fn chunk_markdown(content: &str) -> Vec<Chunk> {
    let mut raw_chunks: Vec<Chunk> = Vec::new();
    let mut current_heading = String::new();
    let mut current_body = String::new();

    for line in content.lines() {
        if line.starts_with("## ") {
            if !current_body.trim().is_empty() || !current_heading.is_empty() {
                raw_chunks.push(Chunk {
                    heading: current_heading.clone(),
                    content: current_body.trim().to_string(),
                    index: raw_chunks.len(),
                });
            }
            current_heading = line.to_string();
            current_body = String::new();
        } else {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }

    if !current_body.trim().is_empty() || !current_heading.is_empty() {
        raw_chunks.push(Chunk {
            heading: current_heading,
            content: current_body.trim().to_string(),
            index: raw_chunks.len(),
        });
    }

    let mut merged: Vec<Chunk> = Vec::new();
    let mut carry = String::new();

    for chunk in raw_chunks {
        let combined = format!("{} {}", chunk.heading, chunk.content);
        if combined.trim().len() < 20 {
            if !carry.is_empty() {
                carry.push('\n');
            }
            if !chunk.heading.is_empty() {
                carry.push_str(&chunk.heading);
                carry.push('\n');
            }
            carry.push_str(&chunk.content);
        } else {
            let content = if carry.is_empty() {
                chunk.content
            } else {
                let result = format!("{}\n{}", carry, chunk.content);
                carry.clear();
                result
            };
            merged.push(Chunk {
                heading: chunk.heading,
                content,
                index: merged.len(),
            });
        }
    }

    if !carry.is_empty() {
        if let Some(last) = merged.last_mut() {
            last.content = format!("{}\n{}", last.content, carry);
        } else {
            merged.push(Chunk {
                heading: String::new(),
                content: carry.trim().to_string(),
                index: 0,
            });
        }
    }

    merged
}

/// Call Ollama `/api/embed` to generate embeddings for the given texts.
pub async fn generate_embeddings(texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let config = OllamaConfig::from_env();
    let client = reqwest::Client::new();

    let request = EmbedRequest {
        model: config.model,
        input: texts.to_vec(),
    };

    let response = client
        .post(format!("{}/api/embed", config.url))
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("Failed to connect to Ollama: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Ollama error {status}: {body}"));
    }

    let parsed: EmbedResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Ollama response: {e}"))?;

    Ok(parsed.embeddings)
}

/// Chunk content, generate embeddings, and upsert into both tables.
/// Returns the number of chunks indexed.
pub async fn index_document(
    db: &DbPool,
    source_type: &str,
    source_name: &str,
    content: &str,
) -> Result<i64, String> {
    let chunks = chunk_markdown(content);
    if chunks.is_empty() {
        return Ok(0);
    }

    let texts: Vec<String> = chunks
        .iter()
        .map(|c| {
            if c.heading.is_empty() {
                c.content.clone()
            } else {
                format!("{}\n{}", c.heading, c.content)
            }
        })
        .collect();

    let embeddings = generate_embeddings(&texts).await?;

    if embeddings.len() != chunks.len() {
        return Err(format!(
            "Embedding count mismatch: got {} embeddings for {} chunks",
            embeddings.len(),
            chunks.len()
        ));
    }

    let conn = db.get().map_err(|e| format!("Pool error: {e}"))?;

    let old_ids: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM embedding_documents WHERE source_type = ?1 AND source_name = ?2",
            )
            .map_err(|e| format!("DB error: {e}"))?;
        let ids = stmt
            .query_map(params![source_type, source_name], |row| row.get(0))
            .map_err(|e| format!("DB error: {e}"))?
            .filter_map(|r| r.ok())
            .collect();
        ids
    };

    for old_id in &old_ids {
        conn.execute(
            "DELETE FROM vec_embeddings WHERE document_id = ?1",
            params![old_id],
        )
        .map_err(|e| format!("DB error deleting vec: {e}"))?;
    }

    conn.execute(
        "DELETE FROM embedding_documents WHERE source_type = ?1 AND source_name = ?2",
        params![source_type, source_name],
    )
    .map_err(|e| format!("DB error deleting docs: {e}"))?;

    let chunk_count = chunks.len() as i64;
    for (chunk, embedding) in chunks.iter().zip(embeddings.iter()) {
        let doc_id = Uuid::new_v4().to_string();

        conn.execute(
            "INSERT INTO embedding_documents (id, source_type, source_name, chunk_index, heading, content)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                doc_id,
                source_type,
                source_name,
                chunk.index as i64,
                chunk.heading,
                chunk.content
            ],
        )
        .map_err(|e| format!("DB error inserting doc: {e}"))?;

        let embedding_bytes = embedding.as_slice().as_bytes();
        conn.execute(
            "INSERT INTO vec_embeddings (document_id, embedding) VALUES (?1, ?2)",
            params![doc_id, embedding_bytes],
        )
        .map_err(|e| format!("DB error inserting vec: {e}"))?;
    }

    Ok(chunk_count)
}

/// Embed the query and run KNN search against vec_embeddings joined with embedding_documents.
pub async fn search(
    db: &DbPool,
    query: &str,
    source_type: Option<&str>,
    top_k: i64,
) -> Result<Vec<SearchResult>, String> {
    let embeddings = generate_embeddings(&[query.to_string()]).await?;

    let query_embedding = embeddings
        .into_iter()
        .next()
        .ok_or_else(|| "No embedding returned for query".to_string())?;

    let conn = db.get().map_err(|e| format!("Pool error: {e}"))?;

    let query_bytes = query_embedding.as_bytes();
    let effective_k = if source_type.is_some() {
        top_k * 3
    } else {
        top_k
    };

    let mut stmt = conn
        .prepare(
            "SELECT ed.heading, ed.content, ed.source_name, ed.source_type, ve.distance
             FROM vec_embeddings ve
             JOIN embedding_documents ed ON ed.id = ve.document_id
             WHERE ve.embedding MATCH ?1
             AND k = ?2
             ORDER BY ve.distance",
        )
        .map_err(|e| format!("DB error: {e}"))?;

    let results: Vec<SearchResult> = stmt
        .query_map(params![query_bytes, effective_k], |row| {
            Ok(SearchResult {
                heading: row.get(0)?,
                content: row.get(1)?,
                source_name: row.get(2)?,
                source_type: row.get(3)?,
                distance: row.get(4)?,
            })
        })
        .map_err(|e| format!("Search error: {e}"))?
        .filter_map(|r| r.ok())
        .collect();

    if let Some(st) = source_type {
        Ok(results
            .into_iter()
            .filter(|r| r.source_type == st)
            .take(top_k as usize)
            .collect())
    } else {
        Ok(results)
    }
}
