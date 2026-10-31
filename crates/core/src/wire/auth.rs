use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

use crate::session::{Credentials, SessionInfo, UserRole};

pub const PROTOCOL_VERSION: u8 = 5;

const LOGIN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("wrong protocol version")]
    WrongProtocolVersion,
    #[error("access denied")]
    AccessDenied,
    #[error("auth service rejected login with result code {0}")]
    Rejected(i64),
    #[error("unexpected login response: {0}")]
    UnexpectedResponse(String),
    #[error("transport error: {0}")]
    Transport(String),
}

fn transport<E: std::error::Error>(error: E) -> AuthError {
    AuthError::Transport(error.to_string())
}

pub fn auth_url(host: &str, port: u16, tls: bool) -> String {
    let scheme = if tls { "wss" } else { "ws" };
    format!("{scheme}://{host}:{port}/")
}

#[derive(Serialize)]
struct LoginRequest<'a> {
    data: LoginRequestData<'a>,
    r#type: u8,
    version: u8,
}

#[derive(Serialize)]
struct LoginRequestData<'a> {
    username: &'a str,
    password: &'a str,
}

pub fn login_request(credentials: &Credentials) -> String {
    let request = LoginRequest {
        data: LoginRequestData {
            username: &credentials.username,
            password: &credentials.password,
        },
        r#type: 1,
        version: PROTOCOL_VERSION,
    };
    serde_json::to_string(&request).expect("login request serialization cannot fail")
}

#[derive(Deserialize)]
struct LoginResponse {
    #[serde(default)]
    data: Option<LoginResponseData>,
    result: i64,
}

#[derive(Deserialize)]
struct LoginResponseData {
    role: i64,
    session_id: String,
    #[serde(default)]
    influx_token: Option<String>,
}

impl LoginResponseData {
    fn into_session_info(self) -> Result<SessionInfo, AuthError> {
        let role = match self.role {
            1 => UserRole::Admin,
            2 => UserRole::Operator,
            other => {
                return Err(AuthError::UnexpectedResponse(format!(
                    "unknown role {other}"
                )));
            }
        };
        Ok(SessionInfo {
            session_id: self.session_id,
            role,
            influx_token: self.influx_token,
        })
    }
}

pub fn parse_login_response(raw: &str) -> Result<SessionInfo, AuthError> {
    let response: LoginResponse = serde_json::from_str(raw)
        .map_err(|error| AuthError::UnexpectedResponse(error.to_string()))?;

    match response.result {
        1 => response
            .data
            .ok_or_else(|| AuthError::UnexpectedResponse("missing data".to_owned()))
            .and_then(LoginResponseData::into_session_info),
        3 => Err(AuthError::InvalidCredentials),
        4 => Err(AuthError::WrongProtocolVersion),
        5 => Err(AuthError::AccessDenied),
        code => Err(AuthError::Rejected(code)),
    }
}

pub async fn login(
    url: &str,
    credentials: &Credentials,
) -> Result<(WebSocketStream<MaybeTlsStream<TcpStream>>, SessionInfo), AuthError> {
    let (mut stream, _) = connect_async(url).await.map_err(transport)?;
    let request = login_request(credentials);

    let session = timeout(LOGIN_TIMEOUT, async {
        stream
            .send(Message::Text(request.into()))
            .await
            .map_err(transport)?;

        let message = stream
            .next()
            .await
            .ok_or_else(|| AuthError::UnexpectedResponse("closed before response".to_owned()))?
            .map_err(transport)?;

        let text = match message {
            Message::Text(text) => text,
            other => {
                return Err(AuthError::UnexpectedResponse(format!(
                    "expected text response, got {other:?}"
                )));
            }
        };

        parse_login_response(&text)
    })
    .await
    .map_err(|_| AuthError::Transport("login timed out".to_owned()))??;

    Ok((stream, session))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn credentials() -> Credentials {
        Credentials {
            username: "admin".to_owned(),
            password: "secret".to_owned(),
        }
    }

    #[test]
    fn login_request_matches_the_wnt_envelope() {
        let raw: serde_json::Value = serde_json::from_str(&login_request(&credentials())).unwrap();
        assert_eq!(raw["data"]["username"], "admin");
        assert_eq!(raw["data"]["password"], "secret");
        assert_eq!(raw["type"], 1);
        assert_eq!(raw["version"], 5);
    }

    #[test]
    fn parses_login_response_with_influx_token() {
        let raw = r#"{
            "data": { "role": 1, "session_id": "abc", "influx_token": "tok" },
            "result": 1, "type": 1, "version": 5
        }"#;
        let session = parse_login_response(raw).unwrap();
        assert_eq!(session.session_id, "abc");
        assert_eq!(session.role, UserRole::Admin);
        assert_eq!(session.influx_token.as_deref(), Some("tok"));
    }

    #[test]
    fn parses_login_response_without_influx_token() {
        let raw = r#"{ "data": { "role": 2, "session_id": "abc" }, "result": 1 }"#;
        let session = parse_login_response(raw).unwrap();
        assert_eq!(session.role, UserRole::Operator);
        assert_eq!(session.influx_token, None);
    }

    #[test]
    fn maps_result_codes_to_typed_errors() {
        let cases = [
            (r#"{ "result": 3 }"#, "invalid credentials"),
            (r#"{ "result": 4 }"#, "wrong protocol version"),
            (r#"{ "result": 5 }"#, "access denied"),
        ];
        for (raw, expected) in cases {
            let error = parse_login_response(raw).unwrap_err().to_string();
            assert_eq!(error, expected);
        }
        assert!(matches!(
            parse_login_response(r#"{ "result": 9 }"#),
            Err(AuthError::Rejected(9))
        ));
    }

    #[test]
    fn rejects_unknown_roles_and_broken_payloads() {
        let raw = r#"{ "data": { "role": 7, "session_id": "abc" }, "result": 1 }"#;
        assert!(matches!(
            parse_login_response(raw),
            Err(AuthError::UnexpectedResponse(_))
        ));
        assert!(matches!(
            parse_login_response("not json"),
            Err(AuthError::UnexpectedResponse(_))
        ));
    }

    #[test]
    fn auth_url_selects_scheme_by_tls() {
        assert_eq!(
            auth_url("wnt.example.com", 8813, true),
            "wss://wnt.example.com:8813/"
        );
        assert_eq!(auth_url("10.0.0.5", 8813, false), "ws://10.0.0.5:8813/");
    }
}
