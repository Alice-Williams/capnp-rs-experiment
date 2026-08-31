//! Bounded JSON-RPC 2.0 mapping and Content-Length transport framing.

use std::collections::BTreeSet;
use std::fmt;

use capnp_json::{JsonCodec, JsonError, JsonLimits, JsonValue};

/// Independent bounds for JSON-RPC state and transport buffering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JsonRpcLimits {
    pub max_pending_calls: usize,
    pub max_method_bytes: usize,
    pub max_frame_bytes: usize,
    pub max_header_bytes: usize,
}

impl Default for JsonRpcLimits {
    fn default() -> Self {
        Self {
            max_pending_calls: 1024,
            max_method_bytes: 4096,
            max_frame_bytes: 16 * 1024 * 1024,
            max_header_bytes: 16 * 1024,
        }
    }
}

/// JSON-RPC request/response correlation ID. Source number spelling is kept.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum JsonRpcId {
    Number(String),
    String(String),
}

impl JsonRpcId {
    fn from_value(value: JsonValue) -> Result<Self, JsonRpcError> {
        match value {
            JsonValue::Number(value) => Ok(Self::Number(value)),
            JsonValue::String(value) => Ok(Self::String(value)),
            _ => Err(JsonRpcError::InvalidId),
        }
    }

    fn into_value(self) -> JsonValue {
        match self {
            Self::Number(value) => JsonValue::Number(value),
            Self::String(value) => JsonValue::String(value),
        }
    }
}

/// Lossless JSON-RPC error payload.
#[derive(Clone, Debug, PartialEq)]
pub struct JsonRpcFailure {
    pub code: i32,
    pub message: String,
    pub data: Option<JsonValue>,
}

/// A validated JSON-RPC message.
#[derive(Clone, Debug, PartialEq)]
pub enum JsonRpcMessage {
    Request {
        id: Option<JsonRpcId>,
        method: String,
        params: JsonValue,
    },
    Response {
        id: JsonRpcId,
        result: Result<JsonValue, JsonRpcFailure>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JsonRpcError {
    Json(JsonError),
    ExpectedObject,
    DuplicateField(&'static str),
    InvalidVersion,
    InvalidId,
    InvalidPayload,
    InvalidError,
    MethodTooLong,
    PendingLimit,
    CallIdOverflow,
    UnknownResponseId(JsonRpcId),
    HeaderTooLarge,
    MissingContentLength,
    DuplicateContentLength,
    InvalidContentLength,
    FrameTooLarge,
    BufferLimit,
    InvalidHeaderUtf8,
    InvalidBodyUtf8,
}

impl fmt::Display for JsonRpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => error.fmt(formatter),
            Self::ExpectedObject => formatter.write_str("JSON-RPC message must be an object"),
            Self::DuplicateField(field) => write!(formatter, "duplicate JSON-RPC field: {field}"),
            Self::InvalidVersion => formatter.write_str("jsonrpc must be exactly \"2.0\""),
            Self::InvalidId => formatter.write_str("JSON-RPC id must be a string or number"),
            Self::InvalidPayload => {
                formatter.write_str("exactly one of params, result, or error is required")
            }
            Self::InvalidError => formatter.write_str("invalid JSON-RPC error object"),
            Self::MethodTooLong => formatter.write_str("JSON-RPC method exceeds configured limit"),
            Self::PendingLimit => formatter.write_str("JSON-RPC pending-call limit reached"),
            Self::CallIdOverflow => formatter.write_str("JSON-RPC numeric call ID exhausted"),
            Self::UnknownResponseId(id) => {
                write!(formatter, "unknown JSON-RPC response id: {id:?}")
            }
            Self::HeaderTooLarge => formatter.write_str("Content-Length header exceeds limit"),
            Self::MissingContentLength => formatter.write_str("missing Content-Length header"),
            Self::DuplicateContentLength => formatter.write_str("duplicate Content-Length header"),
            Self::InvalidContentLength => formatter.write_str("invalid Content-Length header"),
            Self::FrameTooLarge => formatter.write_str("Content-Length body exceeds limit"),
            Self::BufferLimit => formatter.write_str("Content-Length buffered input exceeds limit"),
            Self::InvalidHeaderUtf8 => formatter.write_str("Content-Length header is not UTF-8"),
            Self::InvalidBodyUtf8 => formatter.write_str("JSON-RPC body is not UTF-8"),
        }
    }
}

impl std::error::Error for JsonRpcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<JsonError> for JsonRpcError {
    fn from(value: JsonError) -> Self {
        Self::Json(value)
    }
}

/// Stateless JSON-RPC syntax mapper using the native bounded JSON codec.
#[derive(Clone, Debug)]
pub struct JsonRpcCodec {
    json: JsonCodec,
    limits: JsonRpcLimits,
}

impl JsonRpcCodec {
    pub fn new(limits: JsonRpcLimits) -> Self {
        let mut json = JsonCodec::new();
        json.set_limits(JsonLimits {
            max_input_bytes: limits.max_frame_bytes,
            ..JsonLimits::default()
        });
        Self { json, limits }
    }

    pub fn parse(&self, source: &str) -> Result<JsonRpcMessage, JsonRpcError> {
        if source.len() > self.limits.max_frame_bytes {
            return Err(JsonRpcError::FrameTooLarge);
        }
        let JsonValue::Object(mut fields) = self.json.parse(source)? else {
            return Err(JsonRpcError::ExpectedObject);
        };
        let version = take_unique(&mut fields, "jsonrpc")?;
        if version != Some(JsonValue::String("2.0".to_owned())) {
            return Err(JsonRpcError::InvalidVersion);
        }
        let id = take_unique(&mut fields, "id")?
            .map(JsonRpcId::from_value)
            .transpose()?;
        let method = take_unique(&mut fields, "method")?;
        let params = take_unique(&mut fields, "params")?;
        let result = take_unique(&mut fields, "result")?;
        let error = take_unique(&mut fields, "error")?;
        let payload_count = usize::from(params.is_some())
            + usize::from(result.is_some())
            + usize::from(error.is_some());
        if payload_count != 1 {
            return Err(JsonRpcError::InvalidPayload);
        }

        if let Some(params) = params {
            let Some(JsonValue::String(method)) = method else {
                return Err(JsonRpcError::InvalidPayload);
            };
            if method.len() > self.limits.max_method_bytes {
                return Err(JsonRpcError::MethodTooLong);
            }
            return Ok(JsonRpcMessage::Request { id, method, params });
        }
        if method.is_some() || id.is_none() {
            return Err(JsonRpcError::InvalidPayload);
        }
        let id = id.expect("response ID checked above");
        if let Some(result) = result {
            return Ok(JsonRpcMessage::Response {
                id,
                result: Ok(result),
            });
        }
        Ok(JsonRpcMessage::Response {
            id,
            result: Err(parse_error(error.expect("error payload counted above"))?),
        })
    }

    pub fn format(&self, message: &JsonRpcMessage) -> Result<String, JsonRpcError> {
        let mut fields = vec![("jsonrpc".to_owned(), JsonValue::String("2.0".to_owned()))];
        match message {
            JsonRpcMessage::Request { id, method, params } => {
                if method.len() > self.limits.max_method_bytes {
                    return Err(JsonRpcError::MethodTooLong);
                }
                if let Some(id) = id {
                    fields.push(("id".to_owned(), id.clone().into_value()));
                }
                fields.push(("method".to_owned(), JsonValue::String(method.clone())));
                fields.push(("params".to_owned(), params.clone()));
            }
            JsonRpcMessage::Response { id, result } => {
                fields.push(("id".to_owned(), id.clone().into_value()));
                match result {
                    Ok(value) => fields.push(("result".to_owned(), value.clone())),
                    Err(error) => {
                        let mut error_fields = vec![
                            ("code".to_owned(), JsonValue::Number(error.code.to_string())),
                            (
                                "message".to_owned(),
                                JsonValue::String(error.message.clone()),
                            ),
                        ];
                        if let Some(data) = &error.data {
                            error_fields.push(("data".to_owned(), data.clone()));
                        }
                        fields.push(("error".to_owned(), JsonValue::Object(error_fields)));
                    }
                }
            }
        }
        let output = self.json.format(&JsonValue::Object(fields))?;
        if output.len() > self.limits.max_frame_bytes {
            return Err(JsonRpcError::FrameTooLarge);
        }
        Ok(output)
    }
}

impl Default for JsonRpcCodec {
    fn default() -> Self {
        Self::new(JsonRpcLimits::default())
    }
}

/// Correlation state for independently completing multiple local calls.
///
/// Locally-created calls use monotonically increasing numeric IDs. Incoming
/// string or number IDs are preserved exactly when constructing a reply.
///
/// ```
/// use capnp_compat::{JsonRpcMessage, JsonRpcSession, JsonValue};
///
/// let mut client = JsonRpcSession::default();
/// let server = JsonRpcSession::default();
/// let (id, request) = client.begin_call("sum", JsonValue::Array(vec![
///     JsonValue::Number("2".into()),
///     JsonValue::Number("3".into()),
/// ]))?;
/// assert!(request.contains("\"jsonrpc\":\"2.0\""));
/// let response = server.reply_result(id, JsonValue::Number("5".into()))?;
/// assert!(matches!(
///     client.receive(&response)?,
///     JsonRpcMessage::Response { result: Ok(JsonValue::Number(value)), .. }
///         if value == "5"
/// ));
/// assert_eq!(client.pending_calls(), 0);
/// # Ok::<(), capnp_compat::JsonRpcError>(())
/// ```
#[derive(Clone, Debug)]
pub struct JsonRpcSession {
    codec: JsonRpcCodec,
    next_call_id: u64,
    pending: BTreeSet<JsonRpcId>,
}

impl JsonRpcSession {
    pub fn new(limits: JsonRpcLimits) -> Self {
        Self {
            codec: JsonRpcCodec::new(limits),
            next_call_id: 0,
            pending: BTreeSet::new(),
        }
    }

    pub fn pending_calls(&self) -> usize {
        self.pending.len()
    }

    pub fn begin_call(
        &mut self,
        method: impl Into<String>,
        params: JsonValue,
    ) -> Result<(JsonRpcId, String), JsonRpcError> {
        if self.pending.len() >= self.codec.limits.max_pending_calls {
            return Err(JsonRpcError::PendingLimit);
        }
        let method = method.into();
        if method.len() > self.codec.limits.max_method_bytes {
            return Err(JsonRpcError::MethodTooLong);
        }
        let id = JsonRpcId::Number(self.next_call_id.to_string());
        let next = self
            .next_call_id
            .checked_add(1)
            .ok_or(JsonRpcError::CallIdOverflow)?;
        let message = JsonRpcMessage::Request {
            id: Some(id.clone()),
            method,
            params,
        };
        let text = self.codec.format(&message)?;
        self.pending.insert(id.clone());
        self.next_call_id = next;
        Ok((id, text))
    }

    pub fn notification(
        &self,
        method: impl Into<String>,
        params: JsonValue,
    ) -> Result<String, JsonRpcError> {
        self.codec.format(&JsonRpcMessage::Request {
            id: None,
            method: method.into(),
            params,
        })
    }

    pub fn receive(&mut self, source: &str) -> Result<JsonRpcMessage, JsonRpcError> {
        let message = self.codec.parse(source)?;
        if let JsonRpcMessage::Response { id, .. } = &message {
            if !self.pending.remove(id) {
                return Err(JsonRpcError::UnknownResponseId(id.clone()));
            }
        }
        Ok(message)
    }

    pub fn reply_result(&self, id: JsonRpcId, result: JsonValue) -> Result<String, JsonRpcError> {
        self.codec.format(&JsonRpcMessage::Response {
            id,
            result: Ok(result),
        })
    }

    pub fn reply_error(
        &self,
        id: JsonRpcId,
        failure: JsonRpcFailure,
    ) -> Result<String, JsonRpcError> {
        self.codec.format(&JsonRpcMessage::Response {
            id,
            result: Err(failure),
        })
    }
}

impl Default for JsonRpcSession {
    fn default() -> Self {
        Self::new(JsonRpcLimits::default())
    }
}

/// Incremental VS Code-style `Content-Length` framing.
#[derive(Clone, Debug)]
pub struct ContentLengthCodec {
    limits: JsonRpcLimits,
    buffer: Vec<u8>,
}

impl ContentLengthCodec {
    pub fn new(limits: JsonRpcLimits) -> Self {
        Self {
            limits,
            buffer: Vec::new(),
        }
    }

    pub fn encode(&self, body: &str) -> Result<Vec<u8>, JsonRpcError> {
        if body.len() > self.limits.max_frame_bytes {
            return Err(JsonRpcError::FrameTooLarge);
        }
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let capacity = header
            .len()
            .checked_add(body.len())
            .ok_or(JsonRpcError::BufferLimit)?;
        let mut output = Vec::with_capacity(capacity);
        output.extend_from_slice(header.as_bytes());
        output.extend_from_slice(body.as_bytes());
        Ok(output)
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, JsonRpcError> {
        let maximum = self
            .limits
            .max_header_bytes
            .checked_add(self.limits.max_frame_bytes)
            .and_then(|value| value.checked_add(4))
            .ok_or(JsonRpcError::BufferLimit)?;
        let new_len = self
            .buffer
            .len()
            .checked_add(chunk.len())
            .ok_or(JsonRpcError::BufferLimit)?;
        if new_len > maximum {
            return Err(JsonRpcError::BufferLimit);
        }
        self.buffer.extend_from_slice(chunk);
        let mut frames = Vec::new();
        loop {
            let Some(header_end) = find_header_end(&self.buffer) else {
                if self.buffer.len() > self.limits.max_header_bytes {
                    return Err(JsonRpcError::HeaderTooLarge);
                }
                break;
            };
            if header_end > self.limits.max_header_bytes {
                return Err(JsonRpcError::HeaderTooLarge);
            }
            let content_length = parse_content_length(&self.buffer[..header_end])?;
            if content_length > self.limits.max_frame_bytes {
                return Err(JsonRpcError::FrameTooLarge);
            }
            let body_start = header_end.checked_add(4).ok_or(JsonRpcError::BufferLimit)?;
            let frame_end = body_start
                .checked_add(content_length)
                .ok_or(JsonRpcError::BufferLimit)?;
            if self.buffer.len() < frame_end {
                break;
            }
            let body = std::str::from_utf8(&self.buffer[body_start..frame_end])
                .map_err(|_| JsonRpcError::InvalidBodyUtf8)?
                .to_owned();
            self.buffer.drain(..frame_end);
            frames.push(body);
        }
        Ok(frames)
    }

    pub fn buffered_bytes(&self) -> usize {
        self.buffer.len()
    }
}

impl Default for ContentLengthCodec {
    fn default() -> Self {
        Self::new(JsonRpcLimits::default())
    }
}

fn take_unique(
    fields: &mut Vec<(String, JsonValue)>,
    name: &'static str,
) -> Result<Option<JsonValue>, JsonRpcError> {
    let mut found = None;
    let mut index = 0;
    while index < fields.len() {
        if fields[index].0 == name {
            if found.is_some() {
                return Err(JsonRpcError::DuplicateField(name));
            }
            found = Some(fields.remove(index).1);
        } else {
            index += 1;
        }
    }
    Ok(found)
}

fn parse_error(value: JsonValue) -> Result<JsonRpcFailure, JsonRpcError> {
    let JsonValue::Object(mut fields) = value else {
        return Err(JsonRpcError::InvalidError);
    };
    let code = match take_unique(&mut fields, "code")? {
        Some(JsonValue::Number(value)) => value
            .parse::<i32>()
            .map_err(|_| JsonRpcError::InvalidError)?,
        _ => return Err(JsonRpcError::InvalidError),
    };
    let message = match take_unique(&mut fields, "message")? {
        Some(JsonValue::String(value)) => value,
        _ => return Err(JsonRpcError::InvalidError),
    };
    let data = take_unique(&mut fields, "data")?;
    Ok(JsonRpcFailure {
        code,
        message,
        data,
    })
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(header: &[u8]) -> Result<usize, JsonRpcError> {
    let header = std::str::from_utf8(header).map_err(|_| JsonRpcError::InvalidHeaderUtf8)?;
    let mut found = None;
    for line in header.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            return Err(JsonRpcError::InvalidContentLength);
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            if found.is_some() {
                return Err(JsonRpcError::DuplicateContentLength);
            }
            let value = value.trim();
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(JsonRpcError::InvalidContentLength);
            }
            found = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| JsonRpcError::InvalidContentLength)?,
            );
        }
    }
    found.ok_or(JsonRpcError::MissingContentLength)
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    fn object(fields: &[(&str, JsonValue)]) -> JsonValue {
        JsonValue::Object(
            fields
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect(),
        )
    }

    #[test]
    fn basic_call_and_string_id_round_trip() {
        let mut client = JsonRpcSession::default();
        let server = JsonRpcSession::default();
        let params = object(&[
            ("i", JsonValue::Number("123".to_owned())),
            ("j", JsonValue::Bool(true)),
        ]);
        let (id, request) = client
            .begin_call("foo", params.clone())
            .expect("begin call");
        assert_eq!(id, JsonRpcId::Number("0".to_owned()));
        assert_eq!(
            server.codec.parse(&request).expect("parse request"),
            JsonRpcMessage::Request {
                id: Some(id.clone()),
                method: "foo".to_owned(),
                params,
            }
        );
        let response = server
            .reply_result(
                id.clone(),
                object(&[("x", JsonValue::String("foo".into()))]),
            )
            .expect("reply");
        assert!(matches!(
            client.receive(&response),
            Ok(JsonRpcMessage::Response { id: received, result: Ok(_) }) if received == id
        ));
        assert_eq!(client.pending_calls(), 0);

        let inbound = r#"{"jsonrpc":"2.0","id":"client-7","method":"foo","params":{}}"#;
        let JsonRpcMessage::Request { id: Some(id), .. } =
            server.codec.parse(inbound).expect("string ID")
        else {
            panic!("expected request");
        };
        assert_eq!(id, JsonRpcId::String("client-7".to_owned()));
        let reflected = server
            .reply_result(id, JsonValue::Null)
            .expect("reflect ID");
        assert!(reflected.contains("\"id\":\"client-7\""));
    }

    #[test]
    fn error_code_message_and_optional_data_are_lossless() {
        let mut client = JsonRpcSession::default();
        let server = JsonRpcSession::default();
        let (id, _) = client
            .begin_call("bar", JsonValue::Object(Vec::new()))
            .expect("call");
        let failure = JsonRpcFailure {
            code: -32601,
            message: "Method not implemented".to_owned(),
            data: Some(object(&[("method", JsonValue::String("bar".to_owned()))])),
        };
        let response = server
            .reply_error(id, failure.clone())
            .expect("error response");
        assert!(matches!(
            client.receive(&response),
            Ok(JsonRpcMessage::Response { result: Err(received), .. }) if received == failure
        ));
    }

    #[test]
    fn multiple_calls_complete_independently_and_notifications_do_not_wait() {
        let mut client = JsonRpcSession::default();
        let server = JsonRpcSession::default();
        let (first, _) = client.begin_call("foo", JsonValue::Null).expect("first");
        let (second, _) = client
            .begin_call("baz", JsonValue::Array(Vec::new()))
            .expect("second");
        assert_eq!(client.pending_calls(), 2);
        let notification = client
            .notification("tick", JsonValue::Null)
            .expect("notification");
        assert!(matches!(
            server.codec.parse(&notification),
            Ok(JsonRpcMessage::Request { id: None, .. })
        ));
        assert_eq!(client.pending_calls(), 2);

        let second_response = server
            .reply_result(second, JsonValue::Bool(true))
            .expect("second response");
        client.receive(&second_response).expect("finish second");
        assert_eq!(client.pending_calls(), 1);
        let first_response = server
            .reply_result(first, JsonValue::String("foo".to_owned()))
            .expect("first response");
        client.receive(&first_response).expect("finish first");
        assert_eq!(client.pending_calls(), 0);
    }

    #[test]
    fn invalid_messages_and_unknown_responses_fail_closed() {
        let codec = JsonRpcCodec::default();
        assert!(matches!(
            codec.parse(r#"{"jsonrpc":"1.0","id":1,"result":null}"#),
            Err(JsonRpcError::InvalidVersion)
        ));
        assert!(matches!(
            codec.parse(r#"{"jsonrpc":"2.0","id":null,"result":null}"#),
            Err(JsonRpcError::InvalidId)
        ));
        assert!(matches!(
            codec.parse(r#"{"jsonrpc":"2.0","id":1,"result":null,"error":{}}"#),
            Err(JsonRpcError::InvalidPayload)
        ));
        assert!(matches!(
            codec.parse(r#"{"jsonrpc":"2.0","jsonrpc":"2.0","id":1,"result":null}"#),
            Err(JsonRpcError::DuplicateField("jsonrpc"))
        ));
        let mut session = JsonRpcSession::default();
        assert!(matches!(
            session.receive(r#"{"jsonrpc":"2.0","id":9,"result":null}"#),
            Err(JsonRpcError::UnknownResponseId(JsonRpcId::Number(id))) if id == "9"
        ));
    }

    #[test]
    fn pending_limit_is_complete_or_unchanged() {
        let mut session = JsonRpcSession::new(JsonRpcLimits {
            max_pending_calls: 1,
            ..JsonRpcLimits::default()
        });
        session.begin_call("first", JsonValue::Null).expect("first");
        assert!(matches!(
            session.begin_call("second", JsonValue::Null),
            Err(JsonRpcError::PendingLimit)
        ));
        assert_eq!(session.pending_calls(), 1);
    }

    #[test]
    fn content_length_handles_partial_and_multiple_frames() {
        let mut framing = ContentLengthCodec::default();
        let first = framing.encode("{\"one\":1}").expect("first frame");
        let second = framing.encode("[]").expect("second frame");
        assert!(framing.push(&first[..8]).expect("prefix").is_empty());
        let mut rest = first[8..].to_vec();
        rest.extend_from_slice(&second);
        assert_eq!(
            framing.push(&rest).expect("remaining frames"),
            ["{\"one\":1}".to_owned(), "[]".to_owned()]
        );
        assert_eq!(framing.buffered_bytes(), 0);
    }

    #[test]
    fn content_length_rejects_duplicate_invalid_and_oversize_headers() {
        let mut framing = ContentLengthCodec::new(JsonRpcLimits {
            max_frame_bytes: 4,
            max_header_bytes: 64,
            ..JsonRpcLimits::default()
        });
        assert!(matches!(
            framing.push(b"Content-Length: 5\r\n\r\nhello"),
            Err(JsonRpcError::FrameTooLarge)
        ));
        let mut framing = ContentLengthCodec::default();
        assert!(matches!(
            framing.push(b"Content-Length: 1\r\nContent-Length: 1\r\n\r\nx"),
            Err(JsonRpcError::DuplicateContentLength)
        ));
        let mut framing = ContentLengthCodec::default();
        assert!(matches!(
            framing.push(b"Content-Length: -1\r\n\r\n"),
            Err(JsonRpcError::InvalidContentLength)
        ));
    }
}
