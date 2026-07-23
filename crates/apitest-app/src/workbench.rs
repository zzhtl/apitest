use std::time::{Duration, Instant};

use apitest_core::EntityId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    Api,
    Scenario,
    Mock,
    Environment,
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DocumentId {
    pub kind: DocumentKind,
    pub entity_id: EntityId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentTab {
    pub id: DocumentId,
    pub title: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentTabs {
    items: Vec<DocumentTab>,
    active: Option<DocumentId>,
}

impl DocumentTabs {
    pub fn open(&mut self, id: DocumentId, title: impl Into<String>) {
        if let Some(existing) = self.items.iter_mut().find(|item| item.id == id) {
            existing.title = title.into();
        } else {
            self.items.push(DocumentTab {
                id,
                title: title.into(),
            });
        }
        self.active = Some(id);
    }

    pub fn close(&mut self, id: DocumentId) -> Option<DocumentId> {
        let index = self.items.iter().position(|item| item.id == id)?;
        self.items.remove(index);
        if self.active == Some(id) {
            self.active = self
                .items
                .get(index.min(self.items.len().saturating_sub(1)))
                .map(|item| item.id);
        }
        self.active
    }

    pub fn activate(&mut self, id: DocumentId) -> bool {
        if self.items.iter().any(|item| item.id == id) {
            self.active = Some(id);
            true
        } else {
            false
        }
    }

    pub fn rename(&mut self, id: DocumentId, title: impl Into<String>) -> bool {
        let Some(tab) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        tab.title = title.into();
        true
    }

    pub fn active(&self) -> Option<DocumentId> {
        self.active
    }

    pub fn items(&self) -> &[DocumentTab] {
        &self.items
    }

    pub fn retain(&mut self, mut keep: impl FnMut(DocumentId) -> bool) {
        self.items.retain(|item| keep(item.id));
        if self
            .active
            .is_some_and(|active| !self.items.iter().any(|item| item.id == active))
        {
            self.active = self.items.first().map(|item| item.id);
        }
    }
}

#[derive(Debug, Clone)]
pub struct AutoSaveState {
    debounce: Duration,
    current_revision: u64,
    saved_revision: u64,
    queued_revision: Option<u64>,
    due_at: Option<Instant>,
}

impl AutoSaveState {
    pub fn new(debounce: Duration) -> Self {
        Self {
            debounce,
            current_revision: 0,
            saved_revision: 0,
            queued_revision: None,
            due_at: None,
        }
    }

    pub fn mark_changed(&mut self, now: Instant) -> u64 {
        self.current_revision = self.current_revision.saturating_add(1);
        self.due_at = Some(now + self.debounce);
        self.current_revision
    }

    pub fn is_due(&self, now: Instant) -> bool {
        self.is_dirty() && self.due_at.is_some_and(|due_at| now >= due_at)
    }

    pub fn mark_queued(&mut self, revision: u64) {
        self.queued_revision = Some(revision);
        if revision == self.current_revision {
            self.due_at = None;
        }
    }

    pub fn mark_saved(&mut self, revision: u64) {
        self.saved_revision = self.saved_revision.max(revision);
        if self
            .queued_revision
            .is_some_and(|queued| queued <= revision)
        {
            self.queued_revision = None;
        }
    }

    pub fn mark_failed(&mut self, revision: u64, now: Instant) {
        if self.queued_revision == Some(revision) {
            self.queued_revision = None;
        }
        if self.is_dirty() {
            self.due_at = Some(now + self.debounce);
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.current_revision > self.saved_revision
    }

    pub fn current_revision(&self) -> u64 {
        self.current_revision
    }
}
