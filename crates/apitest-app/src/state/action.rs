use std::collections::VecDeque;
use std::time::{Duration, Instant};

use apitest_core::{EntityId, ExecutionError, ExecutionEvent, ProtocolKind};
use apitest_runtime::{MockServer, ScenarioReport};

use crate::state::workspace::Navigation;
use crate::workbench::DocumentId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenApiPreviewTab {
    Yaml,
    Validation,
    Documentation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InteropAction {
    ImportOpenApi,
    ImportPostman,
    ImportHar,
    ImportCurl,
    PreviewOpenApi,
    ExportOpenApi,
    ExportPostman,
    ExportHar,
    ExportCurl,
    ExportDocumentation,
}

pub(crate) enum RuntimeMessage {
    Event(u64, Result<ExecutionEvent, ExecutionError>),
    Closed(u64),
    ScenarioFinished(u64, Result<ScenarioReport, String>),
    MockStarted(u64, Result<MockServer, String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToastKind {
    Success,
    Info,
    Error,
}

pub(crate) struct Toast {
    pub(crate) message: String,
    pub(crate) kind: ToastKind,
    pub(crate) created_at: Instant,
}

impl Toast {
    /// Errors stay long enough to be read and copied; confirmations do not.
    pub(crate) fn lifetime(&self) -> Duration {
        match self.kind {
            ToastKind::Error => Duration::from_secs(8),
            ToastKind::Success | ToastKind::Info => Duration::from_secs(4),
        }
    }

    pub(crate) fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= self.lifetime()
    }
}

/// Newest-first queue of transient notices.
///
/// A single slot used to drop the previous notice, so a burst of validation
/// errors only ever showed the last one.
#[derive(Default)]
pub(crate) struct Toasts {
    items: VecDeque<Toast>,
}

impl Toasts {
    /// How many notices stay on screen before the oldest is dropped.
    const VISIBLE: usize = 4;

    pub(crate) fn push(&mut self, kind: ToastKind, message: impl Into<String>) {
        self.items.push_front(Toast {
            message: message.into(),
            kind,
            created_at: Instant::now(),
        });
        self.items.truncate(Self::VISIBLE);
    }

    pub(crate) fn retain_live(&mut self) {
        self.items.retain(|toast| !toast.is_expired());
    }

    pub(crate) fn remove(&mut self, index: usize) {
        self.items.remove(index);
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Toast> {
        self.items.iter()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn latest(&self) -> Option<&Toast> {
        self.items.front()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum PendingAction {
    SelectRequest(EntityId),
    NewRequest(ProtocolKind),
    SelectScenario(EntityId),
    NewScenario,
    SelectMock(EntityId),
    NewMock,
    SelectProject(EntityId),
    NewProject,
    Navigate(Navigation),
    SelectEnvironment(EntityId),
    NewEnvironment,
    CloseDocument(DocumentId),
    Close,
}

impl PendingAction {
    pub(crate) fn leaves_workspace(self) -> bool {
        matches!(
            self,
            Self::SelectProject(_) | Self::NewProject | Self::Close
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Confirmation {
    Unsaved(PendingAction),
    DeleteRequest(EntityId),
    DeleteEnvironment(EntityId),
}
