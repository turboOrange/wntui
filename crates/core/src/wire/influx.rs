use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
const DATABASE: &str = "wirepas";

// Column naming is `Message_<field#>` decoded against message.proto field
// numbers; see docs/api/time-series.md. These are the pinned defaults —
// `wntui probe` dumps the real names from the live backend.
pub mod columns {
    pub const NETWORK_ID: &str = "Message_2";
    pub const SOURCE_ADDRESS: &str = "Message_7";
    pub const VOLTAGE: &str = "Message_50_7";
    pub const BOOT_REASON: &str = "Message_50_90";
    pub const ONLINE_STATUS: &str = "Message_71_1";
    pub const TRAVELTIME_QOS0_AVERAGE: &str = "Message_72_2";
    pub const TRAVELTIME_QOS1_AVERAGE: &str = "Message_72_12";
}

pub mod measurements {
    // The voltage field lives in DiagnosticsData (50.7); which measurement
    // stores it per backend is confirmed by `probe`, analytics_boot is the
    // documented boot-diagnostics table.
    pub const DIAGNOSTICS: &str = "analytics_boot";
    pub const TRAVELTIME_KPI: &str = "analytics_traveltime_kpi";
    pub const NODESTATE: &str = "analytics_nodestate";
    pub const BOOT: &str = "analytics_boot";
    pub const PACKET: &str = "analytics_packet";
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum InfluxCredentials {
    // WNT >= 4.4: token from the login response.
    Token(String),
    // WNT < 4.4: same username/password as the auth service.
    Basic { username: String, password: String },
}

impl InfluxCredentials {
    pub fn from_session(
        info: &crate::session::SessionInfo,
        login: &crate::session::Credentials,
    ) -> Self {
        match &info.influx_token {
            Some(token) => Self::Token(token.clone()),
            None => Self::Basic {
                username: login.username.clone(),
                password: login.password.clone(),
            },
        }
    }

    fn authorization_header(&self) -> String {
        match self {
            Self::Token(token) => format!("Token {token}"),
            Self::Basic { username, password } => {
                // Influx v1 accepts HTTP basic auth; base64 of user:password.
                let raw = format!("{username}:{password}");
                format!("Basic {}", base64_encode(raw.as_bytes()))
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InfluxError {
    #[error("influx transport error: {0}")]
    Transport(String),
    #[error("influx returned HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("influx error response: {0}")]
    QueryError(String),
    #[error("unexpected influx response: {0}")]
    UnexpectedResponse(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GraphRange {
    Hour1,
    Hour6,
    Hour24,
    Day7,
}

impl GraphRange {
    pub fn next(self) -> Self {
        match self {
            Self::Hour1 => Self::Hour6,
            Self::Hour6 => Self::Hour24,
            Self::Hour24 => Self::Day7,
            Self::Day7 => Self::Hour1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Hour1 => "1h",
            Self::Hour6 => "6h",
            Self::Hour24 => "24h",
            Self::Day7 => "7d",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "1h" => Some(Self::Hour1),
            "6h" => Some(Self::Hour6),
            "24h" | "24" => Some(Self::Hour24),
            "7d" | "7" => Some(Self::Day7),
            _ => None,
        }
    }

    // Window sized so a chart keeps a readable point count per range.
    fn window(self) -> &'static str {
        match self {
            Self::Hour1 => "1m",
            Self::Hour6 => "5m",
            Self::Hour24 => "30m",
            Self::Day7 => "6h",
        }
    }

    fn span(self) -> &'static str {
        match self {
            Self::Hour1 => "1h",
            Self::Hour6 => "6h",
            Self::Hour24 => "24h",
            Self::Day7 => "7d",
        }
    }

    // The range in seconds: the fetched window is [now - span, now].
    pub fn seconds(self) -> i64 {
        match self {
            Self::Hour1 => 3600,
            Self::Hour6 => 6 * 3600,
            Self::Hour24 => 24 * 3600,
            Self::Day7 => 7 * 86_400,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GraphMetric {
    Voltage,
    TraveltimeQos0,
    TraveltimeQos1,
    Online,
    Boots,
    Packets,
}

impl GraphMetric {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().as_str() {
            "voltage" => Some(Self::Voltage),
            "traveltime" | "traveltime-qos0" | "traveltime_qos0" => Some(Self::TraveltimeQos0),
            "traveltime-qos1" | "traveltime_qos1" => Some(Self::TraveltimeQos1),
            "online" | "nodestate" => Some(Self::Online),
            "boots" | "boot" => Some(Self::Boots),
            "packets" | "packet" => Some(Self::Packets),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Voltage => "voltage",
            Self::TraveltimeQos0 => "travel time qos0",
            Self::TraveltimeQos1 => "travel time qos1",
            Self::Online => "online status",
            Self::Boots => "boot events",
            Self::Packets => "packet rate",
        }
    }
}

// A time window for a graph query: (start, end) in unix seconds. None
// means the whole range ending now.
pub type GraphWindow = Option<(i64, i64)>;

pub fn graph_query(
    metric: GraphMetric,
    network_id: u32,
    address: u32,
    range: GraphRange,
    window: GraphWindow,
) -> String {
    let filter = format!(
        "WHERE \"{}\" = {network_id} AND \"{}\" = {address}",
        columns::NETWORK_ID,
        columns::SOURCE_ADDRESS
    );

    let selection = match metric {
        GraphMetric::Voltage => format!(
            "SELECT mean(\"{}\") FROM \"{}\"",
            columns::VOLTAGE,
            measurements::DIAGNOSTICS
        ),
        GraphMetric::TraveltimeQos0 => format!(
            "SELECT mean(\"{}\") FROM \"{}\"",
            columns::TRAVELTIME_QOS0_AVERAGE,
            measurements::TRAVELTIME_KPI
        ),
        GraphMetric::TraveltimeQos1 => format!(
            "SELECT mean(\"{}\") FROM \"{}\"",
            columns::TRAVELTIME_QOS1_AVERAGE,
            measurements::TRAVELTIME_KPI
        ),
        GraphMetric::Online => format!(
            "SELECT max(\"{}\") FROM \"{}\"",
            columns::ONLINE_STATUS,
            measurements::NODESTATE
        ),
        GraphMetric::Boots => format!(
            "SELECT count(\"{}\") FROM \"{}\"",
            columns::BOOT_REASON,
            measurements::BOOT
        ),
        GraphMetric::Packets => {
            format!(
                "SELECT count(\"{}\") FROM \"{}\"",
                columns::SOURCE_ADDRESS,
                measurements::PACKET
            )
        }
    };

    // A window pins both edges to absolute times (pan/zoom, N4); without
    // one the range ends at now as before.
    let time_filter = match window {
        Some((start, end)) => format!(
            "time >= {} AND time <= {}",
            start * 1_000_000_000,
            end * 1_000_000_000
        ),
        None => format!("time > now() - {}", range.span()),
    };

    format!(
        "{selection} {filter} AND {time_filter} GROUP BY time({}) fill(null)",
        range.window()
    )
}

pub fn show_measurements_query() -> String {
    "SHOW MEASUREMENTS ON wirepas".to_owned()
}

pub fn show_field_keys_query() -> String {
    "SHOW FIELD KEYS ON wirepas".to_owned()
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Series {
    pub name: String,
    pub columns: Vec<String>,
    pub points: Vec<SeriesPoint>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct SeriesPoint {
    pub timestamp_ns: i64,
    pub values: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct QueryResponse {
    #[serde(default)]
    results: Vec<QueryResult>,
}

#[derive(Deserialize)]
struct QueryResult {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    series: Option<Vec<QuerySeries>>,
}

#[derive(Deserialize)]
struct QuerySeries {
    name: Option<String>,
    columns: Vec<String>,
    #[serde(default)]
    values: Vec<Vec<serde_json::Value>>,
}

pub fn parse_query_response(raw: &str) -> Result<Vec<Series>, InfluxError> {
    let response: QueryResponse = serde_json::from_str(raw)
        .map_err(|error| InfluxError::UnexpectedResponse(error.to_string()))?;

    let mut series = Vec::new();
    for result in response.results {
        if let Some(error) = result.error {
            return Err(InfluxError::QueryError(error));
        }
        for query_series in result.series.unwrap_or_default() {
            let mut points = Vec::with_capacity(query_series.values.len());
            for mut row in query_series.values {
                let timestamp_ns = row.first().and_then(timestamp_ns_of).ok_or_else(|| {
                    InfluxError::UnexpectedResponse("series row without a timestamp".to_owned())
                })?;
                row.remove(0);
                points.push(SeriesPoint {
                    timestamp_ns,
                    values: row,
                });
            }
            series.push(Series {
                name: query_series.name.unwrap_or_default(),
                columns: query_series.columns,
                points,
            });
        }
    }
    Ok(series)
}

// Influx timestamps can be numbers (epoch=ns) or RFC3339 strings (when the
// backend ignores the epoch parameter); both are projected to epoch ns.
pub fn timestamp_ns_of(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::String(text) => rfc3339_ns(text),
        _ => None,
    }
}

// Minimal RFC3339 parser: YYYY-MM-DD[TH ]HH:MM:SS[.frac][Z|±HH:MM] -> epoch ns.
// Backends occasionally ignore epoch=ns and return string timestamps.
fn rfc3339_ns(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, mut time) = text
        .split_once('T')
        .or_else(|| text.split_once(' '))
        .unwrap_or((text, ""));

    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;

    // Trim a trailing timezone: Z, +HH:MM, or -HH:MM.
    if let Some(position) = time.rfind(['+', '-'])
        && position > 0
        && time.as_bytes().get(position.wrapping_sub(1)) != Some(&b'E')
    {
        time = &time[..position];
    }
    let time = time.trim_end_matches('Z');

    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second_and_frac = time_parts.next().unwrap_or("0");
    let (second, fraction) = match second_and_frac.split_once('.') {
        Some((second, fraction)) => (second.parse::<i64>().ok()?, fraction),
        None => (second_and_frac.parse::<i64>().ok()?, ""),
    };

    let days = days_from_civil(year, month, day)?;
    let fraction: i64 = if fraction.is_empty() {
        0
    } else {
        let digits: i64 = fraction
            .chars()
            .take(9)
            .map(|c| c.to_digit(10))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .fold(0, |acc, digit| acc * 10 + digit as i64);
        digits * 10_i64.pow(9 - fraction.len().min(9) as u32)
    };

    Some((((days * 24 + hour) * 60 + minute) * 60 + second) * 1_000_000_000 + fraction)
}

// Howard Hinnant's days_from_civil; avoids pulling in chrono/time for this.
fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_shift = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_shift + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    Some(era * 146_097 + day_of_era - 719_468)
}

pub struct InfluxClient {
    url: String,
    authorization: String,
}

impl InfluxClient {
    pub fn new(host: &str, port: u16, tls: bool, credentials: &InfluxCredentials) -> Self {
        let scheme = if tls { "https" } else { "http" };
        Self {
            url: format!("{scheme}://{host}:{port}"),
            authorization: credentials.authorization_header(),
        }
    }

    pub fn from_url(url: &str, credentials: &InfluxCredentials) -> Self {
        Self {
            url: url.trim_end_matches('/').to_owned(),
            authorization: credentials.authorization_header(),
        }
    }

    pub async fn query(&self, query: &str) -> Result<Vec<Series>, InfluxError> {
        let path = format!(
            "/query?db={DATABASE}&epoch=ns&pretty=true&q={}",
            percent_encode(query)
        );
        let body = self.get(&path).await?;
        parse_query_response(&body)
    }

    async fn get(&self, path: &str) -> Result<String, InfluxError> {
        let parsed = split_url(&self.url)?;
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: {}\r\nConnection: close\r\nAccept: application/json\r\n\r\n",
            parsed.host_header, self.authorization
        );

        let response = tokio::time::timeout(QUERY_TIMEOUT, async {
            let mut stream = connect(parsed.tls, &parsed.host, parsed.port).await?;
            stream
                .write_all(request.as_bytes())
                .await
                .map_err(|error| InfluxError::Transport(error.to_string()))?;

            let mut body = String::new();
            stream
                .read_to_string(&mut body)
                .await
                .map_err(|error| InfluxError::Transport(error.to_string()))?;
            Ok(body)
        })
        .await
        .map_err(|_| InfluxError::Transport("influx query timed out".to_owned()))??;

        let (status, payload) = split_http_response(&response)?;
        if !(200..300).contains(&status) {
            return Err(InfluxError::Http {
                status,
                body: payload.chars().take(200).collect(),
            });
        }
        Ok(payload)
    }

    pub async fn probe(&self) -> Result<Vec<Series>, InfluxError> {
        let measurements = self.query(&show_measurements_query()).await?;
        let field_keys = self.query(&show_field_keys_query()).await?;
        let mut all = measurements;
        all.extend(field_keys);
        Ok(all)
    }
}

struct ParsedUrl {
    host: String,
    host_header: String,
    port: u16,
    tls: bool,
}

fn split_url(url: &str) -> Result<ParsedUrl, InfluxError> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| InfluxError::UnexpectedResponse(format!("invalid influx url: {url}")))?;
    let tls = match scheme {
        "https" => true,
        "http" => false,
        other => {
            return Err(InfluxError::UnexpectedResponse(format!(
                "invalid influx scheme: {other}"
            )));
        }
    };

    let authority = rest.split('/').next().unwrap_or(rest);
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() => (
            host.to_owned(),
            port.parse()
                .map_err(|_| InfluxError::UnexpectedResponse(format!("invalid port: {port}")))?,
        ),
        _ => (authority.to_owned(), if tls { 443 } else { 80 }),
    };

    let host_header = if authority.contains(':') {
        authority.to_owned()
    } else {
        format!("{host}:{port}")
    };

    Ok(ParsedUrl {
        host,
        host_header,
        port,
        tls,
    })
}

async fn connect(tls: bool, host: &str, port: u16) -> Result<InfluxStream, InfluxError> {
    let tcp = tokio::net::TcpStream::connect((host, port))
        .await
        .map_err(|error| InfluxError::Transport(error.to_string()))?;

    if !tls {
        return Ok(InfluxStream::Plain(tcp));
    }

    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // rustls is built without its default aws-lc provider (ring feature via
    // tokio-tungstenite), so the provider must be supplied explicitly.
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(rustls::DEFAULT_VERSIONS)
    .map_err(|error| InfluxError::Transport(error.to_string()))?
    .with_root_certificates(roots)
    .with_no_client_auth();

    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
    let server_name = rustls::pki_types::ServerName::try_from(host.to_owned())
        .map_err(|_| InfluxError::Transport(format!("invalid tls host: {host}")))?;
    let tls_stream = connector
        .connect(server_name, tcp)
        .await
        .map_err(|error| InfluxError::Transport(error.to_string()))?;
    Ok(InfluxStream::Tls(Box::new(tls_stream)))
}

enum InfluxStream {
    Plain(tokio::net::TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>),
}

impl tokio::io::AsyncRead for InfluxStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Plain(stream) => std::pin::Pin::new(stream).poll_read(cx, buf),
            Self::Tls(stream) => std::pin::Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for InfluxStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        match &mut *self {
            Self::Plain(stream) => std::pin::Pin::new(stream).poll_write(cx, buf),
            Self::Tls(stream) => std::pin::Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        match &mut *self {
            Self::Plain(stream) => std::pin::Pin::new(stream).poll_flush(cx),
            Self::Tls(stream) => std::pin::Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        match &mut *self {
            Self::Plain(stream) => std::pin::Pin::new(stream).poll_shutdown(cx),
            Self::Tls(stream) => std::pin::Pin::new(stream).poll_shutdown(cx),
        }
    }
}

fn split_http_response(raw: &str) -> Result<(u16, String), InfluxError> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| InfluxError::UnexpectedResponse("truncated http response".to_owned()))?;

    let status = head
        .lines()
        .next()
        .and_then(|status_line| status_line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| InfluxError::UnexpectedResponse("no http status".to_owned()))?;

    Ok((status, body.to_owned()))
}

fn percent_encode(text: &str) -> String {
    // RFC 3986: keep unreserved characters, escape everything else.
    let mut output = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(byte as char);
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();

    for chunk in input.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let triple = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);

        output.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        output.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn graph_queries_pin_measurements_fields_filters_and_windows() {
        assert_eq!(
            graph_query(GraphMetric::Voltage, 42, 3736, GraphRange::Hour24, None),
            "SELECT mean(\"Message_50_7\") FROM \"analytics_boot\" \
             WHERE \"Message_2\" = 42 AND \"Message_7\" = 3736 \
             AND time > now() - 24h GROUP BY time(30m) fill(null)"
        );
        assert_eq!(
            graph_query(GraphMetric::TraveltimeQos0, 1, 2, GraphRange::Hour1, None),
            "SELECT mean(\"Message_72_2\") FROM \"analytics_traveltime_kpi\" \
             WHERE \"Message_2\" = 1 AND \"Message_7\" = 2 \
             AND time > now() - 1h GROUP BY time(1m) fill(null)"
        );
    }

    #[test]
    fn windowed_queries_pin_both_edges_to_absolute_times() {
        let query = graph_query(
            GraphMetric::Voltage,
            42,
            3736,
            GraphRange::Hour24,
            Some((1_700_000_000, 1_700_003_600)),
        );
        assert!(query.contains("AND time >= 1700000000000000000 AND time <= 1700003600000000000"));
        assert!(!query.contains("now()"));
    }

    #[test]
    fn every_metric_produces_a_select_with_the_node_filter() {
        for metric in [
            GraphMetric::Voltage,
            GraphMetric::TraveltimeQos0,
            GraphMetric::TraveltimeQos1,
            GraphMetric::Online,
            GraphMetric::Boots,
            GraphMetric::Packets,
        ] {
            let query = graph_query(metric, 1, 2, GraphRange::Hour1, None);
            assert!(query.contains("SELECT "));
            assert!(query.contains("WHERE \"Message_2\" = 1 AND \"Message_7\" = 2"));
            assert!(query.contains("GROUP BY time(1m)"));
        }
    }

    #[test]
    fn ranges_cycle_and_parse() {
        assert_eq!(GraphRange::Hour1.next(), GraphRange::Hour6);
        assert_eq!(GraphRange::Hour6.next(), GraphRange::Hour24);
        assert_eq!(GraphRange::Hour24.next(), GraphRange::Day7);
        assert_eq!(GraphRange::Day7.next(), GraphRange::Hour1);
        assert_eq!(GraphRange::parse("24h"), Some(GraphRange::Hour24));
        assert_eq!(GraphRange::parse("nonsense"), None);
    }

    #[test]
    fn metrics_parse_from_cli_words() {
        assert_eq!(GraphMetric::parse("Voltage"), Some(GraphMetric::Voltage));
        assert_eq!(
            GraphMetric::parse("traveltime-qos1"),
            Some(GraphMetric::TraveltimeQos1)
        );
        assert_eq!(GraphMetric::parse("online"), Some(GraphMetric::Online));
        assert_eq!(GraphMetric::parse("banana"), None);
    }

    #[test]
    fn parses_a_recorded_influx_response() {
        let raw = r#"{
            "results": [
                {
                    "statement_id": 0,
                    "series": [
                        {
                            "name": "analytics_boot",
                            "columns": ["time", "mean"],
                            "values": [
                                [1700000000000000000, 3.15],
                                [1700000180000000000, 3.15]
                            ]
                        }
                    ]
                }
            ]
        }"#;

        let series = parse_query_response(raw).unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].name, "analytics_boot");
        assert_eq!(series[0].columns, vec!["time", "mean"]);
        assert_eq!(series[0].points.len(), 2);
        assert_eq!(series[0].points[0].timestamp_ns, 1_700_000_000_000_000_000);
        let second_value = series[0].points[1].values[0].as_f64().unwrap();
        assert!((second_value - 3.15).abs() < 1e-9, "got {second_value}");
    }

    #[test]
    fn parses_error_and_empty_results() {
        assert!(matches!(
            parse_query_response(r#"{"results":[{"error":"measurement not found"}]}"#),
            Err(InfluxError::QueryError(_))
        ));
        assert_eq!(
            parse_query_response(r#"{"results":[]}"#).unwrap(),
            Vec::new()
        );
        assert_eq!(
            parse_query_response(r#"{"results":[{"statement_id":0}]}"#).unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn query_errors_are_surfaced_not_swallowed() {
        // a failing result aborts the parse even after a good series
        let raw = r#"{
            "results": [
                {"series": [{"name": "a", "columns": ["time"], "values": [[1]]}]},
                {"error": "boom"}
            ]
        }"#;
        assert!(matches!(
            parse_query_response(raw),
            Err(InfluxError::QueryError(_))
        ));
    }

    #[test]
    fn show_queries_target_the_wirepas_database() {
        assert_eq!(show_measurements_query(), "SHOW MEASUREMENTS ON wirepas");
        assert_eq!(show_field_keys_query(), "SHOW FIELD KEYS ON wirepas");
    }

    #[test]
    fn percent_encoding_keeps_queries_safe_for_a_get() {
        assert_eq!(
            percent_encode("SELECT mean(\"Message_50_7\") FROM x"),
            "SELECT%20mean%28%22Message_50_7%22%29%20FROM%20x"
        );
    }

    #[test]
    fn basic_auth_header_is_standard_base64() {
        let credentials = InfluxCredentials::Basic {
            username: "admin".to_owned(),
            password: "secret".to_owned(),
        };
        let header = credentials.authorization_header();
        assert_eq!(header, "Basic YWRtaW46c2VjcmV0");

        let token = InfluxCredentials::Token("tok".to_owned());
        assert_eq!(token.authorization_header(), "Token tok");
    }

    #[test]
    fn credentials_choose_token_when_the_session_has_one() {
        let info = crate::session::SessionInfo {
            session_id: "s".to_owned(),
            role: crate::session::UserRole::Admin,
            influx_token: Some("tok".to_owned()),
        };
        let login = crate::session::Credentials {
            username: "admin".to_owned(),
            password: "secret".to_owned(),
        };
        assert_eq!(
            InfluxCredentials::from_session(&info, &login),
            InfluxCredentials::Token("tok".to_owned())
        );

        let v1_info = crate::session::SessionInfo {
            session_id: "s".to_owned(),
            role: crate::session::UserRole::Admin,
            influx_token: None,
        };
        assert_eq!(
            InfluxCredentials::from_session(&v1_info, &login),
            InfluxCredentials::Basic {
                username: "admin".to_owned(),
                password: "secret".to_owned()
            }
        );
    }

    #[test]
    fn url_splitting_handles_schemes_and_default_ports() {
        let parsed = split_url("https://wnt.example.com:8886").unwrap();
        assert_eq!(parsed.host, "wnt.example.com");
        assert_eq!(parsed.port, 8886);
        assert!(parsed.tls);
        assert_eq!(parsed.host_header, "wnt.example.com:8886");

        let parsed = split_url("http://10.0.0.5").unwrap();
        assert_eq!(parsed.port, 80);
        assert_eq!(parsed.host_header, "10.0.0.5:80");
        assert!(!parsed.tls);
    }

    #[test]
    fn http_response_splitting_extracts_status_and_body() {
        let (status, body) =
            split_http_response("HTTP/1.1 200 OK\r\nContent-Type: json\r\n\r\n{\"a\":1}").unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, "{\"a\":1}");

        assert!(split_http_response("garbage").is_err());
    }

    #[test]
    fn rfc3339_timestamps_project_to_epoch_ns() {
        assert_eq!(
            timestamp_ns_of(&serde_json::json!("2023-11-14T22:13:20Z")),
            Some(1_700_000_000_000_000_000)
        );
        assert_eq!(
            timestamp_ns_of(&serde_json::json!("2023-11-14T22:13:20.5Z")),
            Some(1_700_000_000_500_000_000)
        );
        assert_eq!(
            timestamp_ns_of(&serde_json::json!("2023-11-14T22:13:20.123456789Z")),
            Some(1_700_000_000_123_456_789)
        );
        // offset timestamps are accepted; the offset is treated as UTC
        // (Influx lab deployments are UTC in practice).
        assert!(timestamp_ns_of(&serde_json::json!("2023-11-14T22:13:20+02:00")).is_some());
        assert_eq!(
            timestamp_ns_of(&serde_json::json!(1_700_000_000)),
            Some(1_700_000_000)
        );
        assert_eq!(timestamp_ns_of(&serde_json::json!("garbage")), None);
    }
}
