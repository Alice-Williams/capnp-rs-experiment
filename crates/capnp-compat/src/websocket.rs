//! Ordered, bounded WebSocket frame state used by HTTP-over-Cap'n-Proto.

use std::collections::VecDeque;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebSocketLimits {
    pub max_frame_bytes: usize,
    pub max_queued_frames: usize,
    pub max_queued_bytes: usize,
}

impl Default for WebSocketLimits {
    fn default() -> Self {
        Self {
            max_frame_bytes: 16 * 1024 * 1024,
            max_queued_frames: 1024,
            max_queued_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebSocketFrame {
    Text(String),
    Data(Vec<u8>),
    Close { code: u16, reason: String },
}

impl WebSocketFrame {
    fn payload_bytes(&self) -> Result<usize, WebSocketError> {
        match self {
            Self::Text(value) => Ok(value.len()),
            Self::Data(value) => Ok(value.len()),
            Self::Close { reason, .. } => reason
                .len()
                .checked_add(2)
                .ok_or(WebSocketError::CountOverflow),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebSocketState {
    Open,
    Closed,
    Aborted,
    Overloaded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebSocketPoll {
    Pending,
    Frame(WebSocketFrame),
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebSocketError {
    NotOpen(WebSocketState),
    FrameTooLarge,
    QueueFull,
    CountOverflow,
    Disconnected,
}

impl fmt::Display for WebSocketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOpen(state) => write!(formatter, "WebSocket is not open ({state:?})"),
            Self::FrameTooLarge => formatter.write_str("WebSocket frame exceeds configured limit"),
            Self::QueueFull => formatter.write_str("WebSocket send queue is full"),
            Self::CountOverflow => formatter.write_str("WebSocket byte count overflow"),
            Self::Disconnected => formatter.write_str("WebSocket disconnected"),
        }
    }
}

impl std::error::Error for WebSocketError {}

/// Executor-neutral ordered WebSocket send queue.
///
/// Overload follows the pinned adapter behavior: an idle socket receives close
/// code 1013, while already-queued data or a prior close is delivered before
/// the peer observes disconnection.
///
/// ```
/// use capnp_compat::{WebSocketFrame, WebSocketPoll, WebSocketSession};
/// let mut socket = WebSocketSession::default();
/// socket.send_text("hello")?;
/// socket.send_data(vec![1, 2, 3])?;
/// socket.close(1000, "done")?;
/// assert_eq!(
///     socket.poll_receive()?,
///     WebSocketPoll::Frame(WebSocketFrame::Text("hello".into())),
/// );
/// # Ok::<(), capnp_compat::WebSocketError>(())
/// ```
#[derive(Clone, Debug)]
pub struct WebSocketSession {
    limits: WebSocketLimits,
    state: WebSocketState,
    queued: VecDeque<WebSocketFrame>,
    queued_bytes: usize,
    sent_bytes: u64,
    delivered_bytes: u64,
}

impl WebSocketSession {
    pub fn new(limits: WebSocketLimits) -> Self {
        Self {
            limits,
            state: WebSocketState::Open,
            queued: VecDeque::new(),
            queued_bytes: 0,
            sent_bytes: 0,
            delivered_bytes: 0,
        }
    }

    pub fn state(&self) -> WebSocketState {
        self.state
    }

    pub fn queued_frames(&self) -> usize {
        self.queued.len()
    }

    pub fn sent_payload_bytes(&self) -> u64 {
        self.sent_bytes
    }

    pub fn delivered_payload_bytes(&self) -> u64 {
        self.delivered_bytes
    }

    pub fn send_text(&mut self, text: impl Into<String>) -> Result<(), WebSocketError> {
        self.enqueue(WebSocketFrame::Text(text.into()))
    }

    pub fn send_data(&mut self, data: impl Into<Vec<u8>>) -> Result<(), WebSocketError> {
        self.enqueue(WebSocketFrame::Data(data.into()))
    }

    pub fn close(&mut self, code: u16, reason: impl Into<String>) -> Result<(), WebSocketError> {
        self.require_open()?;
        self.enqueue_open(WebSocketFrame::Close {
            code,
            reason: reason.into(),
        })?;
        self.state = WebSocketState::Closed;
        Ok(())
    }

    pub fn abort(&mut self) {
        self.queued.clear();
        self.queued_bytes = 0;
        self.state = WebSocketState::Aborted;
    }

    pub fn mark_overloaded(&mut self) -> Result<(), WebSocketError> {
        match self.state {
            WebSocketState::Aborted | WebSocketState::Overloaded => return Ok(()),
            WebSocketState::Open if self.queued.is_empty() => {
                self.enqueue_open(WebSocketFrame::Close {
                    code: 1013,
                    reason: "Service overloaded; retry later.".to_owned(),
                })?;
            }
            WebSocketState::Open | WebSocketState::Closed => {}
        }
        self.state = WebSocketState::Overloaded;
        Ok(())
    }

    pub fn poll_receive(&mut self) -> Result<WebSocketPoll, WebSocketError> {
        if let Some(frame) = self.queued.pop_front() {
            let bytes = frame.payload_bytes()?;
            self.queued_bytes = self
                .queued_bytes
                .checked_sub(bytes)
                .ok_or(WebSocketError::CountOverflow)?;
            self.delivered_bytes = self
                .delivered_bytes
                .checked_add(u64::try_from(bytes).map_err(|_| WebSocketError::CountOverflow)?)
                .ok_or(WebSocketError::CountOverflow)?;
            return Ok(WebSocketPoll::Frame(frame));
        }
        match self.state {
            WebSocketState::Open => Ok(WebSocketPoll::Pending),
            WebSocketState::Closed => Ok(WebSocketPoll::Closed),
            WebSocketState::Aborted | WebSocketState::Overloaded => {
                Err(WebSocketError::Disconnected)
            }
        }
    }

    fn enqueue(&mut self, frame: WebSocketFrame) -> Result<(), WebSocketError> {
        self.require_open()?;
        self.enqueue_open(frame)
    }

    fn enqueue_open(&mut self, frame: WebSocketFrame) -> Result<(), WebSocketError> {
        let bytes = frame.payload_bytes()?;
        if bytes > self.limits.max_frame_bytes {
            return Err(WebSocketError::FrameTooLarge);
        }
        if self.queued.len() >= self.limits.max_queued_frames {
            return Err(WebSocketError::QueueFull);
        }
        let queued_bytes = self
            .queued_bytes
            .checked_add(bytes)
            .ok_or(WebSocketError::CountOverflow)?;
        if queued_bytes > self.limits.max_queued_bytes {
            return Err(WebSocketError::QueueFull);
        }
        let sent_bytes = self
            .sent_bytes
            .checked_add(u64::try_from(bytes).map_err(|_| WebSocketError::CountOverflow)?)
            .ok_or(WebSocketError::CountOverflow)?;
        self.queued.push_back(frame);
        self.queued_bytes = queued_bytes;
        self.sent_bytes = sent_bytes;
        Ok(())
    }

    fn require_open(&self) -> Result<(), WebSocketError> {
        if self.state == WebSocketState::Open {
            Ok(())
        } else {
            Err(WebSocketError::NotOpen(self.state))
        }
    }
}

impl Default for WebSocketSession {
    fn default() -> Self {
        Self::new(WebSocketLimits::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_data_close_are_ordered_and_counted_exactly() {
        let mut socket = WebSocketSession::default();
        socket.send_text("foo").expect("text");
        socket.send_data(b"bar".to_vec()).expect("data");
        socket.close(1234, "baz").expect("close");
        assert_eq!(socket.sent_payload_bytes(), 11);
        assert_eq!(
            socket.poll_receive(),
            Ok(WebSocketPoll::Frame(WebSocketFrame::Text("foo".into())))
        );
        assert_eq!(
            socket.poll_receive(),
            Ok(WebSocketPoll::Frame(WebSocketFrame::Data(b"bar".to_vec())))
        );
        assert_eq!(
            socket.poll_receive(),
            Ok(WebSocketPoll::Frame(WebSocketFrame::Close {
                code: 1234,
                reason: "baz".into(),
            }))
        );
        assert_eq!(socket.delivered_payload_bytes(), 11);
        assert_eq!(socket.poll_receive(), Ok(WebSocketPoll::Closed));
    }

    #[test]
    fn overload_matches_idle_pending_and_closed_cases() {
        let mut idle = WebSocketSession::default();
        idle.mark_overloaded().expect("idle overload");
        assert_eq!(
            idle.poll_receive(),
            Ok(WebSocketPoll::Frame(WebSocketFrame::Close {
                code: 1013,
                reason: "Service overloaded; retry later.".into(),
            }))
        );
        assert_eq!(idle.poll_receive(), Err(WebSocketError::Disconnected));

        let mut pending = WebSocketSession::default();
        pending.send_text("before").expect("pending frame");
        pending.mark_overloaded().expect("pending overload");
        assert_eq!(
            pending.poll_receive(),
            Ok(WebSocketPoll::Frame(WebSocketFrame::Text("before".into())))
        );
        assert_eq!(pending.poll_receive(), Err(WebSocketError::Disconnected));

        let mut closed = WebSocketSession::default();
        closed.close(1234, "closed").expect("close");
        closed.mark_overloaded().expect("closed overload");
        assert!(matches!(
            closed.poll_receive(),
            Ok(WebSocketPoll::Frame(WebSocketFrame::Close {
                code: 1234,
                ..
            }))
        ));
        assert_eq!(closed.poll_receive(), Err(WebSocketError::Disconnected));
    }

    #[test]
    fn quota_failures_do_not_mutate_the_queue() {
        let mut socket = WebSocketSession::new(WebSocketLimits {
            max_frame_bytes: 4,
            max_queued_frames: 1,
            max_queued_bytes: 4,
        });
        assert_eq!(
            socket.send_text("12345"),
            Err(WebSocketError::FrameTooLarge)
        );
        assert_eq!(socket.queued_frames(), 0);
        socket.send_text("1234").expect("bounded frame");
        assert_eq!(socket.send_data([1]), Err(WebSocketError::QueueFull));
        assert_eq!(socket.queued_frames(), 1);
        assert_eq!(socket.sent_payload_bytes(), 4);
    }

    #[test]
    fn abort_discards_pending_frames_and_disconnects() {
        let mut socket = WebSocketSession::default();
        socket.send_text("discarded").expect("frame");
        socket.abort();
        assert_eq!(socket.queued_frames(), 0);
        assert_eq!(socket.poll_receive(), Err(WebSocketError::Disconnected));
    }
}
