use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
    time::timeout,
};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};
use wntui_core::{
    session::RetryPolicy,
    wire::{proto::wnt, realtime::spawn_realtime_client},
};

fn fast_retry() -> RetryPolicy {
    RetryPolicy {
        initial: Duration::from_millis(10),
        max: Duration::from_millis(50),
        factor: 2.0,
    }
}

async fn next_event(
    events: &mut mpsc::Receiver<wntui_core::wire::realtime::RealtimeEvent>,
) -> wntui_core::wire::realtime::RealtimeEvent {
    timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timed out waiting for a realtime event")
        .expect("realtime client stopped sending events")
}

async fn read_handshake(stream: &mut WebSocketStream<TcpStream>) -> String {
    let message = stream
        .next()
        .await
        .expect("closed before handshake")
        .expect("malformed handshake");
    match message {
        Message::Text(text) => text.to_string(),
        other => panic!("expected text handshake, got {other:?}"),
    }
}

#[tokio::test]
async fn handshake_streams_decoded_collections_until_close() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let collection = wnt::MessageCollection {
        message_collection: vec![
            wnt::Message {
                network_id: Some(42),
                source_address: Some(7),
                diagnostics: Some(wnt::DiagnosticsData {
                    voltage: Some(3.2),
                    ..Default::default()
                }),
                ..Default::default()
            },
            wnt::Message {
                network_id: Some(42),
                source_address: Some(8),
                ..Default::default()
            },
        ],
    };

    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut stream = accept_async(socket).await.unwrap();

        let handshake = read_handshake(&mut stream).await;
        stream
            .send(Message::Text(r#"{ "result": 1, "version": 5 }"#.into()))
            .await
            .unwrap();
        stream
            .send(Message::Binary(collection.encode_to_vec().into()))
            .await
            .unwrap();
        stream.close(None).await.unwrap();
        handshake
    });

    let (events, mut receiver) = mpsc::channel(16);
    let client = spawn_realtime_client(
        format!("ws://127.0.0.1:{port}/"),
        "s-1".to_owned(),
        fast_retry(),
        events,
    );

    let handshake: serde_json::Value = serde_json::from_str(&server.await.unwrap()).unwrap();
    assert_eq!(handshake["session_id"], "s-1");
    assert_eq!(handshake["version"], 5);

    assert!(matches!(
        next_event(&mut receiver).await,
        wntui_core::wire::realtime::RealtimeEvent::Connected
    ));

    match next_event(&mut receiver).await {
        wntui_core::wire::realtime::RealtimeEvent::Messages(messages) => {
            assert_eq!(messages.len(), 2);
            assert_eq!(messages[0].network_id, Some(42));
            assert_eq!(messages[0].source_address, Some(7));
            assert_eq!(messages[0].diagnostics.as_ref().unwrap().voltage, Some(3.2));
        }
        other => panic!("expected messages, got {other:?}"),
    }

    match next_event(&mut receiver).await {
        wntui_core::wire::realtime::RealtimeEvent::Disconnected { reason } => {
            assert!(reason.contains("closed"), "unexpected reason: {reason}");
        }
        other => panic!("expected disconnect, got {other:?}"),
    }

    client.abort();
}

#[tokio::test]
async fn rejected_handshake_reports_the_result_code() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut stream = accept_async(socket).await.unwrap();
        let _ = read_handshake(&mut stream).await;
        stream
            .send(Message::Text(r#"{ "result": 9, "version": 5 }"#.into()))
            .await
            .unwrap();
        let _ = stream.close(None).await;
    });

    let (events, mut receiver) = mpsc::channel(16);
    let client = spawn_realtime_client(
        format!("ws://127.0.0.1:{port}/"),
        "stale-session".to_owned(),
        fast_retry(),
        events,
    );

    match next_event(&mut receiver).await {
        wntui_core::wire::realtime::RealtimeEvent::Disconnected { reason } => {
            assert!(reason.contains("result 9"), "unexpected reason: {reason}");
        }
        other => panic!("expected disconnect, got {other:?}"),
    }

    client.abort();
    server.abort();
}
