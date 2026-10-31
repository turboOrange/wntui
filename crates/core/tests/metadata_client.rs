use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};
use wntui_core::wire::metadata::{
    MetadataClient, MetadataError, NodeMetadataPatch, NodeMetadataUpdate,
};

const NETWORKS_REPLY: &str = r#"{
    "data": {
        "networks": [
            { "id": 42, "name": "Plant floor" },
            { "id": 7, "name": "Lab" }
        ]
    },
    "result": 1, "type": 1041, "version": 5
}"#;

async fn read_request(stream: &mut WebSocketStream<TcpStream>) -> String {
    let message = stream
        .next()
        .await
        .expect("closed before request")
        .expect("malformed request");
    match message {
        Message::Text(text) => text.to_string(),
        other => panic!("expected text request, got {other:?}"),
    }
}

async fn serve(listener: TcpListener, reply: &'static str) -> String {
    let (socket, _) = listener.accept().await.unwrap();
    let mut stream = accept_async(socket).await.unwrap();
    let request = read_request(&mut stream).await;
    stream
        .send(Message::Text(reply.to_owned().into()))
        .await
        .unwrap();
    let _ = stream.close(None).await;
    request
}

fn url_for(listener: &TcpListener) -> String {
    format!("ws://127.0.0.1:{}/", listener.local_addr().unwrap().port())
}

#[tokio::test]
async fn get_networks_round_trips_named_networks() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = url_for(&listener);

    let server = tokio::spawn(serve(listener, NETWORKS_REPLY));

    let mut client = MetadataClient::connect(&url, "s-1").await.unwrap();
    let networks = client.get_networks().await.unwrap();

    assert_eq!(networks.len(), 2);
    assert_eq!(networks[0].id, 42);
    assert_eq!(networks[0].name, "Plant floor");
    assert_eq!(networks[1].name, "Lab");

    let request: serde_json::Value = serde_json::from_str(&server.await.unwrap()).unwrap();
    assert_eq!(request["session_id"], "s-1");
    assert_eq!(request["type"], 1041);
    assert_eq!(request["version"], 5);
    assert_eq!(request["data"], serde_json::json!({}));
}

#[tokio::test]
async fn denied_requests_surface_typed_errors() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = url_for(&listener);

    let server = tokio::spawn(serve(listener, r#"{ "result": 5 }"#));

    let mut client = MetadataClient::connect(&url, "s-1").await.unwrap();
    let error = client.get_networks().await.unwrap_err();

    assert_eq!(error.to_string(), "access denied");
    assert!(matches!(error, MetadataError::AccessDenied));

    server.abort();
}

#[tokio::test]
async fn closed_socket_reports_transport_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = url_for(&listener);

    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut stream = accept_async(socket).await.unwrap();
        let _ = read_request(&mut stream).await;
        let _ = stream.close(None).await;
    });

    let mut client = MetadataClient::connect(&url, "s-1").await.unwrap();
    let error = client.get_networks().await.unwrap_err();

    assert!(matches!(error, MetadataError::UnexpectedResponse(_)));
}

#[tokio::test]
async fn set_node_metadata_round_trips_the_1061_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = url_for(&listener);

    let server = tokio::spawn(serve(listener, r#"{ "result": 1 }"#));

    let mut client = MetadataClient::connect(&url, "s-1").await.unwrap();
    let update = NodeMetadataUpdate::new(42, 3)
        .with(NodeMetadataPatch::Name("Line 3 sensor".to_owned()))
        .with(NodeMetadataPatch::Description(String::new()));

    client
        .set_node_metadata(&[update], "wntui-tok-1")
        .await
        .unwrap();

    let request: serde_json::Value = serde_json::from_str(&server.await.unwrap()).unwrap();
    assert_eq!(request["session_id"], "s-1");
    assert_eq!(request["type"], 1061);
    assert_eq!(request["version"], 5);
    assert_eq!(request["data"]["originator_token"], "wntui-tok-1");
    assert_eq!(request["data"]["nodes"][0]["id"], 3);
    assert_eq!(request["data"]["nodes"][0]["network_id"], 42);
    assert_eq!(request["data"]["nodes"][0]["name"], "Line 3 sensor");
    assert_eq!(request["data"]["nodes"][0]["description"], "");
}

#[tokio::test]
async fn set_node_metadata_denied_surfaces_the_typed_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = url_for(&listener);

    let server = tokio::spawn(serve(listener, r#"{ "result": 5 }"#));

    let mut client = MetadataClient::connect(&url, "s-1").await.unwrap();
    let update = NodeMetadataUpdate::new(42, 3).with(NodeMetadataPatch::Name("x".to_owned()));

    let error = client.set_node_metadata(&[update], "t").await.unwrap_err();
    assert!(matches!(error, MetadataError::AccessDenied));

    server.abort();
}
