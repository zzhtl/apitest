use std::time::Instant;

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
