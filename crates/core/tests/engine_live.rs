use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};
use wntui_core::{
    engine::{Engine, EngineCommand, EngineEvent, EngineSource, spawn_engine},
    session::{Credentials, RetryPolicy},
    wire::{
        auth::auth_url,
        metadata::{NodeMetadataPatch, NodeMetadataUpdate},
    },
};

const OK_LOGIN: &str = r#"{
    "data": { "role": 1, "session_id": "s-1" },
    "result": 1, "type": 1, "version": 5
}"#;

fn fast_retry() -> RetryPolicy {
    RetryPolicy {
        initial: Duration::from_millis(10),
        max: Duration::from_millis(50),
        factor: 2.0,
    }
}

type Captured = Arc<Mutex<Option<String>>>;

// The supervisor holds the auth socket open for the process lifetime; the
// fake must do the same or the session is dropped right after login.
async fn serve_auth(listener: TcpListener) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut stream = accept_async(socket).await.unwrap();

    let login = stream.next().await.unwrap().unwrap();
    assert!(matches!(login, Message::Text(_)));
    stream
        .send(Message::Text(OK_LOGIN.to_owned().into()))
        .await
        .unwrap();

    std::future::pending::<()>().await;
}

// Serves any number of metadata connections. 1041 (network refresh) gets an
// empty ok; 1061 gets `result_1061` and the raw request is captured.
async fn serve_metadata(listener: TcpListener, result_1061: &'static str, captured: Captured) {
    loop {
        let (socket, _) = listener.accept().await.unwrap();
        let mut stream = accept_async(socket).await.unwrap();

        while let Some(text) = next_request(&mut stream).await {
            let request: serde_json::Value = serde_json::from_str(&text).unwrap();

            let reply = match request["type"].as_i64() {
                Some(1061) => {
                    captured.lock().unwrap().replace(text);
                    result_1061.to_owned()
                }
                _ => r#"{ "data": { "networks": [] }, "result": 1 }"#.to_owned(),
            };

            stream.send(Message::Text(reply.into())).await.unwrap();
        }
    }
}

async fn next_request(stream: &mut WebSocketStream<TcpStream>) -> Option<String> {
    loop {
        match stream.next().await {
            Some(Ok(Message::Text(text))) => return Some(text.to_string()),
            Some(Ok(_)) => continue,
            _ => return None,
        }
    }
}

async fn await_authenticated(engine: &Engine) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let authenticated = {
                let state = engine
                    .state
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.session.is_some()
            };

            if authenticated {
                return;
            }

            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("engine never authenticated");
}

fn spawn_test_engine(auth_port: u16, metadata_port: u16) -> Engine {
    spawn_engine(
        EngineSource::Live {
            auth_url: auth_url("127.0.0.1", auth_port, false),
            metadata_url: auth_url("127.0.0.1", metadata_port, false),
            realtime_url: "ws://127.0.0.1:1/".to_owned(),
            influx_url: "http://127.0.0.1:1".to_owned(),
            credentials: Credentials {
                username: "admin".to_owned(),
                password: "secret".to_owned(),
            },
        },
        fast_retry(),
    )
}

const OK_1061: &str = r#"{ "data": {}, "result": 1, "type": 1061, "version": 5 }"#;
const DENIED: &str = r#"{ "result": 5 }"#;

#[tokio::test]
async fn live_engine_sends_1061_and_reports_acceptance() {
    let auth_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let metadata_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

    let auth_port = auth_listener.local_addr().unwrap().port();
    let metadata_port = metadata_listener.local_addr().unwrap().port();

    let auth_server = tokio::spawn(serve_auth(auth_listener));
    let captured: Captured = Arc::new(Mutex::new(None));
    let metadata_server = tokio::spawn(serve_metadata(
        metadata_listener,
        OK_1061,
        Arc::clone(&captured),
    ));

    let mut engine = spawn_test_engine(auth_port, metadata_port);
    await_authenticated(&engine).await;

    let update = NodeMetadataUpdate::new(42, 7)
        .with(NodeMetadataPatch::Name("Gate west renamed".to_owned()));

    engine
        .handle
        .send(EngineCommand::SetNodeMetadata(update))
        .expect("engine command channel is open");

    let event = tokio::time::timeout(Duration::from_secs(5), engine.events.recv())
        .await
        .expect("timed out waiting for an engine event")
        .expect("engine event stream closed");

    assert_eq!(event, EngineEvent::MetadataEditAccepted { address: 7 });

    let request = captured
        .lock()
        .unwrap()
        .clone()
        .expect("the 1061 request was captured");
    let request: serde_json::Value = serde_json::from_str(&request).unwrap();
    assert_eq!(request["session_id"], "s-1");
    assert_eq!(request["type"], 1061);
    assert_eq!(request["version"], 5);
    assert_eq!(request["data"]["nodes"][0]["name"], "Gate west renamed");
    assert!(
        request["data"]["originator_token"]
            .as_str()
            .is_some_and(|token| token.starts_with("wntui-"))
    );

    engine.task.abort();
    auth_server.abort();
    metadata_server.abort();
}

#[tokio::test]
async fn live_engine_reports_1061_failure_with_the_reason() {
    let auth_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let metadata_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

    let auth_port = auth_listener.local_addr().unwrap().port();
    let metadata_port = metadata_listener.local_addr().unwrap().port();

    let auth_server = tokio::spawn(serve_auth(auth_listener));
    let metadata_server = tokio::spawn(serve_metadata(
        metadata_listener,
        DENIED,
        Arc::new(Mutex::new(None)),
    ));

    let mut engine = spawn_test_engine(auth_port, metadata_port);
    await_authenticated(&engine).await;

    let update =
        NodeMetadataUpdate::new(42, 7).with(NodeMetadataPatch::Name("Gate west".to_owned()));

    engine
        .handle
        .send(EngineCommand::SetNodeMetadata(update))
        .expect("engine command channel is open");

    let event = tokio::time::timeout(Duration::from_secs(5), engine.events.recv())
        .await
        .expect("timed out waiting for an engine event")
        .expect("engine event stream closed");

    assert_eq!(
        event,
        EngineEvent::MetadataEditFailed {
            address: 7,
            reason: "access denied".to_owned(),
        }
    );

    engine.task.abort();
    auth_server.abort();
    metadata_server.abort();
}
