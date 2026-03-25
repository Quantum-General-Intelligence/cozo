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

/// Helper: run a system command and return JSON with `"ok": true` added.
fn run_sys(db: &DbInstance, script: &str) -> BackendResult {
    match db.run_script(script, BTreeMap::new(), ScriptMutability::Immutable) {
        Ok(rows) => {
            let mut val = rows.into_json();
            val["ok"] = json!(true);
            Ok(val)
        }
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
                response["ok"] = json!(true);
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
                    Ok(rows) => {
                        let mut val = rows.into_json();
                        val["ok"] = json!(true);
                        results.push(val);
                    }
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
                    Ok(rows) => {
                        let mut val = rows.into_json();
                        val["ok"] = json!(true);
                        results.push(val);
                    }
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
            format!(
                "::describe {} \"{}\"",
                relation,
                desc.replace('\\', "\\\\").replace('"', "\\\"")
            )
        } else {
            format!("::describe {}", relation)
        };
        match self
            .db
            .run_script(&script, BTreeMap::new(), ScriptMutability::Mutable)
        {
            Ok(rows) => {
                let mut val = rows.into_json();
                val["ok"] = json!(true);
                Ok(val)
            }
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
            Ok(rows) => {
                let mut val = rows.into_json();
                val["ok"] = json!(true);
                Ok(val)
            }
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

// ═══════════════════════════════════════════════════════════════════════════════
// Tests — every CozoBackend method on EmbeddedBackend, real DB, no mocks
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn new_embedded() -> EmbeddedBackend {
        EmbeddedBackend::new("mem", "", "{}").expect("failed to create in-memory db")
    }

    /// Seed a test relation with some data.
    fn seed_data(b: &EmbeddedBackend) {
        let r = b.query(
            ":create people {name: String => age: Int, city: String}",
            &BTreeMap::new(),
            false,
            0,
            0,
        );
        assert!(r.is_ok());
        let val = r.unwrap();
        assert_ne!(val.get("ok"), Some(&Value::Bool(false)), "create failed: {}", val);

        let r = b.query(
            r#"?[name, age, city] <- [
                ["Alice", 30, "NYC"],
                ["Bob", 25, "LA"],
                ["Charlie", 35, "NYC"],
                ["Diana", 28, "Chicago"],
                ["Eve", 40, "LA"]
            ]
            :put people {name => age, city}"#,
            &BTreeMap::new(),
            false,
            0,
            0,
        );
        assert!(r.is_ok());
        let val = r.unwrap();
        assert_ne!(val.get("ok"), Some(&Value::Bool(false)), "put failed: {}", val);
    }

    // ── Health ──────────────────────────────────────────────────────────────

    #[test]
    fn test_health() {
        let b = new_embedded();
        let r = b.health().unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["status"], "healthy");
        assert_eq!(r["mode"], "embedded");
        assert_eq!(r["engine"], "mem");
        assert!(r["relation_count"].as_u64().is_some());
        assert!(r["fixed_rule_count"].as_u64().unwrap() > 0);
    }

    // ── Query basics ────────────────────────────────────────────────────────

    #[test]
    fn test_query_simple() {
        let b = new_embedded();
        let r = b
            .query(
                "?[] <- [[1, 'hello'], [2, 'world']]",
                &BTreeMap::new(),
                false,
                0,
                0,
            )
            .unwrap();
        assert_eq!(r["ok"], true);
        let rows = r["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(r["elapsed_ms"].as_u64().is_some());
        assert_eq!(r["total_rows"], 2);
        assert_eq!(r["returned_rows"], 2);
    }

    #[test]
    fn test_query_with_params() {
        let b = new_embedded();
        let mut params = BTreeMap::new();
        params.insert("x".to_string(), json!(42));
        params.insert("y".to_string(), json!("hello"));
        let r = b
            .query("?[a, b] <- [[$x, $y]]", &params, false, 0, 0)
            .unwrap();
        assert_eq!(r["ok"], true);
        let rows = r["rows"].as_array().unwrap();
        assert_eq!(rows[0][0], 42);
        assert_eq!(rows[0][1], "hello");
    }

    #[test]
    fn test_query_immutable_prevents_writes() {
        let b = new_embedded();
        seed_data(&b);
        // Immutable query should succeed for reads
        let r = b
            .query(
                "?[name, age] := *people{name, age}",
                &BTreeMap::new(),
                true,
                0,
                0,
            )
            .unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["rows"].as_array().unwrap().len(), 5);

        // Immutable query should fail for writes
        let r = b
            .query(
                r#"?[name, age, city] <- [["Zara", 22, "SF"]] :put people {name => age, city}"#,
                &BTreeMap::new(),
                true,
                0,
                0,
            )
            .unwrap();
        assert_eq!(r["ok"], false);
    }

    #[test]
    fn test_query_limit_offset() {
        let b = new_embedded();
        let r = b
            .query(
                "?[x] <- [[1], [2], [3], [4], [5]]",
                &BTreeMap::new(),
                false,
                2,
                1,
            )
            .unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["total_rows"], 5);
        assert_eq!(r["returned_rows"], 2);
        assert_eq!(r["offset"], 1);
        assert_eq!(r["limit"], 2);
        let rows = r["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_query_limit_only() {
        let b = new_embedded();
        let r = b
            .query(
                "?[x] <- [[1], [2], [3], [4], [5]]",
                &BTreeMap::new(),
                false,
                3,
                0,
            )
            .unwrap();
        assert_eq!(r["total_rows"], 5);
        assert_eq!(r["returned_rows"], 3);
    }

    #[test]
    fn test_query_offset_past_end() {
        let b = new_embedded();
        let r = b
            .query(
                "?[x] <- [[1], [2], [3]]",
                &BTreeMap::new(),
                false,
                0,
                100,
            )
            .unwrap();
        assert_eq!(r["total_rows"], 3);
        assert_eq!(r["returned_rows"], 0);
    }

    #[test]
    fn test_query_error_bad_script() {
        let b = new_embedded();
        let r = b
            .query("THIS IS NOT VALID", &BTreeMap::new(), false, 0, 0)
            .unwrap();
        assert_eq!(r["ok"], false);
        assert!(r["message"].as_str().is_some());
        assert!(r["elapsed_ms"].as_u64().is_some());
    }

    // ── Explain ─────────────────────────────────────────────────────────────

    #[test]
    fn test_explain() {
        let b = new_embedded();
        seed_data(&b);
        let r = b.explain("?[name] := *people{name}").unwrap();
        assert_eq!(r["ok"], true);
        assert!(r["rows"].as_array().is_some());
        assert!(r["headers"].as_array().is_some());
    }

    #[test]
    fn test_explain_invalid_query() {
        let b = new_embedded();
        let r = b.explain("GARBAGE QUERY").unwrap();
        assert_eq!(r["ok"], false);
    }

    // ── Validate ────────────────────────────────────────────────────────────

    #[test]
    fn test_validate_valid_query() {
        let b = new_embedded();
        let r = b.validate("?[] <- [[1, 2, 3]]").unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["valid"], true);
        assert!(r["script"].as_str().is_some());
    }

    #[test]
    fn test_validate_invalid_query() {
        let b = new_embedded();
        let r = b.validate("NOT VALID SQL").unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["valid"], false);
        assert!(r["error"].as_str().is_some());
    }

    // ── Relations & Columns ─────────────────────────────────────────────────

    #[test]
    fn test_list_relations_empty() {
        let b = new_embedded();
        let r = b.list_relations().unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["rows"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_list_relations_with_data() {
        let b = new_embedded();
        seed_data(&b);
        let r = b.list_relations().unwrap();
        assert_eq!(r["ok"], true);
        let rows = r["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        // First column should be the name
        assert_eq!(rows[0][0], "people");
    }

    #[test]
    fn test_list_columns() {
        let b = new_embedded();
        seed_data(&b);
        let r = b.list_columns("people").unwrap();
        assert_eq!(r["ok"], true);
        let rows = r["rows"].as_array().unwrap();
        assert!(rows.len() >= 3); // name, age, city
        let headers = r["headers"].as_array().unwrap();
        assert!(!headers.is_empty());
    }

    #[test]
    fn test_list_columns_nonexistent() {
        let b = new_embedded();
        let r = b.list_columns("nonexistent").unwrap();
        assert_eq!(r["ok"], false);
    }

    // ── Indices ─────────────────────────────────────────────────────────────

    #[test]
    fn test_list_indices_empty() {
        let b = new_embedded();
        seed_data(&b);
        let r = b.list_indices("people").unwrap();
        assert_eq!(r["ok"], true);
    }

    #[test]
    fn test_create_and_list_and_drop_index() {
        let b = new_embedded();
        seed_data(&b);

        // Create index
        let r = b
            .create_index(
                "people",
                "idx_age",
                &["age".to_string()],
            )
            .unwrap();
        assert_eq!(r["ok"], true, "create index failed: {}", r);

        // List indices — should have our new index
        let r = b.list_indices("people").unwrap();
        assert_eq!(r["ok"], true);
        let rows = r["rows"].as_array().unwrap();
        let found = rows.iter().any(|row| {
            row.as_array()
                .map(|a| a.iter().any(|v| v.as_str() == Some("idx_age")))
                .unwrap_or(false)
        });
        assert!(found, "idx_age not found in indices: {:?}", rows);

        // Drop index
        let r = b.drop_index("people", "idx_age").unwrap();
        assert_eq!(r["ok"], true, "drop index failed: {}", r);

        // Verify dropped
        let r = b.list_indices("people").unwrap();
        let rows = r["rows"].as_array().unwrap();
        let found = rows.iter().any(|row| {
            row.as_array()
                .map(|a| a.iter().any(|v| v.as_str() == Some("idx_age")))
                .unwrap_or(false)
        });
        assert!(!found, "idx_age should be dropped");
    }

    // ── Triggers ────────────────────────────────────────────────────────────

    #[test]
    fn test_show_triggers() {
        let b = new_embedded();
        seed_data(&b);
        let r = b.show_triggers("people").unwrap();
        // May succeed or return empty triggers
        assert_eq!(r["ok"], true);
    }

    // ── Describe ────────────────────────────────────────────────────────────

    #[test]
    fn test_describe_get_and_set() {
        let b = new_embedded();
        seed_data(&b);

        // Note: ::describe is defined in the grammar but not wired into sys_script,
        // so it fails at parse time. This is a pre-existing CozoDB grammar issue.
        // We test that the backend correctly returns the error without crashing.
        let r = b
            .describe_relation("people", Some("A table of people"))
            .unwrap();
        // Returns ok:false because ::describe isn't in the parser's sys_script rule
        assert_eq!(r["ok"], false);
        assert!(r["message"].as_str().unwrap().contains("parser"));

        // Get (no description) also fails for the same reason
        let r = b.describe_relation("people", None).unwrap();
        assert_eq!(r["ok"], false);
    }

    // ── Full Schema ─────────────────────────────────────────────────────────

    #[test]
    fn test_full_schema_empty() {
        let b = new_embedded();
        let r = b.full_schema().unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["schema"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_full_schema_with_data() {
        let b = new_embedded();
        seed_data(&b);
        let r = b.full_schema().unwrap();
        assert_eq!(r["ok"], true);
        let schema = r["schema"].as_array().unwrap();
        assert_eq!(schema.len(), 1);
        let people = &schema[0];
        assert_eq!(people["name"], "people");
        assert!(people["columns"].is_object() || people["columns"].is_array());
    }

    // ── Batch ───────────────────────────────────────────────────────────────

    #[test]
    fn test_batch_non_transactional() {
        let b = new_embedded();
        let queries = vec![
            ("?[] <- [[1]]".to_string(), BTreeMap::new()),
            ("?[] <- [[2]]".to_string(), BTreeMap::new()),
            ("?[] <- [[3]]".to_string(), BTreeMap::new()),
        ];
        let r = b.batch(&queries, false).unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["query_count"], 3);
        let results = r["results"].as_array().unwrap();
        assert_eq!(results.len(), 3);
        for result in results {
            assert_eq!(result["ok"], true);
        }
    }

    #[test]
    fn test_batch_transactional_success() {
        let b = new_embedded();
        // First create the table
        seed_data(&b);

        let queries = vec![
            (
                r#"?[name, age, city] <- [["Frank", 50, "Boston"]] :put people {name => age, city}"#.to_string(),
                BTreeMap::new(),
            ),
            (
                r#"?[name, age, city] <- [["Grace", 33, "Denver"]] :put people {name => age, city}"#.to_string(),
                BTreeMap::new(),
            ),
        ];
        let r = b.batch(&queries, true).unwrap();
        assert_eq!(r["ok"], true, "batch failed: {}", r);
        assert_eq!(r["query_count"], 2);

        // Verify both rows were inserted
        let r = b
            .query(
                "?[count(name)] := *people{name}",
                &BTreeMap::new(),
                true,
                0,
                0,
            )
            .unwrap();
        let count = r["rows"].as_array().unwrap()[0][0].as_u64().unwrap();
        assert_eq!(count, 7); // 5 original + 2 new
    }

    #[test]
    fn test_batch_transactional_rollback_on_error() {
        let b = new_embedded();
        seed_data(&b);

        let queries = vec![
            (
                r#"?[name, age, city] <- [["Hank", 45, "Miami"]] :put people {name => age, city}"#.to_string(),
                BTreeMap::new(),
            ),
            (
                "THIS IS NOT VALID".to_string(),
                BTreeMap::new(),
            ),
        ];
        let r = b.batch(&queries, true).unwrap();
        assert_eq!(r["ok"], false, "batch should have failed: {}", r);

        // Hank should NOT be present (transaction rolled back)
        let r = b
            .query(
                "?[name] := *people{name}, name = 'Hank'",
                &BTreeMap::new(),
                true,
                0,
                0,
            )
            .unwrap();
        assert_eq!(r["rows"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_batch_non_transactional_partial_failure() {
        let b = new_embedded();
        let queries = vec![
            ("?[] <- [[1]]".to_string(), BTreeMap::new()),
            ("INVALID".to_string(), BTreeMap::new()),
            ("?[] <- [[3]]".to_string(), BTreeMap::new()),
        ];
        let r = b.batch(&queries, false).unwrap();
        assert_eq!(r["ok"], true);
        let results = r["results"].as_array().unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0]["ok"], true);
        assert_eq!(results[1]["ok"], false);
        assert_eq!(results[2]["ok"], true);
    }

    // ── Running / Kill ──────────────────────────────────────────────────────

    #[test]
    fn test_list_running() {
        let b = new_embedded();
        let r = b.list_running().unwrap();
        assert_eq!(r["ok"], true);
        assert!(r["rows"].as_array().is_some());
    }

    #[test]
    fn test_kill_nonexistent() {
        let b = new_embedded();
        // Killing a non-existent process should not crash
        let r = b.kill_running(99999).unwrap();
        // Cozo returns ok:true even for non-existent kills
        assert!(r.is_object());
    }

    // ── Fixed Rules ─────────────────────────────────────────────────────────

    #[test]
    fn test_list_fixed_rules() {
        let b = new_embedded();
        let r = b.list_fixed_rules().unwrap();
        assert_eq!(r["ok"], true);
        let rows = r["rows"].as_array().unwrap();
        assert!(rows.len() > 10, "expected many fixed rules, got {}", rows.len());
    }

    // ── Compact ─────────────────────────────────────────────────────────────

    #[test]
    fn test_compact() {
        let b = new_embedded();
        seed_data(&b);
        let r = b.compact().unwrap();
        assert_eq!(r["ok"], true);
    }

    // ── Remove Relations ────────────────────────────────────────────────────

    #[test]
    fn test_remove_relations() {
        let b = new_embedded();
        seed_data(&b);
        assert_eq!(b.list_relations().unwrap()["rows"].as_array().unwrap().len(), 1);

        let r = b.remove_relations(&["people".to_string()]).unwrap();
        assert_eq!(r["ok"], true);

        assert_eq!(b.list_relations().unwrap()["rows"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_remove_nonexistent_relation() {
        let b = new_embedded();
        let r = b.remove_relations(&["does_not_exist".to_string()]).unwrap();
        // Cozo may return ok:true even for non-existent removal
        assert!(r.is_object());
    }

    // ── Rename Relations ────────────────────────────────────────────────────

    #[test]
    fn test_rename_relations() {
        let b = new_embedded();
        seed_data(&b);

        let r = b
            .rename_relations(&[("people".to_string(), "humans".to_string())])
            .unwrap();
        assert_eq!(r["ok"], true, "rename failed: {}", r);

        // Old name should not exist
        let r = b.list_columns("people").unwrap();
        assert_eq!(r["ok"], false);

        // New name should work
        let r = b.list_columns("humans").unwrap();
        assert_eq!(r["ok"], true);
        assert!(r["rows"].as_array().unwrap().len() >= 3);

        // Data should be preserved
        let r = b
            .query(
                "?[count(name)] := *humans{name}",
                &BTreeMap::new(),
                true,
                0,
                0,
            )
            .unwrap();
        assert_eq!(r["rows"].as_array().unwrap()[0][0], 5);
    }

    // ── Access Level ────────────────────────────────────────────────────────

    #[test]
    fn test_set_access_level() {
        let b = new_embedded();
        seed_data(&b);

        // Set to read-only
        let r = b
            .set_access_level("read_only", &["people".to_string()])
            .unwrap();
        assert_eq!(r["ok"], true, "set access level failed: {}", r);

        // Writes should now fail
        let r = b
            .query(
                r#"?[name, age, city] <- [["Zara", 22, "SF"]] :put people {name => age, city}"#,
                &BTreeMap::new(),
                false,
                0,
                0,
            )
            .unwrap();
        assert_eq!(r["ok"], false);

        // Reads should still work
        let r = b
            .query(
                "?[name] := *people{name}",
                &BTreeMap::new(),
                true,
                0,
                0,
            )
            .unwrap();
        assert_eq!(r["ok"], true);

        // Reset to normal
        let r = b
            .set_access_level("normal", &["people".to_string()])
            .unwrap();
        assert_eq!(r["ok"], true);
    }

    // ── Export / Import ─────────────────────────────────────────────────────

    #[test]
    fn test_export_and_import() {
        let b = new_embedded();
        seed_data(&b);

        // Export
        let r = b.export_relations(&["people".to_string()]).unwrap();
        assert_eq!(r["ok"], true, "export failed: {}", r);
        let export_data = r["data"].clone();
        assert!(export_data["people"].is_object(), "missing people data: {}", export_data);

        // Remove original
        let r = b.remove_relations(&["people".to_string()]).unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(b.list_relations().unwrap()["rows"].as_array().unwrap().len(), 0);

        // Re-create the relation schema first
        let _ = b.query(
            ":create people {name: String => age: Int, city: String}",
            &BTreeMap::new(),
            false,
            0,
            0,
        );

        // Import
        let r = b.import_relations(&export_data).unwrap();
        assert_eq!(r["ok"], true, "import failed: {}", r);

        // Verify data is back
        let r = b
            .query(
                "?[count(name)] := *people{name}",
                &BTreeMap::new(),
                true,
                0,
                0,
            )
            .unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["rows"].as_array().unwrap()[0][0], 5);
    }

    #[test]
    fn test_import_bad_data() {
        let b = new_embedded();
        let r = b.import_relations(&json!("not an object")).unwrap();
        assert_eq!(r["ok"], false);
    }

    // ── Backup / Import from Backup ─────────────────────────────────────────

    #[test]
    fn test_backup_and_import_from_backup() {
        let b = new_embedded();
        seed_data(&b);

        let backup_path = format!("/tmp/cozo_test_backup_{}.db", std::process::id());

        // Ensure clean
        let _ = std::fs::remove_file(&backup_path);

        // Backup
        let r = b.backup(&backup_path).unwrap();
        assert_eq!(r["ok"], true, "backup failed: {}", r);
        assert!(
            std::fs::metadata(&backup_path).is_ok(),
            "backup file should exist"
        );

        // Create a fresh DB and import from backup
        let b2 = new_embedded();
        let _ = b2.query(
            ":create people {name: String => age: Int, city: String}",
            &BTreeMap::new(),
            false,
            0,
            0,
        );
        let r = b2
            .import_from_backup(&backup_path, &["people".to_string()])
            .unwrap();
        assert_eq!(r["ok"], true, "import from backup failed: {}", r);

        // Verify data
        let r = b2
            .query(
                "?[count(name)] := *people{name}",
                &BTreeMap::new(),
                true,
                0,
                0,
            )
            .unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["rows"].as_array().unwrap()[0][0], 5);

        // Cleanup
        let _ = std::fs::remove_file(&backup_path);
    }

    // ── Mode Name ───────────────────────────────────────────────────────────

    #[test]
    fn test_mode_name() {
        let b = new_embedded();
        assert_eq!(b.mode_name(), "embedded");
    }

    // ── Complex multi-step workflow ─────────────────────────────────────────

    #[test]
    fn test_full_lifecycle() {
        let b = new_embedded();

        // 1. Health check
        assert_eq!(b.health().unwrap()["ok"], true);

        // 2. No relations yet
        assert_eq!(
            b.list_relations().unwrap()["rows"].as_array().unwrap().len(),
            0
        );

        // 3. Create a relation with data
        seed_data(&b);

        // 4. Verify relation exists
        let rels = b.list_relations().unwrap();
        assert_eq!(rels["rows"].as_array().unwrap().len(), 1);

        // 5. Check columns
        let cols = b.list_columns("people").unwrap();
        assert!(cols["rows"].as_array().unwrap().len() >= 3);

        // 6. Create index
        let r = b
            .create_index("people", "idx_city", &["city".to_string()])
            .unwrap();
        assert_eq!(r["ok"], true);

        // 7. Schema should show everything (may include index relation)
        let schema = b.full_schema().unwrap();
        assert!(schema["schema"].as_array().unwrap().len() >= 1);

        // 8. Query with filtering
        let r = b
            .query(
                "?[name, age] := *people{name, age, city}, city = 'NYC'",
                &BTreeMap::new(),
                true,
                0,
                0,
            )
            .unwrap();
        assert_eq!(r["rows"].as_array().unwrap().len(), 2); // Alice, Charlie

        // 9. Explain the query
        let r = b
            .explain("?[name] := *people{name, city}, city = 'NYC'")
            .unwrap();
        assert_eq!(r["ok"], true);

        // 10. Validate
        let r = b
            .validate("?[name] := *people{name}")
            .unwrap();
        assert_eq!(r["valid"], true);

        // 11. Drop index
        b.drop_index("people", "idx_city").unwrap();

        // 12. Set description (returns error because ::describe isn't wired in grammar)
        let r = b.describe_relation("people", Some("People table")).unwrap();
        assert_eq!(r["ok"], false); // Known grammar issue

        // 13. Compact
        assert_eq!(b.compact().unwrap()["ok"], true);

        // 14. Remove
        assert_eq!(
            b.remove_relations(&["people".to_string()]).unwrap()["ok"],
            true
        );
        assert_eq!(
            b.list_relations().unwrap()["rows"].as_array().unwrap().len(),
            0
        );
    }

    // ── Multiple relations ──────────────────────────────────────────────────

    #[test]
    fn test_multiple_relations() {
        let b = new_embedded();
        seed_data(&b);

        // Create a second relation
        b.query(
            ":create cities {name: String => population: Int}",
            &BTreeMap::new(),
            false,
            0,
            0,
        )
        .unwrap();
        b.query(
            r#"?[name, population] <- [["NYC", 8000000], ["LA", 4000000], ["Chicago", 2700000]]
            :put cities {name => population}"#,
            &BTreeMap::new(),
            false,
            0,
            0,
        )
        .unwrap();

        // Should have 2 relations
        let rels = b.list_relations().unwrap();
        assert_eq!(rels["rows"].as_array().unwrap().len(), 2);

        // Schema should cover both
        let schema = b.full_schema().unwrap();
        assert_eq!(schema["schema"].as_array().unwrap().len(), 2);

        // Join query across relations
        let r = b
            .query(
                "?[name, age, population] := *people{name, age, city}, *cities{name: city, population}",
                &BTreeMap::new(),
                true,
                0,
                0,
            )
            .unwrap();
        assert_eq!(r["ok"], true);
        // Alice(NYC), Bob(LA), Charlie(NYC), Diana(Chicago), Eve(LA)
        assert_eq!(r["rows"].as_array().unwrap().len(), 5);

        // Rename one
        b.rename_relations(&[("cities".to_string(), "metropolis".to_string())])
            .unwrap();

        let rels = b.list_relations().unwrap();
        let names: Vec<&str> = rels["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r[0].as_str().unwrap())
            .collect();
        assert!(names.contains(&"people"));
        assert!(names.contains(&"metropolis"));
        assert!(!names.contains(&"cities"));
    }

    // ── Export multiple relations ────────────────────────────────────────────

    #[test]
    fn test_export_multiple() {
        let b = new_embedded();
        seed_data(&b);

        b.query(
            ":create tags {label: String}",
            &BTreeMap::new(),
            false,
            0,
            0,
        )
        .unwrap();
        b.query(
            r#"?[label] <- [["active"], ["admin"]] :put tags {label}"#,
            &BTreeMap::new(),
            false,
            0,
            0,
        )
        .unwrap();

        let r = b
            .export_relations(&["people".to_string(), "tags".to_string()])
            .unwrap();
        assert_eq!(r["ok"], true);
        assert!(r["data"]["people"].is_object());
        assert!(r["data"]["tags"].is_object());
    }

    // ── Batch with parameters ───────────────────────────────────────────────

    #[test]
    fn test_batch_with_params() {
        let b = new_embedded();
        let mut p1 = BTreeMap::new();
        p1.insert("val".to_string(), json!(100));
        let mut p2 = BTreeMap::new();
        p2.insert("val".to_string(), json!(200));

        let queries = vec![
            ("?[x] <- [[$val]]".to_string(), p1),
            ("?[x] <- [[$val]]".to_string(), p2),
        ];
        let r = b.batch(&queries, false).unwrap();
        assert_eq!(r["ok"], true);
        let results = r["results"].as_array().unwrap();
        assert_eq!(results[0]["rows"].as_array().unwrap()[0][0], 100);
        assert_eq!(results[1]["rows"].as_array().unwrap()[0][0], 200);
    }
}
