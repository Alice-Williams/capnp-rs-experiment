#![doc = "Optional Cap'n Proto text, JSON, and ecosystem adapters."]

mod byte_stream;
mod http;
mod json_rpc;
mod websocket;

pub use byte_stream::{
    ByteSink, ByteStream, ByteStreamError, ByteStreamState, ByteSubstream, SubstreamCallback,
};
pub use capnp_json::JsonValue;
pub use http::{
    BodySize, CommonHeaderName, CommonHeaderValue, ConnectExchange, ConnectSettings, ConnectState,
    HttpBody, HttpBodyState, HttpError, HttpExchange, HttpExchangeState, HttpHeader,
    HttpHeaderValue, HttpLimits, HttpMethod, HttpRequest, HttpResponse,
};
pub use json_rpc::{
    ContentLengthCodec, JsonRpcCodec, JsonRpcError, JsonRpcFailure, JsonRpcId, JsonRpcLimits,
    JsonRpcMessage, JsonRpcSession,
};
pub use websocket::{
    WebSocketError, WebSocketFrame, WebSocketLimits, WebSocketPoll, WebSocketSession,
    WebSocketState,
};
