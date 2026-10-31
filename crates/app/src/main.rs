use std::process::ExitCode;

use clap::Parser;

mod cli;
mod tui;

use cli::{Cli, Command};

fn main() -> ExitCode {
    let mut cli = Cli::parse();

    let result = match cli.command.take() {
        None if cli.demo => tui::run_demo(),
        None => tui::run(cli.profile.as_deref(), cli.config.as_deref()),
        Some(command) => run_command(command, &cli),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run_command(command: Command, cli: &Cli) -> anyhow::Result<()> {
    match command {
        Command::Auth { action } => cli::run_auth(cli, action),
        Command::Networks => cli::run_networks(cli),
        Command::Nodes { network, filter } => cli::run_nodes(cli, network, filter),
        Command::Node { address, network } => cli::run_node(cli, address, network),
        Command::Graph {
            metric,
            address,
            network,
            range,
        } => cli::run_graph(cli, metric, address, network, range),
        Command::Probe => cli::run_probe(cli),
    }
}
