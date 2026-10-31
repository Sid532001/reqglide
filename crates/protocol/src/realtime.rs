use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use api_client_core::{HeaderEntry, RequestOptions};
use api_client_http::{HttpExecutionContext, build_streaming_reqwest_client};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use eventsource_stream::Eventsource;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        handshake::{client::generate_key, derive_accept_key},
        protocol::{Role, WebSocketConfig},
    },
};
use uuid::Uuid;

const MAX_QUEUED_EVENTS: usize = 2_000;
const MAX_QUEUED_EVENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_DRAIN_EVENTS: usize = 500;
const OUTBOUND_QUEUE_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProtocolEventDirection {
    Inbound,
    Outbound,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProtocolEventKind {
    Open,
    Text,
    Binary,
    Ping,
    Pong,
    Event,
    Error,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolEvent {
    pub id: String,
    pub timestamp_ms: u64,
    pub direction: ProtocolEventDirection,
    pub kind: ProtocolEventKind,
    pub data: String,
    pub byte_length: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_ms: Option<u64>,
}

impl ProtocolEvent {
    fn new(
        direction: ProtocolEventDirection,
        kind: ProtocolEventKind,
        data: impl Into<String>,
        byte_length: usize,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            timestamp_ms: now_ms(),
            direction,
            kind,
            data: data.into(),
            byte_length,
            event_name: None,
            event_id: None,
            retry_ms: None,
        }
    }
}

#[derive(Clone)]
pub struct WebSocketInput {
    pub url: String,
    pub headers: Vec<HeaderEntry>,
    pub subprotocols: Vec<String>,
    pub options: RequestOptions,
    pub context: HttpExecutionContext,
    pub max_message_bytes: usize,
    pub redactions: Vec<String>,
}

#[derive(Clone)]
pub struct ServerSentEventsInput {
    pub url: String,
    pub headers: Vec<HeaderEntry>,
    pub last_event_id: String,
    pub options: RequestOptions,
    pub context: HttpExecutionContext,
    pub redactions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WebSocketOutbound {
    Text { data: String },
    Binary { data_base64: String },
    Ping { data: String },
    Close,
}

struct EventQueue {
    events: VecDeque<ProtocolEvent>,
    bytes: usize,
    dropped: usize,
    redactions: Vec<String>,
}

impl EventQueue {
    fn new(redactions: Vec<String>) -> Self {
        Self {
            events: VecDeque::new(),
            bytes: 0,
            dropped: 0,
            redactions,
        }
    }

    fn push(&mut self, mut event: ProtocolEvent) {
        event.data = redact(&event.data, &self.redactions);
        event.event_name = event
            .event_name
            .as_deref()
            .map(|value| redact(value, &self.redactions));
        event.event_id = event
            .event_id
            .as_deref()
            .map(|value| redact(value, &self.redactions));
        let event_bytes = event.data.len()
            + event.event_name.as_ref().map_or(0, String::len)
            + event.event_id.as_ref().map_or(0, String::len);
        while self.events.len() >= MAX_QUEUED_EVENTS
            || self.bytes.saturating_add(event_bytes) > MAX_QUEUED_EVENT_BYTES
        {
            let Some(removed) = self.events.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(event_size(&removed));
            self.dropped += 1;
        }
        if event_bytes > MAX_QUEUED_EVENT_BYTES {
            self.dropped += 1;
            return;
        }
        self.bytes = self.bytes.saturating_add(event_bytes);
        self.events.push_back(event);
    }

    fn drain(&mut self, maximum: usize) -> Vec<ProtocolEvent> {
        let maximum = maximum.clamp(1, MAX_DRAIN_EVENTS);
        let mut drained = Vec::with_capacity(maximum.min(self.events.len() + 1));
        if self.dropped > 0 {
            let dropped = self.dropped;
            self.dropped = 0;
            drained.push(ProtocolEvent::new(
                ProtocolEventDirection::System,
                ProtocolEventKind::Error,
                format!("{dropped} older events were dropped to enforce local memory limits."),
                0,
            ));
        }
        while drained.len() < maximum {
            let Some(event) = self.events.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(event_size(&event));
            drained.push(event);
        }
        drained
    }
}

pub struct ProtocolSession {
    id: String,
    events: Arc<Mutex<EventQueue>>,
    outbound: Option<mpsc::Sender<WebSocketOutbound>>,
    abort: tokio::task::AbortHandle,
    finished: Arc<AtomicBool>,
}

impl ProtocolSession {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    pub fn drain_events(&self, maximum: usize) -> Result<Vec<ProtocolEvent>, String> {
        self.events
            .lock()
            .map_err(|_| "protocol event queue is unavailable".to_owned())
            .map(|mut events| events.drain(maximum))
    }

    pub async fn send(&self, message: WebSocketOutbound) -> Result<(), String> {
        let sender = self
            .outbound
            .as_ref()
            .ok_or_else(|| "this protocol session does not accept outbound messages".to_owned())?;
        sender
            .send(message)
            .await
            .map_err(|_| "WebSocket session is no longer connected".to_owned())
    }

    pub fn stop(&self) {
        self.abort.abort();
        self.finished.store(true, Ordering::Release);
        push_event(
            &self.events,
            ProtocolEvent::new(
                ProtocolEventDirection::System,
                ProtocolEventKind::Closed,
                "Session stopped locally.",
                0,
            ),
        );
    }
}

impl Drop for ProtocolSession {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

pub fn start_websocket(input: WebSocketInput) -> Result<ProtocolSession, String> {
    validate_websocket_input(&input)?;
    let mut websocket_url =
        url::Url::parse(input.url.trim()).map_err(|_| "WebSocket URL is invalid".to_owned())?;
    let transport_scheme = if websocket_url.scheme() == "wss" {
        "https"
    } else {
        "http"
    };
    websocket_url
        .set_scheme(transport_scheme)
        .map_err(|_| "WebSocket URL scheme is invalid".to_owned())?;
    let client = build_streaming_reqwest_client(&input.options, input.context)
        .map_err(|error| error.to_string())?;
    let key = generate_key();
    let expected_accept = derive_accept_key(key.as_bytes());
    let requested_subprotocols = input.subprotocols.clone();
    let mut request = client
        .get(websocket_url)
        .version(reqwest::Version::HTTP_11)
        .header(reqwest::header::CONNECTION, "Upgrade")
        .header(reqwest::header::UPGRADE, "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", &key);
    for header in input.headers.iter().filter(|header| header.enabled) {
        let trimmed_name = header.name.trim();
        if trimmed_name.is_empty() && header.value.is_empty() {
            continue;
        }
        if is_reserved_websocket_header(trimmed_name) {
            return Err(format!(
                "WebSocket header '{trimmed_name}' is managed by the connection engine"
            ));
        }
        let name = reqwest::header::HeaderName::from_bytes(trimmed_name.as_bytes())
            .map_err(|_| format!("WebSocket header name '{trimmed_name}' is invalid"))?;
        let value = reqwest::header::HeaderValue::from_str(&header.value)
            .map_err(|_| format!("WebSocket header '{trimmed_name}' has an invalid value"))?;
        request = request.header(name, value);
    }
    if !input.subprotocols.is_empty() {
        request = request.header("sec-websocket-protocol", input.subprotocols.join(", "));
    }

    let events = Arc::new(Mutex::new(EventQueue::new(input.redactions)));
    let task_events = Arc::clone(&events);
    let finished = Arc::new(AtomicBool::new(false));
    let task_finished = Arc::clone(&finished);
    let (outbound, mut outbound_receiver) = mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    let maximum = input.max_message_bytes.clamp(1, MAX_MESSAGE_BYTES);
    let mut configuration = WebSocketConfig::default();
    configuration.max_message_size = Some(maximum);
    configuration.max_frame_size = Some(maximum);
    let connect_timeout = Duration::from_millis(input.options.timeout_ms.clamp(1, 300_000));

    let task = tokio::spawn(async move {
        let response = match tokio::time::timeout(connect_timeout, request.send()).await {
            Err(_) => {
                push_error(&task_events, "WebSocket connection timed out.");
                task_finished.store(true, Ordering::Release);
                return;
            }
            Ok(Err(error)) => {
                push_error(
                    &task_events,
                    format!("WebSocket connection failed: {error}"),
                );
                task_finished.store(true, Ordering::Release);
                return;
            }
            Ok(Ok(response)) => response,
        };
        if let Err(error) =
            validate_websocket_response(&response, &expected_accept, &requested_subprotocols)
        {
            push_error(&task_events, error);
            task_finished.store(true, Ordering::Release);
            return;
        }
        let status = response.status();
        let upgraded = match tokio::time::timeout(connect_timeout, response.upgrade()).await {
            Err(_) => {
                push_error(&task_events, "WebSocket protocol upgrade timed out.");
                task_finished.store(true, Ordering::Release);
                return;
            }
            Ok(Err(error)) => {
                push_error(
                    &task_events,
                    format!("WebSocket protocol upgrade failed: {error}"),
                );
                task_finished.store(true, Ordering::Release);
                return;
            }
            Ok(Ok(upgraded)) => upgraded,
        };
        let mut socket =
            WebSocketStream::from_raw_socket(upgraded, Role::Client, Some(configuration)).await;
        push_event(
            &task_events,
            ProtocolEvent::new(
                ProtocolEventDirection::System,
                ProtocolEventKind::Open,
                format!("Connected with HTTP {status}."),
                0,
            ),
        );

        loop {
            tokio::select! {
                outgoing = outbound_receiver.recv() => {
                    let Some(outgoing) = outgoing else { break };
                    let (message, event) = match outbound_message(outgoing, maximum) {
                        Ok(value) => value,
                        Err(error) => {
                            push_error(&task_events, error);
                            continue;
                        }
                    };
                    let closes = matches!(message, Message::Close(_));
                    if let Err(error) = socket.send(message).await {
                        push_error(&task_events, format!("WebSocket send failed: {error}"));
                        break;
                    }
                    push_event(&task_events, event);
                    if closes {
                        break;
                    }
                }
                incoming = socket.next() => {
                    match incoming {
                        Some(Ok(message)) => {
                            let closes = matches!(message, Message::Close(_));
                            if let Some(event) = incoming_event(message) {
                                push_event(&task_events, event);
                            }
                            if closes {
                                break;
                            }
                        }
                        Some(Err(error)) => {
                            push_error(&task_events, format!("WebSocket receive failed: {error}"));
                            break;
                        }
                        None => break,
                    }
                }
            }
        }
        push_event(
            &task_events,
            ProtocolEvent::new(
                ProtocolEventDirection::System,
                ProtocolEventKind::Closed,
                "WebSocket connection closed.",
                0,
            ),
        );
        task_finished.store(true, Ordering::Release);
    });

    Ok(ProtocolSession {
        id: Uuid::new_v4().to_string(),
        events,
        outbound: Some(outbound),
        abort: task.abort_handle(),
        finished,
    })
}

fn validate_websocket_response(
    response: &reqwest::Response,
    expected_accept: &str,
    requested_subprotocols: &[String],
) -> Result<(), String> {
    if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
        return Err(format!(
            "WebSocket endpoint returned HTTP {}, expected 101 Switching Protocols.",
            response.status()
        ));
    }
    if !header_has_token(response.headers(), reqwest::header::UPGRADE, "websocket") {
        return Err("WebSocket response is missing the Upgrade: websocket header".to_owned());
    }
    if !header_has_token(response.headers(), reqwest::header::CONNECTION, "upgrade") {
        return Err("WebSocket response is missing the Connection: Upgrade header".to_owned());
    }
    let accept = response
        .headers()
        .get("sec-websocket-accept")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if accept != expected_accept {
        return Err("WebSocket response has an invalid Sec-WebSocket-Accept value".to_owned());
    }
    if response.headers().contains_key("sec-websocket-extensions") {
        return Err("WebSocket server selected an extension that was not requested".to_owned());
    }
    let selected_subprotocol = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok());
    match (requested_subprotocols.is_empty(), selected_subprotocol) {
        (true, Some(_)) => {
            return Err("WebSocket server selected an unsolicited subprotocol".to_owned());
        }
        (false, None) => {
            return Err("WebSocket server did not select a requested subprotocol".to_owned());
        }
        (false, Some(selected))
            if !requested_subprotocols
                .iter()
                .any(|requested| requested == selected) =>
        {
            return Err(format!(
                "WebSocket server selected unexpected subprotocol '{selected}'"
            ));
        }
        _ => {}
    }
    Ok(())
}

fn header_has_token(
    headers: &reqwest::header::HeaderMap,
    name: reqwest::header::HeaderName,
    expected: &str,
) -> bool {
    headers.get_all(name).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case(expected))
        })
    })
}

pub fn start_server_sent_events(input: ServerSentEventsInput) -> Result<ProtocolSession, String> {
    let parsed = url::Url::parse(input.url.trim()).map_err(|_| "SSE URL is invalid".to_owned())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("SSE URL must be HTTP(S) and cannot contain credentials".to_owned());
    }
    let client = build_streaming_reqwest_client(&input.options, input.context)
        .map_err(|error| error.to_string())?;
    let mut request = client.get(parsed).header("accept", "text/event-stream");
    if !input.last_event_id.is_empty() {
        request = request.header("last-event-id", input.last_event_id);
    }
    for header in input.headers.iter().filter(|header| header.enabled) {
        if header.name.trim().is_empty() && header.value.is_empty() {
            continue;
        }
        let name = reqwest::header::HeaderName::from_bytes(header.name.trim().as_bytes())
            .map_err(|_| format!("SSE header name '{}' is invalid", header.name))?;
        let value = reqwest::header::HeaderValue::from_str(&header.value)
            .map_err(|_| format!("SSE header '{}' has an invalid value", header.name))?;
        request = request.header(name, value);
    }

    let events = Arc::new(Mutex::new(EventQueue::new(input.redactions)));
    let task_events = Arc::clone(&events);
    let finished = Arc::new(AtomicBool::new(false));
    let task_finished = Arc::clone(&finished);
    let connect_timeout = Duration::from_millis(input.options.timeout_ms.max(1));
    let task = tokio::spawn(async move {
        // Only response headers have a deadline; an established SSE body is indefinite.
        let response = match tokio::time::timeout(connect_timeout, request.send()).await {
            Err(_) => {
                push_error(&task_events, "SSE connection timed out.");
                task_finished.store(true, Ordering::Release);
                return;
            }
            Ok(Err(error)) => {
                push_error(&task_events, format!("SSE connection failed: {error}"));
                task_finished.store(true, Ordering::Release);
                return;
            }
            Ok(Ok(response)) => response,
        };
        if !response.status().is_success() {
            push_error(
                &task_events,
                format!("SSE endpoint returned HTTP {}.", response.status()),
            );
            task_finished.store(true, Ordering::Release);
            return;
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        if !content_type.eq_ignore_ascii_case("text/event-stream") {
            push_error(
                &task_events,
                format!(
                    "SSE endpoint returned content type '{}', expected text/event-stream.",
                    if content_type.is_empty() {
                        "(missing)"
                    } else {
                        content_type
                    }
                ),
            );
            task_finished.store(true, Ordering::Release);
            return;
        }
        push_event(
            &task_events,
            ProtocolEvent::new(
                ProtocolEventDirection::System,
                ProtocolEventKind::Open,
                "SSE stream connected.",
                0,
            ),
        );
        let mut stream = Box::pin(bounded_sse_chunks(response.bytes_stream()).eventsource());
        while let Some(next) = stream.next().await {
            match next {
                Ok(event) => {
                    if event.data.len() > MAX_MESSAGE_BYTES {
                        push_error(
                            &task_events,
                            format!("SSE event exceeded the {MAX_MESSAGE_BYTES}-byte limit."),
                        );
                        break;
                    }
                    let event_bytes = event.data.len();
                    let mut protocol_event = ProtocolEvent::new(
                        ProtocolEventDirection::Inbound,
                        ProtocolEventKind::Event,
                        event.data,
                        event_bytes,
                    );
                    protocol_event.event_name = (!event.event.is_empty()).then_some(event.event);
                    protocol_event.event_id = (!event.id.is_empty()).then_some(event.id);
                    protocol_event.retry_ms = event
                        .retry
                        .and_then(|duration| u64::try_from(duration.as_millis()).ok());
                    push_event(&task_events, protocol_event);
                }
                Err(error) => {
                    push_error(&task_events, format!("SSE stream failed: {error}"));
                    break;
                }
            }
        }
        push_event(
            &task_events,
            ProtocolEvent::new(
                ProtocolEventDirection::System,
                ProtocolEventKind::Closed,
                "SSE stream closed.",
                0,
            ),
        );
        task_finished.store(true, Ordering::Release);
    });

    Ok(ProtocolSession {
        id: Uuid::new_v4().to_string(),
        events,
        outbound: None,
        abort: task.abort_handle(),
        finished,
    })
}

/// Bounds raw bytes before eventsource-stream can buffer an unfinished line or
/// append more data fields. Blank lines reset the budget, even within one chunk.
#[derive(Default)]
struct SseEventLimit {
    pending_bytes: usize,
    line_has_content: bool,
    previous_was_cr: bool,
    pending_utf8: Vec<u8>,
    pending_line: Vec<u8>,
}

impl SseEventLimit {
    fn push(&mut self, bytes: &[u8]) -> Result<Option<Vec<u8>>, String> {
        self.check(bytes)?;
        if let Some(end) = bytes.iter().rposition(|byte| matches!(byte, b'\r' | b'\n')) {
            let mut lines = std::mem::take(&mut self.pending_line);
            lines.extend_from_slice(&bytes[..=end]);
            self.pending_line.extend_from_slice(&bytes[end + 1..]);
            Ok(Some(lines))
        } else {
            // Keep incomplete lines in our bounded buffer. Passing every partial
            // chunk downstream would repeatedly rescan the entire growing line.
            self.pending_line.extend_from_slice(bytes);
            Ok(None)
        }
    }

    fn check(&mut self, bytes: &[u8]) -> Result<(), String> {
        // Invalid UTF-8 must not hide delimiters from the downstream decoder and
        // let its undecoded buffer grow across otherwise complete event blocks.
        self.validate_utf8(bytes)?;
        if !bytes.iter().any(|byte| matches!(byte, b'\r' | b'\n')) {
            // Long fragmented lines need one bulk update, rather than mutating
            // parser state for every byte while a busy runtime is catching up.
            if !bytes.is_empty() {
                self.line_has_content = true;
                self.previous_was_cr = false;
            }
            return self.add_pending_bytes(bytes.len());
        }
        for &byte in bytes {
            let continues_crlf = self.previous_was_cr && byte == b'\n';
            self.previous_was_cr = false;
            if continues_crlf && self.pending_bytes == 0 {
                continue;
            }
            self.add_pending_bytes(1)?;
            if continues_crlf {
                continue;
            }
            if matches!(byte, b'\r' | b'\n') {
                if !self.line_has_content {
                    self.pending_bytes = 0;
                }
                self.line_has_content = false;
                self.previous_was_cr = byte == b'\r';
            } else {
                self.line_has_content = true;
            }
        }
        Ok(())
    }

    fn add_pending_bytes(&mut self, length: usize) -> Result<(), String> {
        self.pending_bytes = self.pending_bytes.saturating_add(length);
        if self.pending_bytes > MAX_MESSAGE_BYTES {
            Err(format!(
                "SSE event exceeded the {MAX_MESSAGE_BYTES}-byte limit."
            ))
        } else {
            Ok(())
        }
    }

    fn finish(&mut self) -> Result<Option<Vec<u8>>, String> {
        if !self.pending_utf8.is_empty() {
            return Err("SSE stream ended with incomplete UTF-8.".to_owned());
        }
        if self.previous_was_cr {
            // The parser waits for a possible LF after CR. At EOF, resolve that
            // completed line without inventing an extra blank line/event.
            return Ok(Some(vec![b'\n']));
        }
        if self.pending_line.is_empty() {
            Ok(None)
        } else {
            // Forward the final fragment only once, at EOF. The parser discards
            // unterminated events and still sees any terminal decoding error.
            Ok(Some(std::mem::take(&mut self.pending_line)))
        }
    }

    fn validate_utf8(&mut self, mut bytes: &[u8]) -> Result<(), String> {
        if !self.pending_utf8.is_empty() {
            while let Some((&byte, rest)) = bytes.split_first() {
                self.pending_utf8.push(byte);
                bytes = rest;
                match std::str::from_utf8(&self.pending_utf8) {
                    Ok(_) => {
                        self.pending_utf8.clear();
                        break;
                    }
                    Err(error) if error.error_len().is_none() => {}
                    Err(_) => return Err("SSE stream is not valid UTF-8.".to_owned()),
                }
            }
        }
        match std::str::from_utf8(bytes) {
            Ok(_) => Ok(()),
            Err(error) if error.error_len().is_none() => {
                // An incomplete code point contains at most three pending bytes.
                self.pending_utf8
                    .extend_from_slice(&bytes[error.valid_up_to()..]);
                Ok(())
            }
            Err(_) => Err("SSE stream is not valid UTF-8.".to_owned()),
        }
    }
}

fn bounded_sse_chunks<S, B, E>(
    source: S,
) -> impl futures_util::Stream<Item = Result<Vec<u8>, String>>
where
    S: futures_util::Stream<Item = Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    futures_util::stream::unfold(
        (source, SseEventLimit::default(), false),
        |(mut source, mut limit, mut finished)| async move {
            if finished {
                return None;
            }
            loop {
                let next = match source.next().await {
                    Some(Ok(chunk)) => limit.push(chunk.as_ref()),
                    Some(Err(error)) => Err(error.to_string()),
                    None => {
                        finished = true;
                        limit.finish()
                    }
                };
                match next {
                    Ok(None) if !finished => continue,
                    Ok(None) => return None,
                    Ok(Some(lines)) => return Some((Ok(lines), (source, limit, finished))),
                    Err(error) => return Some((Err(error), (source, limit, true))),
                }
            }
        },
    )
}

fn validate_websocket_input(input: &WebSocketInput) -> Result<(), String> {
    let parsed =
        url::Url::parse(input.url.trim()).map_err(|_| "WebSocket URL is invalid".to_owned())?;
    if !matches!(parsed.scheme(), "ws" | "wss") {
        return Err("WebSocket URL must use ws:// or wss://".to_owned());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("WebSocket URL cannot contain credentials".to_owned());
    }
    if input.subprotocols.len() > 64 {
        return Err("WebSocket cannot request more than 64 subprotocols".to_owned());
    }
    for protocol in &input.subprotocols {
        if protocol.is_empty()
            || protocol.len() > 256
            || !protocol
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && byte != b',')
        {
            return Err(format!("WebSocket subprotocol '{protocol}' is invalid"));
        }
    }
    Ok(())
}

fn is_reserved_websocket_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("host")
        || name.eq_ignore_ascii_case("connection")
        || name.eq_ignore_ascii_case("upgrade")
        || name.to_ascii_lowercase().starts_with("sec-websocket-")
}

fn outbound_message(
    message: WebSocketOutbound,
    maximum: usize,
) -> Result<(Message, ProtocolEvent), String> {
    match message {
        WebSocketOutbound::Text { data } => {
            if data.len() > maximum {
                return Err(format!(
                    "WebSocket text message exceeds the {maximum}-byte limit"
                ));
            }
            let event = ProtocolEvent::new(
                ProtocolEventDirection::Outbound,
                ProtocolEventKind::Text,
                data.clone(),
                data.len(),
            );
            Ok((Message::Text(data.into()), event))
        }
        WebSocketOutbound::Binary { data_base64 } => {
            let bytes = BASE64
                .decode(&data_base64)
                .map_err(|_| "WebSocket binary payload is not valid base64".to_owned())?;
            if bytes.len() > maximum {
                return Err(format!(
                    "WebSocket binary message exceeds the {maximum}-byte limit"
                ));
            }
            let event = ProtocolEvent::new(
                ProtocolEventDirection::Outbound,
                ProtocolEventKind::Binary,
                data_base64,
                bytes.len(),
            );
            Ok((Message::Binary(bytes.into()), event))
        }
        WebSocketOutbound::Ping { data } => {
            if data.len() > 125 {
                return Err("WebSocket ping payload exceeds 125 bytes".to_owned());
            }
            let event = ProtocolEvent::new(
                ProtocolEventDirection::Outbound,
                ProtocolEventKind::Ping,
                data.clone(),
                data.len(),
            );
            Ok((Message::Ping(data.into_bytes().into()), event))
        }
        WebSocketOutbound::Close => Ok((
            Message::Close(None),
            ProtocolEvent::new(
                ProtocolEventDirection::Outbound,
                ProtocolEventKind::Closed,
                "Close frame sent.",
                0,
            ),
        )),
    }
}

fn incoming_event(message: Message) -> Option<ProtocolEvent> {
    match message {
        Message::Text(data) => Some(ProtocolEvent::new(
            ProtocolEventDirection::Inbound,
            ProtocolEventKind::Text,
            data.to_string(),
            data.len(),
        )),
        Message::Binary(data) => Some(ProtocolEvent::new(
            ProtocolEventDirection::Inbound,
            ProtocolEventKind::Binary,
            BASE64.encode(&data),
            data.len(),
        )),
        Message::Ping(data) => Some(ProtocolEvent::new(
            ProtocolEventDirection::Inbound,
            ProtocolEventKind::Ping,
            BASE64.encode(&data),
            data.len(),
        )),
        Message::Pong(data) => Some(ProtocolEvent::new(
            ProtocolEventDirection::Inbound,
            ProtocolEventKind::Pong,
            BASE64.encode(&data),
            data.len(),
        )),
        Message::Close(frame) => Some(ProtocolEvent::new(
            ProtocolEventDirection::Inbound,
            ProtocolEventKind::Closed,
            frame.map_or_else(
                || "Remote peer closed the connection.".to_owned(),
                |frame| format!("{} {}", u16::from(frame.code), frame.reason),
            ),
            0,
        )),
        Message::Frame(_) => None,
    }
}

fn push_error(events: &Arc<Mutex<EventQueue>>, error: impl Into<String>) {
    push_event(
        events,
        ProtocolEvent::new(
            ProtocolEventDirection::System,
            ProtocolEventKind::Error,
            error,
            0,
        ),
    );
}

fn push_event(events: &Arc<Mutex<EventQueue>>, event: ProtocolEvent) {
    if let Ok(mut events) = events.lock() {
        events.push(event);
    }
}

fn event_size(event: &ProtocolEvent) -> usize {
    event.data.len()
        + event.event_name.as_ref().map_or(0, String::len)
        + event.event_id.as_ref().map_or(0, String::len)
}

fn redact(value: &str, redactions: &[String]) -> String {
    redactions
        .iter()
        .filter(|secret| secret.len() >= 3)
        .fold(value.to_owned(), |redacted, secret| {
            redacted.replace(secret, "[REDACTED]")
        })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use api_client_core::RequestOptions;
    use api_client_http::{HttpExecutionContext, ProxyConfiguration};
    use eventsource_stream::Eventsource;
    use futures_util::{SinkExt, StreamExt};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    use super::{
        EventQueue, MAX_MESSAGE_BYTES, MAX_QUEUED_EVENTS, ProtocolEvent, ProtocolEventDirection,
        ProtocolEventKind, ProtocolSession, ServerSentEventsInput, SseEventLimit, WebSocketInput,
        WebSocketOutbound, bounded_sse_chunks, start_server_sent_events, start_websocket,
    };

    #[test]
    fn event_queue_redacts_and_bounds_messages() {
        let queue = Arc::new(Mutex::new(EventQueue::new(vec!["secret-value".to_owned()])));
        for index in 0..=MAX_QUEUED_EVENTS {
            super::push_event(
                &queue,
                ProtocolEvent::new(
                    ProtocolEventDirection::Inbound,
                    ProtocolEventKind::Text,
                    format!("{index}: secret-value"),
                    12,
                ),
            );
        }
        let drained = queue.lock().unwrap().drain(500);
        assert!(drained[0].data.contains("older events were dropped"));
        assert!(
            drained
                .iter()
                .all(|event| !event.data.contains("secret-value"))
        );
    }

    #[test]
    fn sse_limit_handles_split_utf8_and_all_event_delimiters() {
        for newline in ["\n", "\r", "\r\n"] {
            let mut limit = SseEventLimit::default();
            let event = format!("data: café🙂{newline}{newline}");
            for _ in 0..3 {
                for byte in event.as_bytes().chunks(1) {
                    limit.check(byte).unwrap();
                }
                assert_eq!(limit.pending_bytes, 0);
                assert!(limit.pending_utf8.is_empty());
            }
        }
    }

    #[test]
    fn sse_limit_rejects_invalid_utf8_before_it_can_hide_event_boundaries() {
        let mut limit = SseEventLimit::default();
        assert!(
            limit
                .check(b"data: \xff\n\n")
                .unwrap_err()
                .contains("UTF-8")
        );
    }

    #[tokio::test]
    async fn sse_fragmented_unterminated_lines_never_reach_the_event_parser() {
        let mut body = b"data: ".to_vec();
        body.resize(MAX_MESSAGE_BYTES + 1, b'x');
        for chunk_size in [257, 16 * 1024] {
            let source = futures_util::stream::iter(body.chunks(chunk_size).map(Ok::<_, String>));
            let forwarded_bytes = std::cell::Cell::new(0);
            let bounded = bounded_sse_chunks(source).inspect(|chunk| {
                if let Ok(bytes) = chunk {
                    forwarded_bytes.set(forwarded_bytes.get() + bytes.len());
                }
            });
            let mut parser = Box::pin(bounded.eventsource());

            let error = parser.next().await.unwrap().unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("{MAX_MESSAGE_BYTES}-byte limit"))
            );
            assert_eq!(forwarded_bytes.get(), 0, "no partial line may be reparsed");
            assert!(
                parser.next().await.is_none(),
                "limit errors terminate input"
            );
        }
    }

    #[tokio::test]
    async fn sse_eof_preserves_completed_events_and_discards_unterminated_events() {
        let fixtures: &[&[&[u8]]] = &[
            &[b"data: kept\n\ndata: discarded"],
            &[b"data: kept\r\rdata: discarded"],
            &[b"data: kept\r", b"\n\r", b"\ndata: discarded"],
            &[b"data: kept\n\ndata: discarded\r"],
            &[b"data: kept\r\r"],
            &[b"data: kept\r\n\r\n"],
        ];
        for chunks in fixtures {
            let source = futures_util::stream::iter(chunks.iter().copied().map(Ok::<_, String>));
            let events = bounded_sse_chunks(source)
                .eventsource()
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].data, "kept");
        }
    }

    #[tokio::test]
    async fn sse_buffered_lines_preserve_split_utf8_and_crlf() {
        let chunks: &[&[u8]] = &[b"data: \xf0\x9f", b"\x99\x82\r", b"\n\r", b"\n"];
        let source = futures_util::stream::iter(chunks.iter().copied().map(Ok::<_, String>));
        let events = bounded_sse_chunks(source)
            .eventsource()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "🙂");
    }

    #[tokio::test]
    async fn sse_eof_reports_incomplete_utf8_in_a_buffered_line() {
        let source = futures_util::stream::iter([Ok::<_, String>(b"data: \xe2\x82")]);
        let mut parser = Box::pin(bounded_sse_chunks(source).eventsource());
        let error = parser.next().await.unwrap().unwrap_err();
        assert!(error.to_string().contains("incomplete UTF-8"));
        assert!(parser.next().await.is_none());
    }

    #[tokio::test]
    async fn sse_closes_on_an_oversized_unterminated_line() {
        let mut body = b"data: ".to_vec();
        body.extend(vec![b'x'; 5 * 1024 * 1024]);
        assert_sse_event_limit_closes(body).await;
    }

    #[tokio::test]
    async fn sse_closes_on_many_data_lines_in_one_unterminated_event() {
        let line = format!("data: {}\r\n", "x".repeat(1024));
        let body = line.repeat(5 * 1024).into_bytes();
        assert_sse_event_limit_closes(body).await;
    }

    #[tokio::test]
    async fn sse_times_out_while_waiting_for_response_headers() {
        let (url, server) = local_sse_server(None).await;
        let session = start_server_sent_events(sse_input(url, 100)).unwrap();
        let events = wait_for_session_finish(&session).await;

        assert!(events.iter().any(|event| {
            event.kind == ProtocolEventKind::Error && event.data == "SSE connection timed out."
        }));
        assert!(
            events
                .iter()
                .all(|event| event.kind != ProtocolEventKind::Open)
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn sse_resets_the_event_budget_and_keeps_established_streams_indefinite() {
        let data = "x".repeat(64 * 1024);
        let mut body = Vec::new();
        let event_count = 80;
        for index in 0..event_count {
            let newline = ["\n", "\r", "\r\n"][index % 3];
            body.extend_from_slice(
                format!(
                    "id: {index}{newline}event: tick{newline}retry: 250{newline}data: {data}{newline}{newline}"
                )
                .as_bytes(),
            );
        }
        // Resolve a trailing CR without relying on the peer ending the stream.
        body.extend_from_slice(b": keep-alive\n\n");
        assert!(body.len() > MAX_MESSAGE_BYTES);
        let (url, server) = local_sse_server(Some(body)).await;
        let session = start_server_sent_events(sse_input(url, 500)).unwrap();
        let events = tokio::time::timeout(Duration::from_secs(5), async {
            let mut received = Vec::new();
            loop {
                for event in session.drain_events(100).unwrap() {
                    assert_ne!(event.kind, ProtocolEventKind::Error, "{}", event.data);
                    assert_ne!(event.kind, ProtocolEventKind::Closed);
                    if event.kind == ProtocolEventKind::Event {
                        received.push(event);
                    }
                }
                if received.len() == event_count {
                    break received;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out receiving bounded SSE events");

        for (index, event) in events.iter().enumerate() {
            assert_eq!(event.data, data);
            assert_eq!(event.byte_length, data.len());
            assert_eq!(event.event_name.as_deref(), Some("tick"));
            assert_eq!(event.event_id.as_deref(), Some(index.to_string().as_str()));
            assert_eq!(event.retry_ms, Some(250));
        }
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        assert!(
            !session.is_finished(),
            "established SSE bodies have no deadline"
        );
        session.stop();
        server.await.unwrap();
    }

    fn sse_input(url: String, timeout_ms: u64) -> ServerSentEventsInput {
        ServerSentEventsInput {
            url,
            headers: Vec::new(),
            last_event_id: String::new(),
            options: RequestOptions {
                timeout_ms,
                ..RequestOptions::default()
            },
            context: HttpExecutionContext {
                proxy: ProxyConfiguration::Disabled,
                ..HttpExecutionContext::default()
            },
            redactions: Vec::new(),
        }
    }

    async fn local_sse_server(body: Option<Vec<u8>>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_http_request(&mut socket).await;
            if let Some(body) = body {
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
                for chunk in body.chunks(16 * 1024) {
                    if socket.write_all(chunk).await.is_err() {
                        // An oversized event can close the client during upload.
                        return;
                    }
                }
            }
            // Keep the peer open: success must come from the client's own limit
            // or timeout, never from this fixture sending EOF.
            let mut byte = [0_u8; 1];
            let result = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut byte))
                .await
                .expect("SSE client did not close the TCP connection");
            assert!(matches!(result, Ok(0) | Err(_)));
        });
        (format!("http://{address}/events"), server)
    }

    async fn read_http_request(socket: &mut TcpStream) {
        let mut request = Vec::new();
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let mut chunk = [0_u8; 4096];
            let length = socket.read(&mut chunk).await.unwrap();
            assert_ne!(length, 0, "client disconnected before sending HTTP headers");
            request.extend_from_slice(&chunk[..length]);
            assert!(request.len() <= 16 * 1024);
        }
    }

    async fn wait_for_session_finish(session: &ProtocolSession) -> Vec<ProtocolEvent> {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !session.is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("SSE session stayed connected");
        session.drain_events(100).unwrap()
    }

    async fn assert_sse_event_limit_closes(body: Vec<u8>) {
        let (url, server) = local_sse_server(Some(body)).await;
        let session = start_server_sent_events(sse_input(url, 2_000)).unwrap();
        let events = wait_for_session_finish(&session).await;
        assert_eq!(
            events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            vec![
                ProtocolEventKind::Open,
                ProtocolEventKind::Error,
                ProtocolEventKind::Closed,
            ]
        );
        assert!(
            events[1]
                .data
                .contains(&format!("{MAX_MESSAGE_BYTES}-byte limit"))
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn websocket_uses_native_http_upgrade_and_exchanges_messages() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let message = socket.next().await.unwrap().unwrap();
            let Message::Text(message) = message else {
                panic!("expected a text message");
            };
            socket
                .send(Message::Text(format!("echo:{message}").into()))
                .await
                .unwrap();
        });

        let session = start_websocket(WebSocketInput {
            url: format!("ws://{address}/socket"),
            headers: Vec::new(),
            subprotocols: Vec::new(),
            options: RequestOptions {
                timeout_ms: 2_000,
                ..RequestOptions::default()
            },
            context: HttpExecutionContext::default(),
            max_message_bytes: 64 * 1024,
            redactions: Vec::new(),
        })
        .unwrap();

        wait_for_event(&session, |event| event.kind == ProtocolEventKind::Open).await;
        session
            .send(WebSocketOutbound::Text {
                data: "hello".to_owned(),
            })
            .await
            .unwrap();
        let event = wait_for_event(&session, |event| {
            event.kind == ProtocolEventKind::Text && event.data == "echo:hello"
        })
        .await;
        assert_eq!(event.direction, ProtocolEventDirection::Inbound);
        session.stop();
        server.await.unwrap();
    }

    async fn wait_for_event(
        session: &super::ProtocolSession,
        predicate: impl Fn(&ProtocolEvent) -> bool,
    ) -> ProtocolEvent {
        for _ in 0..100 {
            if let Some(event) = session
                .drain_events(100)
                .unwrap()
                .into_iter()
                .find(&predicate)
            {
                return event;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for WebSocket event");
    }
}
