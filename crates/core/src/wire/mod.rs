use std::time::Duration;

use futures_util::{Sink, SinkExt};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

pub mod auth;
pub mod influx;
pub mod metadata;
pub mod proto;
pub mod realtime;

pub(crate) const PING_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) fn spawn_keepalive_pinger<S>(mut writer: S) -> JoinHandle<()>
where
    S: Sink<Message> + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(PING_INTERVAL).await;
            if writer.send(Message::Ping(Vec::new().into())).await.is_err() {
                break;
            }
        }
    })
}
