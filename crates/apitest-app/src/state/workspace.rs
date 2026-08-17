use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use apitest_core::{
    ApiDefinition, EntityId, GraphQlSpec, GrpcCallKind, GrpcSpec, HttpMethod, ProjectNode,
    ProjectNodeKind, ProtocolKind, ProtocolSpec, RequestCase, SseSpec, WebSocketSpec,
};
use chrono::Utc;

use crate::draft::RequestDraft;
use crate::workbench::AutoSaveState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Navigation {
    Api,
    Scenario,
    Mock,
    History,
    Environment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorTab {
    Params,
    Headers,
    Cookies,
    Body,
    Auth,
}

#[derive(Default)]
pub(crate) struct ResourcePage {
    pub(crate) items: Vec<ProjectNode>,
    pub(crate) total: usize,
}

#[derive(Clone)]
pub(crate) enum ResourceRow {
    Node {
        node: ProjectNode,
        depth: usize,
    },
    More {
        parent_id: Option<EntityId>,
        depth: usize,
    },
}

pub(crate) struct WorkspaceRequest {
    pub(crate) definition: ApiDefinition,
    pub(crate) request_case: RequestCase,
    pub(crate) name: String,
    pub(crate) draft: RequestDraft,
    pub(crate) alternate_protocol: Option<ProtocolSpec>,
    pub(crate) persisted: bool,
    pub(crate) sync_contract: bool,
    pub(crate) autosave: AutoSaveState,
    pub(crate) observed_snapshot: Vec<u8>,
}

impl WorkspaceRequest {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Self::new_protocol(name, ProtocolKind::Http)
    }

    pub(crate) fn new_protocol(name: impl Into<String>, kind: ProtocolKind) -> Self {
        let protocol = default_protocol(kind);
        let definition = ApiDefinition::new(name, protocol.clone());
        let request_case = RequestCase::for_definition(&definition, "Default");
        let request_id = definition.id;
        let (draft, alternate_protocol) = match protocol {
            ProtocolSpec::Http(spec) => (RequestDraft::from_http_spec(&spec, request_id), None),
            protocol => (RequestDraft::default(), Some(protocol)),
        };
        let name = definition.name.clone();
        let observed_snapshot = request_snapshot(
            &name,
            alternate_protocol
                .clone()
                .unwrap_or_else(|| ProtocolSpec::Http(draft.to_http_spec())),
        );
        let mut autosave = AutoSaveState::new(Duration::from_millis(500));
        autosave.mark_changed(Instant::now());
        Self {
            request_case,
            name,
            draft,
            alternate_protocol,
            definition,
            persisted: false,
            sync_contract: true,
            autosave,
            observed_snapshot,
        }
    }

    pub(crate) fn from_definition(
        definition: ApiDefinition,
        request_case: Option<RequestCase>,
    ) -> Self {
        let persisted = request_case.is_some();
        let request_case =
            request_case.unwrap_or_else(|| RequestCase::for_definition(&definition, "Default"));
        let (draft, alternate_protocol) = match &request_case.protocol {
            ProtocolSpec::Http(spec) => (RequestDraft::from_http_spec(spec, definition.id), None),
            protocol => (RequestDraft::default(), Some(protocol.clone())),
        };
        let name = definition.name.clone();
        let observed_snapshot = request_snapshot(&name, request_case.protocol.clone());
        let mut autosave = AutoSaveState::new(Duration::from_millis(500));
        if !persisted {
            autosave.mark_changed(Instant::now());
        }
        Self {
            request_case,
            name,
            draft,
            alternate_protocol,
            definition,
            persisted,
            sync_contract: false,
            autosave,
            observed_snapshot,
        }
    }

    pub(crate) fn id(&self) -> EntityId {
        self.definition.id
    }

    pub(crate) fn is_dirty(&self) -> bool {
        !self.persisted
            || self.name != self.definition.name
            || self.edited_protocol() != self.request_case.protocol
            || self.draft.has_pending_secret()
            || self.autosave.is_dirty()
    }

    pub(crate) fn discard(&mut self) {
        self.name = self.definition.name.clone();
        match &self.request_case.protocol {
            ProtocolSpec::Http(spec) => {
                self.draft = RequestDraft::from_http_spec(spec, self.definition.id);
                self.alternate_protocol = None;
            }
            protocol => {
                self.draft = RequestDraft::default();
                self.alternate_protocol = Some(protocol.clone());
            }
        }
        self.autosave = AutoSaveState::new(Duration::from_millis(500));
        self.observed_snapshot = request_snapshot(&self.name, self.edited_protocol());
    }

    pub(crate) fn sync_edit_revision(&mut self, now: Instant) {
        let snapshot = request_snapshot(&self.name, self.edited_protocol());
        if snapshot != self.observed_snapshot {
            self.observed_snapshot = snapshot;
            self.autosave.mark_changed(now);
        }
    }

    pub(crate) fn save_snapshot(&self) -> (ApiDefinition, RequestCase) {
        let mut definition = self.definition.clone();
        definition.name = self.name.clone();
        definition.updated_at = Utc::now();
        let protocol = self.edited_protocol();
        if self.sync_contract {
            definition.contract = protocol.clone().into();
        }
        let mut request_case = self.request_case.clone();
        request_case.protocol = protocol;
        request_case.updated_at = Utc::now();
        (definition, request_case)
    }

    pub(crate) fn edited_protocol(&self) -> ProtocolSpec {
        self.alternate_protocol
            .clone()
            .unwrap_or_else(|| ProtocolSpec::Http(self.draft.to_http_spec()))
    }

    pub(crate) fn protocol_kind(&self) -> ProtocolKind {
        self.alternate_protocol
            .as_ref()
            .map(ProtocolSpec::kind)
            .unwrap_or(ProtocolKind::Http)
    }

    pub(crate) fn endpoint(&self) -> &str {
        match self.alternate_protocol.as_ref() {
            None => &self.draft.url,
            Some(ProtocolSpec::GraphQl(spec)) => &spec.endpoint,
            Some(ProtocolSpec::Sse(spec)) => &spec.request.url,
            Some(ProtocolSpec::WebSocket(spec)) => &spec.url,
            Some(ProtocolSpec::Grpc(spec)) => &spec.endpoint,
            Some(ProtocolSpec::Http(_)) => &self.draft.url,
        }
    }

    pub(crate) fn mark_saved(
        &mut self,
        definition: ApiDefinition,
        request_case: RequestCase,
        revision: u64,
    ) {
        self.definition = definition;
        self.request_case = request_case;
        self.persisted = true;
        self.sync_contract = false;
        self.autosave.mark_saved(revision);
    }
}

pub(crate) fn request_snapshot(name: &str, protocol: ProtocolSpec) -> Vec<u8> {
    serde_json::to_vec(&(name, protocol)).expect("request editor state should serialize")
}

pub(crate) fn default_protocol(kind: ProtocolKind) -> ProtocolSpec {
    match kind {
        ProtocolKind::Http => ProtocolSpec::Http(apitest_core::HttpSpec::new(HttpMethod::Get, "")),
        ProtocolKind::GraphQl => ProtocolSpec::GraphQl(GraphQlSpec {
            endpoint: String::new(),
            query: "query {\n  __typename\n}".into(),
            variables: "{}".into(),
            operation_name: None,
            headers: Vec::new(),
            auth: Default::default(),
            timeout_ms: 30_000,
        }),
        ProtocolKind::Sse => ProtocolSpec::Sse(SseSpec {
            request: apitest_core::HttpSpec::new(HttpMethod::Get, ""),
            reconnect: true,
        }),
        ProtocolKind::WebSocket => ProtocolSpec::WebSocket(WebSocketSpec {
            url: String::new(),
            query: Vec::new(),
            headers: Vec::new(),
            subprotocols: Vec::new(),
            validate_tls: true,
            connect_timeout_ms: 30_000,
        }),
        ProtocolKind::Grpc => ProtocolSpec::Grpc(GrpcSpec {
            endpoint: String::new(),
            service: String::new(),
            method: String::new(),
            call_kind: GrpcCallKind::Unary,
            descriptor_set: None,
            proto_files: Vec::new(),
            import_paths: Vec::new(),
            use_reflection: true,
            metadata: Vec::new(),
            message_json: "{}".into(),
            validate_tls: true,
            timeout_ms: 30_000,
        }),
    }
}

pub(crate) fn collect_resource_rows(
    parent_id: Option<EntityId>,
    depth: usize,
    pages: &HashMap<Option<EntityId>, ResourcePage>,
    expanded: &HashSet<EntityId>,
    visiting: &mut HashSet<EntityId>,
    rows: &mut Vec<ResourceRow>,
) {
    let Some(page) = pages.get(&parent_id) else {
        return;
    };
    for node in &page.items {
        rows.push(ResourceRow::Node {
            node: node.clone(),
            depth,
        });
        if node.kind == ProjectNodeKind::Folder
            && expanded.contains(&node.id)
            && visiting.insert(node.id)
        {
            collect_resource_rows(
                Some(node.id),
                depth.saturating_add(1),
                pages,
                expanded,
                visiting,
                rows,
            );
            visiting.remove(&node.id);
        }
    }
    if page.items.len() < page.total {
        rows.push(ResourceRow::More { parent_id, depth });
    }
}
