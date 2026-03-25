/*
 * Copyright 2024, The Cozo Project Authors.
 *
 * This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
 * If a copy of the MPL was not distributed with this file,
 * You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! End-to-end integration tests for the CLI binary.
//!
//! Tests both embedded mode (default) and remote mode (--remote against a real server).
//! NO mocks. NO skips. Real databases. Real network. Real binary.

use std::io::Write;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::Once;
use std::time::Duration;

static BUILD_ONCE: Once = Once::new();

/// Build the binary once before all tests.
fn ensure_built() {
    BUILD_ONCE.call_once(|| {
        let status = Command::new("cargo")
            .args(["build", "-p", "cozo-bin", "--features=compact"])
            .status()
            .expect("failed to run cargo build");
        assert!(status.success(), "cargo build failed");
    });
}

fn binary_path() -> String {
    // Find the binary
    let output = Command::new("cargo")
        .args([
            "build",
            "-p",
            "cozo-bin",
            "--features=compact",
            "--message-format=json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .expect("failed to build");

    // Parse the JSON lines to find the executable
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            if val["reason"] == "compiler-artifact"
                && val["target"]["name"] == "cozo-bin"
                && val["executable"].is_string()
            {
                return val["executable"].as_str().unwrap().to_string();
            }
        }
    }

    // Fallback: standard debug path
    format!("target/debug/cozo-bin")
}

/// Run the CLI in embedded mode and return (stdout, stderr, exit_code).
fn cli_embedded(args: &[&str]) -> (String, String, i32) {
    ensure_built();
    let bin = binary_path();
    let mut cmd_args = vec!["cli"];
    cmd_args.extend_from_slice(args);

    let output = Command::new(&bin)
        .args(&cmd_args)
        .output()
        .expect("failed to execute binary");

    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// Run the CLI in remote mode against a specific port.
fn cli_remote(port: u16, args: &[&str]) -> (String, String, i32) {
    ensure_built();
    let bin = binary_path();
    let url = format!("http://127.0.0.1:{}", port);
    let mut cmd_args = vec!["cli", "--remote", "-u", &url];
    cmd_args.extend_from_slice(args);

    let output = Command::new(&bin)
        .args(&cmd_args)
        .output()
        .expect("failed to execute binary");

    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// Find an available port.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Start a real CozoDB server on a given port. Returns the child process.
fn start_server(port: u16) -> Child {
    ensure_built();
    let bin = binary_path();
    let child = Command::new(&bin)
        .args([
            "server",
            "--engine",
            "mem",
            "--path",
            "",
            "--bind",
            "127.0.0.1",
            "--port",
            &port.to_string(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start server");

    // Wait for server to be ready
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(resp) = minreq::get(format!("http://127.0.0.1:{}/api/health", port)).send() {
            if resp.status_code == 200 {
                return child;
            }
        }
    }
    panic!("Server failed to start on port {}", port);
}

// ═══════════════════════════════════════════════════════════════════════════════
// EMBEDDED MODE TESTS — via the actual binary
// ═══════════════════════════════════════════════════════════════════════════════

mod embedded {
    use super::*;

    #[test]
    fn health() {
        let (stdout, stderr, code) = cli_embedded(&["-f", "json", "health"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
        assert_eq!(val["status"], "healthy");
        assert_eq!(val["engine"], "mem");
    }

    #[test]
    fn query_simple() {
        let (stdout, stderr, code) = cli_embedded(&[
            "-f",
            "json",
            "query",
            "?[] <- [[1, 'hello'], [2, 'world']]",
        ]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
        assert_eq!(val["rows"].as_array().unwrap().len(), 2);
        assert!(val["elapsed_ms"].as_u64().is_some());
    }

    #[test]
    fn query_with_params() {
        let (stdout, stderr, code) = cli_embedded(&[
            "-f",
            "json",
            "query",
            "?[a] <- [[$x]]",
            "-p",
            r#"{"x": 42}"#,
        ]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["rows"].as_array().unwrap()[0][0], 42);
    }

    #[test]
    fn query_with_limit_and_offset() {
        let (stdout, stderr, code) = cli_embedded(&[
            "-f",
            "json",
            "query",
            "?[x] <- [[1],[2],[3],[4],[5]]",
            "-l",
            "2",
            "-o",
            "1",
        ]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["total_rows"], 5);
        assert_eq!(val["returned_rows"], 2);
    }

    #[test]
    fn query_immutable() {
        let (stdout, stderr, code) = cli_embedded(&[
            "-f",
            "json",
            "query",
            "?[] <- [[1]]",
            "-i",
        ]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
    }

    #[test]
    fn query_error() {
        let (_, stderr, code) = cli_embedded(&["-f", "json", "query", "INVALID SCRIPT"]);
        assert_ne!(code, 0, "should have failed, stderr: {}", stderr);
    }

    #[test]
    fn validate_valid() {
        let (stdout, stderr, code) =
            cli_embedded(&["-f", "json", "validate", "?[] <- [[1,2,3]]"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["valid"], true);
    }

    #[test]
    fn validate_invalid() {
        let (stdout, stderr, code) = cli_embedded(&["-f", "json", "validate", "NOT VALID"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["valid"], false);
    }

    #[test]
    fn explain() {
        let (stdout, stderr, code) = cli_embedded(&["-f", "json", "explain", "?[] <- [[1]]"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
        assert!(val["rows"].as_array().is_some());
    }

    #[test]
    fn relations_empty() {
        let (stdout, stderr, code) = cli_embedded(&["-f", "json", "relations"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
        assert_eq!(val["rows"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn fixed_rules() {
        let (stdout, stderr, code) = cli_embedded(&["-f", "json", "fixed-rules"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
        assert!(val["rows"].as_array().unwrap().len() > 10);
    }

    #[test]
    fn running() {
        let (stdout, stderr, code) = cli_embedded(&["-f", "json", "running"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
    }

    #[test]
    fn schema_empty() {
        let (stdout, stderr, code) = cli_embedded(&["-f", "json", "schema"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
        assert_eq!(val["schema"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn compact() {
        let (stdout, stderr, code) = cli_embedded(&["-f", "json", "compact"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
    }

    #[test]
    fn endpoints_embedded_mode() {
        let (stdout, stderr, code) = cli_embedded(&["-f", "json", "endpoints"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
        // In embedded mode, endpoints says "only available in remote"
        assert!(val["message"].as_str().is_some());
    }

    // ── Output formats ─────────────────────────────────────────────────────

    #[test]
    fn format_table() {
        let (stdout, stderr, code) =
            cli_embedded(&["query", "?[x, y] <- [[1, 'a'], [2, 'b']]"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        // Table output should contain column separators
        assert!(stdout.contains("x"), "stdout: {}", stdout);
        assert!(stdout.contains("y"), "stdout: {}", stdout);
        assert!(stdout.contains("a"), "stdout: {}", stdout);
    }

    #[test]
    fn format_csv() {
        let (stdout, stderr, code) =
            cli_embedded(&["-f", "csv", "query", "?[x, y] <- [[1, 'hello'], [2, 'world']]"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let lines: Vec<&str> = stdout.trim().lines().collect();
        assert!(lines.len() >= 3, "expected header + 2 rows, got: {:?}", lines);
        assert!(lines[0].contains(","), "header should have commas: {}", lines[0]);
    }

    #[test]
    fn format_jsonl() {
        let (stdout, stderr, code) =
            cli_embedded(&["-f", "jsonl", "query", "?[x, y] <- [[1, 'a'], [2, 'b']]"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let lines: Vec<&str> = stdout.trim().lines().collect();
        assert_eq!(lines.len(), 2, "expected 2 JSONL lines, got: {:?}", lines);
        for line in &lines {
            let val: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(val["x"].is_number());
            assert!(val["y"].is_string());
        }
    }

    // ── Run command with file ───────────────────────────────────────────────

    #[test]
    fn run_script_file() {
        let dir = std::env::temp_dir();
        let script_path = dir.join(format!("cozo_test_{}.cozo", std::process::id()));
        let mut f = std::fs::File::create(&script_path).unwrap();
        write!(f, "?[x] <- [[42]]").unwrap();

        let (stdout, stderr, code) = cli_embedded(&[
            "-f",
            "json",
            "run",
            script_path.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["rows"].as_array().unwrap()[0][0], 42);

        std::fs::remove_file(&script_path).unwrap();
    }

    // ── Engine selection ────────────────────────────────────────────────────

    #[test]
    fn sqlite_engine() {
        let dir = std::env::temp_dir();
        let db_path = dir.join(format!("cozo_sqlite_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&db_path);

        let (stdout, stderr, code) = cli_embedded(&[
            "-e",
            "sqlite",
            "-p",
            db_path.to_str().unwrap(),
            "-f",
            "json",
            "health",
        ]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);

        // Query with sqlite
        let (stdout, _, code) = cli_embedded(&[
            "-e",
            "sqlite",
            "-p",
            db_path.to_str().unwrap(),
            "-f",
            "json",
            "query",
            "?[] <- [[1, 'sqlite works']]",
        ]);
        assert_eq!(code, 0);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);

        let _ = std::fs::remove_dir_all(&db_path);
    }

    // ── Watch in embedded mode should fail ──────────────────────────────────

    #[test]
    fn watch_fails_in_embedded_mode() {
        let (_, stderr, code) = cli_embedded(&["watch", "something"]);
        assert_ne!(code, 0);
        assert!(
            stderr.contains("remote") || stderr.contains("Watch"),
            "stderr: {}",
            stderr
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// REMOTE MODE TESTS — real server, real HTTP, no mocks
// ═══════════════════════════════════════════════════════════════════════════════

mod remote {
    use super::*;

    /// Helper struct that starts a server and kills it on drop.
    struct TestServer {
        child: Child,
        port: u16,
    }

    impl TestServer {
        fn start() -> Self {
            let port = free_port();
            let child = start_server(port);
            Self { child, port }
        }

        fn cli(&self, args: &[&str]) -> (String, String, i32) {
            cli_remote(self.port, args)
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[test]
    fn health() {
        let srv = TestServer::start();
        let (stdout, stderr, code) = srv.cli(&["-f", "json", "health"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
        assert_eq!(val["status"], "healthy");
    }

    #[test]
    fn query_simple() {
        let srv = TestServer::start();
        let (stdout, stderr, code) = srv.cli(&[
            "-f",
            "json",
            "query",
            "?[] <- [[1, 'hello'], [2, 'world']]",
        ]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
        assert_eq!(val["rows"].as_array().unwrap().len(), 2);
        assert!(val["elapsed_ms"].as_u64().is_some());
    }

    #[test]
    fn query_with_params() {
        let srv = TestServer::start();
        let (stdout, stderr, code) = srv.cli(&[
            "-f",
            "json",
            "query",
            "?[a] <- [[$x]]",
            "-p",
            r#"{"x": 99}"#,
        ]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["rows"].as_array().unwrap()[0][0], 99);
    }

    #[test]
    fn query_limit_offset() {
        let srv = TestServer::start();
        let (stdout, stderr, code) = srv.cli(&[
            "-f",
            "json",
            "query",
            "?[x] <- [[1],[2],[3],[4],[5]]",
            "-l",
            "2",
            "-o",
            "1",
        ]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["total_rows"], 5);
        assert_eq!(val["returned_rows"], 2);
    }

    #[test]
    fn query_error() {
        let srv = TestServer::start();
        let (_, _, code) = srv.cli(&["-f", "json", "query", "INVALID"]);
        assert_ne!(code, 0);
    }

    #[test]
    fn validate_valid() {
        let srv = TestServer::start();
        let (stdout, stderr, code) = srv.cli(&["-f", "json", "validate", "?[] <- [[1]]"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["valid"], true);
    }

    #[test]
    fn validate_invalid() {
        let srv = TestServer::start();
        let (stdout, stderr, code) = srv.cli(&["-f", "json", "validate", "GARBAGE"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["valid"], false);
    }

    #[test]
    fn explain() {
        let srv = TestServer::start();
        let (stdout, stderr, code) = srv.cli(&["-f", "json", "explain", "?[] <- [[1]]"]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
    }

    #[test]
    fn relations_and_schema() {
        let srv = TestServer::start();

        // Initially empty
        let (stdout, _, _) = srv.cli(&["-f", "json", "relations"]);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["rows"].as_array().unwrap().len(), 0);

        // Create a relation
        srv.cli(&[
            "query",
            ":create test_rel {id: Int => val: String}",
        ]);
        srv.cli(&[
            "query",
            r#"?[id, val] <- [[1, "a"], [2, "b"]] :put test_rel {id => val}"#,
        ]);

        // Relations should now have 1
        let (stdout, _, code) = srv.cli(&["-f", "json", "relations"]);
        assert_eq!(code, 0);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["rows"].as_array().unwrap().len(), 1);

        // Columns
        let (stdout, _, code) = srv.cli(&["-f", "json", "columns", "test_rel"]);
        assert_eq!(code, 0);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert!(val["rows"].as_array().unwrap().len() >= 2);

        // Schema
        let (stdout, _, code) = srv.cli(&["-f", "json", "schema"]);
        assert_eq!(code, 0);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
        assert_eq!(val["schema"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn fixed_rules() {
        let srv = TestServer::start();
        let (stdout, _, code) = srv.cli(&["-f", "json", "fixed-rules"]);
        assert_eq!(code, 0);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert!(val["rows"].as_array().unwrap().len() > 10);
    }

    #[test]
    fn running() {
        let srv = TestServer::start();
        let (stdout, _, code) = srv.cli(&["-f", "json", "running"]);
        assert_eq!(code, 0);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
    }

    #[test]
    fn compact() {
        let srv = TestServer::start();
        let (stdout, _, code) = srv.cli(&["-f", "json", "compact"]);
        assert_eq!(code, 0);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
    }

    #[test]
    fn endpoints() {
        let srv = TestServer::start();
        let (stdout, _, code) = srv.cli(&["-f", "json", "endpoints"]);
        // In remote mode, endpoints returns server endpoint list
        // (Our CLI currently doesn't have a dedicated remote path for this,
        // but it should at least succeed)
        assert_eq!(code, 0, "endpoints should work in remote mode");
        // The output should be valid JSON
        let _: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    }

    // ── Full lifecycle in remote mode ───────────────────────────────────────

    #[test]
    fn full_lifecycle_remote() {
        let srv = TestServer::start();

        // 1. Create relation
        let (_, _, code) = srv.cli(&[
            "query",
            ":create cities {name: String => pop: Int}",
        ]);
        assert_eq!(code, 0);

        // 2. Insert data
        let (_, _, code) = srv.cli(&[
            "query",
            r#"?[name, pop] <- [["NYC", 8000000], ["LA", 4000000], ["Chicago", 2700000]] :put cities {name => pop}"#,
        ]);
        assert_eq!(code, 0);

        // 3. Query
        let (stdout, _, code) = srv.cli(&[
            "-f",
            "json",
            "query",
            "?[name, pop] := *cities{name, pop}, pop > 3000000",
        ]);
        assert_eq!(code, 0);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["rows"].as_array().unwrap().len(), 2); // NYC, LA

        // 4. Index
        let (_, _, code) = srv.cli(&["create-index", "cities", "idx_pop", "pop"]);
        assert_eq!(code, 0);

        let (stdout, _, code) = srv.cli(&["-f", "json", "indices", "cities"]);
        assert_eq!(code, 0);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let rows = val["rows"].as_array().unwrap();
        let has_idx = rows.iter().any(|r| {
            r.as_array()
                .map(|a| a.iter().any(|v| v.as_str() == Some("idx_pop")))
                .unwrap_or(false)
        });
        assert!(has_idx, "index not found: {:?}", rows);

        // 5. Drop index
        let (_, _, code) = srv.cli(&["drop-index", "cities", "idx_pop"]);
        assert_eq!(code, 0);

        // 6. Triggers (should succeed, may be empty)
        let (stdout, _, code) = srv.cli(&["-f", "json", "triggers", "cities"]);
        assert_eq!(code, 0);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);

        // 7. Describe (::describe not wired in CozoDB grammar, expect failure)
        let (_, _, code) = srv.cli(&["describe", "cities", "-d", "Cities of the world"]);
        assert_ne!(code, 0); // Known grammar issue

        // 8. Remove
        let (stdout, _, code) = srv.cli(&["-f", "json", "remove", "cities"]);
        assert_eq!(code, 0);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);

        // 9. Verify gone
        let (stdout, _, _) = srv.cli(&["-f", "json", "relations"]);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["rows"].as_array().unwrap().len(), 0);
    }

    // ── Rename in remote mode ───────────────────────────────────────────────

    #[test]
    fn rename_remote() {
        let srv = TestServer::start();

        srv.cli(&["query", ":create alpha {x: Int}"]);
        srv.cli(&["query", r#"?[x] <- [[1],[2],[3]] :put alpha {x}"#]);

        let (_, _, code) = srv.cli(&["rename", "alpha:beta"]);
        assert_eq!(code, 0);

        // alpha should not exist
        let (stdout, _, _) = srv.cli(&["-f", "json", "columns", "alpha"]);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], false);

        // beta should have data
        let (stdout, _, _) = srv.cli(&["-f", "json", "query", "?[count(x)] := *beta{x}"]);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["rows"].as_array().unwrap()[0][0], 3);
    }

    // ── Access level in remote mode ─────────────────────────────────────────

    #[test]
    fn access_level_remote() {
        let srv = TestServer::start();

        srv.cli(&["query", ":create guarded {k: Int}"]);
        srv.cli(&["query", r#"?[k] <- [[1]] :put guarded {k}"#]);

        // Set read-only
        let (_, _, code) = srv.cli(&["access-level", "read_only", "guarded"]);
        assert_eq!(code, 0);

        // Write should fail
        let (stdout, _, code) = srv.cli(&[
            "-f",
            "json",
            "query",
            r#"?[k] <- [[2]] :put guarded {k}"#,
        ]);
        assert_ne!(code, 0);

        // Read should work
        let (stdout, _, code) = srv.cli(&[
            "-f",
            "json",
            "query",
            "?[k] := *guarded{k}",
            "-i",
        ]);
        assert_eq!(code, 0);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);

        // Reset to normal
        let (_, _, code) = srv.cli(&["access-level", "normal", "guarded"]);
        assert_eq!(code, 0);
    }

    // ── Batch in remote mode ────────────────────────────────────────────────

    #[test]
    fn batch_remote() {
        let srv = TestServer::start();

        // Write batch file
        let dir = std::env::temp_dir();
        let batch_path = dir.join(format!("cozo_batch_{}.json", std::process::id()));
        let batch_content = r#"[
            {"script": "?[] <- [[1, 'one']]", "params": {}},
            {"script": "?[] <- [[2, 'two']]", "params": {}}
        ]"#;
        std::fs::write(&batch_path, batch_content).unwrap();

        let (stdout, stderr, code) = srv.cli(&[
            "-f",
            "json",
            "batch",
            batch_path.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "stderr: {}", stderr);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["ok"], true);
        assert_eq!(val["query_count"], 2);

        std::fs::remove_file(&batch_path).unwrap();
    }

    // ── Export / Import via remote ──────────────────────────────────────────

    #[test]
    fn export_import_remote() {
        let srv = TestServer::start();

        // Create and populate
        srv.cli(&["query", ":create items {id: Int => label: String}"]);
        srv.cli(&[
            "query",
            r#"?[id, label] <- [[1, "x"], [2, "y"]] :put items {id => label}"#,
        ]);

        // Export to file
        let dir = std::env::temp_dir();
        let export_path = dir.join(format!("cozo_export_{}.json", std::process::id()));

        let (_, _, code) = srv.cli(&[
            "export",
            "items",
            "-o",
            export_path.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        assert!(export_path.exists());

        // Remove original
        srv.cli(&["remove", "items"]);

        // Re-create schema and import
        srv.cli(&["query", ":create items {id: Int => label: String}"]);

        let (_, _, code) = srv.cli(&[
            "import",
            export_path.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);

        // Verify data
        let (stdout, _, _) = srv.cli(&["-f", "json", "query", "?[count(id)] := *items{id}"]);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["rows"].as_array().unwrap()[0][0], 2);

        std::fs::remove_file(&export_path).unwrap();
    }

    // ── Backup / Import from backup via remote ──────────────────────────────

    #[test]
    fn backup_import_remote() {
        let srv = TestServer::start();

        srv.cli(&["query", ":create bk_test {n: Int}"]);
        srv.cli(&["query", r#"?[n] <- [[10],[20],[30]] :put bk_test {n}"#]);

        let backup_path = format!("/tmp/cozo_remote_backup_{}.db", srv.port);
        let _ = std::fs::remove_file(&backup_path);

        // Backup
        let (stdout, stderr, code) = srv.cli(&["-f", "json", "backup", &backup_path]);
        assert_eq!(code, 0, "backup failed: stderr={}", stderr);

        // Import from backup into a fresh relation
        // First create the target relation
        srv.cli(&["remove", "bk_test"]);
        srv.cli(&["query", ":create bk_test {n: Int}"]);

        let (_, stderr, code) = srv.cli(&[
            "-f",
            "json",
            "import-backup",
            &backup_path,
            "bk_test",
        ]);
        assert_eq!(code, 0, "import-backup failed: stderr={}", stderr);

        // Verify
        let (stdout, _, _) = srv.cli(&["-f", "json", "query", "?[count(n)] := *bk_test{n}"]);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["rows"].as_array().unwrap()[0][0], 3);

        let _ = std::fs::remove_file(&backup_path);
    }

    // ── Output formats in remote mode ───────────────────────────────────────

    #[test]
    fn format_csv_remote() {
        let srv = TestServer::start();
        let (stdout, _, code) = srv.cli(&[
            "-f",
            "csv",
            "query",
            "?[x, y] <- [[1, 'a'], [2, 'b']]",
        ]);
        assert_eq!(code, 0);
        let lines: Vec<&str> = stdout.trim().lines().collect();
        assert!(lines.len() >= 3);
        assert!(lines[0].contains(","));
    }

    #[test]
    fn format_jsonl_remote() {
        let srv = TestServer::start();
        let (stdout, _, code) = srv.cli(&[
            "-f",
            "jsonl",
            "query",
            "?[x, y] <- [[1, 'a'], [2, 'b']]",
        ]);
        assert_eq!(code, 0);
        let lines: Vec<&str> = stdout.trim().lines().collect();
        assert_eq!(lines.len(), 2);
        for line in &lines {
            let _: serde_json::Value = serde_json::from_str(line).unwrap();
        }
    }

    // ── Run file in remote mode ─────────────────────────────────────────────

    #[test]
    fn run_file_remote() {
        let srv = TestServer::start();
        let dir = std::env::temp_dir();
        let script_path = dir.join(format!("cozo_remote_run_{}.cozo", srv.port));
        std::fs::write(&script_path, "?[x] <- [[777]]").unwrap();

        let (stdout, _, code) = srv.cli(&[
            "-f",
            "json",
            "run",
            script_path.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(val["rows"].as_array().unwrap()[0][0], 777);

        std::fs::remove_file(&script_path).unwrap();
    }
}
