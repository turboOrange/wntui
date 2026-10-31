use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
    time::timeout,
};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};
use wntui_core::{
    session::{Credentials, RetryPolicy, SessionEvent, UserRole, spawn_session_supervisor},
    wire::auth::auth_url,
};

const OK_LOGIN: &str = r#"{
    "data": { "role": 1, "session_id": "s-1", "influx_token": "tok-1" },
    "result": 1, "type": 1, "version": 5
}"#;

const REJECTED_LOGIN: &str = r#"{ "result": 3 }"#;

fn test_credentials() -> Credentials {
    Credentials {
        username: "admin".to_owned(),
        password: "secret".to_owned(),
    }
}

fn fast_retry() -> RetryPolicy {
    RetryPolicy {
        initial: Duration::from_millis(10),
        max: Duration::from_millis(50),
        factor: 2.0,
    }
}

async fn next_event(events: &mut mpsc::Receiver<SessionEvent>) -> SessionEvent {
    timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timed out waiting for a session event")
        .expect("session supervisor stopped sending events")
}

async fn reply_to_login(stream: &mut WebSocketStream<TcpStream>, reply: &str) -> String {
    let message = stream
        .next()
        .await
        .expect("connection closed before login request")
        .expect("malformed login request");

    let login = match message {
        Message::Text(text) => text,
        other => panic!("expected text login request, got {other:?}"),
    };

    stream
        .send(Message::Text(reply.to_owned().into()))
        .await
        .expect("cannot send login reply");
    login.to_string()
}

#[tokio::test]
async fn login_handshake_yields_an_authenticated_session() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = auth_url("127.0.0.1", listener.local_addr().unwrap().port(), false);

    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut stream = accept_async(socket).await.unwrap();
        let login = reply_to_login(&mut stream, OK_LOGIN).await;
        stream.close(None).await.unwrap();
        login
    });

    let (events, mut receiver) = mpsc::channel(16);
    let supervisor = spawn_session_supervisor(url, test_credentials(), fast_retry(), events);

    match next_event(&mut receiver).await {
        SessionEvent::Authenticated(session) => {
            assert_eq!(session.role, UserRole::Admin);
            assert_eq!(session.session_id, "s-1");
            assert_eq!(session.influx_token.as_deref(), Some("tok-1"));
        }
        other => panic!("expected authentication, got {other:?}"),
    }

    assert!(matches!(
        next_event(&mut receiver).await,
        SessionEvent::Disconnected { .. }
    ));

    let login: serde_json::Value = serde_json::from_str(&server.await.unwrap()).unwrap();
    assert_eq!(login["data"]["username"], "admin");
    assert_eq!(login["type"], 1);
    assert_eq!(login["version"], 5);

    supervisor.abort();
}

#[tokio::test]
async fn rejected_logins_surface_the_reason_and_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = auth_url("127.0.0.1", listener.local_addr().unwrap().port(), false);

    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (socket, _) = listener.accept().await.unwrap();
            let mut stream = accept_async(socket).await.unwrap();
            reply_to_login(&mut stream, REJECTED_LOGIN).await;
            let _ = stream.close(None).await;
        }
    });

    let (events, mut receiver) = mpsc::channel(16);
    let supervisor = spawn_session_supervisor(url, test_credentials(), fast_retry(), events);

    match next_event(&mut receiver).await {
        SessionEvent::Disconnected { attempt, reason } => {
            assert_eq!(attempt, 0);
            assert_eq!(reason, "invalid credentials");
        }
        other => panic!("expected a rejected login, got {other:?}"),
    }

    match next_event(&mut receiver).await {
        SessionEvent::Disconnected { attempt, reason } => {
            assert_eq!(attempt, 1);
            assert_eq!(reason, "invalid credentials");
        }
        other => panic!("expected a second rejected login, got {other:?}"),
    }

    supervisor.abort();
    server.abort();
}

#[tokio::test]
async fn supervisor_logs_in_again_after_the_server_drops_the_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = auth_url("127.0.0.1", listener.local_addr().unwrap().port(), false);

    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (socket, _) = listener.accept().await.unwrap();
            let mut stream = accept_async(socket).await.unwrap();
            reply_to_login(&mut stream, OK_LOGIN).await;
            let _ = stream.close(None).await;
        }
    });

    let (events, mut receiver) = mpsc::channel(16);
    let supervisor = spawn_session_supervisor(url, test_credentials(), fast_retry(), events);

    for expected_session_id in ["s-1", "s-1"] {
        match next_event(&mut receiver).await {
            SessionEvent::Authenticated(session) => {
                assert_eq!(session.session_id, expected_session_id);
            }
            other => panic!("expected authentication, got {other:?}"),
        }
        assert!(matches!(
            next_event(&mut receiver).await,
            SessionEvent::Disconnected { .. }
        ));
    }

    supervisor.abort();
    server.abort();
}
