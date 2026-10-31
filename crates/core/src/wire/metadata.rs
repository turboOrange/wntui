use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

use crate::model::Network;

const CALL_TIMEOUT: Duration = Duration::from_secs(10);

pub const GET_NETWORKS: i64 = 1041;
pub const SET_NODE_METADATA: i64 = 1061;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeMetadataPatch {
    // Empty string clears the field; a field not mentioned in `patches`
    // is left unchanged (metadata.md: "send only changes").
    Name(String),
    Description(String),
    Approved(bool),
    Virtual(bool),
    Anchor(bool),
    RssiOffset(i32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeMetadataUpdate {
    pub address: u32,
    pub network_id: u32,
    pub patches: Vec<NodeMetadataPatch>,
}

impl NodeMetadataUpdate {
    pub fn new(network_id: u32, address: u32) -> Self {
        Self {
            address,
            network_id,
            patches: Vec::new(),
        }
    }

    pub fn with(mut self, patch: NodeMetadataPatch) -> Self {
        self.patches.push(patch);
        self
    }
}

// metadata.md "Set node metadata (1061)": id + network_id mandatory, other
// fields optional (send only changes).
pub fn set_node_metadata_data(updates: &[NodeMetadataUpdate], originator_token: &str) -> Value {
    let nodes = updates
        .iter()
        .map(|update| {
            let mut node = serde_json::json!({
                "id": update.address,
                "network_id": update.network_id,
            });
            let object = node.as_object_mut().expect("literal is an object");

            for patch in &update.patches {
                match patch {
                    NodeMetadataPatch::Name(name) => {
                        object.insert("name".to_owned(), Value::String(name.clone()));
                    }
                    NodeMetadataPatch::Description(description) => {
                        object.insert("description".to_owned(), Value::String(description.clone()));
                    }
                    NodeMetadataPatch::Approved(approved) => {
                        object.insert("is_approved".to_owned(), Value::Bool(*approved));
                    }
                    NodeMetadataPatch::Virtual(virtual_node) => {
                        object.insert("is_virtual".to_owned(), Value::Bool(*virtual_node));
                    }
                    NodeMetadataPatch::Anchor(anchor) => {
                        object.insert("is_anchor".to_owned(), Value::Bool(*anchor));
                    }
                    NodeMetadataPatch::RssiOffset(offset) => {
                        object.insert("rssi_offset".to_owned(), Value::Number((*offset).into()));
                    }
                }
            }

            node
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "nodes": nodes,
        "originator_token": originator_token,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("wrong protocol version")]
    WrongProtocolVersion,
    #[error("access denied")]
    AccessDenied,
    #[error("invalid session")]
    InvalidSession,
    #[error("invalid id in request")]
    InvalidId,
    #[error("request was invalid")]
    InvalidMessage,
    #[error("metadata service error (result {0})")]
    Result(i64),
    #[error("unexpected response: {0}")]
    UnexpectedResponse(String),
}

fn transport<E: std::error::Error>(error: E) -> MetadataError {
    MetadataError::Transport(error.to_string())
}

#[derive(Serialize)]
struct RequestEnvelope<'a> {
    data: &'a Value,
    session_id: &'a str,
    r#type: i64,
    version: u8,
}

#[derive(Deserialize)]
struct ResponseEnvelope {
    #[serde(default)]
    data: Value,
    result: i64,
}

pub fn parse_response(raw: &str) -> Result<Value, MetadataError> {
    let response: ResponseEnvelope = serde_json::from_str(raw)
        .map_err(|error| MetadataError::UnexpectedResponse(error.to_string()))?;

    match response.result {
        1 => Ok(response.data),
        4 => Err(MetadataError::WrongProtocolVersion),
        5 => Err(MetadataError::AccessDenied),
        6 => Err(MetadataError::InvalidId),
        8 => Err(MetadataError::InvalidMessage),
        9 => Err(MetadataError::InvalidSession),
        code => Err(MetadataError::Result(code)),
    }
}

pub fn networks_from_data(data: &Value) -> Result<Vec<Network>, MetadataError> {
    let networks = data
        .get("networks")
        .ok_or_else(|| MetadataError::UnexpectedResponse("missing networks array".to_owned()))?;

    serde_json::from_value(networks.clone())
        .map_err(|error| MetadataError::UnexpectedResponse(error.to_string()))
}

pub struct MetadataClient {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    session_id: String,
}

impl MetadataClient {
    pub async fn connect(url: &str, session_id: &str) -> Result<Self, MetadataError> {
        let (stream, _) = connect_async(url).await.map_err(transport)?;
        Ok(Self {
            stream,
            session_id: session_id.to_owned(),
        })
    }

    pub async fn call(&mut self, message_type: i64, data: Value) -> Result<Value, MetadataError> {
        let request = serde_json::to_string(&RequestEnvelope {
            data: &data,
            session_id: &self.session_id,
            r#type: message_type,
            version: crate::wire::auth::PROTOCOL_VERSION,
        })
        .expect("request envelope serialization cannot fail");

        let reply = timeout(CALL_TIMEOUT, async {
            self.stream
                .send(Message::Text(request.into()))
                .await
                .map_err(transport)?;

            let message = self
                .stream
                .next()
                .await
                .ok_or_else(|| {
                    MetadataError::UnexpectedResponse("closed before response".to_owned())
                })?
                .map_err(transport)?;

            match message {
                Message::Text(text) => Ok(text.to_string()),
                other => Err(MetadataError::UnexpectedResponse(format!(
                    "expected text response, got {other:?}"
                ))),
            }
        })
        .await
        .map_err(|_| MetadataError::Transport("metadata call timed out".to_owned()))??;

        parse_response(&reply)
    }

    pub async fn get_networks(&mut self) -> Result<Vec<Network>, MetadataError> {
        let data = self.call(GET_NETWORKS, serde_json::json!({})).await?;
        networks_from_data(&data)
    }

    pub async fn set_node_metadata(
        &mut self,
        updates: &[NodeMetadataUpdate],
        originator_token: &str,
    ) -> Result<Value, MetadataError> {
        self.call(
            SET_NODE_METADATA,
            set_node_metadata_data(updates, originator_token),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_networks_from_response_data() {
        let data = serde_json::json!({
            "networks": [
                { "id": 42, "name": "Plant floor" },
                { "id": 7, "name": "Lab" }
            ]
        });
        let networks = networks_from_data(&data).unwrap();
        assert_eq!(networks.len(), 2);
        assert_eq!(networks[0].name, "Plant floor");
    }

    #[test]
    fn maps_result_codes_to_typed_errors() {
        let cases = [
            (4, "wrong protocol version"),
            (5, "access denied"),
            (9, "invalid session"),
        ];
        for (code, expected) in cases {
            let raw = format!(r#"{{ "result": {code} }}"#);
            let error = parse_response(&raw).unwrap_err().to_string();
            assert_eq!(error, expected);
        }
        assert!(matches!(
            parse_response(r#"{ "result": 2 }"#),
            Err(MetadataError::Result(2))
        ));
    }

    #[test]
    fn ok_response_returns_its_data_section() {
        let raw = r#"{ "data": { "networks": [] }, "result": 1 }"#;
        let data = parse_response(raw).unwrap();
        assert!(data.get("networks").is_some());
    }

    #[test]
    fn set_node_metadata_data_sends_only_changes_with_mandatory_ids() {
        let update = NodeMetadataUpdate::new(42, 3)
            .with(NodeMetadataPatch::Name("Line 3 sensor".to_owned()))
            .with(NodeMetadataPatch::Description(String::new()));

        let data = set_node_metadata_data(&[update], "tok-1");
        let node = &data["nodes"][0];

        assert_eq!(node["id"], 3);
        assert_eq!(node["network_id"], 42);
        assert_eq!(node["name"], "Line 3 sensor");
        assert_eq!(node["description"], "");
        assert!(
            node.get("is_approved").is_none(),
            "unchanged fields are absent"
        );
        assert_eq!(data["originator_token"], "tok-1");
    }

    #[test]
    fn set_node_metadata_data_covers_every_patch_kind() {
        let update = NodeMetadataUpdate::new(7, 1)
            .with(NodeMetadataPatch::Approved(true))
            .with(NodeMetadataPatch::Virtual(false))
            .with(NodeMetadataPatch::Anchor(true))
            .with(NodeMetadataPatch::RssiOffset(-12));

        let node = &set_node_metadata_data(&[update], "t")["nodes"][0];
        assert_eq!(node["is_approved"], true);
        assert_eq!(node["is_virtual"], false);
        assert_eq!(node["is_anchor"], true);
        assert_eq!(node["rssi_offset"], -12);
    }

    #[test]
    fn set_node_metadata_data_batches_multiple_nodes() {
        let updates = [
            NodeMetadataUpdate::new(42, 1).with(NodeMetadataPatch::Name("a".to_owned())),
            NodeMetadataUpdate::new(42, 2).with(NodeMetadataPatch::Name("b".to_owned())),
        ];

        let data = set_node_metadata_data(&updates, "t");
        assert_eq!(data["nodes"].as_array().map(Vec::len), Some(2));
    }
}
