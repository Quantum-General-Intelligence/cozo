/*
 * Copyright 2022, The Cozo Project Authors.
 *
 * This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
 * If a copy of the MPL was not distributed with this file,
 * You can obtain one at https://mozilla.org/MPL/2.0/.
 */

extern crate core;

use std::process::exit;

use clap::{Parser, Subcommand};
use env_logger::Env;

use crate::cli::{cli_main, CliArgs};
use crate::repl::{repl_main, ReplArgs};
use crate::server::{server_main, ServerArgs};

mod api;
mod backend;
mod cli;
mod client;
mod repl;
mod server;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct AppArgs {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the CozoDB HTTP server
    Server(ServerArgs),
    /// Start an interactive REPL session
    Repl(ReplArgs),
    /// CLI client for interacting with a running CozoDB server
    Cli(CliArgs),
}

fn main() {
    match AppArgs::parse().command {
        Commands::Server(args) => {
            env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(server_main(args))
        }
        Commands::Repl(args) => {
            if let Err(e) = repl_main(args) {
                eprintln!("{e}");
                exit(-1);
            }
        }
        Commands::Cli(args) => {
            if let Err(e) = cli_main(args) {
                eprintln!("{e}");
                exit(1);
            }
        }
    };
}
