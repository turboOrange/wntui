use std::{fmt, time::Duration};

use futures_util::StreamExt;
use tokio::{net::TcpStream, sync::mpsc, task::JoinHandle, time::sleep};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

use crate::wire::auth;

#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"********")
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UserRole {
    Admin,
    Operator,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SessionInfo {
    pub session_id: String,
    pub role: UserRole,
    pub influx_token: Option<String>,
}

#[derive(Clone, Debug)]
pub enum SessionEvent {
    Authenticated(SessionInfo),
    Disconnected { attempt: u32, reason: String },
}

#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    pub initial: Duration,
    pub max: Duration,
    pub factor: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(30),
            factor: 2.0,
        }
    }
}

pub fn reconnect_delay(policy: &RetryPolicy, attempt: u32) -> Duration {
    let growth = policy.factor.powi(attempt.min(32) as i32);
    policy.initial.mul_f64(growth).min(policy.max)
}

pub fn spawn_session_supervisor(
    url: String,
    credentials: Credentials,
    retry: RetryPolicy,
    events: mpsc::Sender<SessionEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut attempt = 0_u32;

        loop {
            match auth::login(&url, &credentials).await {
                Ok((stream, session)) => {
                    attempt = 0;
                    let _ = events.send(SessionEvent::Authenticated(session)).await;
                    let reason = hold_auth_socket_open(stream).await;
                    let _ = events
                        .send(SessionEvent::Disconnected { attempt, reason })
                        .await;
                }
                Err(error) => {
                    let _ = events
                        .send(SessionEvent::Disconnected {
                            attempt,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }

            sleep(reconnect_delay(&retry, attempt)).await;
            attempt += 1;
        }
    })
}

type AuthSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn hold_auth_socket_open(stream: AuthSocket) -> String {
    let (writer, mut reader) = stream.split();
    let pinger = crate::wire::spawn_keepalive_pinger(writer);

    let reason = match reader.next().await {
        Some(Ok(Message::Close(_))) => "auth socket closed by server".to_owned(),
        Some(Ok(_)) => "unexpected traffic on auth socket".to_owned(),
        Some(Err(error)) => format!("auth socket error: {error}"),
        None => "auth socket closed".to_owned(),
    };

    pinger.abort();
    reason
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_debug_never_exposes_the_password() {
        let credentials = Credentials {
            username: "admin".to_owned(),
            password: "hunter2".to_owned(),
        };
        let rendered = format!("{credentials:?}");
        assert!(rendered.contains("admin"));
        assert!(!rendered.contains("hunter2"));
    }

    #[test]
    fn reconnect_delay_starts_at_initial_and_caps_at_max() {
        let policy = RetryPolicy {
            initial: Duration::from_millis(100),
            max: Duration::from_secs(5),
            factor: 2.0,
        };
        assert_eq!(reconnect_delay(&policy, 0), Duration::from_millis(100));
        assert_eq!(reconnect_delay(&policy, 3), Duration::from_millis(800));
        assert_eq!(reconnect_delay(&policy, 20), Duration::from_secs(5));
    }
}
