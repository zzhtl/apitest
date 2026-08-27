use std::collections::HashMap;
use std::ops::Range;

use apitest_core::{ExecutionCommand, RunRecord};
use apitest_storage::RedactingBodySink;
use tokio_util::sync::CancellationToken;

use crate::state::response::{ResponseBodyMode, ResponseTab, ResponseView};
use crate::state::verification::VerificationOutcome;
use crate::state::workspace::EditorTab;
use crate::workbench::DocumentId;

/// Everything about one open request tab that must not leak into its siblings.
///
/// These lived as single fields on the app, so sending in one tab overwrote the
/// response shown in every other tab, and merely switching tabs cancelled
/// whatever request was in flight.
#[derive(Default)]
pub(crate) struct DocumentSession {
    pub(crate) editor_tab: EditorTab,
    pub(crate) response_tab: ResponseTab,
    pub(crate) response_body_mode: ResponseBodyMode,
    pub(crate) response: ResponseView,
    pub(crate) websocket_message: String,
    /// Identifies the run whose events this session accepts; a superseded run
    /// keeps streaming until its task notices the cancellation.
    pub(crate) run: u64,
    pub(crate) cancellation: Option<CancellationToken>,
    pub(crate) execution_commands: Option<tokio::sync::mpsc::Sender<ExecutionCommand>>,
    /// The history record being written for the run in this tab. Per-session
    /// because several tabs can now stream at once.
    pub(crate) history_record: Option<RunRecord>,
    pub(crate) history_body: Option<RedactingBodySink>,
    pub(crate) history_redactions: Vec<String>,
    /// Assertion and extractor outcome for the most recent run in this tab.
    pub(crate) verification: Option<VerificationOutcome>,
    /// Response viewer state: search box contents and soft-wrap preference.
    pub(crate) body_search: String,
    pub(crate) body_wrap: bool,
    /// Derived render state for the response body; see `BodyRenderCache`.
    pub(crate) render_cache: BodyRenderCache,
}

impl DocumentSession {
    /// Abandon whatever is in flight, keeping the editor selection.
    pub(crate) fn cancel(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        self.execution_commands = None;
    }
}

/// Everything derived from the response body that used to be recomputed per
/// frame: the match scan, the JSON tree parse, the display rows for the
/// virtualized viewer and the owned buffer `TextEdit` needs.
///
/// `(run, revision)` identifies the content ((`run` is unique per send, so a
/// fresh response restarting at revision 0 can never collide with the last
/// one); `mode` selects raw vs pretty text and `search` the match set.
#[derive(Default)]
pub(crate) struct BodyRenderCache {
    pub(crate) run: u64,
    pub(crate) revision: u64,
    pub(crate) mode: ResponseBodyMode,
    pub(crate) search: String,
    /// Case-insensitive match spans of `search`, sorted by start.
    pub(crate) matches: Vec<Range<usize>>,
    /// Display rows for the virtualized large-body viewer.
    pub(crate) rows: Vec<Range<usize>>,
    /// Owned copy of the body for the small-body `TextEdit` path.
    pub(crate) edit_buffer: String,
    /// Parsed JSON document for the tree view, when the body parses.
    pub(crate) tree: Option<serde_json::Value>,
}

#[derive(Default)]
pub(crate) struct Sessions {
    items: HashMap<DocumentId, DocumentSession>,
    next_run: u64,
}

impl Sessions {
    /// Run identifiers are handed out globally so a late event can never be
    /// mistaken for a different tab's current run.
    pub(crate) fn next_run(&mut self) -> u64 {
        self.next_run = self.next_run.wrapping_add(1);
        self.next_run
    }

    pub(crate) fn get(&self, id: DocumentId) -> Option<&DocumentSession> {
        self.items.get(&id)
    }

    pub(crate) fn entry(&mut self, id: DocumentId) -> &mut DocumentSession {
        self.items.entry(id).or_default()
    }

    /// The document whose session still considers `run` current.
    pub(crate) fn owner(&self, run: u64) -> Option<DocumentId> {
        if run == 0 {
            return None;
        }
        self.items
            .iter()
            .find(|(_, session)| session.run == run)
            .map(|(id, _)| *id)
    }

    pub(crate) fn close(&mut self, id: DocumentId) {
        if let Some(mut session) = self.items.remove(&id) {
            session.cancel();
        }
    }

    /// Whether any tab is still streaming, i.e. the live progress readouts
    /// need periodic repaints.
    pub(crate) fn any_active(&self) -> bool {
        self.items
            .values()
            .any(|session| session.response.is_active())
    }

    /// Drop every session, cancelling what they were running.
    ///
    /// `next_run` deliberately keeps counting so a late event from the old
    /// project can never match a run identifier handed out afterwards.
    pub(crate) fn clear(&mut self) {
        for session in self.items.values_mut() {
            session.cancel();
        }
        self.items.clear();
    }
}
