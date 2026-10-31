use std::collections::{BTreeSet, HashMap};

use crate::{
    model::{
        AppConfigState, FirmwareVersion, NeighborSnapshot, Network, Node, NodeKey, OnlineStatus,
        PositioningRole, Role, TraveltimeKpi,
    },
    wire::proto::wnt,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Loading {
    pub expected_nodes: u64,
    pub seen_nodes: u64,
}

#[derive(Default)]
struct SnapshotProgress {
    cluster_size: Option<u32>,
    seen_cells: BTreeSet<u32>,
    expected_node_count: u64,
}

#[derive(Default)]
pub struct NodeStore {
    nodes: HashMap<NodeKey, Node>,
    snapshot: SnapshotProgress,
    // Bumped on every mutation so cached projections can detect staleness
    // cheaply (compare a remembered version instead of rescanning).
    version: u64,
}

impl NodeStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    pub fn node(&self, key: &NodeKey) -> Option<&Node> {
        self.nodes.get(key)
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    // The version bumps on the borrow, not the write: a caller that only
    // reads through the reference forces a spurious recompute, which is
    // safe (cache invalidation may over-fire, never under-fire).
    pub fn node_mut(&mut self, key: &NodeKey) -> Option<&mut Node> {
        self.version = self.version.wrapping_add(1);
        self.nodes.get_mut(key)
    }

    pub fn upsert(&mut self, node: Node) {
        self.version = self.version.wrapping_add(1);
        self.nodes.insert(node.key, node);
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn network_ids(&self) -> Vec<u32> {
        self.nodes
            .keys()
            .map(|key| key.network_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn networks(&self, names: &[Network]) -> Vec<Network> {
        let named: HashMap<u32, &str> = names
            .iter()
            .map(|network| (network.id, network.name.as_str()))
            .collect();

        self.network_ids()
            .into_iter()
            .map(|id| Network {
                id,
                name: named.get(&id).copied().unwrap_or("").to_owned(),
            })
            .collect()
    }

    pub fn loading(&self) -> Option<Loading> {
        let all_cells_seen = self
            .snapshot
            .cluster_size
            .is_some_and(|size| self.snapshot.seen_cells.len() as u32 >= size);
        let all_nodes_seen = self.nodes.len() as u64 >= self.snapshot.expected_node_count;

        (!all_cells_seen || !all_nodes_seen).then_some(Loading {
            expected_nodes: self.snapshot.expected_node_count,
            seen_nodes: self.nodes.len() as u64,
        })
    }

    // Marks the snapshot complete for sources that populate the store by
    // means other than cluster adverts (fixtures, bulk imports).
    pub fn mark_snapshot_complete(&mut self) {
        self.snapshot.cluster_size = Some(0);
        self.snapshot.seen_cells.clear();
        self.snapshot.expected_node_count = self.nodes.len() as u64;
        self.version += 1;
    }

    pub fn apply(&mut self, message: &wnt::Message) {
        if let Some(advert) = &message.rtsituation_metadata {
            self.record_cluster_advert(advert);
        }

        let Some(key) = node_key(message) else {
            return;
        };

        if message
            .node_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.is_deleted.unwrap_or(false))
        {
            self.version = self.version.wrapping_add(1);
            self.nodes.remove(&key);
            return;
        }

        self.version = self.version.wrapping_add(1);
        let node = self.nodes.entry(key).or_insert_with(|| Node::new(key));

        if let Some(diagnostics) = &message.diagnostics {
            apply_diagnostics(node, diagnostics);
        }
        if let Some(status) = &message.online_status {
            apply_online_status(node, status);
        }
        if let Some(metadata) = &message.node_metadata {
            apply_metadata(node, metadata);
        }
        if let Some(app_config) = &message.app_config {
            apply_app_config(node, app_config);
        }
        if let Some(kpi) = &message.traveltime_kpi {
            apply_traveltime(node, kpi);
        }
        if let Some(rx_time) = message.rx_time.or(message.tx_time) {
            node.last_seen_ms = Some(rx_time);
        }
        if let Some(gateway) = &message.gateway_id {
            node.gateway_id = Some(gateway.clone());
        }
    }

    pub fn apply_all(&mut self, messages: &[wnt::Message]) {
        messages.iter().for_each(|message| self.apply(message));
    }

    fn record_cluster_advert(&mut self, advert: &wnt::RtSituationMetadata) {
        if self.snapshot.cluster_size.is_none() {
            self.snapshot.cluster_size = advert.cluster_size;
        }
        if self
            .snapshot
            .seen_cells
            .insert(advert.cluster_no.unwrap_or(0))
        {
            self.snapshot.expected_node_count += advert.node_count.unwrap_or(0) as u64;
        }
    }
}

fn node_key(message: &wnt::Message) -> Option<NodeKey> {
    Some(NodeKey::new(message.network_id?, message.source_address?))
}

fn online_from_proto(status: Option<i32>) -> OnlineStatus {
    use wnt::online_status::Status;

    match status {
        value if value == Some(Status::Online as i32) => OnlineStatus::Online,
        value if value == Some(Status::Uncertain as i32) => OnlineStatus::Uncertain,
        _ => OnlineStatus::Offline,
    }
}

fn role_from_proto(role: Option<i32>) -> Role {
    match role {
        value if value == Some(wnt::BaseRole::Sink as i32) => Role::Sink,
        value if value == Some(wnt::BaseRole::Headnode as i32) => Role::Headnode,
        value if value == Some(wnt::BaseRole::Subnode as i32) => Role::Subnode,
        _ => Role::Unknown,
    }
}

fn apply_diagnostics(node: &mut Node, diagnostics: &wnt::DiagnosticsData) {
    node.role = role_from_proto(diagnostics.role);
    node.voltage = diagnostics.voltage;
    node.boot_count = diagnostics.boot_count;
    node.hardware = diagnostics.hw_magic.map(|magic| magic as u32);
    node.cb_mac = diagnostics.cb_mac;
    node.is_autorole = diagnostics.is_autorole;
    node.stack_profile = diagnostics.stack_profile.map(|profile| profile as u32);
    node.boot_reason = diagnostics.boot_reason.map(|reason| reason as u32);
    node.otap_enabled = diagnostics.otap_enabled;
    node.scratchpad_sequence = diagnostics.scratchpad_sequence;
    node.sink_address = diagnostics.sink_address;

    node.neighbors = diagnostics
        .neighbors
        .iter()
        .take(16)
        .map(|neighbor| NeighborSnapshot {
            address: neighbor.address.unwrap_or(0),
            rssi_dbm: neighbor.rssi_d_bm.unwrap_or(0.0),
            is_next_hop: neighbor.neighbor_type
                == Some(wnt::neighbor::NeighborType::AssociatedCluster as i32),
        })
        .collect();

    node.firmware = diagnostics
        .sw_version_major
        .or(diagnostics.sw_version_minor)
        .or(diagnostics.sw_version_maintenance)
        .or(diagnostics.sw_version_devel)
        .map(|_| FirmwareVersion {
            major: diagnostics.sw_version_major.unwrap_or(0),
            minor: diagnostics.sw_version_minor.unwrap_or(0),
            maintenance: diagnostics.sw_version_maintenance.unwrap_or(0),
            devel: diagnostics.sw_version_devel.unwrap_or(0),
        });
}

fn apply_online_status(node: &mut Node, status: &wnt::OnlineStatus) {
    node.online = online_from_proto(status.status);
    node.online_updated_s = status.update_time;
    node.previous_online = status
        .previous_status
        .map(|previous| online_from_proto(Some(previous)));
    node.is_sink_online_in_gateway = status.is_sink_online_in_gateway;
}

fn apply_metadata(node: &mut Node, metadata: &wnt::NodeMetadata) {
    if let Some(name) = &metadata.name {
        node.name = Some(name.clone());
    }
    if let Some(description) = &metadata.description {
        node.description = Some(description.clone());
    }
    if let Some(approved) = metadata.is_approved {
        node.approved = Some(approved);
    }
    if let Some(virtual_node) = metadata.is_virtual {
        node.is_virtual = Some(virtual_node);
    }
    if let Some(role) = metadata.positioning_role {
        node.positioning_role = Some(match role {
            value if value == wnt::node_metadata::PositioningRole::Anchor as i32 => {
                PositioningRole::Anchor
            }
            value if value == wnt::node_metadata::PositioningRole::Tag as i32 => {
                PositioningRole::Tag
            }
            _ => PositioningRole::Unknown,
        });
    }
    // 1061 echoes carry only changed fields (metadata.md "send only
    // changes"); absent fields keep their previous value.
    if let Some(offset) = metadata.rssi_offset {
        node.rssi_offset = Some(offset);
    }
    if let Some(latitude) = metadata.latitude {
        node.latitude = Some(latitude);
    }
    if let Some(longitude) = metadata.longitude {
        node.longitude = Some(longitude);
    }
    if let Some(altitude) = metadata.altitude {
        node.altitude = Some(altitude);
    }
}

fn apply_app_config(node: &mut Node, app_config: &wnt::AppConfigData) {
    node.app_config = Some(AppConfigState {
        diagnostics_interval_s: app_config.interval.unwrap_or(0),
        sequence: app_config.sequence.unwrap_or(0),
        is_override_on: app_config.is_override_on.unwrap_or(false),
    });
}

fn apply_traveltime(node: &mut Node, kpi: &wnt::TraveltimeKpi) {
    node.traveltime_kpi = Some(TraveltimeKpi {
        qos0_minimum_ms: kpi.qos0_minimum.unwrap_or(0.0),
        qos0_average_ms: kpi.qos0_average.unwrap_or(0.0),
        qos0_maximum_ms: kpi.qos0_maximum.unwrap_or(0.0),
        qos1_minimum_ms: kpi.qos1_minimum.unwrap_or(0.0),
        qos1_average_ms: kpi.qos1_average.unwrap_or(0.0),
        qos1_maximum_ms: kpi.qos1_maximum.unwrap_or(0.0),
        window_s: kpi.window_duration.unwrap_or(0),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn node_message(network_id: u32, address: u32) -> wnt::Message {
        wnt::Message {
            network_id: Some(network_id),
            source_address: Some(address),
            ..Default::default()
        }
    }

    fn cluster_advert(cluster_no: u32, cluster_size: u32, node_count: u32) -> wnt::Message {
        wnt::Message {
            rtsituation_metadata: Some(wnt::RtSituationMetadata {
                cluster_no: Some(cluster_no),
                cluster_size: Some(cluster_size),
                node_count: Some(node_count),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn version_bumps_on_every_mutating_path() {
        let mut store = NodeStore::new();
        assert_eq!(store.version(), 0);

        store.apply(&node_message(42, 1));
        let after_insert = store.version();
        assert_eq!(after_insert, 1);

        // A message with no node key does not touch the store.
        store.apply(&wnt::Message::default());
        assert_eq!(store.version(), after_insert);

        // Re-applying the same node still bumps: content may have changed.
        store.apply(&node_message(42, 1));
        assert_eq!(store.version(), after_insert + 1);

        store.upsert(Node::new(NodeKey::new(42, 2)));
        assert_eq!(store.version(), after_insert + 2);

        store.node_mut(&NodeKey::new(42, 2));
        assert_eq!(store.version(), after_insert + 3);

        // A missing key through node_mut still bumps (conservative).
        store.node_mut(&NodeKey::new(9, 9));
        assert_eq!(store.version(), after_insert + 4);

        // Deletion bumps.
        let mut deleted = node_message(42, 2);
        deleted.node_metadata = Some(wnt::NodeMetadata {
            is_deleted: Some(true),
            ..Default::default()
        });
        store.apply(&deleted);
        assert_eq!(store.version(), after_insert + 5);
    }

    #[test]
    fn diagnostics_populate_the_node() {
        let mut store = NodeStore::new();
        let mut message = node_message(42, 7);
        message.diagnostics = Some(wnt::DiagnosticsData {
            voltage: Some(3.2),
            role: Some(wnt::BaseRole::Sink as i32),
            boot_count: Some(5),
            hw_magic: Some(3),
            sw_version_major: Some(5),
            sw_version_minor: Some(2),
            sw_version_maintenance: Some(1),
            sw_version_devel: Some(0),
            ..Default::default()
        });
        message.rx_time = Some(1_700_000_000_000);
        message.gateway_id = Some("gw-1".to_owned());

        store.apply(&message);

        let node = store.node(&NodeKey::new(42, 7)).unwrap();
        assert_eq!(node.voltage, Some(3.2));
        assert_eq!(node.role, Role::Sink);
        assert_eq!(node.boot_count, Some(5));
        assert_eq!(node.hardware, Some(3));
        assert_eq!(node.firmware.unwrap().to_string(), "5.2.1.0");
        assert_eq!(node.last_seen_ms, Some(1_700_000_000_000));
        assert_eq!(node.gateway_id.as_deref(), Some("gw-1"));
    }

    #[test]
    fn online_status_maps_and_missing_status_means_offline() {
        let mut store = NodeStore::new();

        let mut online = node_message(1, 1);
        online.online_status = Some(wnt::OnlineStatus {
            status: Some(wnt::online_status::Status::Online as i32),
            ..Default::default()
        });
        store.apply(&online);
        assert_eq!(
            store.node(&NodeKey::new(1, 1)).unwrap().online,
            OnlineStatus::Online
        );

        let mut without_status = node_message(1, 1);
        without_status.online_status = Some(wnt::OnlineStatus::default());
        store.apply(&without_status);
        assert_eq!(
            store.node(&NodeKey::new(1, 1)).unwrap().online,
            OnlineStatus::Offline
        );
    }

    #[test]
    fn node_metadata_sets_identity_and_delete_removes() {
        let mut store = NodeStore::new();

        let mut named = node_message(42, 9);
        named.node_metadata = Some(wnt::NodeMetadata {
            name: Some("Line 3 sensor".to_owned()),
            description: Some("near the conveyor".to_owned()),
            is_approved: Some(true),
            ..Default::default()
        });
        store.apply(&named);

        let node = store.node(&NodeKey::new(42, 9)).unwrap();
        assert_eq!(node.name.as_deref(), Some("Line 3 sensor"));
        assert_eq!(node.description.as_deref(), Some("near the conveyor"));
        assert_eq!(node.approved, Some(true));

        let mut deleted = node_message(42, 9);
        deleted.node_metadata = Some(wnt::NodeMetadata {
            is_deleted: Some(true),
            ..Default::default()
        });
        store.apply(&deleted);
        assert_eq!(store.node_count(), 0);
    }

    #[test]
    fn messages_without_node_identity_are_ignored() {
        let mut store = NodeStore::new();
        store.apply(&cluster_advert(0, 1, 3));
        store.apply(&wnt::Message::default());
        store.apply(&wnt::Message {
            network_id: Some(42),
            ..Default::default()
        });
        assert_eq!(store.node_count(), 0);
    }

    #[test]
    fn cluster_adverts_drive_loading_progress() {
        let mut store = NodeStore::new();
        store.apply(&cluster_advert(0, 2, 3));
        store.apply(&cluster_advert(1, 2, 2));

        assert_eq!(
            store.loading(),
            Some(Loading {
                expected_nodes: 5,
                seen_nodes: 0
            })
        );

        for address in 0..4 {
            store.apply(&node_message(42, address));
        }
        assert_eq!(
            store.loading(),
            Some(Loading {
                expected_nodes: 5,
                seen_nodes: 4
            })
        );

        store.apply(&node_message(42, 4));
        assert_eq!(store.loading(), None);
    }

    #[test]
    fn duplicate_cluster_adverts_do_not_double_count() {
        let mut store = NodeStore::new();
        store.apply(&cluster_advert(0, 1, 3));
        store.apply(&cluster_advert(0, 1, 3));

        assert_eq!(
            store.loading(),
            Some(Loading {
                expected_nodes: 3,
                seen_nodes: 0
            })
        );
    }

    #[test]
    fn network_listing_merges_names_with_seen_ids() {
        let mut store = NodeStore::new();
        store.apply(&node_message(42, 1));
        store.apply(&node_message(42, 2));
        store.apply(&node_message(7, 1));

        let networks = store.networks(&[Network {
            id: 42,
            name: "Plant floor".to_owned(),
        }]);
        assert_eq!(
            networks,
            vec![
                Network {
                    id: 7,
                    name: String::new()
                },
                Network {
                    id: 42,
                    name: "Plant floor".to_owned()
                },
            ]
        );
    }
}

#[cfg(test)]
mod detail_field_tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn node_message(network_id: u32, address: u32) -> wnt::Message {
        wnt::Message {
            network_id: Some(network_id),
            source_address: Some(address),
            ..Default::default()
        }
    }

    #[test]
    fn sink_app_config_lands_on_the_node() {
        let mut store = NodeStore::new();
        let mut message = node_message(42, 1);
        message.app_config = Some(wnt::AppConfigData {
            interval: Some(60),
            sequence: Some(7),
            is_override_on: Some(true),
            ..Default::default()
        });

        store.apply(&message);

        let app_config = store.node(&NodeKey::new(42, 1)).unwrap().app_config;
        assert_eq!(
            app_config,
            Some(crate::model::AppConfigState {
                diagnostics_interval_s: 60,
                sequence: 7,
                is_override_on: true
            })
        );
    }

    #[test]
    fn traveltime_kpi_maps_all_fields() {
        let mut store = NodeStore::new();
        let mut message = node_message(42, 4);
        message.traveltime_kpi = Some(wnt::TraveltimeKpi {
            qos0_minimum: Some(48.0),
            qos0_average: Some(120.5),
            qos0_maximum: Some(310.0),
            qos1_minimum: Some(61.0),
            qos1_average: Some(155.0),
            qos1_maximum: Some(402.0),
            window_duration: Some(120),
        });

        store.apply(&message);

        let kpi = store.node(&NodeKey::new(42, 4)).unwrap().traveltime_kpi;
        let kpi = kpi.expect("kpi stored");
        assert_eq!(kpi.qos0_average_ms, 120.5);
        assert_eq!(kpi.qos1_maximum_ms, 402.0);
        assert_eq!(kpi.window_s, 120);
    }

    #[test]
    fn neighbors_mark_the_next_hop() {
        let mut store = NodeStore::new();
        let mut message = node_message(42, 2);
        message.diagnostics = Some(wnt::DiagnosticsData {
            neighbors: vec![
                wnt::Neighbor {
                    address: Some(1),
                    rssi_d_bm: Some(-45.0),
                    neighbor_type: Some(wnt::neighbor::NeighborType::AssociatedCluster as i32),
                    ..Default::default()
                },
                wnt::Neighbor {
                    address: Some(3),
                    rssi_d_bm: Some(-61.0),
                    neighbor_type: Some(wnt::neighbor::NeighborType::Member as i32),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        store.apply(&message);

        let neighbors = &store.node(&NodeKey::new(42, 2)).unwrap().neighbors;
        assert_eq!(neighbors.len(), 2);
        assert_eq!(neighbors[0].address, 1);
        assert_eq!(neighbors[0].rssi_dbm, -45.0);
        assert!(neighbors[0].is_next_hop);
        assert!(!neighbors[1].is_next_hop);
    }

    #[test]
    fn positioning_metadata_maps_to_the_model() {
        let mut store = NodeStore::new();
        let mut message = node_message(42, 6);
        message.node_metadata = Some(wnt::NodeMetadata {
            positioning_role: Some(wnt::node_metadata::PositioningRole::Anchor as i32),
            rssi_offset: Some(-10),
            latitude: Some(61.45),
            longitude: Some(23.88),
            altitude: Some(12.0),
            is_virtual: Some(false),
            ..Default::default()
        });

        store.apply(&message);

        let node = store.node(&NodeKey::new(42, 6)).unwrap();
        assert_eq!(
            node.positioning_role,
            Some(crate::model::PositioningRole::Anchor)
        );
        assert_eq!(node.rssi_offset, Some(-10));
        assert_eq!(node.latitude, Some(61.45));
        assert_eq!(node.longitude, Some(23.88));
        assert_eq!(node.altitude, Some(12.0));
        assert_eq!(node.is_virtual, Some(false));
    }

    #[test]
    fn partial_metadata_updates_keep_absent_fields() {
        let mut store = NodeStore::new();

        let mut full = node_message(42, 6);
        full.node_metadata = Some(wnt::NodeMetadata {
            name: Some("Gate west".to_owned()),
            description: Some("by the loading bay".to_owned()),
            rssi_offset: Some(-10),
            latitude: Some(61.45),
            longitude: Some(23.88),
            altitude: Some(12.0),
            ..Default::default()
        });
        store.apply(&full);

        // A 1061 echo renaming the node must not wipe the other fields.
        let mut partial = node_message(42, 6);
        partial.node_metadata = Some(wnt::NodeMetadata {
            name: Some("Gate west 2".to_owned()),
            ..Default::default()
        });
        store.apply(&partial);

        let node = store.node(&NodeKey::new(42, 6)).unwrap();
        assert_eq!(node.name.as_deref(), Some("Gate west 2"));
        assert_eq!(node.description.as_deref(), Some("by the loading bay"));
        assert_eq!(node.rssi_offset, Some(-10));
        assert_eq!(node.latitude, Some(61.45));
        assert_eq!(node.altitude, Some(12.0));
    }

    #[test]
    fn online_status_history_is_kept() {
        let mut store = NodeStore::new();
        let mut message = node_message(42, 7);
        message.online_status = Some(wnt::OnlineStatus {
            status: Some(wnt::online_status::Status::Uncertain as i32),
            update_time: Some(1_700_000_290),
            previous_status: Some(wnt::online_status::Status::Online as i32),
            is_sink_online_in_gateway: Some(true),
            ..Default::default()
        });

        store.apply(&message);

        let node = store.node(&NodeKey::new(42, 7)).unwrap();
        assert_eq!(node.online, OnlineStatus::Uncertain);
        assert_eq!(node.previous_online, Some(OnlineStatus::Online));
        assert_eq!(node.online_updated_s, Some(1_700_000_290));
        assert_eq!(node.is_sink_online_in_gateway, Some(true));
    }
}
