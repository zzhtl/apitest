use std::time::Instant;

use apitest_core::ExecutionMetrics;

pub(crate) const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResponseTab {
    #[default]
    Body,
    Headers,
    Timeline,
    Tests,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResponseBodyMode {
    #[default]
    Pretty,
    Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunState {
    Idle,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimelinePhase {
    Started,
    ResponseHead,
    FirstByte,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TimelineEntry {
    pub(crate) phase: TimelinePhase,
    pub(crate) elapsed_ms: u64,
}

pub(crate) struct ResponseView {
    pub(crate) state: RunState,
    pub(crate) status: Option<u16>,
    pub(crate) version: Option<String>,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
    pub(crate) pending_utf8: Vec<u8>,
    pub(crate) preview_bytes: usize,
    pub(crate) pretty_body: Option<String>,
    pub(crate) metrics: Option<ExecutionMetrics>,
    pub(crate) error: Option<String>,
    pub(crate) truncated: bool,
    pub(crate) started_at: Option<Instant>,
    pub(crate) timeline: Vec<TimelineEntry>,
    pub(crate) saw_first_byte: bool,
}

impl Default for ResponseView {
    fn default() -> Self {
        Self {
            state: RunState::Idle,
            status: None,
            version: None,
            headers: Vec::new(),
            body: String::new(),
            pending_utf8: Vec::new(),
            preview_bytes: 0,
            pretty_body: None,
            metrics: None,
            error: None,
            truncated: false,
            started_at: None,
            timeline: Vec::new(),
            saw_first_byte: false,
        }
    }
}

impl ResponseView {
    pub(crate) fn running() -> Self {
        Self {
            state: RunState::Running,
            started_at: Some(Instant::now()),
            timeline: vec![TimelineEntry {
                phase: TimelinePhase::Started,
                elapsed_ms: 0,
            }],
            ..Default::default()
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        matches!(self.state, RunState::Running | RunState::Cancelling)
    }

    pub(crate) fn elapsed_ms(&self) -> u64 {
        self.started_at
            .map(|started| started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default()
    }

    pub(crate) fn push_timeline(&mut self, phase: TimelinePhase) {
        if self.timeline.iter().any(|entry| entry.phase == phase) {
            return;
        }
        self.timeline.push(TimelineEntry {
            phase,
            elapsed_ms: self.elapsed_ms(),
        });
    }

    pub(crate) fn append_body(&mut self, bytes: &[u8]) {
        if self.preview_bytes >= MAX_RESPONSE_BYTES {
            self.truncated = true;
            return;
        }
        if !bytes.is_empty() && !self.saw_first_byte {
            self.saw_first_byte = true;
            self.push_timeline(TimelinePhase::FirstByte);
        }
        let available = MAX_RESPONSE_BYTES - self.preview_bytes;
        let accepted = bytes.len().min(available);
        self.preview_bytes += accepted;
        self.truncated |= accepted < bytes.len();

        let mut data = std::mem::take(&mut self.pending_utf8);
        data.extend_from_slice(&bytes[..accepted]);
        let mut cursor = 0;
        while cursor < data.len() {
            match std::str::from_utf8(&data[cursor..]) {
                Ok(valid) => {
                    self.body.push_str(valid);
                    cursor = data.len();
                }
                Err(error) => {
                    let valid_end = cursor + error.valid_up_to();
                    if let Ok(valid) = std::str::from_utf8(&data[cursor..valid_end]) {
                        self.body.push_str(valid);
                    }
                    if let Some(error_len) = error.error_len() {
                        self.body.push('\u{fffd}');
                        cursor = valid_end + error_len;
                    } else {
                        self.pending_utf8.extend_from_slice(&data[valid_end..]);
                        cursor = data.len();
                    }
                }
            }
        }
    }

    pub(crate) fn finish_body(&mut self) {
        if !self.pending_utf8.is_empty() {
            self.body.push('\u{fffd}');
            self.pending_utf8.clear();
        }
        self.pretty_body = serde_json::from_str::<serde_json::Value>(&self.body)
            .ok()
            .and_then(|value| serde_json::to_string_pretty(&value).ok());
    }
}
