use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, task::JoinHandle, time::timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{
    session::RetryPolicy,
    wire::{auth::PROTOCOL_VERSION, proto::wnt, spawn_keepalive_pinger},
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub enum RealtimeEvent {
    Connected,
    Messages(Vec<wnt::Message>),
    Disconnected { reason: String },
}

#[derive(Serialize)]
struct HandshakeRequest<'a> {
    session_id: &'a str,
    version: u8,
}

#[derive(Deserialize)]
struct HandshakeResponse {
    result: i64,
}

pub fn spawn_realtime_client(
    url: String,
    session_id: String,
    retry: RetryPolicy,
    events: mpsc::Sender<RealtimeEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut attempt = 0_u32;

        loop {
            let reason = realtime_session(&url, &session_id, &events).await;
            let _ = events.send(RealtimeEvent::Disconnected { reason }).await;

            tokio::time::sleep(crate::session::reconnect_delay(&retry, attempt)).await;
            attempt += 1;
        }
    })
}

async fn realtime_session(
    url: &str,
    session_id: &str,
    events: &mpsc::Sender<RealtimeEvent>,
) -> String {
    let mut stream = match connect_async(url).await {
        Ok((stream, _)) => stream,
        Err(error) => return format!("connect error: {error}"),
    };

    let handshake = serde_json::to_string(&HandshakeRequest {
        session_id,
        version: PROTOCOL_VERSION,
    })
    .expect("handshake serialization cannot fail");

    let outcome = timeout(HANDSHAKE_TIMEOUT, async {
        stream
            .send(Message::Text(handshake.into()))
            .await
            .map_err(|error| format!("handshake send error: {error}"))?;

        let message = stream
            .next()
            .await
            .ok_or_else(|| "closed before handshake response".to_owned())?
            .map_err(|error| format!("handshake receive error: {error}"))?;

        let text = match message {
            Message::Text(text) => text,
            other => return Err(format!("expected text handshake response, got {other:?}")),
        };

        let response: HandshakeResponse = serde_json::from_str(&text)
            .map_err(|error| format!("malformed handshake response: {error}"))?;

        match response.result {
            1 => Ok(()),
            code => Err(format!("realtime handshake rejected with result {code}")),
        }
    })
    .await;

    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(reason)) => return reason,
        Err(_) => return "handshake timed out".to_owned(),
    }

    let _ = events.send(RealtimeEvent::Connected).await;

    let (writer, mut reader) = stream.split();
    let pinger = spawn_keepalive_pinger(writer);

    // one serialized MessageCollection per binary frame
    let reason = loop {
        match reader.next().await {
            Some(Ok(Message::Binary(bytes))) => {
                match wnt::MessageCollection::decode(bytes.as_ref()) {
                    Ok(collection) => {
                        if events
                            .send(RealtimeEvent::Messages(collection.message_collection))
                            .await
                            .is_err()
                        {
                            break "event channel closed".to_owned();
                        }
                    }
                    Err(error) => break format!("protobuf decode error: {error}"),
                }
            }
            Some(Ok(Message::Close(_))) => break "closed by server".to_owned(),
            Some(Ok(_)) => continue,
            Some(Err(error)) => break format!("socket error: {error}"),
            None => break "socket closed".to_owned(),
        }
    };

    pinger.abort();
    reason
}
