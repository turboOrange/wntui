use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default, Serialize, Deserialize,
)]
pub struct NodeKey {
    pub network_id: u32,
    pub address: u32,
}

impl NodeKey {
    pub const fn new(network_id: u32, address: u32) -> Self {
        Self {
            network_id,
            address,
        }
    }
}

#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum OnlineStatus {
    #[default]
    Offline,
    Uncertain,
    Online,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Sink,
    Headnode,
    Subnode,
    #[default]
    Unknown,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Network {
    pub id: u32,
    pub name: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct FirmwareVersion {
    pub major: u32,
    pub minor: u32,
    pub maintenance: u32,
    pub devel: u32,
}

impl fmt::Display for FirmwareVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}.{}.{}.{}",
            self.major, self.minor, self.maintenance, self.devel
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PositioningRole {
    #[default]
    Unknown,
    Anchor,
    Tag,
}

#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct TraveltimeKpi {
    pub qos0_minimum_ms: f32,
    pub qos0_average_ms: f32,
    pub qos0_maximum_ms: f32,
    pub qos1_minimum_ms: f32,
    pub qos1_average_ms: f32,
    pub qos1_maximum_ms: f32,
    pub window_s: i32,
}

#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct NeighborSnapshot {
    pub address: u32,
    pub rssi_dbm: f32,
    pub is_next_hop: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct AppConfigState {
    pub diagnostics_interval_s: u32,
    pub sequence: u32,
    pub is_override_on: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub key: NodeKey,
    pub online: OnlineStatus,
    pub online_updated_s: Option<u32>,
    pub previous_online: Option<OnlineStatus>,
    pub is_sink_online_in_gateway: Option<bool>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub approved: Option<bool>,
    pub is_virtual: Option<bool>,
    pub positioning_role: Option<PositioningRole>,
    pub rssi_offset: Option<i32>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub altitude: Option<f64>,
    pub role: Role,
    pub cb_mac: Option<bool>,
    pub is_autorole: Option<bool>,
    pub sink_address: Option<u32>,
    pub voltage: Option<f32>,
    pub firmware: Option<FirmwareVersion>,
    pub hardware: Option<u32>,
    pub stack_profile: Option<u32>,
    pub boot_count: Option<u32>,
    pub boot_reason: Option<u32>,
    pub otap_enabled: Option<bool>,
    pub scratchpad_sequence: Option<u32>,
    pub gateway_id: Option<String>,
    pub last_seen_ms: Option<u64>,
    pub traveltime_kpi: Option<TraveltimeKpi>,
    pub neighbors: Vec<NeighborSnapshot>,
    pub app_config: Option<AppConfigState>,
}

impl Node {
    pub fn new(key: NodeKey) -> Self {
        Self {
            key,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_online_status_defaults_to_offline() {
        assert_eq!(OnlineStatus::default(), OnlineStatus::Offline);
    }

    #[test]
    fn node_keys_order_by_network_then_address() {
        let mut keys = [NodeKey::new(2, 1), NodeKey::new(1, 9), NodeKey::new(1, 3)];
        keys.sort();
        assert_eq!(
            keys,
            [NodeKey::new(1, 3), NodeKey::new(1, 9), NodeKey::new(2, 1)]
        );
    }

    #[test]
    fn firmware_version_renders_dotted() {
        let version = FirmwareVersion {
            major: 5,
            minor: 2,
            maintenance: 1,
            devel: 0,
        };
        assert_eq!(version.to_string(), "5.2.1.0");
    }
}
