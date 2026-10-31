use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use wntui_core::{
    config::{Config, Profile},
    engine::{ConnectionState, Engine, EngineSource, SharedEngineState, spawn_engine},
    model::Node,
    session::{Credentials, RetryPolicy},
    wire::{
        auth::auth_url,
        influx::{GraphMetric, GraphRange, InfluxClient, InfluxCredentials, Series},
    },
};

const SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Parser, Debug)]
#[command(
    name = "wntui",
    version,
    about = "Terminal client for Wirepas Network Tool"
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub profile: Option<String>,

    #[arg(long, global = true)]
    pub host: Option<String>,

    #[arg(long, global = true)]
    pub username: Option<String>,

    #[arg(long, global = true)]
    pub insecure: bool,

    #[arg(long, global = true)]
    pub json: bool,

    #[arg(long, global = true)]
    pub no_color: bool,

    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,

    #[arg(long, global = true)]
    pub demo: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    Networks,
    Nodes {
        #[arg(long)]
        network: Option<u32>,
        #[arg(long)]
        filter: Option<String>,
    },
    Node {
        address: String,
        #[arg(long)]
        network: Option<u32>,
    },
    Graph {
        metric: String,
        address: u32,
        #[arg(long)]
        network: Option<u32>,
        #[arg(long, default_value = "24h")]
        range: String,
    },
    Probe,
}

#[derive(Subcommand, Debug)]
pub enum AuthAction {
    // Prompts for the password, verifies it against the backend, and
    // stores it in the OS keyring on success.
    Login,
    // Removes the stored credential (if any).
    Logout,
    // Reports whether a credential is stored for the resolved profile.
    Status,
}

// The keyring is keyed by profile name, so resolution returns both: the
// name a credential is stored under and the effective profile (flags like
// --host/--username applied).
fn resolve_profile_with_name(cli: &Cli) -> anyhow::Result<(String, Profile)> {
    let path = match &cli.config {
        Some(path) => path.clone(),
        None => wntui_core::config::config_path()
            .map_err(|error| anyhow::anyhow!("cannot determine the config directory: {error}"))?,
    };

    let config = Config::load(&path).with_context(|| {
        format!(
            "cannot load config from {} — create it or run `wntui --demo`",
            path.display()
        )
    })?;

    let name = cli
        .profile
        .clone()
        .or_else(|| config.default_profile.clone())
        .ok_or_else(|| anyhow::anyhow!("no profile given and no default_profile set"))?;

    let mut profile = config.resolve_profile(Some(&name))?.clone();

    if let Some(host) = &cli.host {
        profile.host = host.clone();
    }
    if let Some(username) = &cli.username {
        profile.username = username.clone();
    }
    if cli.insecure {
        profile.tls = false;
    }

    Ok((name, profile))
}

// Password resolution order: WNT_PASSWORD env, then the OS keyring, then
// an interactive prompt. The keyring keeps plaintext out of config files
// and process environments while still allowing unattended runs.
enum PasswordSource {
    Environment,
    Keyring,
    Prompt,
}

fn resolve_credentials(profile_name: &str, profile: &Profile) -> anyhow::Result<Credentials> {
    let password = match std::env::var("WNT_PASSWORD") {
        Ok(password) => password,
        Err(_) => match wntui_core::secrets::load(profile_name, profile) {
            Ok(Some(password)) => password,
            Ok(None) => rpassword::prompt_password(format!(
                "WNT password for {}@{}: ",
                profile.username, profile.host
            ))
            .context("no password given")?,
            Err(error) => {
                // A missing or locked keyring falls back to the prompt
                // instead of blocking the launch.
                eprintln!("warning: keyring unavailable ({error}) — prompting instead");
                rpassword::prompt_password(format!(
                    "WNT password for {}@{}: ",
                    profile.username, profile.host
                ))
                .context("no password given")?
            }
        },
    };

    Ok(Credentials {
        username: profile.username.to_owned(),
        password,
    })
}

fn password_source(profile_name: &str, profile: &Profile) -> PasswordSource {
    if std::env::var_os("WNT_PASSWORD").is_some() {
        return PasswordSource::Environment;
    }
    if matches!(
        wntui_core::secrets::load(profile_name, profile),
        Ok(Some(_))
    ) {
        return PasswordSource::Keyring;
    }
    PasswordSource::Prompt
}

// Logs in once, derives the Influx auth branch from the login response
// (token present -> v2, absent -> v1 basic; time-series.md), and returns
// the client plus the effective profile.
async fn influx_client(cli: &Cli) -> anyhow::Result<(InfluxClient, Profile)> {
    let (profile_name, profile) = resolve_profile_with_name(cli)?;
    let credentials = resolve_credentials(&profile_name, &profile)?;

    let session = wntui_core::wire::auth::login(
        &auth_url(&profile.host, profile.ports.auth, profile.tls),
        &credentials,
    )
    .await
    .map_err(|error| anyhow::anyhow!("login failed: {error}"))?
    .1;

    let influx_credentials = InfluxCredentials::from_session(&session, &credentials);
    let client = InfluxClient::new(
        &profile.host,
        profile.ports.influx,
        profile.tls,
        &influx_credentials,
    );

    Ok((client, profile))
}

// The read commands share one flow: engine up, wait for the RTS snapshot
// to land, then project the store. --demo answers the same path without
// a backend.
async fn synced_engine_state(cli: &Cli) -> anyhow::Result<SharedEngineState> {
    let source = if cli.demo {
        EngineSource::Demo
    } else {
        let (profile_name, profile) = resolve_profile_with_name(cli)?;
        let credentials = resolve_credentials(&profile_name, &profile)?;
        EngineSource::live(&profile, credentials)
    };

    let engine = spawn_engine(source, RetryPolicy::default());

    let synced = tokio::time::timeout(SYNC_TIMEOUT, async {
        loop {
            let connection = {
                let state = engine
                    .state
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.connection()
            };

            if connection == ConnectionState::Live {
                return;
            }

            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;

    if synced.is_err() {
        anyhow::bail!(
            "timed out waiting for the node snapshot — check the backend address and credentials"
        );
    }

    // Destructuring drops the command sender, which shuts the engine loop
    // down (the demo task aborts its fixture stream); the state Arc survives.
    let Engine { state, .. } = engine;
    Ok(state)
}

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| anyhow::anyhow!("cannot start the async runtime: {error}"))
}

pub fn run_auth(cli: &Cli, action: AuthAction) -> anyhow::Result<()> {
    match action {
        AuthAction::Login => {
            let (profile_name, profile) = resolve_profile_with_name(cli)?;
            let credentials = Credentials {
                username: profile.username.clone(),
                password: rpassword::prompt_password(format!(
                    "WNT password for {}@{}: ",
                    profile.username, profile.host
                ))
                .context("no password given")?,
            };

            // Verify before storing: a bad password must not land in the
            // keyring (and the operator learns the backend is reachable).
            let runtime = runtime()?;
            runtime
                .block_on(async {
                    wntui_core::wire::auth::login(
                        &auth_url(&profile.host, profile.ports.auth, profile.tls),
                        &credentials,
                    )
                    .await
                })
                .map_err(|error| anyhow::anyhow!("login failed: {error}"))?;

            wntui_core::secrets::store(&profile_name, &profile, &credentials.password).map_err(
                |error| anyhow::anyhow!("cannot store the password in the keyring: {error}"),
            )?;

            println!(
                "credentials for {}@{} (profile '{profile_name}') stored in the OS keyring",
                profile.username, profile.host
            );
            Ok(())
        }
        AuthAction::Logout => {
            let (profile_name, profile) = resolve_profile_with_name(cli)?;
            match wntui_core::secrets::delete(&profile_name, &profile) {
                Ok(true) => {
                    println!(
                        "removed the stored credentials for {}@{} (profile '{profile_name}')",
                        profile.username, profile.host
                    );
                    Ok(())
                }
                Ok(false) => {
                    println!(
                        "no stored credentials for {}@{} (profile '{profile_name}')",
                        profile.username, profile.host
                    );
                    Ok(())
                }
                Err(error) => Err(anyhow::anyhow!("cannot reach the keyring: {error}")),
            }
        }
        AuthAction::Status => {
            let (profile_name, profile) = resolve_profile_with_name(cli)?;
            let source = match password_source(&profile_name, &profile) {
                PasswordSource::Environment => "WNT_PASSWORD environment variable",
                PasswordSource::Keyring => "OS keyring",
                PasswordSource::Prompt => "interactive prompt (nothing stored)",
            };
            println!(
                "profile '{profile_name}' — {}@{}: password from {source}",
                profile.username, profile.host
            );
            Ok(())
        }
    }
}

pub fn run_probe(cli: &Cli) -> anyhow::Result<()> {
    use anyhow::Context;

    let runtime = runtime()?;
    let series = runtime.block_on(async {
        let (client, _profile) = influx_client(cli).await?;
        client.probe().await.context("influx probe failed")
    })?;

    print_series(&series, cli.json);
    Ok(())
}

pub fn run_graph(
    cli: &Cli,
    metric: String,
    address: u32,
    network: Option<u32>,
    range: String,
) -> anyhow::Result<()> {
    use anyhow::Context;

    let metric = GraphMetric::parse(&metric).with_context(|| {
        format!("unknown metric '{metric}' (voltage, traveltime, traveltime-qos1, online, boots, packets)")
    })?;
    let range = GraphRange::parse(&range)
        .with_context(|| format!("unknown range '{range}' (1h, 6h, 24h, 7d)"))?;

    let runtime = runtime()?;
    let series = runtime.block_on(async {
        let (client, _profile) = influx_client(cli).await?;
        let network_id = network.unwrap_or(0);
        let query = wntui_core::wire::influx::graph_query(metric, network_id, address, range, None);
        client.query(&query).await.context("influx query failed")
    })?;

    print_series(&series, cli.json);
    Ok(())
}

pub fn run_networks(cli: &Cli) -> anyhow::Result<()> {
    let runtime = runtime()?;
    let state = runtime.block_on(synced_engine_state(cli))?;
    let guard = state
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let networks = guard.networks.clone();
    drop(guard);

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&networks).unwrap_or_default()
        );
    } else {
        if networks.is_empty() {
            println!("no networks");
            return Ok(());
        }

        let width = networks
            .iter()
            .map(|network| network.id.to_string().len())
            .max()
            .unwrap_or(0);
        for network in &networks {
            println!(
                "{:>width$}  {}",
                network.id,
                if network.name.is_empty() {
                    "(unnamed)"
                } else {
                    &network.name
                },
                width = width
            );
        }
    }

    Ok(())
}

pub fn run_nodes(cli: &Cli, network: Option<u32>, filter: Option<String>) -> anyhow::Result<()> {
    let filter = match &filter {
        Some(expression) => Some(
            wntui_core::filter::parse(expression)
                .with_context(|| format!("bad filter: {expression}"))?,
        ),
        None => None,
    };

    let runtime = runtime()?;
    let state = runtime.block_on(synced_engine_state(cli))?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);

    let nodes = {
        let guard = state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        project_nodes(&guard, network, filter.as_ref(), now_ms)
    };

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&nodes).unwrap_or_default()
        );
        return Ok(());
    }

    for node in &nodes {
        println!(
            "{:>8}  {:<20}  {:<10}  {:>7}  {}",
            format!("0x{:04x}", node.key.address),
            node.name.clone().unwrap_or_else(|| "-".to_owned()),
            status_word(node.online),
            node.voltage
                .map(|voltage| format!("{voltage:.2}"))
                .unwrap_or_else(|| "-".to_owned()),
            age_of(now_ms, node.last_seen_ms),
        );
    }

    Ok(())
}

pub fn run_node(cli: &Cli, address: String, network: Option<u32>) -> anyhow::Result<()> {
    let address = parse_address(&address)?;

    let runtime = runtime()?;
    let state = runtime.block_on(synced_engine_state(cli))?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);

    let node = {
        let guard = state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let candidates = guard.store.nodes().filter(|node| {
            node.key.address == address && network.is_none_or(|id| node.key.network_id == id)
        });
        candidates.min_by_key(|node| node.key.network_id).cloned()
    };

    let Some(node) = node else {
        anyhow::bail!(
            "no node with address {address}{}",
            network_label_suffix(network)
        );
    };

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&node).unwrap_or_default()
        );
        return Ok(());
    }

    println!("{}", node_detail_text(&node, now_ms));
    Ok(())
}

fn network_label_suffix(network: Option<u32>) -> String {
    match network {
        Some(id) => format!(" in network {id}"),
        None => String::new(),
    }
}

fn parse_address(raw: &str) -> anyhow::Result<u32> {
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16)
            .with_context(|| format!("bad address '{raw}' (decimal or 0x-prefixed hex)"))
    } else {
        raw.parse::<u32>()
            .with_context(|| format!("bad address '{raw}' (decimal or 0x-prefixed hex)"))
    }
}

fn status_word(status: wntui_core::model::OnlineStatus) -> &'static str {
    match status {
        wntui_core::model::OnlineStatus::Online => "online",
        wntui_core::model::OnlineStatus::Uncertain => "uncertain",
        wntui_core::model::OnlineStatus::Offline => "offline",
    }
}

fn age_of(now_ms: u64, last_seen_ms: Option<u64>) -> String {
    let Some(seen) = last_seen_ms else {
        return "-".to_owned();
    };

    let seconds = now_ms.saturating_sub(seen) / 1000;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

fn project_nodes(
    engine: &wntui_core::engine::EngineState,
    network: Option<u32>,
    filter: Option<&wntui_core::filter::Filter>,
    now_ms: u64,
) -> Vec<Node> {
    let mut nodes: Vec<Node> = engine
        .store
        .nodes()
        .filter(|node| network.is_none_or(|id| node.key.network_id == id))
        .filter(|node| filter.is_none_or(|filter| filter.matches(node, now_ms)))
        .cloned()
        .collect();

    nodes.sort_by_key(|node| node.key);
    nodes
}

fn node_detail_text(node: &Node, now_ms: u64) -> String {
    let mut lines = vec![format!(
        "{} ({})  {}",
        node.name.clone().unwrap_or_else(|| "node".to_owned()),
        node.key.network_id,
        node.key.address
    )];

    let mut row = |label: &str, value: String| {
        lines.push(format!("  {label:<16} {value}"));
    };

    row("status", status_word(node.online).to_owned());
    row(
        "voltage",
        node.voltage
            .map(|voltage| format!("{voltage:.2} V"))
            .unwrap_or_else(|| "-".to_owned()),
    );
    row(
        "role",
        match node.role {
            wntui_core::model::Role::Sink => "sink".to_owned(),
            wntui_core::model::Role::Headnode => "headnode".to_owned(),
            wntui_core::model::Role::Subnode => "subnode".to_owned(),
            wntui_core::model::Role::Unknown => "unknown".to_owned(),
        },
    );
    row(
        "firmware",
        node.firmware
            .map(|version| version.to_string())
            .unwrap_or_else(|| "-".to_owned()),
    );
    row("last seen", age_of(now_ms, node.last_seen_ms));
    row(
        "boot count",
        node.boot_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "-".to_owned()),
    );

    lines.join("\n")
}

fn print_series(series: &[Series], json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(series).unwrap_or_default()
        );
        return;
    }

    for one in series {
        println!("measurement: {}", one.name);
        println!("  columns: {}", one.columns.join(", "));
        for point in &one.points {
            let values = point
                .values
                .iter()
                .map(|value| match value {
                    serde_json::Value::Number(number) => number.to_string(),
                    serde_json::Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            println!("  {}  {}", point.timestamp_ns, values);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn bare_invocation_enters_tui_mode() {
        let cli = Cli::try_parse_from(["wntui"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn nodes_with_flags_parse() {
        let cli = Cli::try_parse_from([
            "wntui",
            "nodes",
            "--network",
            "42",
            "--filter",
            "status:offline",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Nodes {
                network: Some(42),
                filter: Some(ref filter),
            }) => assert_eq!(filter, "status:offline"),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn graph_defaults_to_24h() {
        let cli = Cli::try_parse_from(["wntui", "graph", "voltage", "1234"]).unwrap();
        match cli.command {
            Some(Command::Graph {
                metric,
                address,
                range,
                ..
            }) => {
                assert_eq!(metric, "voltage");
                assert_eq!(address, 1234);
                assert_eq!(range, "24h");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn demo_is_global_so_subcommands_take_it_too() {
        let cli = Cli::try_parse_from(["wntui", "nodes", "--demo"]).unwrap();
        assert!(cli.demo);
    }

    #[test]
    fn addresses_parse_as_decimal_or_hex() {
        assert_eq!(parse_address("6688").unwrap(), 6688);
        assert_eq!(parse_address("0x1a20").unwrap(), 0x1a20);
        assert_eq!(parse_address("0X1A20").unwrap(), 0x1a20);
        assert!(parse_address("0xzz").is_err());
        assert!(parse_address("-1").is_err());
    }

    #[test]
    fn projection_filters_by_network_expression_and_sorts_by_key() {
        let mut engine = wntui_core::engine::EngineState::default();
        wntui_core::engine::demo_network()
            .into_iter()
            .for_each(|message| engine.store.apply(&message));

        let all = project_nodes(&engine, None, None, 0);
        assert_eq!(all.len(), 8);
        assert_eq!(all[0].key.address, 1);
        assert_eq!(all[7].key.address, 8);

        let offline = project_nodes(
            &engine,
            None,
            wntui_core::filter::parse("status:offline").ok().as_ref(),
            0,
        );
        assert_eq!(offline.len(), 2);

        let in_network = project_nodes(&engine, Some(42), None, 0);
        assert_eq!(in_network.len(), 8);

        let wrong_network = project_nodes(&engine, Some(7), None, 0);
        assert!(wrong_network.is_empty());
    }

    #[test]
    fn node_detail_renders_key_fields() {
        let mut engine = wntui_core::engine::EngineState::default();
        wntui_core::engine::demo_network()
            .into_iter()
            .for_each(|message| engine.store.apply(&message));
        let node = engine
            .store
            .node(&wntui_core::model::NodeKey::new(42, 1))
            .unwrap()
            .clone();

        let text = node_detail_text(&node, 1_700_000_005_000);
        assert!(text.contains("Sink A (42)  1"));
        assert!(text.contains("status           online"));
        assert!(text.contains("sink"));
        assert!(text.contains("3.31 V"));
        assert!(text.contains("5.2.1.0"));
    }

    #[test]
    fn json_node_round_trips() {
        let mut engine = wntui_core::engine::EngineState::default();
        wntui_core::engine::demo_network()
            .into_iter()
            .for_each(|message| engine.store.apply(&message));
        let node = engine
            .store
            .node(&wntui_core::model::NodeKey::new(42, 4))
            .unwrap()
            .clone();

        let rendered = serde_json::to_value(&node).unwrap();
        assert_eq!(rendered["name"], "Line 3 sensor");
        assert_eq!(rendered["online"], "online");
        assert_eq!(rendered["role"], "subnode");
        assert_eq!(rendered["key"]["network_id"], 42);
        assert_eq!(rendered["key"]["address"], 4);
    }

    #[test]
    fn nodes_run_against_the_demo_backend() {
        let cli = Cli::try_parse_from(["wntui", "nodes", "--demo"]).unwrap();
        run_nodes(&cli, Some(42), None).unwrap();
    }

    #[test]
    fn networks_run_against_the_demo_backend() {
        let cli = Cli::try_parse_from(["wntui", "networks", "--demo"]).unwrap();
        run_networks(&cli).unwrap();
    }

    #[test]
    fn node_run_against_the_demo_backend_picks_the_hex_address() {
        let cli = Cli::try_parse_from(["wntui", "node", "0x4", "--demo"]).unwrap();
        run_node(&cli, "0x4".to_owned(), Some(42)).unwrap();
    }

    #[test]
    fn node_run_reports_missing_addresses() {
        let cli = Cli::try_parse_from(["wntui", "node", "0x99", "--demo"]).unwrap();
        let error = run_node(&cli, "0x99".to_owned(), None).unwrap_err();
        assert!(format!("{error:#}").contains("no node with address 153"));
    }

    #[test]
    fn bad_filters_are_rejected_before_connecting() {
        let cli = Cli::try_parse_from(["wntui", "nodes", "--demo"]).unwrap();
        let error = run_nodes(&cli, None, Some("bogus:<2".to_owned())).unwrap_err();
        assert!(format!("{error:#}").contains("unknown field 'bogus'"));
    }

    #[test]
    fn auth_subcommands_parse() {
        let cli = Cli::try_parse_from(["wntui", "auth", "login"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Auth {
                action: AuthAction::Login
            })
        ));

        let cli = Cli::try_parse_from(["wntui", "auth", "logout", "--profile", "prod"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Auth {
                action: AuthAction::Logout
            })
        ));
        assert_eq!(cli.profile.as_deref(), Some("prod"));

        let cli = Cli::try_parse_from(["wntui", "auth", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Auth {
                action: AuthAction::Status
            })
        ));
    }

    #[test]
    fn missing_keyring_entry_reports_the_prompt_source() {
        let profile = Profile {
            host: "wnt.invalid".to_owned(),
            username: "tester".to_owned(),
            ..Profile::default()
        };

        // A NoEntry load falls through to Prompt; on hosts with no Secret
        // Service the load errors and still reports Prompt. Either way the
        // resolution must not report the keyring.
        let source = password_source("missing-entry-test", &profile);
        assert!(matches!(source, PasswordSource::Prompt));
    }
}
