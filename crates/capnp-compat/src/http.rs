//! HTTP-over-Cap'n-Proto metadata and lifecycle state machines.

use std::fmt;
use std::sync::Arc;

use crate::{WebSocketLimits, WebSocketSession};

macro_rules! http_methods {
    ($(($variant:ident, $ordinal:literal, $text:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum HttpMethod { $($variant),+ }

        impl HttpMethod {
            pub fn ordinal(self) -> u16 {
                match self { $(Self::$variant => $ordinal),+ }
            }

            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }

            pub fn from_ordinal(ordinal: u16) -> Result<Self, HttpError> {
                match ordinal {
                    $($ordinal => Ok(Self::$variant),)+
                    _ => Err(HttpError::InvalidMethod(ordinal)),
                }
            }
        }
    };
}

http_methods!(
    (Get, 0, "GET"),
    (Head, 1, "HEAD"),
    (Post, 2, "POST"),
    (Put, 3, "PUT"),
    (Delete, 4, "DELETE"),
    (Patch, 5, "PATCH"),
    (Purge, 6, "PURGE"),
    (Options, 7, "OPTIONS"),
    (Trace, 8, "TRACE"),
    (Copy, 9, "COPY"),
    (Lock, 10, "LOCK"),
    (Mkcol, 11, "MKCOL"),
    (Move, 12, "MOVE"),
    (Propfind, 13, "PROPFIND"),
    (Proppatch, 14, "PROPPATCH"),
    (Search, 15, "SEARCH"),
    (Unlock, 16, "UNLOCK"),
    (Acl, 17, "ACL"),
    (Report, 18, "REPORT"),
    (Mkactivity, 19, "MKACTIVITY"),
    (Checkout, 20, "CHECKOUT"),
    (Merge, 21, "MERGE"),
    (Msearch, 22, "M-SEARCH"),
    (Notify, 23, "NOTIFY"),
    (Subscribe, 24, "SUBSCRIBE"),
    (Unsubscribe, 25, "UNSUBSCRIBE"),
    (Query, 26, "QUERY"),
    (Ban, 27, "BAN"),
);

macro_rules! common_headers {
    ($(($variant:ident, $ordinal:literal, $text:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum CommonHeaderName { $($variant),+ }

        impl CommonHeaderName {
            pub fn ordinal(self) -> u16 {
                match self { $(Self::$variant => $ordinal),+ }
            }

            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }

            pub fn from_ordinal(ordinal: u16) -> Result<Self, HttpError> {
                match ordinal {
                    $($ordinal => Ok(Self::$variant),)+
                    _ => Err(HttpError::InvalidCommonHeaderName(ordinal)),
                }
            }
        }
    };
}

common_headers!(
    (AcceptCharset, 1, "Accept-Charset"),
    (AcceptEncoding, 2, "Accept-Encoding"),
    (AcceptLanguage, 3, "Accept-Language"),
    (AcceptRanges, 4, "Accept-Ranges"),
    (Accept, 5, "Accept"),
    (AccessControlAllowOrigin, 6, "Access-Control-Allow-Origin"),
    (Age, 7, "Age"),
    (Allow, 8, "Allow"),
    (Authorization, 9, "Authorization"),
    (CacheControl, 10, "Cache-Control"),
    (ContentDisposition, 11, "Content-Disposition"),
    (ContentEncoding, 12, "Content-Encoding"),
    (ContentLanguage, 13, "Content-Language"),
    (ContentLength, 14, "Content-Length"),
    (ContentLocation, 15, "Content-Location"),
    (ContentRange, 16, "Content-Range"),
    (ContentType, 17, "Content-Type"),
    (Cookie, 18, "Cookie"),
    (Date, 19, "Date"),
    (Etag, 20, "ETag"),
    (Expect, 21, "Expect"),
    (Expires, 22, "Expires"),
    (From, 23, "From"),
    (Host, 24, "Host"),
    (IfMatch, 25, "If-Match"),
    (IfModifiedSince, 26, "If-Modified-Since"),
    (IfNoneMatch, 27, "If-None-Match"),
    (IfRange, 28, "If-Range"),
    (IfUnmodifiedSince, 29, "If-Unmodified-Since"),
    (LastModified, 30, "Last-Modified"),
    (Link, 31, "Link"),
    (Location, 32, "Location"),
    (MaxForwards, 33, "Max-Forwards"),
    (ProxyAuthenticate, 34, "Proxy-Authenticate"),
    (ProxyAuthorization, 35, "Proxy-Authorization"),
    (Range, 36, "Range"),
    (Referer, 37, "Referer"),
    (Refresh, 38, "Refresh"),
    (RetryAfter, 39, "Retry-After"),
    (Server, 40, "Server"),
    (SetCookie, 41, "Set-Cookie"),
    (StrictTransportSecurity, 42, "Strict-Transport-Security"),
    (TransferEncoding, 43, "Transfer-Encoding"),
    (UserAgent, 44, "User-Agent"),
    (Vary, 45, "Vary"),
    (Via, 46, "Via"),
    (WwwAuthenticate, 47, "WWW-Authenticate"),
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommonHeaderValue {
    GzipDeflate,
}

impl CommonHeaderValue {
    pub fn ordinal(self) -> u16 {
        match self {
            Self::GzipDeflate => 1,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::GzipDeflate => "gzip, deflate",
        }
    }

    pub fn from_ordinal(ordinal: u16) -> Result<Self, HttpError> {
        match ordinal {
            1 => Ok(Self::GzipDeflate),
            _ => Err(HttpError::InvalidCommonHeaderValue(ordinal)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HttpHeaderValue {
    Common(CommonHeaderValue),
    Text(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HttpHeader {
    Common {
        name: CommonHeaderName,
        value: HttpHeaderValue,
    },
    Uncommon {
        name: String,
        value: String,
    },
}

impl HttpHeader {
    pub fn name(&self) -> &str {
        match self {
            Self::Common { name, .. } => name.as_str(),
            Self::Uncommon { name, .. } => name,
        }
    }

    pub fn value(&self) -> &str {
        match self {
            Self::Common {
                value: HttpHeaderValue::Common(value),
                ..
            } => value.as_str(),
            Self::Common {
                value: HttpHeaderValue::Text(value),
                ..
            }
            | Self::Uncommon { value, .. } => value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodySize {
    Unknown,
    Fixed(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<HttpHeader>,
    pub body_size: BodySize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpResponse {
    pub status_code: u16,
    pub status_text: String,
    pub headers: Vec<HttpHeader>,
    pub body_size: BodySize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HttpLimits {
    pub max_headers: usize,
    pub max_header_name_bytes: usize,
    pub max_header_value_bytes: usize,
    pub max_url_bytes: usize,
    pub max_host_bytes: usize,
    pub max_body_bytes: u64,
    pub websocket: WebSocketLimits,
}

impl Default for HttpLimits {
    fn default() -> Self {
        Self {
            max_headers: 256,
            max_header_name_bytes: 8192,
            max_header_value_bytes: 64 * 1024,
            max_url_bytes: 64 * 1024,
            max_host_bytes: 8192,
            max_body_bytes: 1 << 34,
            websocket: WebSocketLimits::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpBodyState {
    Open,
    Ended,
    Canceled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpBody {
    expected: Option<u64>,
    received: u64,
    maximum: u64,
    state: HttpBodyState,
}

impl HttpBody {
    pub fn new(size: BodySize, maximum: u64) -> Result<Option<Self>, HttpError> {
        let expected = match size {
            BodySize::Unknown => None,
            BodySize::Fixed(0) => return Ok(None),
            BodySize::Fixed(size) => Some(size),
        };
        if expected.is_some_and(|size| size > maximum) {
            return Err(HttpError::BodyTooLarge);
        }
        Ok(Some(Self {
            expected,
            received: 0,
            maximum,
            state: HttpBodyState::Open,
        }))
    }

    pub fn state(&self) -> HttpBodyState {
        self.state
    }

    pub fn received_bytes(&self) -> u64 {
        self.received
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<(), HttpError> {
        self.require_open()?;
        let amount = u64::try_from(bytes.len()).map_err(|_| HttpError::CountOverflow)?;
        let received = self
            .received
            .checked_add(amount)
            .ok_or(HttpError::CountOverflow)?;
        if received > self.maximum || self.expected.is_some_and(|expected| received > expected) {
            return Err(HttpError::BodyTooLarge);
        }
        self.received = received;
        Ok(())
    }

    pub fn end(&mut self) -> Result<(), HttpError> {
        self.require_open()?;
        if self
            .expected
            .is_some_and(|expected| expected != self.received)
        {
            return Err(HttpError::BodyLengthMismatch {
                expected: self.expected.expect("checked fixed size"),
                actual: self.received,
            });
        }
        self.state = HttpBodyState::Ended;
        Ok(())
    }

    pub fn cancel(&mut self) {
        if self.state == HttpBodyState::Open {
            self.state = HttpBodyState::Canceled;
        }
    }

    fn require_open(&self) -> Result<(), HttpError> {
        if self.state == HttpBodyState::Open {
            Ok(())
        } else {
            Err(HttpError::BodyNotOpen(self.state))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpExchangeState {
    Open,
    Responding,
    WebSocket,
    Complete,
    Canceled,
}

/// One request lifecycle. The service `Arc` remains retained until this value
/// is completed, canceled, or dropped.
///
/// ```
/// use capnp_compat::{
///     BodySize, HttpExchange, HttpLimits, HttpMethod, HttpRequest, HttpResponse,
/// };
/// use std::sync::Arc;
///
/// let request = HttpRequest {
///     method: HttpMethod::Post,
///     url: "/items".into(),
///     headers: Vec::new(),
///     body_size: BodySize::Fixed(3),
/// };
/// let mut exchange = HttpExchange::new(Arc::new("service"), request, HttpLimits::default())?;
/// exchange.request_body_mut().unwrap().write(b"new")?;
/// exchange.request_body_mut().unwrap().end()?;
/// let has_body = exchange.start_response(HttpResponse {
///     status_code: 200,
///     status_text: "OK".into(),
///     headers: Vec::new(),
///     body_size: BodySize::Fixed(2),
/// })?;
/// assert!(has_body);
/// exchange.response_body_mut().unwrap().write(b"ok")?;
/// exchange.response_body_mut().unwrap().end()?;
/// exchange.complete()?;
/// assert!(exchange.service().is_none());
/// # Ok::<(), capnp_compat::HttpError>(())
/// ```
#[derive(Debug)]
pub struct HttpExchange<S> {
    service: Option<Arc<S>>,
    limits: HttpLimits,
    request: HttpRequest,
    request_body: Option<HttpBody>,
    response: Option<HttpResponse>,
    response_body: Option<HttpBody>,
    websocket: Option<WebSocketSession>,
    websocket_headers: Option<Vec<HttpHeader>>,
    state: HttpExchangeState,
}

impl<S> HttpExchange<S> {
    pub fn new(
        service: Arc<S>,
        request: HttpRequest,
        limits: HttpLimits,
    ) -> Result<Self, HttpError> {
        validate_request(&request, limits)?;
        let request_body = HttpBody::new(request.body_size, limits.max_body_bytes)?;
        Ok(Self {
            service: Some(service),
            limits,
            request,
            request_body,
            response: None,
            response_body: None,
            websocket: None,
            websocket_headers: None,
            state: HttpExchangeState::Open,
        })
    }

    pub fn service(&self) -> Option<&S> {
        self.service.as_deref()
    }

    pub fn request(&self) -> &HttpRequest {
        &self.request
    }

    pub fn state(&self) -> HttpExchangeState {
        self.state
    }

    /// Borrows the pipelined request body exclusively for one operation.
    ///
    /// The body borrow cannot alias a response state transition.
    ///
    /// ```compile_fail,E0499
    /// use capnp_compat::{BodySize, HttpExchange, HttpLimits, HttpMethod, HttpRequest, HttpResponse};
    /// use std::sync::Arc;
    /// let mut exchange = HttpExchange::new(Arc::new(()), HttpRequest {
    ///     method: HttpMethod::Post,
    ///     url: "/".into(),
    ///     headers: Vec::new(),
    ///     body_size: BodySize::Unknown,
    /// }, HttpLimits::default()).unwrap();
    /// let body = exchange.request_body_mut().unwrap();
    /// exchange.start_response(HttpResponse {
    ///     status_code: 200,
    ///     status_text: "OK".into(),
    ///     headers: Vec::new(),
    ///     body_size: BodySize::Fixed(0),
    /// }).unwrap();
    /// body.write(b"cannot alias").unwrap();
    /// ```
    pub fn request_body_mut(&mut self) -> Option<&mut HttpBody> {
        self.request_body.as_mut()
    }

    pub fn response(&self) -> Option<&HttpResponse> {
        self.response.as_ref()
    }

    pub fn start_response(&mut self, response: HttpResponse) -> Result<bool, HttpError> {
        self.require_open()?;
        validate_response(&response, self.limits)?;
        let body_allowed = response_body_allowed(self.request.method, response.status_code);
        let response_body = if body_allowed {
            HttpBody::new(response.body_size, self.limits.max_body_bytes)?
        } else {
            None
        };
        self.response = Some(response);
        self.response_body = response_body;
        self.state = HttpExchangeState::Responding;
        Ok(self.response_body.is_some())
    }

    pub fn start_websocket(&mut self, headers: Vec<HttpHeader>) -> Result<(), HttpError> {
        self.require_open()?;
        validate_headers(&headers, self.limits)?;
        self.websocket_headers = Some(headers);
        self.websocket = Some(WebSocketSession::new(self.limits.websocket));
        self.state = HttpExchangeState::WebSocket;
        Ok(())
    }

    pub fn response_body_mut(&mut self) -> Option<&mut HttpBody> {
        self.response_body.as_mut()
    }

    pub fn websocket_mut(&mut self) -> Option<&mut WebSocketSession> {
        self.websocket.as_mut()
    }

    pub fn websocket_headers(&self) -> Option<&[HttpHeader]> {
        self.websocket_headers.as_deref()
    }

    pub fn complete(&mut self) -> Result<(), HttpError> {
        match self.state {
            HttpExchangeState::Responding => {
                if self
                    .response_body
                    .as_ref()
                    .is_some_and(|body| body.state() == HttpBodyState::Open)
                {
                    return Err(HttpError::BodyStillOpen);
                }
            }
            HttpExchangeState::WebSocket => {
                if self
                    .websocket
                    .as_ref()
                    .is_some_and(|socket| socket.state() == crate::WebSocketState::Open)
                {
                    return Err(HttpError::WebSocketStillOpen);
                }
            }
            _ => return Err(HttpError::InvalidExchangeState(self.state)),
        }
        if let Some(body) = &mut self.request_body {
            body.cancel();
        }
        self.service = None;
        self.state = HttpExchangeState::Complete;
        Ok(())
    }

    pub fn cancel(&mut self) {
        if let Some(body) = &mut self.request_body {
            body.cancel();
        }
        if let Some(body) = &mut self.response_body {
            body.cancel();
        }
        if let Some(websocket) = &mut self.websocket {
            websocket.abort();
        }
        self.service = None;
        self.state = HttpExchangeState::Canceled;
    }

    fn require_open(&self) -> Result<(), HttpError> {
        if self.state == HttpExchangeState::Open {
            Ok(())
        } else {
            Err(HttpError::InvalidExchangeState(self.state))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectSettings {
    pub use_tls: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectState {
    Waiting,
    Accepted,
    Rejected,
    Complete,
    Canceled,
}

#[derive(Debug)]
pub struct ConnectExchange<S> {
    service: Option<Arc<S>>,
    limits: HttpLimits,
    host: String,
    headers: Vec<HttpHeader>,
    settings: ConnectSettings,
    state: ConnectState,
    response: Option<HttpResponse>,
    rejection_body: Option<HttpBody>,
    tls_hostname: Option<String>,
}

impl<S> ConnectExchange<S> {
    pub fn new(
        service: Arc<S>,
        host: impl Into<String>,
        headers: Vec<HttpHeader>,
        settings: ConnectSettings,
        limits: HttpLimits,
    ) -> Result<Self, HttpError> {
        let host = host.into();
        if host.len() > limits.max_host_bytes {
            return Err(HttpError::HostTooLong);
        }
        validate_headers(&headers, limits)?;
        Ok(Self {
            service: Some(service),
            limits,
            host,
            headers,
            settings,
            state: ConnectState::Waiting,
            response: None,
            rejection_body: None,
            tls_hostname: None,
        })
    }

    pub fn service(&self) -> Option<&S> {
        self.service.as_deref()
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn headers(&self) -> &[HttpHeader] {
        &self.headers
    }

    pub fn settings(&self) -> ConnectSettings {
        self.settings
    }

    pub fn state(&self) -> ConnectState {
        self.state
    }

    pub fn response(&self) -> Option<&HttpResponse> {
        self.response.as_ref()
    }

    pub fn accept(&mut self, response: HttpResponse) -> Result<(), HttpError> {
        self.require_waiting()?;
        validate_response(&response, self.limits)?;
        if !(200..300).contains(&response.status_code) {
            return Err(HttpError::InvalidConnectStatus(response.status_code));
        }
        self.response = Some(response);
        self.state = ConnectState::Accepted;
        Ok(())
    }

    pub fn reject(&mut self, response: HttpResponse) -> Result<bool, HttpError> {
        self.require_waiting()?;
        validate_response(&response, self.limits)?;
        if (200..300).contains(&response.status_code) {
            return Err(HttpError::InvalidConnectStatus(response.status_code));
        }
        self.rejection_body = HttpBody::new(response.body_size, self.limits.max_body_bytes)?;
        self.response = Some(response);
        self.state = ConnectState::Rejected;
        Ok(self.rejection_body.is_some())
    }

    pub fn rejection_body_mut(&mut self) -> Option<&mut HttpBody> {
        self.rejection_body.as_mut()
    }

    pub fn start_tls(&mut self, hostname: impl Into<String>) -> Result<(), HttpError> {
        if self.state != ConnectState::Accepted {
            return Err(HttpError::InvalidConnectState(self.state));
        }
        let hostname = hostname.into();
        if hostname.len() > self.limits.max_host_bytes {
            return Err(HttpError::HostTooLong);
        }
        self.tls_hostname = Some(hostname);
        Ok(())
    }

    pub fn tls_hostname(&self) -> Option<&str> {
        self.tls_hostname.as_deref()
    }

    pub fn complete(&mut self) -> Result<(), HttpError> {
        match self.state {
            ConnectState::Accepted => {}
            ConnectState::Rejected => {
                if self
                    .rejection_body
                    .as_ref()
                    .is_some_and(|body| body.state() == HttpBodyState::Open)
                {
                    return Err(HttpError::BodyStillOpen);
                }
            }
            _ => return Err(HttpError::InvalidConnectState(self.state)),
        }
        self.service = None;
        self.state = ConnectState::Complete;
        Ok(())
    }

    pub fn cancel(&mut self) {
        if let Some(body) = &mut self.rejection_body {
            body.cancel();
        }
        self.service = None;
        self.state = ConnectState::Canceled;
    }

    fn require_waiting(&self) -> Result<(), HttpError> {
        if self.state == ConnectState::Waiting {
            Ok(())
        } else {
            Err(HttpError::InvalidConnectState(self.state))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HttpError {
    InvalidMethod(u16),
    InvalidCommonHeaderName(u16),
    InvalidCommonHeaderValue(u16),
    TooManyHeaders,
    HeaderNameTooLong,
    HeaderValueTooLong,
    InvalidHeaderName,
    InvalidHeaderValue,
    UrlTooLong,
    HostTooLong,
    InvalidStatus(u16),
    BodyTooLarge,
    CountOverflow,
    BodyLengthMismatch { expected: u64, actual: u64 },
    BodyNotOpen(HttpBodyState),
    BodyStillOpen,
    WebSocketStillOpen,
    InvalidExchangeState(HttpExchangeState),
    InvalidConnectState(ConnectState),
    InvalidConnectStatus(u16),
}

impl fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMethod(value) => write!(formatter, "invalid HTTP method ordinal: {value}"),
            Self::InvalidCommonHeaderName(value) => {
                write!(formatter, "invalid common header-name ordinal: {value}")
            }
            Self::InvalidCommonHeaderValue(value) => {
                write!(formatter, "invalid common header-value ordinal: {value}")
            }
            Self::TooManyHeaders => formatter.write_str("HTTP header count exceeds limit"),
            Self::HeaderNameTooLong => formatter.write_str("HTTP header name exceeds limit"),
            Self::HeaderValueTooLong => formatter.write_str("HTTP header value exceeds limit"),
            Self::InvalidHeaderName => formatter.write_str("invalid HTTP header name"),
            Self::InvalidHeaderValue => formatter.write_str("invalid HTTP header value"),
            Self::UrlTooLong => formatter.write_str("HTTP URL exceeds limit"),
            Self::HostTooLong => formatter.write_str("CONNECT host exceeds limit"),
            Self::InvalidStatus(value) => write!(formatter, "invalid HTTP status: {value}"),
            Self::BodyTooLarge => {
                formatter.write_str("HTTP body exceeds declared or configured size")
            }
            Self::CountOverflow => formatter.write_str("HTTP byte count overflow"),
            Self::BodyLengthMismatch { expected, actual } => {
                write!(
                    formatter,
                    "HTTP body length mismatch: expected {expected}, got {actual}"
                )
            }
            Self::BodyNotOpen(state) => write!(formatter, "HTTP body is not open ({state:?})"),
            Self::BodyStillOpen => formatter.write_str("HTTP body is still open"),
            Self::WebSocketStillOpen => formatter.write_str("WebSocket is still open"),
            Self::InvalidExchangeState(state) => {
                write!(formatter, "invalid HTTP exchange state: {state:?}")
            }
            Self::InvalidConnectState(state) => {
                write!(formatter, "invalid CONNECT state: {state:?}")
            }
            Self::InvalidConnectStatus(value) => {
                write!(formatter, "invalid CONNECT status: {value}")
            }
        }
    }
}

impl std::error::Error for HttpError {}

fn validate_request(request: &HttpRequest, limits: HttpLimits) -> Result<(), HttpError> {
    if request.url.len() > limits.max_url_bytes {
        return Err(HttpError::UrlTooLong);
    }
    validate_headers(&request.headers, limits)?;
    validate_body_size(request.body_size, limits)
}

fn validate_response(response: &HttpResponse, limits: HttpLimits) -> Result<(), HttpError> {
    if !(100..=999).contains(&response.status_code) {
        return Err(HttpError::InvalidStatus(response.status_code));
    }
    validate_headers(&response.headers, limits)?;
    validate_body_size(response.body_size, limits)
}

fn validate_body_size(size: BodySize, limits: HttpLimits) -> Result<(), HttpError> {
    if matches!(size, BodySize::Fixed(value) if value > limits.max_body_bytes) {
        Err(HttpError::BodyTooLarge)
    } else {
        Ok(())
    }
}

fn validate_headers(headers: &[HttpHeader], limits: HttpLimits) -> Result<(), HttpError> {
    if headers.len() > limits.max_headers {
        return Err(HttpError::TooManyHeaders);
    }
    for header in headers {
        let name = header.name();
        let value = header.value();
        if name.len() > limits.max_header_name_bytes {
            return Err(HttpError::HeaderNameTooLong);
        }
        if value.len() > limits.max_header_value_bytes {
            return Err(HttpError::HeaderValueTooLong);
        }
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(HttpError::InvalidHeaderName);
        }
        if value.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
            return Err(HttpError::InvalidHeaderValue);
        }
    }
    Ok(())
}

fn response_body_allowed(method: HttpMethod, status: u16) -> bool {
    method != HttpMethod::Head && !matches!(status, 204 | 205 | 304)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn request(method: HttpMethod, size: BodySize) -> HttpRequest {
        HttpRequest {
            method,
            url: "/resource".to_owned(),
            headers: vec![HttpHeader::Common {
                name: CommonHeaderName::AcceptEncoding,
                value: HttpHeaderValue::Common(CommonHeaderValue::GzipDeflate),
            }],
            body_size: size,
        }
    }

    fn response(status: u16, size: BodySize) -> HttpResponse {
        HttpResponse {
            status_code: status,
            status_text: String::new(),
            headers: vec![HttpHeader::Uncommon {
                name: "X-Test".to_owned(),
                value: "yes".to_owned(),
            }],
            body_size: size,
        }
    }

    #[test]
    fn all_28_method_ordinals_match_the_pinned_enum() {
        let names = [
            "GET",
            "HEAD",
            "POST",
            "PUT",
            "DELETE",
            "PATCH",
            "PURGE",
            "OPTIONS",
            "TRACE",
            "COPY",
            "LOCK",
            "MKCOL",
            "MOVE",
            "PROPFIND",
            "PROPPATCH",
            "SEARCH",
            "UNLOCK",
            "ACL",
            "REPORT",
            "MKACTIVITY",
            "CHECKOUT",
            "MERGE",
            "M-SEARCH",
            "NOTIFY",
            "SUBSCRIBE",
            "UNSUBSCRIBE",
            "QUERY",
            "BAN",
        ];
        for (ordinal, name) in names.iter().enumerate() {
            let method = HttpMethod::from_ordinal(u16::try_from(ordinal).expect("ordinal"))
                .expect("known method");
            assert_eq!(method.ordinal(), ordinal as u16);
            assert_eq!(method.as_str(), *name);
        }
        assert!(matches!(
            HttpMethod::from_ordinal(28),
            Err(HttpError::InvalidMethod(28))
        ));
    }

    #[test]
    fn invalid_common_wire_values_are_rejected() {
        assert!(matches!(
            CommonHeaderName::from_ordinal(0),
            Err(HttpError::InvalidCommonHeaderName(0))
        ));
        assert!(matches!(
            CommonHeaderName::from_ordinal(48),
            Err(HttpError::InvalidCommonHeaderName(48))
        ));
        assert!(matches!(
            CommonHeaderValue::from_ordinal(0),
            Err(HttpError::InvalidCommonHeaderValue(0))
        ));
        assert_eq!(
            CommonHeaderValue::from_ordinal(1)
                .expect("known value")
                .as_str(),
            "gzip, deflate"
        );
    }

    #[test]
    fn fixed_bodies_are_exact_and_limit_failures_do_not_advance() {
        let mut body = HttpBody::new(BodySize::Fixed(5), 10)
            .expect("body")
            .expect("non-zero body");
        body.write(b"test").expect("prefix");
        assert!(matches!(body.write(b"xx"), Err(HttpError::BodyTooLarge)));
        assert_eq!(body.received_bytes(), 4);
        assert!(matches!(
            body.end(),
            Err(HttpError::BodyLengthMismatch {
                expected: 5,
                actual: 4
            })
        ));
        body.write(b"!").expect("last byte");
        body.end().expect("exact end");
    }

    #[test]
    fn request_pipeline_response_and_205_suppression_are_distinct() {
        let service = Arc::new(());
        let mut exchange = HttpExchange::new(
            service,
            request(HttpMethod::Post, BodySize::Fixed(3)),
            HttpLimits::default(),
        )
        .expect("exchange");
        assert!(
            exchange
                .start_response(response(200, BodySize::Fixed(2)))
                .expect("response body")
        );
        exchange
            .request_body_mut()
            .expect("request body")
            .write(b"abc")
            .expect("pipelined request body");
        exchange
            .request_body_mut()
            .expect("request body")
            .end()
            .expect("request end");
        let response_body = exchange.response_body_mut().expect("response body");
        response_body.write(b"ok").expect("response write");
        response_body.end().expect("response end");
        exchange.complete().expect("complete");

        let mut reset = HttpExchange::new(
            Arc::new(()),
            request(HttpMethod::Get, BodySize::Fixed(0)),
            HttpLimits::default(),
        )
        .expect("reset exchange");
        assert!(
            !reset
                .start_response(response(205, BodySize::Unknown))
                .expect("205 headers are immediately available")
        );
        assert!(reset.response().is_some());
        reset.complete().expect("205 complete");
    }

    #[test]
    fn websocket_upgrade_and_cancel_propagate() {
        let mut exchange = HttpExchange::new(
            Arc::new(()),
            request(HttpMethod::Get, BodySize::Fixed(0)),
            HttpLimits::default(),
        )
        .expect("exchange");
        exchange.start_websocket(Vec::new()).expect("upgrade");
        exchange
            .websocket_mut()
            .expect("socket")
            .send_text("hello")
            .expect("frame");
        exchange.cancel();
        assert_eq!(exchange.state(), HttpExchangeState::Canceled);
        assert_eq!(
            exchange.websocket_mut().expect("socket").state(),
            crate::WebSocketState::Aborted
        );
    }

    #[test]
    fn websocket_service_cannot_complete_before_socket_closes() {
        let mut exchange = HttpExchange::new(
            Arc::new(()),
            request(HttpMethod::Get, BodySize::Fixed(0)),
            HttpLimits::default(),
        )
        .expect("exchange");
        exchange.start_websocket(Vec::new()).expect("upgrade");
        assert_eq!(exchange.complete(), Err(HttpError::WebSocketStillOpen));
        exchange
            .websocket_mut()
            .expect("socket")
            .close(1000, "done")
            .expect("close");
        exchange.complete().expect("complete after close");
    }

    #[test]
    fn outstanding_exchange_retains_service_lifetime() {
        #[derive(Debug)]
        struct Service(Arc<AtomicUsize>);
        impl Drop for Service {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let drops = Arc::new(AtomicUsize::new(0));
        let service = Arc::new(Service(Arc::clone(&drops)));
        let exchange = HttpExchange::new(
            Arc::clone(&service),
            request(HttpMethod::Get, BodySize::Fixed(0)),
            HttpLimits::default(),
        )
        .expect("exchange");
        drop(service);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(exchange);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn connect_accept_reject_and_tls_are_one_shot() {
        let mut accepted = ConnectExchange::new(
            Arc::new(()),
            "example.test:443",
            Vec::new(),
            ConnectSettings { use_tls: false },
            HttpLimits::default(),
        )
        .expect("connect");
        accepted
            .accept(response(200, BodySize::Unknown))
            .expect("accept");
        accepted.start_tls("example.test").expect("start TLS");
        assert_eq!(accepted.tls_hostname(), Some("example.test"));
        accepted.complete().expect("accepted complete");

        let mut rejected = ConnectExchange::new(
            Arc::new(()),
            "example.test:443",
            Vec::new(),
            ConnectSettings { use_tls: false },
            HttpLimits::default(),
        )
        .expect("connect");
        assert!(
            rejected
                .reject(response(500, BodySize::Fixed(5)))
                .expect("reject")
        );
        let body = rejected.rejection_body_mut().expect("rejection body");
        body.write(b"Error").expect("body");
        body.end().expect("body end");
        rejected.complete().expect("rejection complete");
    }
}
