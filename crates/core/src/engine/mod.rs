use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    model::{Network, NodeKey},
    session::{Credentials, RetryPolicy, SessionEvent, SessionInfo, spawn_session_supervisor},
    store::NodeStore,
    wire::{
        influx::{self, GraphMetric, GraphRange, InfluxCredentials, Series},
        metadata::{MetadataClient, NodeMetadataPatch, NodeMetadataUpdate},
        realtime::{RealtimeEvent, spawn_realtime_client},
    },
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnectionState {
    Connecting,
    Reconnecting,
    Syncing,
    Live,
}

#[derive(Clone, Debug)]
pub enum EngineCommand {
    SetNodeMetadata(NodeMetadataUpdate),
    FetchGraph {
        key: NodeKey,
        metric: GraphMetric,
        range: GraphRange,
        // Absolute window (start, end) in unix seconds; None = the range
        // ending at the engine's now (N4 pan/zoom).
        window: Option<(i64, i64)>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    MetadataEditAccepted {
        address: u32,
    },
    MetadataEditFailed {
        address: u32,
        reason: String,
    },
    GraphData {
        key: NodeKey,
        metric: GraphMetric,
        range: GraphRange,
        // Echoes the request's window so stale replies are dropped by a
        // (key, metric, range/window) match.
        window: Option<(i64, i64)>,
        series: Vec<Series>,
    },
    GraphFailed {
        key: NodeKey,
        metric: GraphMetric,
        range: GraphRange,
        window: Option<(i64, i64)>,
        reason: String,
    },
}

#[derive(Clone)]
pub struct EngineHandle {
    commands: mpsc::Sender<EngineCommand>,
}

impl EngineHandle {
    // Non-blocking on purpose: the TUI reducer is sync, edits are rare, and a
    // closed/full channel just means the engine is shutting down.
    pub fn send(&self, command: EngineCommand) -> Result<(), EngineCommand> {
        self.commands
            .try_send(command)
            .map_err(|error| error.into_inner())
    }
}

pub struct Engine {
    pub state: SharedEngineState,
    pub handle: EngineHandle,
    pub task: JoinHandle<()>,
    pub events: mpsc::Receiver<EngineEvent>,
}

impl Engine {
    // Splits out the event receiver; the remaining parts stay usable.
    pub fn into_parts(
        self,
    ) -> (
        SharedEngineState,
        EngineHandle,
        JoinHandle<()>,
        mpsc::Receiver<EngineEvent>,
    ) {
        (self.state, self.handle, self.task, self.events)
    }
}

#[derive(Default)]
pub struct EngineState {
    pub session: Option<SessionInfo>,
    pub realtime_connected: bool,
    pub networks: Vec<Network>,
    pub store: NodeStore,
    named_networks: Vec<Network>,
    ever_authenticated: bool,
}

impl EngineState {
    pub fn connection(&self) -> ConnectionState {
        if self.realtime_connected {
            if self.store.loading().is_some() {
                ConnectionState::Syncing
            } else {
                ConnectionState::Live
            }
        } else if self.ever_authenticated {
            ConnectionState::Reconnecting
        } else {
            ConnectionState::Connecting
        }
    }
}

pub type SharedEngineState = Arc<RwLock<EngineState>>;

pub enum EngineSource {
    Live {
        auth_url: String,
        metadata_url: String,
        realtime_url: String,
        influx_url: String,
        credentials: Credentials,
    },
    Demo,
}

impl EngineSource {
    pub fn live(profile: &crate::config::Profile, credentials: Credentials) -> Self {
        let host = profile.host.as_str();
        let tls = profile.tls;
        let influx_scheme = if tls { "https" } else { "http" };

        Self::Live {
            auth_url: crate::wire::auth::auth_url(host, profile.ports.auth, tls),
            metadata_url: crate::wire::auth::auth_url(host, profile.ports.metadata, tls),
            realtime_url: crate::wire::auth::auth_url(host, profile.ports.rts, tls),
            influx_url: format!("{influx_scheme}://{host}:{}", profile.ports.influx),
            credentials,
        }
    }
}

pub fn spawn_engine(source: EngineSource, retry: RetryPolicy) -> Engine {
    let state = Arc::new(RwLock::new(EngineState::default()));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (event_tx, event_rx) = mpsc::channel(64);

    let task = match source {
        EngineSource::Live {
            auth_url,
            metadata_url,
            realtime_url,
            influx_url,
            credentials,
        } => tokio::spawn(run_live(
            auth_url,
            metadata_url,
            realtime_url,
            influx_url,
            credentials,
            retry,
            Arc::clone(&state),
            command_rx,
            event_tx,
        )),
        EngineSource::Demo => tokio::spawn(run_demo(Arc::clone(&state), command_rx, event_tx)),
    };

    Engine {
        state,
        handle: EngineHandle {
            commands: command_tx,
        },
        task,
        events: event_rx,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_live(
    auth_url: String,
    metadata_url: String,
    realtime_url: String,
    influx_url: String,
    credentials: Credentials,
    retry: RetryPolicy,
    state: SharedEngineState,
    mut commands: mpsc::Receiver<EngineCommand>,
    events: mpsc::Sender<EngineEvent>,
) {
    let (session_events, mut session_events_rx) = mpsc::channel(64);
    let _auth = spawn_session_supervisor(auth_url, credentials.clone(), retry, session_events);

    let (realtime_events, mut realtime_events_rx) = mpsc::channel(256);
    let mut realtime_client: Option<JoinHandle<()>> = None;
    // Built (and rebuilt) from the login response: token present -> v2 token
    // auth, absent -> v1 basic with the login credentials (time-series.md).
    let mut influx: Option<(InfluxCredentials, String)> = None;

    loop {
        tokio::select! {
            event = session_events_rx.recv() => match event {
                Some(SessionEvent::Authenticated(info)) => {
                    influx = Some((
                        InfluxCredentials::from_session(&info, &credentials),
                        influx_url.clone(),
                    ));
                    refresh_named_networks(&metadata_url, &info, &state).await;
                    write_state(&state, |state| {
                        state.session = Some(info.clone());
                        state.ever_authenticated = true;
                    });

                    if let Some(client) = realtime_client.take() {
                        client.abort();
                    }
                    realtime_client = Some(spawn_realtime_client(
                        realtime_url.clone(),
                        info.session_id.clone(),
                        retry,
                        realtime_events.clone(),
                    ));
                }
                Some(SessionEvent::Disconnected { .. }) => {
                    if let Some(client) = realtime_client.take() {
                        client.abort();
                    }
                    write_state(&state, |state| {
                        state.session = None;
                        state.realtime_connected = false;
                    });
                }
                None => break,
            },

            event = realtime_events_rx.recv() => match event {
                Some(RealtimeEvent::Connected) => {
                    write_state(&state, |state| state.realtime_connected = true);
                }
                Some(RealtimeEvent::Messages(messages)) => {
                    write_state(&state, |state| {
                        state.store.apply_all(&messages);
                        state.refresh_networks();
                    });
                }
                Some(RealtimeEvent::Disconnected { .. }) => {
                    write_state(&state, |state| state.realtime_connected = false);
                }
                None => {
                    if session_events_rx.is_closed() {
                        break;
                    }
                }
            },

            command = commands.recv() => match command {
                Some(command) => {
                    match command {
                        EngineCommand::SetNodeMetadata(update) => {
                            handle_metadata_command(update, &metadata_url, &state, &events).await;
                        }
                        EngineCommand::FetchGraph {
                            key,
                            metric,
                            range,
                            window,
                        } => {
                            handle_graph_command(
                                key, metric, range, window, &influx, &events,
                            )
                            .await;
                        }
                    }
                }
                None => break,
            },
        }
    }
}

async fn handle_metadata_command(
    update: NodeMetadataUpdate,
    metadata_url: &str,
    state: &SharedEngineState,
    events: &mpsc::Sender<EngineEvent>,
) {
    let session_id = {
        let engine = state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        engine
            .session
            .as_ref()
            .map(|session| session.session_id.clone())
    };

    let Some(session_id) = session_id else {
        let _ = events
            .send(EngineEvent::MetadataEditFailed {
                address: update.address,
                reason: "not connected".to_owned(),
            })
            .await;
        return;
    };

    let token = uuid::Uuid::new_v4().to_string();
    let originator_token = format!("wntui-{token}");

    let result = match MetadataClient::connect(metadata_url, &session_id).await {
        Ok(mut client) => {
            client
                .set_node_metadata(std::slice::from_ref(&update), &originator_token)
                .await
        }
        Err(error) => Err(error),
    };

    let event = match result {
        Ok(_) => EngineEvent::MetadataEditAccepted {
            address: update.address,
        },
        Err(error) => EngineEvent::MetadataEditFailed {
            address: update.address,
            reason: error.to_string(),
        },
    };

    let _ = events.send(event).await;
}

async fn handle_graph_command(
    key: NodeKey,
    metric: GraphMetric,
    range: GraphRange,
    window: Option<(i64, i64)>,
    influx: &Option<(InfluxCredentials, String)>,
    events: &mpsc::Sender<EngineEvent>,
) {
    // Fetches happen on view-open, range change, and pan/zoom beyond the
    // fetched bounds only — never on a timer (docs/api/time-series.md:
    // polling overloads the backend).
    let event = match influx {
        Some((credentials, url)) => {
            let query = influx::graph_query(metric, key.network_id, key.address, range, window);
            match influx::InfluxClient::from_url(url, credentials)
                .query(&query)
                .await
            {
                Ok(series) => EngineEvent::GraphData {
                    key,
                    metric,
                    range,
                    window,
                    series,
                },
                Err(error) => EngineEvent::GraphFailed {
                    key,
                    metric,
                    range,
                    window,
                    reason: error.to_string(),
                },
            }
        }
        None => EngineEvent::GraphFailed {
            key,
            metric,
            range,
            window,
            reason: "not connected".to_owned(),
        },
    };

    let _ = events.send(event).await;
}

async fn refresh_named_networks(url: &str, info: &SessionInfo, state: &SharedEngineState) {
    let named = match MetadataClient::connect(url, &info.session_id).await {
        Ok(mut client) => client.get_networks().await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    write_state(state, |state| state.named_networks = named);
}

async fn run_demo(
    state: SharedEngineState,
    mut commands: mpsc::Receiver<EngineCommand>,
    events: mpsc::Sender<EngineEvent>,
) {
    let fixtures = demo_network();

    let advert = fixtures.first().cloned();

    write_state(&state, |state| {
        state.session = Some(SessionInfo {
            session_id: "demo".to_owned(),
            role: crate::session::UserRole::Admin,
            influx_token: Some("demo-token".to_owned()),
        });
        state.ever_authenticated = true;
        state.realtime_connected = true;
        state.named_networks = vec![Network {
            id: 42,
            name: "Demo plant".to_owned(),
        }];
        if let Some(advert) = &advert {
            state.store.apply(advert);
        }
        state.refresh_networks();
    });

    let demo_task = tokio::spawn({
        let state = Arc::clone(&state);
        async move {
            for chunk in fixtures[1..].chunks(3) {
                tokio::time::sleep(Duration::from_millis(150)).await;
                write_state(&state, |state| {
                    state.store.apply_all(chunk);
                    state.refresh_networks();
                });
            }
        }
    });

    loop {
        let Some(command) = commands.recv().await else {
            demo_task.abort();
            break;
        };

        let event = match command {
            EngineCommand::SetNodeMetadata(update) => {
                // No backend in demo mode: apply locally and confirm, the way
                // the live backend echoes a 1061 through RTS.
                let accepted = write_state(&state, |state| apply_demo_edit(state, &update));
                if accepted {
                    EngineEvent::MetadataEditAccepted {
                        address: update.address,
                    }
                } else {
                    EngineEvent::MetadataEditFailed {
                        address: update.address,
                        reason: "node not found".to_owned(),
                    }
                }
            }
            EngineCommand::FetchGraph {
                key,
                metric,
                range,
                window,
            } => EngineEvent::GraphData {
                key,
                metric,
                range,
                window,
                series: demo_graph_series(key, metric, range, window),
            },
        };

        let _ = events.send(event).await;
    }
}

// Deterministic synthetic history so --demo exercises the graph flow without
// a backend. Points are anchored to the window when one is given (pan/zoom,
// N4), else to the default range grid.
fn demo_graph_series(
    key: NodeKey,
    metric: GraphMetric,
    range: GraphRange,
    window: Option<(i64, i64)>,
) -> Vec<Series> {
    let points = match range {
        GraphRange::Hour1 => 60,
        GraphRange::Hour6 => 72,
        GraphRange::Hour24 => 48,
        GraphRange::Day7 => 28,
    };

    let (name, value_at): (String, Box<dyn Fn(usize) -> f64>) = match metric {
        GraphMetric::Voltage => (
            "analytics_boot".to_owned(),
            Box::new(|i| 3.3 - (i as f64 * 0.004) + (i % 7) as f64 * 0.01),
        ),
        GraphMetric::TraveltimeQos0 => (
            "analytics_traveltime_kpi".to_owned(),
            Box::new(|i| 120.0 + (i as f64 * 1.5).sin() * 30.0),
        ),
        GraphMetric::TraveltimeQos1 => (
            "analytics_traveltime_kpi".to_owned(),
            Box::new(|i| 155.0 + (i as f64 * 1.1).sin() * 25.0),
        ),
        GraphMetric::Online => (
            "analytics_nodestate".to_owned(),
            Box::new(|i| if i % 23 == 0 { 1.0 } else { 2.0 }),
        ),
        GraphMetric::Boots => (
            "analytics_boot".to_owned(),
            Box::new(|i| if i % 12 == 0 { 1.0 } else { 0.0 }),
        ),
        GraphMetric::Packets => (
            "analytics_packet".to_owned(),
            Box::new(|i| 40.0 + (i % 5) as f64 * 3.0 + (key.address % 7) as f64),
        ),
    };

    // Spread the samples evenly across the window when given, else over
    // the fixed demo grid ending at the demo epoch + range.
    let (start_ns, step_ns) = match window {
        Some((start, end)) => (
            start * 1_000_000_000,
            ((end - start).max(1) * 1_000_000_000) / points as i64,
        ),
        None => (1_700_000_000_000_000_000, 60_000_000_000),
    };

    let points = (0..points)
        .map(|i| crate::wire::influx::SeriesPoint {
            timestamp_ns: start_ns + i as i64 * step_ns,
            values: vec![serde_json::json!(value_at(i))],
        })
        .collect();

    vec![Series {
        name,
        columns: vec!["time".to_owned(), "value".to_owned()],
        points,
    }]
}

fn apply_demo_edit(state: &mut EngineState, update: &NodeMetadataUpdate) -> bool {
    let Some(node) = state
        .store
        .node_mut(&NodeKey::new(update.network_id, update.address))
    else {
        return false;
    };

    for patch in &update.patches {
        match patch {
            NodeMetadataPatch::Name(name) => node.name = Some(name.clone()),
            NodeMetadataPatch::Description(description) => {
                node.description = Some(description.clone());
            }
            NodeMetadataPatch::Approved(approved) => node.approved = Some(*approved),
            NodeMetadataPatch::Virtual(virtual_node) => node.is_virtual = Some(*virtual_node),
            NodeMetadataPatch::Anchor(anchor) => {
                node.positioning_role = Some(if *anchor {
                    crate::model::PositioningRole::Anchor
                } else {
                    crate::model::PositioningRole::Tag
                });
            }
            NodeMetadataPatch::RssiOffset(offset) => node.rssi_offset = Some(*offset),
        }
    }

    true
}

impl EngineState {
    fn refresh_networks(&mut self) {
        let named = self.named_networks.clone();
        self.networks = self.store.networks(&named);
    }
}

pub fn demo_network() -> Vec<crate::wire::proto::wnt::Message> {
    use crate::wire::proto::wnt;

    let nodes = [
        (1_u32, "Sink A", wnt::BaseRole::Sink, 2, 3.31, "5.2.1.0"),
        (
            2,
            "Conveyor head 1",
            wnt::BaseRole::Headnode,
            2,
            3.05,
            "5.2.1.0",
        ),
        (
            3,
            "Conveyor head 2",
            wnt::BaseRole::Headnode,
            1,
            2.98,
            "5.2.1.0",
        ),
        (
            4,
            "Line 3 sensor",
            wnt::BaseRole::Subnode,
            2,
            3.12,
            "5.1.3.0",
        ),
        (
            5,
            "Line 7 sensor",
            wnt::BaseRole::Subnode,
            0,
            2.91,
            "4.8.1.0",
        ),
        (6, "Gate west", wnt::BaseRole::Subnode, 2, 3.20, "5.2.1.0"),
        (7, "Gate east", wnt::BaseRole::Subnode, 1, 3.02, "5.2.1.0"),
        (
            8,
            "Cold store tag",
            wnt::BaseRole::Subnode,
            0,
            2.74,
            "5.0.0.1",
        ),
    ];

    let mut messages = vec![wnt::Message {
        rtsituation_metadata: Some(wnt::RtSituationMetadata {
            cluster_no: Some(0),
            cluster_size: Some(1),
            node_count: Some(nodes.len() as u32),
            ..Default::default()
        }),
        ..Default::default()
    }];

    messages.extend(
        nodes
            .iter()
            .map(|(address, name, role, online, voltage, firmware)| {
                let (major, minor, maintenance, devel) = parse_firmware(firmware);
                wnt::Message {
                    network_id: Some(42),
                    source_address: Some(*address),
                    diagnostics: Some(wnt::DiagnosticsData {
                        role: Some(*role as i32),
                        voltage: Some(*voltage),
                        sw_version_major: Some(major),
                        sw_version_minor: Some(minor),
                        sw_version_maintenance: Some(maintenance),
                        sw_version_devel: Some(devel),
                        cb_mac: Some(*address == 1),
                        is_autorole: Some(*address != 1),
                        stack_profile: Some(wnt::StackProfile::Profile24 as i32),
                        boot_count: Some(*address + 3),
                        boot_reason: Some(if *address == 8 {
                            wnt::diagnostics_data::BootReason::WatchdogReset as i32
                        } else {
                            wnt::diagnostics_data::BootReason::NormalPowerOnResetStartup as i32
                        }),
                        otap_enabled: Some(*address % 2 == 0),
                        scratchpad_sequence: Some(11),
                        sink_address: Some(1),
                        neighbors: if *address == 2 {
                            vec![
                                wnt::Neighbor {
                                    address: Some(1),
                                    rssi_d_bm: Some(-45.0),
                                    neighbor_type: Some(
                                        wnt::neighbor::NeighborType::AssociatedCluster as i32,
                                    ),
                                    ..Default::default()
                                },
                                wnt::Neighbor {
                                    address: Some(3),
                                    rssi_d_bm: Some(-61.0),
                                    neighbor_type: Some(wnt::neighbor::NeighborType::Member as i32),
                                    ..Default::default()
                                },
                            ]
                        } else {
                            Vec::new()
                        },
                        ..Default::default()
                    }),
                    online_status: Some(wnt::OnlineStatus {
                        status: Some(*online),
                        update_time: Some(1_700_000_290),
                        previous_status: Some(2),
                        ..Default::default()
                    }),
                    node_metadata: Some(wnt::NodeMetadata {
                        name: Some(name.to_string()),
                        is_approved: Some(true),
                        is_virtual: Some(false),
                        positioning_role: Some(if matches!(*address, 1 | 6 | 7) {
                            wnt::node_metadata::PositioningRole::Anchor as i32
                        } else {
                            wnt::node_metadata::PositioningRole::Tag as i32
                        }),
                        rssi_offset: Some(-10),
                        latitude: Some(61.454_759),
                        longitude: Some(23.885_602),
                        altitude: Some(0.0),
                        ..Default::default()
                    }),
                    app_config: (*address == 1).then(|| wnt::AppConfigData {
                        interval: Some(60),
                        sequence: Some(7),
                        is_override_on: Some(false),
                        ..Default::default()
                    }),
                    traveltime_kpi: matches!(*address, 2 | 4).then(|| wnt::TraveltimeKpi {
                        qos0_minimum: Some(48.0),
                        qos0_average: Some(120.5),
                        qos0_maximum: Some(310.0),
                        qos1_minimum: Some(61.0),
                        qos1_average: Some(155.0),
                        qos1_maximum: Some(402.0),
                        window_duration: Some(120),
                    }),
                    rx_time: Some(1_700_000_000_000),
                    ..Default::default()
                }
            }),
    );

    messages
}

fn parse_firmware(firmware: &str) -> (u32, u32, u32, u32) {
    let parts: Vec<u32> = firmware
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
        parts.get(3).copied().unwrap_or(0),
    )
}

fn write_state<T>(state: &SharedEngineState, write: impl FnOnce(&mut EngineState) -> T) -> T {
    let mut guard = state
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    write(&mut guard)
}
