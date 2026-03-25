/*
 * Copyright 2024, The Cozo Project Authors.
 *
 * This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
 * If a copy of the MPL was not distributed with this file,
 * You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Agent-ready API endpoints for CozoDB.
//!
//! These endpoints provide structured access to database metadata, schema introspection,
//! query validation, and operational controls. Designed for consumption by AI agents
//! and CLI tools.

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_derive::Deserialize;
use serde_json::json;
use tokio::task::spawn_blocking;

use cozo::{DataValue, ScriptMutability};

use crate::server::DbState;

// ─── Health & Metadata ───────────────────────────────────────────────────────

/// GET /api/health
/// Returns server health status and database metadata.
pub async fn health(State(st): State<DbState>) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        let relations = db.run_script("::relations", BTreeMap::new(), ScriptMutability::Immutable);
        let running = db.run_script("::running", BTreeMap::new(), ScriptMutability::Immutable);
        let fixed_rules =
            db.run_script("::fixed_rules", BTreeMap::new(), ScriptMutability::Immutable);
        (relations, running, fixed_rules)
    })
    .await;

    match result {
        Ok((relations, running, fixed_rules)) => {
            let relation_count = relations.as_ref().map(|r| r.rows.len()).unwrap_or(0);
            let running_count = running.as_ref().map(|r| r.rows.len()).unwrap_or(0);
            let fixed_rule_count = fixed_rules.as_ref().map(|r| r.rows.len()).unwrap_or(0);
            (
                StatusCode::OK,
                json!({
                    "ok": true,
                    "status": "healthy",
                    "engine": st.engine,
                    "relation_count": relation_count,
                    "running_queries": running_count,
                    "fixed_rule_count": fixed_rule_count,
                })
                .into(),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "status": "error", "message": err.to_string()}).into(),
        ),
    }
}

// ─── Schema Introspection ────────────────────────────────────────────────────

/// GET /api/relations
/// Lists all stored relations with their metadata.
pub async fn list_relations(State(st): State<DbState>) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        db.run_script("::relations", BTreeMap::new(), ScriptMutability::Immutable)
    })
    .await;

    match result {
        Ok(Ok(rows)) => (StatusCode::OK, rows.into_json().into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

/// GET /api/relations/:name/columns
/// Lists all columns for a specific relation.
pub async fn list_columns(
    State(st): State<DbState>,
    Path(name): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        db.run_script(
            &format!("::columns {}", name),
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )
    })
    .await;

    match result {
        Ok(Ok(rows)) => (StatusCode::OK, rows.into_json().into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

/// GET /api/relations/:name/indices
/// Lists all indices for a specific relation.
pub async fn list_indices(
    State(st): State<DbState>,
    Path(name): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        db.run_script(
            &format!("::indices {}", name),
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )
    })
    .await;

    match result {
        Ok(Ok(rows)) => (StatusCode::OK, rows.into_json().into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

/// GET /api/relations/:name/triggers
/// Shows triggers for a specific relation.
pub async fn show_triggers(
    State(st): State<DbState>,
    Path(name): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        db.run_script(
            &format!("::show_triggers {}", name),
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )
    })
    .await;

    match result {
        Ok(Ok(rows)) => (StatusCode::OK, rows.into_json().into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

/// POST /api/relations/:name/describe
/// Describe a relation (set or get description).
#[derive(Deserialize)]
pub struct DescribePayload {
    pub description: Option<String>,
}

pub async fn describe_relation(
    State(st): State<DbState>,
    Path(name): Path<String>,
    Json(payload): Json<DescribePayload>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        let script = if let Some(desc) = payload.description {
            format!("::describe {} '{}'", name, desc.replace('\'', "\\'"))
        } else {
            format!("::describe {}", name)
        };
        db.run_script(&script, BTreeMap::new(), ScriptMutability::Mutable)
    })
    .await;

    match result {
        Ok(Ok(rows)) => (StatusCode::OK, rows.into_json().into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

/// GET /api/schema
/// Returns full schema: all relations with their columns, indices, and triggers.
pub async fn full_schema(State(st): State<DbState>) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || -> Result<serde_json::Value, String> {
        let relations = db
            .run_script("::relations", BTreeMap::new(), ScriptMutability::Immutable)
            .map_err(|e| e.to_string())?;

        let mut schema = Vec::new();
        for row in &relations.rows {
            let rel_name = match &row[0] {
                DataValue::Str(s) => s.to_string(),
                other => format!("{}", other),
            };

            let columns = db
                .run_script(
                    &format!("::columns {}", rel_name),
                    BTreeMap::new(),
                    ScriptMutability::Immutable,
                )
                .map_err(|e| e.to_string())?;

            let indices = db
                .run_script(
                    &format!("::indices {}", rel_name),
                    BTreeMap::new(),
                    ScriptMutability::Immutable,
                )
                .ok();

            let triggers = db
                .run_script(
                    &format!("::show_triggers {}", rel_name),
                    BTreeMap::new(),
                    ScriptMutability::Immutable,
                )
                .ok();

            let mut rel_info = json!({
                "name": rel_name,
                "columns": columns.into_json(),
            });

            if let Some(idx) = indices {
                rel_info["indices"] = idx.into_json();
            }
            if let Some(trg) = triggers {
                rel_info["triggers"] = trg.into_json();
            }

            // Include other metadata from the relations row
            if relations.headers.len() > 1 {
                for (i, header) in relations.headers.iter().enumerate().skip(1) {
                    rel_info[header] = serde_json::Value::from(row[i].clone());
                }
            }

            schema.push(rel_info);
        }

        Ok(json!({
            "ok": true,
            "schema": schema,
        }))
    })
    .await;

    match result {
        Ok(Ok(val)) => (StatusCode::OK, val.into()),
        Ok(Err(msg)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": msg}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

// ─── Query Operations ────────────────────────────────────────────────────────

/// POST /api/explain
/// Explain a query plan without executing it.
#[derive(Deserialize)]
pub struct ExplainPayload {
    pub script: String,
}

pub async fn explain_query(
    State(st): State<DbState>,
    Json(payload): Json<ExplainPayload>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        db.run_script(
            &format!("::explain {{ {} }}", payload.script),
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )
    })
    .await;

    match result {
        Ok(Ok(rows)) => (StatusCode::OK, rows.into_json().into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

/// POST /api/validate
/// Validate a CozoScript query without executing it. Uses explain to check syntax.
#[derive(Deserialize)]
pub struct ValidatePayload {
    pub script: String,
}

pub async fn validate_query(
    State(st): State<DbState>,
    Json(payload): Json<ValidatePayload>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let script = payload.script.clone();
    let result = spawn_blocking(move || {
        // Try to explain the query - this validates without executing
        db.run_script(
            &format!("::explain {{ {} }}", payload.script),
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )
    })
    .await;

    match result {
        Ok(Ok(_)) => (
            StatusCode::OK,
            json!({"ok": true, "valid": true, "script": script}).into(),
        ),
        Ok(Err(err)) => (
            StatusCode::OK,
            json!({"ok": true, "valid": false, "error": err.to_string(), "script": script}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

/// POST /api/query
/// Execute a query with enhanced response format including timing and metadata.
#[derive(Deserialize)]
pub struct ApiQueryPayload {
    pub script: String,
    #[serde(default)]
    pub params: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub immutable: bool,
    /// Limit the number of rows returned (0 = unlimited)
    #[serde(default)]
    pub limit: usize,
    /// Skip this many rows before returning results
    #[serde(default)]
    pub offset: usize,
}

pub async fn api_query(
    State(st): State<DbState>,
    Json(payload): Json<ApiQueryPayload>,
) -> (StatusCode, Json<serde_json::Value>) {
    let limit = payload.limit;
    let offset = payload.offset;
    let db = st.db.clone();
    let start = std::time::Instant::now();

    let result = spawn_blocking(move || {
        let params = payload
            .params
            .into_iter()
            .map(|(k, v)| (k, DataValue::from(v)))
            .collect();
        let mutability = if payload.immutable {
            ScriptMutability::Immutable
        } else {
            ScriptMutability::Mutable
        };
        db.run_script(&payload.script, params, mutability)
    })
    .await;

    let elapsed_ms = start.elapsed().as_millis();

    match result {
        Ok(Ok(mut rows)) => {
            let total_rows = rows.rows.len();

            // Apply offset and limit
            if offset > 0 {
                if offset >= rows.rows.len() {
                    rows.rows.clear();
                } else {
                    rows.rows = rows.rows.split_off(offset);
                }
            }
            if limit > 0 && rows.rows.len() > limit {
                rows.rows.truncate(limit);
            }

            let returned_rows = rows.rows.len();
            let mut response = rows.into_json();
            response["elapsed_ms"] = json!(elapsed_ms);
            response["total_rows"] = json!(total_rows);
            response["returned_rows"] = json!(returned_rows);
            if offset > 0 {
                response["offset"] = json!(offset);
            }
            if limit > 0 {
                response["limit"] = json!(limit);
            }
            (StatusCode::OK, response.into())
        }
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({
                "ok": false,
                "message": err.to_string(),
                "elapsed_ms": elapsed_ms,
            })
            .into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

// ─── Process Management ──────────────────────────────────────────────────────

/// GET /api/running
/// List currently running queries.
pub async fn list_running(State(st): State<DbState>) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        db.run_script("::running", BTreeMap::new(), ScriptMutability::Immutable)
    })
    .await;

    match result {
        Ok(Ok(rows)) => (StatusCode::OK, rows.into_json().into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

/// DELETE /api/running/:id
/// Kill a running query by its process ID.
pub async fn kill_running(
    State(st): State<DbState>,
    Path(id): Path<u64>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        db.run_script(
            "::kill $id",
            BTreeMap::from([("id".to_string(), DataValue::from(id as i64))]),
            ScriptMutability::Mutable,
        )
    })
    .await;

    match result {
        Ok(Ok(rows)) => (StatusCode::OK, rows.into_json().into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

// ─── Fixed Rules ─────────────────────────────────────────────────────────────

/// GET /api/fixed-rules
/// List all available fixed rules (built-in algorithms).
pub async fn list_fixed_rules(
    State(st): State<DbState>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        db.run_script(
            "::fixed_rules",
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )
    })
    .await;

    match result {
        Ok(Ok(rows)) => (StatusCode::OK, rows.into_json().into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

// ─── Database Operations ─────────────────────────────────────────────────────

/// POST /api/compact
/// Compact the database storage.
pub async fn compact_db(State(st): State<DbState>) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        db.run_script("::compact", BTreeMap::new(), ScriptMutability::Mutable)
    })
    .await;

    match result {
        Ok(Ok(_)) => (StatusCode::OK, json!({"ok": true}).into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

/// POST /api/remove-relations
/// Remove one or more stored relations.
#[derive(Deserialize)]
pub struct RemoveRelationsPayload {
    pub relations: Vec<String>,
}

pub async fn remove_relations(
    State(st): State<DbState>,
    Json(payload): Json<RemoveRelationsPayload>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        let rels = payload.relations.join(", ");
        db.run_script(
            &format!("::remove {}", rels),
            BTreeMap::new(),
            ScriptMutability::Mutable,
        )
    })
    .await;

    match result {
        Ok(Ok(_)) => (StatusCode::OK, json!({"ok": true}).into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

/// POST /api/rename-relations
/// Rename stored relations.
#[derive(Deserialize)]
pub struct RenameRelationsPayload {
    pub renames: Vec<RenameEntry>,
}

#[derive(Deserialize)]
pub struct RenameEntry {
    pub from: String,
    pub to: String,
}

pub async fn rename_relations(
    State(st): State<DbState>,
    Json(payload): Json<RenameRelationsPayload>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        let pairs: Vec<String> = payload
            .renames
            .iter()
            .map(|r| format!("{} -> {}", r.from, r.to))
            .collect();
        db.run_script(
            &format!("::rename {}", pairs.join(", ")),
            BTreeMap::new(),
            ScriptMutability::Mutable,
        )
    })
    .await;

    match result {
        Ok(Ok(_)) => (StatusCode::OK, json!({"ok": true}).into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

/// POST /api/access-level
/// Set access level for relations.
#[derive(Deserialize)]
pub struct AccessLevelPayload {
    pub level: String, // "normal", "protected", "read_only", "hidden"
    pub relations: Vec<String>,
}

pub async fn set_access_level(
    State(st): State<DbState>,
    Json(payload): Json<AccessLevelPayload>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        let rels = payload.relations.join(", ");
        db.run_script(
            &format!("::access_level {} {}", payload.level, rels),
            BTreeMap::new(),
            ScriptMutability::Mutable,
        )
    })
    .await;

    match result {
        Ok(Ok(_)) => (StatusCode::OK, json!({"ok": true}).into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

// ─── Multi-Query Batch ───────────────────────────────────────────────────────

/// POST /api/batch
/// Execute multiple queries in sequence, optionally in a transaction.
#[derive(Deserialize)]
pub struct BatchPayload {
    pub queries: Vec<BatchQuery>,
    /// If true, wrap all queries in a transaction (all-or-nothing).
    #[serde(default)]
    pub transactional: bool,
}

#[derive(Deserialize)]
pub struct BatchQuery {
    pub script: String,
    #[serde(default)]
    pub params: BTreeMap<String, serde_json::Value>,
}

pub async fn batch_query(
    State(st): State<DbState>,
    Json(payload): Json<BatchPayload>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let start = std::time::Instant::now();

    let result = spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        if payload.transactional {
            let tx = db.multi_transaction(true);
            let mut results = Vec::new();
            for q in &payload.queries {
                let params = q
                    .params
                    .iter()
                    .map(|(k, v)| (k.clone(), DataValue::from(v.clone())))
                    .collect();
                match tx.run_script(&q.script, params) {
                    Ok(rows) => results.push(rows.into_json()),
                    Err(err) => {
                        let _ = tx.abort();
                        return Err(err.to_string());
                    }
                }
            }
            tx.commit().map_err(|e| e.to_string())?;
            Ok(results)
        } else {
            let mut results = Vec::new();
            for q in &payload.queries {
                let params = q
                    .params
                    .iter()
                    .map(|(k, v)| (k.clone(), DataValue::from(v.clone())))
                    .collect();
                match db.run_script(&q.script, params, ScriptMutability::Mutable) {
                    Ok(rows) => results.push(rows.into_json()),
                    Err(err) => results.push(json!({"ok": false, "message": err.to_string()})),
                }
            }
            Ok(results)
        }
    })
    .await;

    let elapsed_ms = start.elapsed().as_millis();

    match result {
        Ok(Ok(results)) => (
            StatusCode::OK,
            json!({
                "ok": true,
                "results": results,
                "elapsed_ms": elapsed_ms,
                "query_count": results.len(),
            })
            .into(),
        ),
        Ok(Err(msg)) => (
            StatusCode::BAD_REQUEST,
            json!({
                "ok": false,
                "message": msg,
                "elapsed_ms": elapsed_ms,
            })
            .into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

// ─── Index Management ────────────────────────────────────────────────────────

/// POST /api/relations/:name/indices
/// Create an index on a relation.
#[derive(Deserialize)]
pub struct CreateIndexPayload {
    pub index_name: String,
    pub columns: Vec<String>,
}

pub async fn create_index(
    State(st): State<DbState>,
    Path(name): Path<String>,
    Json(payload): Json<CreateIndexPayload>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        let cols = payload.columns.join(", ");
        db.run_script(
            &format!(
                "::index create {}:{} {{ {} }}",
                name, payload.index_name, cols
            ),
            BTreeMap::new(),
            ScriptMutability::Mutable,
        )
    })
    .await;

    match result {
        Ok(Ok(_)) => (StatusCode::OK, json!({"ok": true}).into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

/// DELETE /api/relations/:name/indices/:index_name
/// Drop an index from a relation.
pub async fn drop_index(
    State(st): State<DbState>,
    Path((name, index_name)): Path<(String, String)>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = st.db.clone();
    let result = spawn_blocking(move || {
        db.run_script(
            &format!("::index drop {}:{}", name, index_name),
            BTreeMap::new(),
            ScriptMutability::Mutable,
        )
    })
    .await;

    match result {
        Ok(Ok(_)) => (StatusCode::OK, json!({"ok": true}).into()),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "message": err.to_string()}).into(),
        ),
    }
}

// ─── Agent Discovery ─────────────────────────────────────────────────────────

/// GET /api/endpoints
/// List all available API endpoints with descriptions. Useful for AI agent discovery.
pub async fn list_endpoints() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::OK,
        json!({
            "ok": true,
            "endpoints": [
                {
                    "method": "GET",
                    "path": "/api/health",
                    "description": "Server health status and database metadata"
                },
                {
                    "method": "GET",
                    "path": "/api/endpoints",
                    "description": "List all available API endpoints (this endpoint)"
                },
                {
                    "method": "GET",
                    "path": "/api/schema",
                    "description": "Full database schema with all relations, columns, indices, and triggers"
                },
                {
                    "method": "GET",
                    "path": "/api/relations",
                    "description": "List all stored relations"
                },
                {
                    "method": "GET",
                    "path": "/api/relations/:name/columns",
                    "description": "List columns for a relation"
                },
                {
                    "method": "GET",
                    "path": "/api/relations/:name/indices",
                    "description": "List indices for a relation"
                },
                {
                    "method": "GET",
                    "path": "/api/relations/:name/triggers",
                    "description": "Show triggers for a relation"
                },
                {
                    "method": "POST",
                    "path": "/api/relations/:name/describe",
                    "description": "Get or set description for a relation",
                    "body": {"description": "optional string"}
                },
                {
                    "method": "POST",
                    "path": "/api/relations/:name/indices",
                    "description": "Create an index on a relation",
                    "body": {"index_name": "string", "columns": ["string"]}
                },
                {
                    "method": "DELETE",
                    "path": "/api/relations/:name/indices/:index_name",
                    "description": "Drop an index from a relation"
                },
                {
                    "method": "POST",
                    "path": "/api/query",
                    "description": "Execute a CozoScript query with enhanced response (timing, pagination)",
                    "body": {"script": "string", "params": {}, "immutable": false, "limit": 0, "offset": 0}
                },
                {
                    "method": "POST",
                    "path": "/api/explain",
                    "description": "Explain a query plan without executing",
                    "body": {"script": "string"}
                },
                {
                    "method": "POST",
                    "path": "/api/validate",
                    "description": "Validate a CozoScript query without executing",
                    "body": {"script": "string"}
                },
                {
                    "method": "POST",
                    "path": "/api/batch",
                    "description": "Execute multiple queries in sequence, optionally in a transaction",
                    "body": {"queries": [{"script": "string", "params": {}}], "transactional": false}
                },
                {
                    "method": "GET",
                    "path": "/api/running",
                    "description": "List currently running queries"
                },
                {
                    "method": "DELETE",
                    "path": "/api/running/:id",
                    "description": "Kill a running query by process ID"
                },
                {
                    "method": "GET",
                    "path": "/api/fixed-rules",
                    "description": "List all available fixed rules (built-in algorithms)"
                },
                {
                    "method": "POST",
                    "path": "/api/compact",
                    "description": "Compact the database storage"
                },
                {
                    "method": "POST",
                    "path": "/api/remove-relations",
                    "description": "Remove one or more stored relations",
                    "body": {"relations": ["string"]}
                },
                {
                    "method": "POST",
                    "path": "/api/rename-relations",
                    "description": "Rename stored relations",
                    "body": {"renames": [{"from": "string", "to": "string"}]}
                },
                {
                    "method": "POST",
                    "path": "/api/access-level",
                    "description": "Set access level for relations",
                    "body": {"level": "normal|protected|read_only|hidden", "relations": ["string"]}
                },
                {
                    "method": "GET",
                    "path": "/export/:relations",
                    "description": "Export relations (comma-separated names)"
                },
                {
                    "method": "PUT",
                    "path": "/import",
                    "description": "Import relations"
                },
                {
                    "method": "POST",
                    "path": "/backup",
                    "description": "Create database backup",
                    "body": {"path": "string"}
                },
                {
                    "method": "POST",
                    "path": "/import-from-backup",
                    "description": "Import specific relations from a backup",
                    "body": {"path": "string", "relations": ["string"]}
                },
                {
                    "method": "POST",
                    "path": "/text-query",
                    "description": "Execute CozoScript query (legacy endpoint)",
                    "body": {"script": "string", "params": {}, "immutable": false}
                }
            ]
        })
        .into(),
    )
}
