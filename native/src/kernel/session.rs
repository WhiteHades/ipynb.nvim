//! Readiness handshake, queued execution, and stdin reply state.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use uuid::Uuid;

pub(super) struct SessionState {
    pub(super) session_id: String,
    pub(super) ready: bool,
    probe_id: Option<String>,
    probe_deadline: Option<Instant>,
    shell_ready: bool,
    iopub_ready: bool,
    iopub_last: Option<Instant>,
    pub(super) pending: VecDeque<(String, String)>,
    pub(super) input_parent: Value,
}

impl SessionState {
    const KERNEL_INFO_TIMEOUT: Duration = Duration::from_secs(1);
    const IOPUB_DRAIN: Duration = Duration::from_millis(200);

    pub(super) fn new() -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            ready: false,
            probe_id: None,
            probe_deadline: None,
            shell_ready: false,
            iopub_ready: false,
            iopub_last: None,
            pending: VecDeque::new(),
            input_parent: json!({}),
        }
    }

    pub(super) fn reset(&mut self) {
        self.ready = false;
        self.probe_id = None;
        self.probe_deadline = None;
        self.shell_ready = false;
        self.iopub_ready = false;
        self.iopub_last = None;
        self.pending.clear();
        self.input_parent = json!({});
    }

    pub(super) fn probe_sent(&mut self, msg_id: String, now: Instant) {
        self.ready = false;
        self.probe_id = Some(msg_id);
        self.probe_deadline = Some(now + Self::KERNEL_INFO_TIMEOUT);
        self.shell_ready = false;
        self.iopub_ready = false;
        self.iopub_last = None;
    }

    pub(super) fn probe_due(&self, now: Instant) -> bool {
        !self.ready
            && !(self.shell_ready && self.iopub_ready)
            && self.probe_deadline.is_some_and(|deadline| now >= deadline)
    }

    pub(super) fn observe(&mut self, message: &Value, channel: &str, now: Instant) {
        let msg_type = message
            .get("header")
            .and_then(|header| header.get("msg_type"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let parent_msg_id = message
            .get("parent_header")
            .and_then(|parent| parent.get("msg_id"))
            .and_then(Value::as_str);
        let matches_probe = self.probe_id.as_deref() == parent_msg_id;

        if channel == "shell" && msg_type == "kernel_info_reply" && matches_probe {
            self.shell_ready = true;
            if self.iopub_ready {
                self.iopub_last = Some(now);
                self.probe_deadline = None;
            } else {
                self.probe_deadline = Some(now + Self::IOPUB_DRAIN);
            }
        }
        if channel == "iopub" {
            if matches_probe {
                self.iopub_ready = true;
                if self.shell_ready {
                    self.iopub_last = Some(now);
                    self.probe_deadline = None;
                }
            } else if self.shell_ready && self.iopub_ready {
                self.iopub_last = Some(now);
            }
        }
        if msg_type == "input_request" {
            self.input_parent = message.get("header").cloned().unwrap_or_else(|| json!({}));
        }

        // A kernel_info reply must be followed by a quiet IOPub window before
        // queued execution can be released. This mirrors Jupyter client's
        // wait_for_ready handshake and avoids racing the SUB socket.
    }

    pub(super) fn maybe_ready(&mut self, now: Instant) -> bool {
        if !self.ready
            && self.shell_ready
            && self.iopub_ready
            && self
                .iopub_last
                .is_some_and(|last| now.duration_since(last) >= Self::IOPUB_DRAIN)
        {
            self.ready = true;
            self.probe_id = None;
            self.probe_deadline = None;
            return true;
        }
        false
    }
}
