/*
 * Copyright 2024, The Cozo Project Authors.
 *
 * This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
 * If a copy of the MPL was not distributed with this file,
 * You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! CLI client for CozoDB — dual mode: embedded (direct) or remote (HTTP).
//!
//! **Embedded mode** (default): Opens the database directly in-process.
//! No network overhead. Use `--engine` and `--path` to configure.
//!
//! **Remote mode** (`--remote`): Connects to a running CozoDB server via HTTP.
//! Use `--url` and `--auth` to configure.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::backend::{CozoBackend, EmbeddedBackend, RemoteBackend};

#[derive(Args, Debug)]
pub(crate) struct CliArgs {
    /// Connect to a remote CozoDB server instead of opening the database directly
    #[clap(long)]
    remote: bool,

    // ── Remote mode options ──

    /// Server URL for remote mode (e.g., http://127.0.0.1:9070)
    #[clap(short = 'u', long, default_value_t = String::from("http://127.0.0.1:9070"))]
    url: String,

    /// Auth token for remote server
    #[clap(short, long, default_value_t = String::new())]
    auth: String,

    // ── Embedded mode options ──

    /// Database engine: mem, sqlite, rocksdb (embedded mode)
    #[clap(short, long, default_value_t = String::from("mem"))]
    engine: String,

    /// Path to database directory (embedded mode)
    #[clap(short, long, default_value_t = String::from("cozo.db"))]
    path: String,

    /// Extra config in JSON format (embedded mode)
    #[clap(short, long, default_value_t = String::from("{}"))]
    config: String,

    // ── Output options ──

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

    /// Show database health and metadata
    Health,

    /// List all API endpoints (remote mode: for agent discovery)
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

    /// Watch for changes on a relation (remote mode only, SSE stream)
    Watch {
        /// Relation name to watch
        relation: String,
    },
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
                let header_strs: Vec<String> = headers
                    .iter()
                    .map(|h| h.as_str().unwrap_or("").to_string())
                    .collect();
                println!("{}", header_strs.join(","));

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
                println!(
                    "{}",
                    serde_json::to_string_pretty(result)
                        .unwrap_or_else(|_| result.to_string())
                );
            }
        }
    }

    if result.get("ok") == Some(&Value::Bool(false)) {
        if let Some(msg) = result.get("message").and_then(|m| m.as_str()) {
            eprintln!("Error: {}", msg);
        }
    }
}

pub(crate) fn cli_main(args: CliArgs) -> Result<(), Box<dyn std::error::Error>> {
    let fmt = args.format.clone();

    // Build the backend based on mode
    let backend: Box<dyn CozoBackend> = if args.remote {
        eprintln!("Mode: remote ({})", args.url);
        Box::new(RemoteBackend::new(&args.url, &args.auth))
    } else {
        eprintln!("Mode: embedded (engine={}, path={})", args.engine, args.path);
        Box::new(
            EmbeddedBackend::new(&args.engine, &args.path, &args.config)
                .map_err(|e| format!("Failed to open database: {}", e))?,
        )
    };

    let result = match args.command {
        CliCommand::Query {
            script,
            params,
            immutable,
            limit,
            offset,
        } => {
            let script = read_script(&script)?;
            let params: BTreeMap<String, Value> = serde_json::from_str(&params)
                .map_err(|e| format!("Invalid params JSON: {}", e))?;
            backend.query(&script, &params, immutable, limit, offset)
        }

        CliCommand::Run { file, params } => {
            let script = fs::read_to_string(&file)
                .map_err(|e| format!("Failed to read file '{}': {}", file, e))?;
            let params: BTreeMap<String, Value> = serde_json::from_str(&params)
                .map_err(|e| format!("Invalid params JSON: {}", e))?;
            backend.query(&script, &params, false, 0, 0)
        }

        CliCommand::Validate { script } => {
            let script = read_script(&script)?;
            backend.validate(&script)
        }

        CliCommand::Explain { script } => {
            let script = read_script(&script)?;
            backend.explain(&script)
        }

        CliCommand::Batch {
            file,
            transactional,
        } => {
            let content = fs::read_to_string(&file)
                .map_err(|e| format!("Failed to read file '{}': {}", file, e))?;
            let raw: Vec<Value> = serde_json::from_str(&content)
                .map_err(|e| format!("Invalid JSON in file: {}", e))?;
            let queries: Vec<(String, BTreeMap<String, Value>)> = raw
                .into_iter()
                .map(|v| {
                    let script = v
                        .get("script")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    let params: BTreeMap<String, Value> = v
                        .get("params")
                        .and_then(|p| serde_json::from_value(p.clone()).ok())
                        .unwrap_or_default();
                    (script, params)
                })
                .collect();
            backend.batch(&queries, transactional)
        }

        CliCommand::Health => backend.health(),

        CliCommand::Endpoints => {
            if args.remote {
                // Only makes sense for remote mode — fetch from server
                let remote = RemoteBackend::new(&args.url, &args.auth);
                remote.health() // Use the get method indirectly
                // Actually, let's use a direct get
            } else {
                Ok(json!({
                    "ok": true,
                    "message": "Endpoints discovery is only available in remote mode (--remote). In embedded mode, all operations are available as CLI subcommands."
                }))
            }
        }

        CliCommand::Schema => backend.full_schema(),
        CliCommand::Relations => backend.list_relations(),
        CliCommand::Columns { relation } => backend.list_columns(&relation),
        CliCommand::Indices { relation } => backend.list_indices(&relation),
        CliCommand::Triggers { relation } => backend.show_triggers(&relation),

        CliCommand::Describe {
            relation,
            description,
        } => backend.describe_relation(&relation, description.as_deref()),

        CliCommand::CreateIndex {
            relation,
            index_name,
            columns,
        } => {
            let cols: Vec<String> = columns.split(',').map(|s| s.trim().to_string()).collect();
            backend.create_index(&relation, &index_name, &cols)
        }

        CliCommand::DropIndex {
            relation,
            index_name,
        } => backend.drop_index(&relation, &index_name),

        CliCommand::Running => backend.list_running(),
        CliCommand::Kill { id } => backend.kill_running(id),
        CliCommand::FixedRules => backend.list_fixed_rules(),
        CliCommand::Compact => backend.compact(),

        CliCommand::Remove { relations } => backend.remove_relations(&relations),

        CliCommand::Rename { pairs } => {
            let renames: Result<Vec<(String, String)>, String> = pairs
                .iter()
                .map(|p| {
                    let parts: Vec<&str> = p.split(':').collect();
                    if parts.len() != 2 {
                        Err(format!(
                            "Invalid rename pair '{}'. Expected 'from:to'",
                            p
                        ))
                    } else {
                        Ok((parts[0].to_string(), parts[1].to_string()))
                    }
                })
                .collect();
            backend.rename_relations(&renames?)
        }

        CliCommand::AccessLevel { level, relations } => {
            backend.set_access_level(&level, &relations)
        }

        CliCommand::Export { relations, output } => {
            let rels: Vec<String> = relations.split(',').map(|s| s.trim().to_string()).collect();
            let result = backend.export_relations(&rels)?;
            if let Some(path) = output {
                // Write only the relation data, not the wrapper
                let export_data = result.get("data").unwrap_or(&result);
                let content =
                    serde_json::to_string_pretty(export_data).map_err(|e| e.to_string())?;
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
            let mut data: Value = serde_json::from_str(&content)
                .map_err(|e| format!("Invalid JSON in file: {}", e))?;
            // If the file has a "data" wrapper (from export), unwrap it
            if data.get("data").is_some() && data.get("ok").is_some() {
                data = data["data"].take();
            }
            backend.import_relations(&data)
        }

        CliCommand::Backup { path } => backend.backup(&path),

        CliCommand::ImportBackup { path, relations } => {
            let rels: Vec<String> = relations.split(',').map(|s| s.trim().to_string()).collect();
            backend.import_from_backup(&path, &rels)
        }

        CliCommand::Watch { relation } => {
            if !args.remote {
                return Err("Watch is only available in remote mode (--remote). Use callbacks in embedded mode.".into());
            }
            eprintln!("Watching changes on '{}' (Ctrl+C to stop)...", relation);
            let url = format!("{}/changes/{}", args.url.trim_end_matches('/'), relation);
            let mut req = minreq::get(&url);
            if !args.auth.is_empty() {
                req = req.with_header("x-cozo-auth", &args.auth);
            }
            match req.send() {
                Ok(resp) => {
                    let body = resp.as_str().map_err(|e| e.to_string())?;
                    for line in body.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if let Ok(parsed) = serde_json::from_str::<Value>(data) {
                                format_output(&parsed, &fmt);
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
            format_output(&val, &fmt);
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
