use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::db::Database;
use crate::http_client::http_client;
use crate::settings::AppSettings;

pub const MEMORY_CAP: usize = 1000;
pub const EMBEDDING_MODEL: &str = "text-embedding-3-small";
const DEFAULT_K: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecord {
    pub id: String,
    pub kind: String,
    pub symbol: Option<String>,
    pub text: String,
    pub source: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

pub fn can_embed(provider: &str) -> bool {
    matches!(
        provider.trim().to_ascii_lowercase().as_str(),
        "openai" | "openrouter"
    )
}

pub fn pack_f32(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

pub fn unpack_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom <= f32::EPSILON {
        0.0
    } else {
        dot / denom
    }
}

pub fn list_memories(conn: &Connection, limit: i64) -> Result<Vec<MemoryRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, symbol, text, source, created_at
         FROM agent_memories ORDER BY created_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit.max(1).min(MEMORY_CAP as i64)], |row| {
        Ok(MemoryRecord {
            id: row.get(0)?,
            kind: row.get(1)?,
            symbol: row.get(2)?,
            text: row.get(3)?,
            source: row.get(4)?,
            created_at: row.get(5)?,
            score: None,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn delete_memory(conn: &Connection, id: &str) -> Result<bool> {
    let n = conn.execute("DELETE FROM agent_memories WHERE id = ?1", [id])?;
    Ok(n > 0)
}

pub fn keyword_search(
    conn: &Connection,
    query: &str,
    symbol: Option<&str>,
    k: usize,
) -> Result<Vec<MemoryRecord>> {
    let like = format!("%{}%", query.trim());
    let limit = k.clamp(1, 40) as i64;
    if let Some(sym) = symbol.filter(|s| !s.is_empty()) {
        let mut stmt = conn.prepare(
            "SELECT id, kind, symbol, text, source, created_at FROM agent_memories
             WHERE symbol = ?1 AND (text LIKE ?2 OR kind LIKE ?2)
             ORDER BY created_at DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![sym.to_uppercase(), like, limit], map_row)?;
        return Ok(rows.filter_map(|r| r.ok()).collect());
    }
    let mut stmt = conn.prepare(
        "SELECT id, kind, symbol, text, source, created_at FROM agent_memories
         WHERE text LIKE ?1 OR kind LIKE ?1 OR IFNULL(symbol,'') LIKE ?1
         ORDER BY created_at DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![like, limit], map_row)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRecord> {
    Ok(MemoryRecord {
        id: row.get(0)?,
        kind: row.get(1)?,
        symbol: row.get(2)?,
        text: row.get(3)?,
        source: row.get(4)?,
        created_at: row.get(5)?,
        score: None,
    })
}

pub fn insert_memory(
    conn: &Connection,
    kind: &str,
    symbol: Option<&str>,
    text: &str,
    source: &str,
    embedding: Option<&[u8]>,
) -> Result<String> {
    let kind = if kind == "symbol_lesson" {
        "symbol_lesson"
    } else {
        "freeform"
    };
    let source = if source == "trade_outcome" {
        "trade_outcome"
    } else {
        "agent_upsert"
    };
    let text = text.trim();
    if text.is_empty() {
        anyhow::bail!("Memory text is empty");
    }
    let id = Uuid::new_v4().to_string();
    let sym = symbol
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_uppercase());
    conn.execute(
        "INSERT INTO agent_memories (id, kind, symbol, text, embedding, source, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'), datetime('now'))",
        params![id, kind, sym, text, embedding, source],
    )?;
    evict_over_cap(conn)?;
    Ok(id)
}

pub fn evict_over_cap(conn: &Connection) -> Result<()> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM agent_memories", [], |r| r.get(0))?;
    let extra = n - MEMORY_CAP as i64;
    if extra > 0 {
        conn.execute(
            "DELETE FROM agent_memories WHERE rowid IN (
                SELECT rowid FROM agent_memories ORDER BY created_at ASC, rowid ASC LIMIT ?1
             )",
            [extra],
        )?;
    }
    Ok(())
}

pub fn vector_search(
    conn: &Connection,
    query_vec: &[f32],
    symbol: Option<&str>,
    k: usize,
) -> Result<Vec<MemoryRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, symbol, text, source, created_at, embedding FROM agent_memories
         WHERE embedding IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            MemoryRecord {
                id: row.get(0)?,
                kind: row.get(1)?,
                symbol: row.get(2)?,
                text: row.get(3)?,
                source: row.get(4)?,
                created_at: row.get(5)?,
                score: None,
            },
            row.get::<_, Vec<u8>>(6)?,
        ))
    })?;
    let want = symbol
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_uppercase());
    let mut scored: Vec<MemoryRecord> = Vec::new();
    for row in rows.flatten() {
        let (mut rec, blob) = row;
        if let Some(ref s) = want {
            if rec.symbol.as_deref() != Some(s.as_str()) {
                continue;
            }
        }
        let vec = unpack_f32(&blob);
        rec.score = Some(cosine(query_vec, &vec));
        scored.push(rec);
    }
    scored.sort_by(|a, b| {
        b.score
            .unwrap_or(0.0)
            .partial_cmp(&a.score.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(k.clamp(1, 40));
    Ok(scored)
}

fn embeddings_url(settings: &AppSettings) -> String {
    match settings.llm_provider.as_str() {
        "openrouter" => {
            let base = settings
                .llm_base_url
                .as_deref()
                .filter(|u| !u.is_empty())
                .unwrap_or("https://openrouter.ai/api/v1");
            format!("{}/embeddings", base.trim_end_matches('/'))
        }
        _ => {
            let base = settings
                .llm_base_url
                .as_deref()
                .filter(|u| !u.is_empty())
                .unwrap_or("https://api.openai.com/v1");
            format!("{}/embeddings", base.trim_end_matches('/'))
        }
    }
}

pub async fn embed_text(settings: &AppSettings, api_key: &str, text: &str) -> Result<Vec<f32>> {
    if !can_embed(&settings.llm_provider) {
        anyhow::bail!("embeddings unavailable");
    }
    if api_key.is_empty() {
        anyhow::bail!("embeddings unavailable");
    }
    let http = http_client()?;
    let url = embeddings_url(settings);
    let res = http
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "model": EMBEDDING_MODEL,
            "input": text,
        }))
        .send()
        .await
        .context("embeddings request")?;
    if !res.status().is_success() {
        let status = res.status();
        let _ = res.text().await;
        anyhow::bail!("embeddings HTTP {status}");
    }
    let body: Value = res.json().await.context("parse embeddings")?;
    let arr = body
        .pointer("/data/0/embedding")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("embeddings response missing vector"))?;
    Ok(arr
        .iter()
        .filter_map(|v| v.as_f64().map(|n| n as f32))
        .collect())
}

pub async fn search_memories(
    db: &Database,
    settings: &AppSettings,
    api_key: &str,
    query: &str,
    symbol: Option<&str>,
    k: Option<usize>,
) -> Result<Vec<MemoryRecord>> {
    let k = k.unwrap_or(DEFAULT_K);
    if can_embed(&settings.llm_provider) && !api_key.is_empty() {
        match embed_text(settings, api_key, query).await {
            Ok(vec) => {
                return db.with_conn(|conn| vector_search(conn, &vec, symbol, k));
            }
            Err(e) => {
                tracing::warn!(target: "memory", error = %e, "embed failed; keyword search");
            }
        }
    }
    db.with_conn(|conn| keyword_search(conn, query, symbol, k))
}

pub async fn upsert_memory(
    db: &Database,
    settings: &AppSettings,
    api_key: &str,
    kind: &str,
    symbol: Option<&str>,
    text: &str,
    source: &str,
) -> Result<String> {
    let embedding = if can_embed(&settings.llm_provider) && !api_key.is_empty() {
        match embed_text(settings, api_key, text).await {
            Ok(v) => Some(pack_f32(&v)),
            Err(e) => {
                tracing::warn!(target: "memory", error = %e, "upsert without embedding");
                None
            }
        }
    } else {
        None
    };
    db.with_conn(|conn| insert_memory(conn, kind, symbol, text, source, embedding.as_deref()))
}

pub async fn backfill_missing_embeddings(
    db: &Database,
    settings: &AppSettings,
    api_key: &str,
) -> Result<usize> {
    if !can_embed(&settings.llm_provider) || api_key.is_empty() {
        return Ok(0);
    }
    let ids: Vec<(String, String)> = db.with_conn(|conn| {
        let mut stmt =
            conn.prepare("SELECT id, text FROM agent_memories WHERE embedding IS NULL LIMIT 32")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    })?;
    let mut n = 0usize;
    for (id, text) in ids {
        let Ok(vec) = embed_text(settings, api_key, &text).await else {
            continue;
        };
        let blob = pack_f32(&vec);
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE agent_memories SET embedding = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![blob, id],
            )?;
            Ok(())
        })?;
        n += 1;
    }
    Ok(n)
}

pub fn handle_tool(
    db: &Database,
    settings: &AppSettings,
    api_key: &str,
    name: &str,
    arguments: &Value,
) -> Result<Value> {
    crate::runtime_util::block_on_local(async {
        match name {
            "memory_search" => {
                if !can_embed(&settings.llm_provider) {
                    return Ok(serde_json::json!({
                        "ok": false,
                        "error": "embeddings unavailable",
                        "hits": []
                    }));
                }
                let query = arguments
                    .get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if query.is_empty() {
                    anyhow::bail!("memory_search requires query");
                }
                let symbol = arguments.get("symbol").and_then(|v| v.as_str());
                let k = arguments.get("k").and_then(|v| v.as_u64()).map(|n| n as usize);
                let hits = search_memories(db, settings, api_key, query, symbol, k).await?;
                Ok(serde_json::json!({ "ok": true, "hits": hits }))
            }
            "memory_upsert" => {
                if !can_embed(&settings.llm_provider) {
                    return Ok(serde_json::json!({
                        "ok": false,
                        "error": "embeddings unavailable"
                    }));
                }
                let text = arguments
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let kind = arguments
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("freeform");
                let symbol = arguments.get("symbol").and_then(|v| v.as_str());
                let id = upsert_memory(db, settings, api_key, kind, symbol, text, "agent_upsert")
                    .await?;
                Ok(serde_json::json!({ "ok": true, "id": id }))
            }
            other => anyhow::bail!("unknown tool {other}"),
        }
    })
}

pub fn get_by_id(conn: &Connection, id: &str) -> Result<Option<MemoryRecord>> {
    conn.query_row(
        "SELECT id, kind, symbol, text, source, created_at FROM agent_memories WHERE id = ?1",
        [id],
        map_row,
    )
    .optional()
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/008_agent_memories.sql"))
            .unwrap();
        conn
    }

    #[test]
    fn cosine_ranks_closer_vector_higher() {
        let q = vec![1.0, 0.0, 0.0];
        let a = vec![0.9, 0.1, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(cosine(&q, &a) > cosine(&q, &b));
    }

    #[test]
    fn cap_evicts_oldest() {
        let conn = setup();
        for i in 0..(MEMORY_CAP + 5) {
            insert_memory(
                &conn,
                "freeform",
                None,
                &format!("note {i}"),
                "agent_upsert",
                None,
            )
            .unwrap();
        }
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, MEMORY_CAP as i64);
        let oldest: String = conn
            .query_row(
                "SELECT text FROM agent_memories ORDER BY rowid ASC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(oldest, "note 5");
    }

    #[test]
    fn tool_search_skips_embed_on_anthropic() {
        assert!(!can_embed("anthropic"));
        assert!(can_embed("openai"));
        assert!(can_embed("openrouter"));
        let dir = std::env::temp_dir().join(format!("ngx-mem-{}", Uuid::new_v4()));
        let db = crate::db::Database::open(&dir).unwrap();
        let mut settings = AppSettings::default();
        settings.llm_provider = "anthropic".into();
        let v = handle_tool(
            &db,
            &settings,
            "sk-test",
            "memory_search",
            &serde_json::json!({ "query": "GTCO" }),
        )
        .unwrap();
        assert_eq!(v["error"], "embeddings unavailable");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn vector_search_picks_neighbor() {
        let conn = setup();
        insert_memory(
            &conn,
            "symbol_lesson",
            Some("GTCO"),
            "avoid chasing GTCO after failed breakout",
            "agent_upsert",
            Some(&pack_f32(&[1.0, 0.0])),
        )
        .unwrap();
        insert_memory(
            &conn,
            "freeform",
            None,
            "unrelated banks",
            "agent_upsert",
            Some(&pack_f32(&[0.0, 1.0])),
        )
        .unwrap();
        let hits = vector_search(&conn, &[1.0, 0.0], None, 2).unwrap();
        assert_eq!(hits[0].symbol.as_deref(), Some("GTCO"));
    }
}
