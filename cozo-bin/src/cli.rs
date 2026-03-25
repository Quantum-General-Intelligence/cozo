/*
 * Copyright 2024, The Cozo Project Authors.
 *
 * This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
 * If a copy of the MPL was not distributed with this file,
 * You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! CLI client for CozoDB server.
//!
//! Provides a rich command-line interface to interact with a running CozoDB server
//! via the HTTP API. Designed for both human users and AI agents.

use std::fs;
use std::io::{self, Read};

use clap::{Args, Subcommand};
use serde_json::{json, Value};

#[derive(Args, Debug)]
pub(crate) struct CliArgs {
    /// Server URL (e.g., http://127.0.0.1:9070)
    #[clap(short = 'u', long, default_value_t = String::from("http://127.0.0.1:9070"))]
    url: String,

    /// Auth token for the server
    #[clap(short, long, default_value_t = String::new())]
    auth: String,

    /// Output format: table, json, jsonl, csv
    #[clap(short, long, default_value_t = String::from("table"))]
    format: String,

    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand, Debug)]
enum CliCommand {
    /// Execute a CozoScript query
    Query {
        /// The CozoScript to execute (use '-' for stdin)
        script: String,
        /// JSON string of parameters
        #[clap(short, long, default_value_t = String::from("{}"))]
        params: String,
        /// Execute as immutable (read-only)
        #[clap(short, long)]
        immutable: bool,
        /// Limit number of returned rows
        #[clap(short, long, default_value_t = 0)]
        limit: usize,
        /// Skip this many rows
        #[clap(short, long, default_value_t = 0)]
        offset: usize,
    },

    /// Run a CozoScript file
    Run {
        /// Path to the script file
        file: String,
        /// JSON string of parameters
        #[clap(short, long, default_value_t = String::from("{}"))]
        params: String,
    },

    /// Validate a query without executing it
    Validate {
        /// The CozoScript to validate (use '-' for stdin)
        script: String,
    },

    /// Explain a query plan
    Explain {
        /// The CozoScript to explain (use '-' for stdin)
        script: String,
    },

    /// Execute multiple queries from a JSON file
    Batch {
        /// Path to JSON file with queries array
        file: String,
        /// Wrap in a transaction
        #[clap(short, long)]
        transactional: bool,
    },

    /// Show server health and metadata
    Health,

    /// List all API endpoints (for agent discovery)
    Endpoints,

    /// Get full database schema
    Schema,

    /// List all stored relations
    Relations,

    /// Show columns for a relation
    Columns {
        /// Relation name
        relation: String,
    },

    /// Show indices for a relation
    Indices {
        /// Relation name
        relation: String,
    },

    /// Show triggers for a relation
    Triggers {
        /// Relation name
        relation: String,
    },

    /// Describe a relation (get or set description)
    Describe {
        /// Relation name
        relation: String,
        /// Optional description to set
        #[clap(short, long)]
        description: Option<String>,
    },

    /// Create an index on a relation
    CreateIndex {
        /// Relation name
        relation: String,
        /// Index name
        index_name: String,
        /// Columns to index (comma-separated)
        columns: String,
    },

    /// Drop an index from a relation
    DropIndex {
        /// Relation name
        relation: String,
        /// Index name
        index_name: String,
    },

    /// List currently running queries
    Running,

    /// Kill a running query
    Kill {
        /// Process ID to kill
        id: u64,
    },

    /// List available fixed rules (algorithms)
    FixedRules,

    /// Compact the database
    Compact,

    /// Remove stored relations
    Remove {
        /// Relation names to remove
        #[clap(required = true)]
        relations: Vec<String>,
    },

    /// Rename stored relations
    Rename {
        /// Rename pairs as "from:to" (can specify multiple)
        #[clap(required = true)]
        pairs: Vec<String>,
    },

    /// Set access level for relations
    AccessLevel {
        /// Access level: normal, protected, read_only, hidden
        level: String,
        /// Relation names
        #[clap(required = true)]
        relations: Vec<String>,
    },

    /// Export relations
    Export {
        /// Relation names to export (comma-separated)
        relations: String,
        /// Output file path (stdout if not specified)
        #[clap(short, long)]
        output: Option<String>,
    },

    /// Import relations from a JSON file
    Import {
        /// Path to JSON file with relation data
        file: String,
    },

    /// Create a database backup
    Backup {
        /// Path for the backup file
        path: String,
    },

    /// Import relations from a backup file
    ImportBackup {
        /// Path to the backup file
        path: String,
        /// Relation names to import (comma-separated)
        relations: String,
    },

    /// Watch for changes on a relation (SSE stream)
    Watch {
        /// Relation name to watch
        relation: String,
    },
}

fn http_get(url: &str, auth: &str) -> Result<Value, String> {
    let mut req = minreq::get(url);
    if !auth.is_empty() {
        req = req.with_header("x-cozo-auth", auth);
    }
    let resp = req.send().map_err(|e| e.to_string())?;
    let body = resp.as_str().map_err(|e| e.to_string())?;
    serde_json::from_str(body).map_err(|e| format!("Failed to parse response: {}", e))
}

fn http_post(url: &str, auth: &str, body: &Value) -> Result<Value, String> {
    let mut req = minreq::post(url)
        .with_header("Content-Type", "application/json")
        .with_body(body.to_string());
    if !auth.is_empty() {
        req = req.with_header("x-cozo-auth", auth);
    }
    let resp = req.send().map_err(|e| e.to_string())?;
    let resp_body = resp.as_str().map_err(|e| e.to_string())?;
    serde_json::from_str(resp_body).map_err(|e| format!("Failed to parse response: {}", e))
}

fn http_put(url: &str, auth: &str, body: &Value) -> Result<Value, String> {
    let mut req = minreq::put(url)
        .with_header("Content-Type", "application/json")
        .with_body(body.to_string());
    if !auth.is_empty() {
        req = req.with_header("x-cozo-auth", auth);
    }
    let resp = req.send().map_err(|e| e.to_string())?;
    let resp_body = resp.as_str().map_err(|e| e.to_string())?;
    serde_json::from_str(resp_body).map_err(|e| format!("Failed to parse response: {}", e))
}

fn http_delete(url: &str, auth: &str) -> Result<Value, String> {
    let mut req = minreq::delete(url);
    if !auth.is_empty() {
        req = req.with_header("x-cozo-auth", auth);
    }
    let resp = req.send().map_err(|e| e.to_string())?;
    let body = resp.as_str().map_err(|e| e.to_string())?;
    serde_json::from_str(body).map_err(|e| format!("Failed to parse response: {}", e))
}

fn read_script(script: &str) -> Result<String, String> {
    if script == "-" {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| e.to_string())?;
        Ok(buf)
    } else {
        Ok(script.to_string())
    }
}

fn format_output(result: &Value, format: &str) {
    match format {
        "json" => {
            println!(
                "{}",
                serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string())
            );
        }
        "jsonl" => {
            // Output each row as a JSON line
            if let (Some(headers), Some(rows)) = (
                result.get("headers").and_then(|h| h.as_array()),
                result.get("rows").and_then(|r| r.as_array()),
            ) {
                for row in rows {
                    if let Some(row_arr) = row.as_array() {
                        let obj: serde_json::Map<String, Value> = headers
                            .iter()
                            .zip(row_arr.iter())
                            .filter_map(|(h, v)| h.as_str().map(|s| (s.to_string(), v.clone())))
                            .collect();
                        println!("{}", Value::Object(obj));
                    }
                }
            } else {
                println!("{}", result);
            }
        }
        "csv" => {
            if let (Some(headers), Some(rows)) = (
                result.get("headers").and_then(|h| h.as_array()),
                result.get("rows").and_then(|r| r.as_array()),
            ) {
                // Print headers
                let header_strs: Vec<String> = headers
                    .iter()
                    .map(|h| h.as_str().unwrap_or("").to_string())
                    .collect();
                println!("{}", header_strs.join(","));

                // Print rows
                for row in rows {
                    if let Some(row_arr) = row.as_array() {
                        let row_strs: Vec<String> = row_arr
                            .iter()
                            .map(|v| match v {
                                Value::String(s) => {
                                    if s.contains(',') || s.contains('"') || s.contains('\n') {
                                        format!("\"{}\"", s.replace('"', "\"\""))
                                    } else {
                                        s.clone()
                                    }
                                }
                                Value::Null => String::new(),
                                other => other.to_string(),
                            })
                            .collect();
                        println!("{}", row_strs.join(","));
                    }
                }
            } else {
                println!("{}", result);
            }
        }
        _ => {
            // table format (default)
            if let (Some(headers), Some(rows)) = (
                result.get("headers").and_then(|h| h.as_array()),
                result.get("rows").and_then(|r| r.as_array()),
            ) {
                let mut table = prettytable::Table::new();
                let header_cells: Vec<prettytable::Cell> = headers
                    .iter()
                    .map(|h| prettytable::Cell::new(h.as_str().unwrap_or("")))
                    .collect();
                table.set_titles(prettytable::Row::new(header_cells));

                for row in rows {
                    if let Some(row_arr) = row.as_array() {
                        let cells: Vec<prettytable::Cell> = row_arr
                            .iter()
                            .map(|v| match v {
                                Value::String(s) => prettytable::Cell::new(s),
                                Value::Null => prettytable::Cell::new("null"),
                                other => prettytable::Cell::new(&other.to_string()),
                            })
                            .collect();
                        table.add_row(prettytable::Row::new(cells));
                    }
                }
                table.set_format(
                    *prettytable::format::consts::FORMAT_NO_BORDER_LINE_SEPARATOR,
                );
                table.printstd();

                // Print metadata if available
                if let Some(elapsed) = result.get("elapsed_ms") {
                    eprintln!("Elapsed: {}ms", elapsed);
                }
                if let Some(total) = result.get("total_rows") {
                    if let Some(returned) = result.get("returned_rows") {
                        if total != returned {
                            eprintln!("Showing {} of {} total rows", returned, total);
                        }
                    }
                }
            } else {
                // Fallback to JSON for non-tabular results
                println!(
                    "{}",
                    serde_json::to_string_pretty(result)
                        .unwrap_or_else(|_| result.to_string())
                );
            }
        }
    }

    // Print error if present
    if result.get("ok") == Some(&Value::Bool(false)) {
        if let Some(msg) = result.get("message").and_then(|m| m.as_str()) {
            eprintln!("Error: {}", msg);
        }
    }
}

pub(crate) fn cli_main(args: CliArgs) -> Result<(), Box<dyn std::error::Error>> {
    let base = args.url.trim_end_matches('/');
    let auth = &args.auth;
    let fmt = &args.format;

    let result = match args.command {
        CliCommand::Query {
            script,
            params,
            immutable,
            limit,
            offset,
        } => {
            let script = read_script(&script)?;
            let params: Value = serde_json::from_str(&params)
                .map_err(|e| format!("Invalid params JSON: {}", e))?;
            http_post(
                &format!("{}/api/query", base),
                auth,
                &json!({
                    "script": script,
                    "params": params,
                    "immutable": immutable,
                    "limit": limit,
                    "offset": offset,
                }),
            )
        }

        CliCommand::Run { file, params } => {
            let script = fs::read_to_string(&file)
                .map_err(|e| format!("Failed to read file '{}': {}", file, e))?;
            let params: Value = serde_json::from_str(&params)
                .map_err(|e| format!("Invalid params JSON: {}", e))?;
            http_post(
                &format!("{}/api/query", base),
                auth,
                &json!({
                    "script": script,
                    "params": params,
                }),
            )
        }

        CliCommand::Validate { script } => {
            let script = read_script(&script)?;
            http_post(
                &format!("{}/api/validate", base),
                auth,
                &json!({"script": script}),
            )
        }

        CliCommand::Explain { script } => {
            let script = read_script(&script)?;
            http_post(
                &format!("{}/api/explain", base),
                auth,
                &json!({"script": script}),
            )
        }

        CliCommand::Batch {
            file,
            transactional,
        } => {
            let content = fs::read_to_string(&file)
                .map_err(|e| format!("Failed to read file '{}': {}", file, e))?;
            let queries: Value = serde_json::from_str(&content)
                .map_err(|e| format!("Invalid JSON in file: {}", e))?;
            http_post(
                &format!("{}/api/batch", base),
                auth,
                &json!({
                    "queries": queries,
                    "transactional": transactional,
                }),
            )
        }

        CliCommand::Health => http_get(&format!("{}/api/health", base), auth),

        CliCommand::Endpoints => http_get(&format!("{}/api/endpoints", base), auth),

        CliCommand::Schema => http_get(&format!("{}/api/schema", base), auth),

        CliCommand::Relations => http_get(&format!("{}/api/relations", base), auth),

        CliCommand::Columns { relation } => {
            http_get(&format!("{}/api/relations/{}/columns", base, relation), auth)
        }

        CliCommand::Indices { relation } => {
            http_get(&format!("{}/api/relations/{}/indices", base, relation), auth)
        }

        CliCommand::Triggers { relation } => http_get(
            &format!("{}/api/relations/{}/triggers", base, relation),
            auth,
        ),

        CliCommand::Describe {
            relation,
            description,
        } => http_post(
            &format!("{}/api/relations/{}/describe", base, relation),
            auth,
            &json!({"description": description}),
        ),

        CliCommand::CreateIndex {
            relation,
            index_name,
            columns,
        } => {
            let cols: Vec<String> = columns.split(',').map(|s| s.trim().to_string()).collect();
            http_post(
                &format!("{}/api/relations/{}/indices", base, relation),
                auth,
                &json!({"index_name": index_name, "columns": cols}),
            )
        }

        CliCommand::DropIndex {
            relation,
            index_name,
        } => http_delete(
            &format!("{}/api/relations/{}/indices/{}", base, relation, index_name),
            auth,
        ),

        CliCommand::Running => http_get(&format!("{}/api/running", base), auth),

        CliCommand::Kill { id } => {
            http_delete(&format!("{}/api/running/{}", base, id), auth)
        }

        CliCommand::FixedRules => http_get(&format!("{}/api/fixed-rules", base), auth),

        CliCommand::Compact => http_post(&format!("{}/api/compact", base), auth, &json!({})),

        CliCommand::Remove { relations } => http_post(
            &format!("{}/api/remove-relations", base),
            auth,
            &json!({"relations": relations}),
        ),

        CliCommand::Rename { pairs } => {
            let renames: Result<Vec<Value>, String> = pairs
                .iter()
                .map(|p| {
                    let parts: Vec<&str> = p.split(':').collect();
                    if parts.len() != 2 {
                        Err(format!(
                            "Invalid rename pair '{}'. Expected 'from:to'",
                            p
                        ))
                    } else {
                        Ok(json!({"from": parts[0], "to": parts[1]}))
                    }
                })
                .collect();
            let renames = renames?;
            http_post(
                &format!("{}/api/rename-relations", base),
                auth,
                &json!({"renames": renames}),
            )
        }

        CliCommand::AccessLevel { level, relations } => http_post(
            &format!("{}/api/access-level", base),
            auth,
            &json!({"level": level, "relations": relations}),
        ),

        CliCommand::Export { relations, output } => {
            let result = http_get(
                &format!("{}/export/{}", base, relations),
                auth,
            )?;
            if let Some(path) = output {
                let content = serde_json::to_string_pretty(&result)
                    .map_err(|e| e.to_string())?;
                fs::write(&path, content)
                    .map_err(|e| format!("Failed to write to '{}': {}", path, e))?;
                eprintln!("Exported to {}", path);
                return Ok(());
            }
            Ok(result)
        }

        CliCommand::Import { file } => {
            let content = fs::read_to_string(&file)
                .map_err(|e| format!("Failed to read file '{}': {}", file, e))?;
            let data: Value = serde_json::from_str(&content)
                .map_err(|e| format!("Invalid JSON in file: {}", e))?;
            http_put(&format!("{}/import", base), auth, &data)
        }

        CliCommand::Backup { path } => {
            http_post(&format!("{}/backup", base), auth, &json!({"path": path}))
        }

        CliCommand::ImportBackup { path, relations } => {
            let rels: Vec<String> = relations.split(',').map(|s| s.trim().to_string()).collect();
            http_post(
                &format!("{}/import-from-backup", base),
                auth,
                &json!({"path": path, "relations": rels}),
            )
        }

        CliCommand::Watch { relation } => {
            eprintln!("Watching changes on '{}' (Ctrl+C to stop)...", relation);
            let url = format!("{}/changes/{}", base, relation);
            let mut req = minreq::get(&url);
            if !auth.is_empty() {
                req = req.with_header("x-cozo-auth", auth);
            }
            // For SSE, we use a streaming approach
            match req.send() {
                Ok(resp) => {
                    let body = resp.as_str().map_err(|e| e.to_string())?;
                    for line in body.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if let Ok(parsed) = serde_json::from_str::<Value>(data) {
                                format_output(&parsed, fmt);
                            } else {
                                println!("{}", data);
                            }
                        }
                    }
                }
                Err(e) => {
                    return Err(format!("SSE connection failed: {}", e).into());
                }
            }
            return Ok(());
        }
    };

    match result {
        Ok(val) => {
            format_output(&val, fmt);
            if val.get("ok") == Some(&Value::Bool(false)) {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}
