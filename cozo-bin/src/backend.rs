/*
 * Copyright 2024, The Cozo Project Authors.
 *
 * This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
 * If a copy of the MPL was not distributed with this file,
 * You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Backend abstraction for CozoDB CLI.
//!
//! Two modes:
//! - **Embedded**: Opens the database directly in-process. Zero network overhead.
//! - **Remote**: Connects to a running CozoDB server via HTTP.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use cozo::{DataValue, DbInstance, NamedRows, ScriptMutability};

/// Result type for backend operations.
pub type BackendResult = Result<Value, String>;

/// Unified interface for both embedded and remote CozoDB access.
/// All methods return JSON Values with the standard `{"ok": true/false, ...}` shape.
#[allow(dead_code)]
pub trait CozoBackend {
    fn query(
        &self,
        script: &str,
        params: &BTreeMap<String, Value>,
        immutable: bool,
        limit: usize,
        offset: usize,
    ) -> BackendResult;

    fn explain(&self, script: &str) -> BackendResult;
    fn validate(&self, script: &str) -> BackendResult;

    fn batch(&self, queries: &[(String, BTreeMap<String, Value>)], transactional: bool)
        -> BackendResult;

    fn health(&self) -> BackendResult;
    fn list_relations(&self) -> BackendResult;
    fn list_columns(&self, relation: &str) -> BackendResult;
    fn list_indices(&self, relation: &str) -> BackendResult;
    fn show_triggers(&self, relation: &str) -> BackendResult;
    fn describe_relation(&self, relation: &str, description: Option<&str>) -> BackendResult;
    fn full_schema(&self) -> BackendResult;

    fn list_running(&self) -> BackendResult;
    fn kill_running(&self, id: u64) -> BackendResult;
    fn list_fixed_rules(&self) -> BackendResult;

    fn compact(&self) -> BackendResult;
    fn remove_relations(&self, relations: &[String]) -> BackendResult;
    fn rename_relations(&self, renames: &[(String, String)]) -> BackendResult;
    fn set_access_level(&self, level: &str, relations: &[String]) -> BackendResult;

    fn create_index(&self, relation: &str, index_name: &str, columns: &[String]) -> BackendResult;
    fn drop_index(&self, relation: &str, index_name: &str) -> BackendResult;

    fn export_relations(&self, relations: &[String]) -> BackendResult;
    fn import_relations(&self, data: &Value) -> BackendResult;
    fn backup(&self, path: &str) -> BackendResult;
    fn import_from_backup(&self, path: &str, relations: &[String]) -> BackendResult;

    fn mode_name(&self) -> &str;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Embedded Backend — direct DbInstance calls, zero overhead
// ═══════════════════════════════════════════════════════════════════════════════

pub struct EmbeddedBackend {
    db: DbInstance,
    engine: String,
}

impl EmbeddedBackend {
    pub fn new(engine: &str, path: &str, config: &str) -> Result<Self, String> {
        let db = DbInstance::new(engine, path, config).map_err(|e| e.to_string())?;
        Ok(Self {
            db,
            engine: engine.to_string(),
        })
    }
}

/// Helper: run a system command and return JSON.
fn run_sys(db: &DbInstance, script: &str) -> BackendResult {
    match db.run_script(script, BTreeMap::new(), ScriptMutability::Immutable) {
        Ok(rows) => Ok(rows.into_json()),
        Err(err) => Ok(json!({"ok": false, "message": err.to_string()})),
    }
}

/// Helper: run a mutable system command.
fn run_sys_mut(db: &DbInstance, script: &str) -> BackendResult {
    match db.run_script(script, BTreeMap::new(), ScriptMutability::Mutable) {
        Ok(_) => Ok(json!({"ok": true})),
        Err(err) => Ok(json!({"ok": false, "message": err.to_string()})),
    }
}

impl CozoBackend for EmbeddedBackend {
    fn query(
        &self,
        script: &str,
        params: &BTreeMap<String, Value>,
        immutable: bool,
        limit: usize,
        offset: usize,
    ) -> BackendResult {
        let start = std::time::Instant::now();
        let params_dv: BTreeMap<String, DataValue> = params
            .iter()
            .map(|(k, v)| (k.clone(), DataValue::from(v.clone())))
            .collect();
        let mutability = if immutable {
            ScriptMutability::Immutable
        } else {
            ScriptMutability::Mutable
        };

        match self.db.run_script(script, params_dv, mutability) {
            Ok(mut rows) => {
                let elapsed_ms = start.elapsed().as_millis();
                let total_rows = rows.rows.len();

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
                Ok(response)
            }
            Err(err) => {
                let elapsed_ms = start.elapsed().as_millis();
                Ok(json!({
                    "ok": false,
                    "message": err.to_string(),
                    "elapsed_ms": elapsed_ms,
                }))
            }
        }
    }

    fn explain(&self, script: &str) -> BackendResult {
        run_sys(&self.db, &format!("::explain {{ {} }}", script))
    }

    fn validate(&self, script: &str) -> BackendResult {
        match self
            .db
            .run_script(
                &format!("::explain {{ {} }}", script),
                BTreeMap::new(),
                ScriptMutability::Immutable,
            ) {
            Ok(_) => Ok(json!({"ok": true, "valid": true, "script": script})),
            Err(err) => Ok(json!({"ok": true, "valid": false, "error": err.to_string(), "script": script})),
        }
    }

    fn batch(
        &self,
        queries: &[(String, BTreeMap<String, Value>)],
        transactional: bool,
    ) -> BackendResult {
        let start = std::time::Instant::now();

        if transactional {
            let tx = self.db.multi_transaction(true);
            let mut results = Vec::new();
            for (script, params) in queries {
                let params_dv: BTreeMap<String, DataValue> = params
                    .iter()
                    .map(|(k, v)| (k.clone(), DataValue::from(v.clone())))
                    .collect();
                match tx.run_script(script, params_dv) {
                    Ok(rows) => results.push(rows.into_json()),
                    Err(err) => {
                        let _ = tx.abort();
                        let elapsed_ms = start.elapsed().as_millis();
                        return Ok(json!({
                            "ok": false,
                            "message": err.to_string(),
                            "elapsed_ms": elapsed_ms,
                        }));
                    }
                }
            }
            if let Err(err) = tx.commit() {
                let elapsed_ms = start.elapsed().as_millis();
                return Ok(json!({
                    "ok": false,
                    "message": err.to_string(),
                    "elapsed_ms": elapsed_ms,
                }));
            }
            let elapsed_ms = start.elapsed().as_millis();
            let count = results.len();
            Ok(json!({
                "ok": true,
                "results": results,
                "elapsed_ms": elapsed_ms,
                "query_count": count,
            }))
        } else {
            let mut results = Vec::new();
            for (script, params) in queries {
                let params_dv: BTreeMap<String, DataValue> = params
                    .iter()
                    .map(|(k, v)| (k.clone(), DataValue::from(v.clone())))
                    .collect();
                match self
                    .db
                    .run_script(script, params_dv, ScriptMutability::Mutable)
                {
                    Ok(rows) => results.push(rows.into_json()),
                    Err(err) => {
                        results.push(json!({"ok": false, "message": err.to_string()}))
                    }
                }
            }
            let elapsed_ms = start.elapsed().as_millis();
            let count = results.len();
            Ok(json!({
                "ok": true,
                "results": results,
                "elapsed_ms": elapsed_ms,
                "query_count": count,
            }))
        }
    }

    fn health(&self) -> BackendResult {
        let relations = self
            .db
            .run_script("::relations", BTreeMap::new(), ScriptMutability::Immutable);
        let running = self
            .db
            .run_script("::running", BTreeMap::new(), ScriptMutability::Immutable);
        let fixed_rules = self
            .db
            .run_script("::fixed_rules", BTreeMap::new(), ScriptMutability::Immutable);

        Ok(json!({
            "ok": true,
            "status": "healthy",
            "mode": "embedded",
            "engine": self.engine,
            "relation_count": relations.as_ref().map(|r| r.rows.len()).unwrap_or(0),
            "running_queries": running.as_ref().map(|r| r.rows.len()).unwrap_or(0),
            "fixed_rule_count": fixed_rules.as_ref().map(|r| r.rows.len()).unwrap_or(0),
        }))
    }

    fn list_relations(&self) -> BackendResult {
        run_sys(&self.db, "::relations")
    }

    fn list_columns(&self, relation: &str) -> BackendResult {
        run_sys(&self.db, &format!("::columns {}", relation))
    }

    fn list_indices(&self, relation: &str) -> BackendResult {
        run_sys(&self.db, &format!("::indices {}", relation))
    }

    fn show_triggers(&self, relation: &str) -> BackendResult {
        run_sys(&self.db, &format!("::show_triggers {}", relation))
    }

    fn describe_relation(&self, relation: &str, description: Option<&str>) -> BackendResult {
        let script = if let Some(desc) = description {
            format!("::describe {} '{}'", relation, desc.replace('\'', "\\'"))
        } else {
            format!("::describe {}", relation)
        };
        match self
            .db
            .run_script(&script, BTreeMap::new(), ScriptMutability::Mutable)
        {
            Ok(rows) => Ok(rows.into_json()),
            Err(err) => Ok(json!({"ok": false, "message": err.to_string()})),
        }
    }

    fn full_schema(&self) -> BackendResult {
        let relations = self
            .db
            .run_script("::relations", BTreeMap::new(), ScriptMutability::Immutable)
            .map_err(|e| e.to_string())?;

        let mut schema = Vec::new();
        for row in &relations.rows {
            let rel_name = match &row[0] {
                DataValue::Str(s) => s.to_string(),
                other => format!("{}", other),
            };

            let columns = self
                .db
                .run_script(
                    &format!("::columns {}", rel_name),
                    BTreeMap::new(),
                    ScriptMutability::Immutable,
                )
                .map_err(|e| e.to_string())?;

            let indices = self
                .db
                .run_script(
                    &format!("::indices {}", rel_name),
                    BTreeMap::new(),
                    ScriptMutability::Immutable,
                )
                .ok();

            let triggers = self
                .db
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

            if relations.headers.len() > 1 {
                for (i, header) in relations.headers.iter().enumerate().skip(1) {
                    rel_info[header] = Value::from(row[i].clone());
                }
            }

            schema.push(rel_info);
        }

        Ok(json!({
            "ok": true,
            "schema": schema,
        }))
    }

    fn list_running(&self) -> BackendResult {
        run_sys(&self.db, "::running")
    }

    fn kill_running(&self, id: u64) -> BackendResult {
        match self.db.run_script(
            "::kill $id",
            BTreeMap::from([("id".to_string(), DataValue::from(id as i64))]),
            ScriptMutability::Mutable,
        ) {
            Ok(rows) => Ok(rows.into_json()),
            Err(err) => Ok(json!({"ok": false, "message": err.to_string()})),
        }
    }

    fn list_fixed_rules(&self) -> BackendResult {
        run_sys(&self.db, "::fixed_rules")
    }

    fn compact(&self) -> BackendResult {
        run_sys_mut(&self.db, "::compact")
    }

    fn remove_relations(&self, relations: &[String]) -> BackendResult {
        let rels = relations.join(", ");
        run_sys_mut(&self.db, &format!("::remove {}", rels))
    }

    fn rename_relations(&self, renames: &[(String, String)]) -> BackendResult {
        let pairs: Vec<String> = renames
            .iter()
            .map(|(from, to)| format!("{} -> {}", from, to))
            .collect();
        run_sys_mut(&self.db, &format!("::rename {}", pairs.join(", ")))
    }

    fn set_access_level(&self, level: &str, relations: &[String]) -> BackendResult {
        let rels = relations.join(", ");
        run_sys_mut(&self.db, &format!("::access_level {} {}", level, rels))
    }

    fn create_index(&self, relation: &str, index_name: &str, columns: &[String]) -> BackendResult {
        let cols = columns.join(", ");
        run_sys_mut(
            &self.db,
            &format!("::index create {}:{} {{ {} }}", relation, index_name, cols),
        )
    }

    fn drop_index(&self, relation: &str, index_name: &str) -> BackendResult {
        run_sys_mut(
            &self.db,
            &format!("::index drop {}:{}", relation, index_name),
        )
    }

    fn export_relations(&self, relations: &[String]) -> BackendResult {
        match self.db.export_relations(relations.iter()) {
            Ok(data) => {
                let s: serde_json::Map<_, _> =
                    data.into_iter().map(|(k, v)| (k, v.into_json())).collect();
                Ok(json!({"ok": true, "data": s}))
            }
            Err(err) => Ok(json!({"ok": false, "message": err.to_string()})),
        }
    }

    fn import_relations(&self, data: &Value) -> BackendResult {
        match data.as_object() {
            None => Ok(json!({"ok": false, "message": "payload must be a JSON object"})),
            Some(obj) => {
                let mut relations = BTreeMap::new();
                for (k, v) in obj {
                    match NamedRows::from_json(v) {
                        Ok(nr) => {
                            relations.insert(k.clone(), nr);
                        }
                        Err(err) => {
                            return Ok(
                                json!({"ok": false, "message": format!("Bad data for '{}': {}", k, err)}),
                            );
                        }
                    }
                }
                match self.db.import_relations(relations) {
                    Ok(_) => Ok(json!({"ok": true})),
                    Err(err) => Ok(json!({"ok": false, "message": err.to_string()})),
                }
            }
        }
    }

    fn backup(&self, path: &str) -> BackendResult {
        match self.db.backup_db(path) {
            Ok(_) => Ok(json!({"ok": true})),
            Err(err) => Ok(json!({"ok": false, "message": err.to_string()})),
        }
    }

    fn import_from_backup(&self, path: &str, relations: &[String]) -> BackendResult {
        match self.db.import_from_backup(path, relations) {
            Ok(_) => Ok(json!({"ok": true})),
            Err(err) => Ok(json!({"ok": false, "message": err.to_string()})),
        }
    }

    fn mode_name(&self) -> &str {
        "embedded"
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Remote Backend — HTTP client to a running CozoDB server
// ═══════════════════════════════════════════════════════════════════════════════

pub struct RemoteBackend {
    base_url: String,
    auth: String,
}

impl RemoteBackend {
    pub fn new(url: &str, auth: &str) -> Self {
        Self {
            base_url: url.trim_end_matches('/').to_string(),
            auth: auth.to_string(),
        }
    }

    fn get(&self, path: &str) -> BackendResult {
        let url = format!("{}{}", self.base_url, path);
        let mut req = minreq::get(&url);
        if !self.auth.is_empty() {
            req = req.with_header("x-cozo-auth", &self.auth);
        }
        let resp = req.send().map_err(|e| e.to_string())?;
        let body = resp.as_str().map_err(|e| e.to_string())?;
        serde_json::from_str(body).map_err(|e| format!("Failed to parse response: {}", e))
    }

    fn post(&self, path: &str, body: &Value) -> BackendResult {
        let url = format!("{}{}", self.base_url, path);
        let mut req = minreq::post(&url)
            .with_header("Content-Type", "application/json")
            .with_body(body.to_string());
        if !self.auth.is_empty() {
            req = req.with_header("x-cozo-auth", &self.auth);
        }
        let resp = req.send().map_err(|e| e.to_string())?;
        let resp_body = resp.as_str().map_err(|e| e.to_string())?;
        serde_json::from_str(resp_body).map_err(|e| format!("Failed to parse response: {}", e))
    }

    fn put(&self, path: &str, body: &Value) -> BackendResult {
        let url = format!("{}{}", self.base_url, path);
        let mut req = minreq::put(&url)
            .with_header("Content-Type", "application/json")
            .with_body(body.to_string());
        if !self.auth.is_empty() {
            req = req.with_header("x-cozo-auth", &self.auth);
        }
        let resp = req.send().map_err(|e| e.to_string())?;
        let resp_body = resp.as_str().map_err(|e| e.to_string())?;
        serde_json::from_str(resp_body).map_err(|e| format!("Failed to parse response: {}", e))
    }

    fn delete(&self, path: &str) -> BackendResult {
        let url = format!("{}{}", self.base_url, path);
        let mut req = minreq::delete(&url);
        if !self.auth.is_empty() {
            req = req.with_header("x-cozo-auth", &self.auth);
        }
        let resp = req.send().map_err(|e| e.to_string())?;
        let body = resp.as_str().map_err(|e| e.to_string())?;
        serde_json::from_str(body).map_err(|e| format!("Failed to parse response: {}", e))
    }
}

impl CozoBackend for RemoteBackend {
    fn query(
        &self,
        script: &str,
        params: &BTreeMap<String, Value>,
        immutable: bool,
        limit: usize,
        offset: usize,
    ) -> BackendResult {
        self.post(
            "/api/query",
            &json!({
                "script": script,
                "params": params,
                "immutable": immutable,
                "limit": limit,
                "offset": offset,
            }),
        )
    }

    fn explain(&self, script: &str) -> BackendResult {
        self.post("/api/explain", &json!({"script": script}))
    }

    fn validate(&self, script: &str) -> BackendResult {
        self.post("/api/validate", &json!({"script": script}))
    }

    fn batch(
        &self,
        queries: &[(String, BTreeMap<String, Value>)],
        transactional: bool,
    ) -> BackendResult {
        let qs: Vec<Value> = queries
            .iter()
            .map(|(s, p)| json!({"script": s, "params": p}))
            .collect();
        self.post(
            "/api/batch",
            &json!({"queries": qs, "transactional": transactional}),
        )
    }

    fn health(&self) -> BackendResult {
        self.get("/api/health")
    }

    fn list_relations(&self) -> BackendResult {
        self.get("/api/relations")
    }

    fn list_columns(&self, relation: &str) -> BackendResult {
        self.get(&format!("/api/relations/{}/columns", relation))
    }

    fn list_indices(&self, relation: &str) -> BackendResult {
        self.get(&format!("/api/relations/{}/indices", relation))
    }

    fn show_triggers(&self, relation: &str) -> BackendResult {
        self.get(&format!("/api/relations/{}/triggers", relation))
    }

    fn describe_relation(&self, relation: &str, description: Option<&str>) -> BackendResult {
        self.post(
            &format!("/api/relations/{}/describe", relation),
            &json!({"description": description}),
        )
    }

    fn full_schema(&self) -> BackendResult {
        self.get("/api/schema")
    }

    fn list_running(&self) -> BackendResult {
        self.get("/api/running")
    }

    fn kill_running(&self, id: u64) -> BackendResult {
        self.delete(&format!("/api/running/{}", id))
    }

    fn list_fixed_rules(&self) -> BackendResult {
        self.get("/api/fixed-rules")
    }

    fn compact(&self) -> BackendResult {
        self.post("/api/compact", &json!({}))
    }

    fn remove_relations(&self, relations: &[String]) -> BackendResult {
        self.post("/api/remove-relations", &json!({"relations": relations}))
    }

    fn rename_relations(&self, renames: &[(String, String)]) -> BackendResult {
        let entries: Vec<Value> = renames
            .iter()
            .map(|(f, t)| json!({"from": f, "to": t}))
            .collect();
        self.post("/api/rename-relations", &json!({"renames": entries}))
    }

    fn set_access_level(&self, level: &str, relations: &[String]) -> BackendResult {
        self.post(
            "/api/access-level",
            &json!({"level": level, "relations": relations}),
        )
    }

    fn create_index(&self, relation: &str, index_name: &str, columns: &[String]) -> BackendResult {
        self.post(
            &format!("/api/relations/{}/indices", relation),
            &json!({"index_name": index_name, "columns": columns}),
        )
    }

    fn drop_index(&self, relation: &str, index_name: &str) -> BackendResult {
        self.delete(&format!("/api/relations/{}/indices/{}", relation, index_name))
    }

    fn export_relations(&self, relations: &[String]) -> BackendResult {
        self.get(&format!("/export/{}", relations.join(",")))
    }

    fn import_relations(&self, data: &Value) -> BackendResult {
        self.put("/import", data)
    }

    fn backup(&self, path: &str) -> BackendResult {
        self.post("/backup", &json!({"path": path}))
    }

    fn import_from_backup(&self, path: &str, relations: &[String]) -> BackendResult {
        self.post(
            "/import-from-backup",
            &json!({"path": path, "relations": relations}),
        )
    }

    fn mode_name(&self) -> &str {
        "remote"
    }
}
