use apitest_core::{EntityId, ProjectNode, ProjectNodeKind};

use crate::app::ApiTestApp;
use crate::state::action::{Confirmation, ToastKind};
use crate::state::workspace::WorkspaceRequest;

/// A structural edit requested from the resource tree.
#[derive(Debug, Clone)]
pub(crate) enum TreeAction {
    NewFolder { parent: Option<EntityId> },
    Rename { node: EntityId, name: String },
    Duplicate { entity_id: EntityId },
    DeleteRequest { entity_id: EntityId },
    DeleteFolder { node: EntityId },
}

impl ApiTestApp {
    pub(crate) fn apply_tree_action(&mut self, action: TreeAction) {
        match action {
            TreeAction::NewFolder { parent } => self.create_folder(parent),
            TreeAction::Rename { node, name } => {
                self.rename_target = Some((node, name));
            }
            TreeAction::Duplicate { entity_id } => self.duplicate_request(entity_id),
            TreeAction::DeleteRequest { entity_id } => {
                self.confirmation = Some(Confirmation::DeleteRequest(entity_id));
            }
            TreeAction::DeleteFolder { node } => {
                self.confirmation = Some(Confirmation::DeleteFolder(node));
            }
        }
    }

    fn create_folder(&mut self, parent: Option<EntityId>) {
        let Some(database) = self.database.clone() else {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return;
        };
        let node = ProjectNode {
            id: EntityId::new(),
            project_id: self.project.id,
            parent_id: parent,
            entity_id: None,
            kind: ProjectNodeKind::Folder,
            name: self.tr("新建文件夹", "New folder").to_owned(),
            sort_order: 0,
        };
        if let Err(error) = database.save_project_node(&node) {
            self.toast(ToastKind::Error, error.to_string());
            return;
        }
        if let Some(parent) = parent {
            self.expanded_resources.insert(parent);
        }
        self.reload_resource_page(parent);
        // Name it immediately: an unnamed folder is never what the user wanted.
        self.rename_target = Some((node.id, node.name));
    }

    /// Move a node under `parent`, or to the root when `parent` is `None`.
    pub(crate) fn move_resource(&mut self, node_id: EntityId, parent: Option<EntityId>) {
        let Some(database) = self.database.clone() else {
            return;
        };
        let Some(mut node) = self.find_resource_node(node_id) else {
            return;
        };
        if node.parent_id == parent {
            return;
        }
        if parent.is_some_and(|parent| self.is_descendant(parent, node_id)) {
            self.toast(
                ToastKind::Error,
                self.tr(
                    "不能把文件夹移动到它自己的子目录",
                    "A folder cannot move inside itself",
                ),
            );
            return;
        }
        let previous_parent = node.parent_id;
        node.parent_id = parent;
        if let Err(error) = database.save_project_node(&node) {
            self.toast(ToastKind::Error, error.to_string());
            return;
        }
        self.reload_resource_page(previous_parent);
        self.reload_resource_page(parent);
        if let Some(parent) = parent {
            self.expanded_resources.insert(parent);
        }
    }

    pub(crate) fn rename_resource(&mut self, node_id: EntityId, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.toast(
                ToastKind::Error,
                self.tr("名称不能为空", "The name cannot be empty"),
            );
            return;
        }
        let Some(database) = self.database.clone() else {
            return;
        };
        let Some(mut node) = self.find_resource_node(node_id) else {
            return;
        };
        node.name = name.to_owned();
        if let Err(error) = database.save_project_node(&node) {
            self.toast(ToastKind::Error, error.to_string());
            return;
        }
        // A definition node mirrors its request's name, so rename both.
        if node.kind == ProjectNodeKind::ApiDefinition
            && let Some(entity_id) = node.entity_id
            && let Some(index) = self
                .requests
                .iter()
                .position(|request| request.id() == entity_id)
        {
            self.requests[index].name = name.to_owned();
            self.queue_request_save(index, false, false);
        }
        self.reload_resource_page(node.parent_id);
    }

    fn duplicate_request(&mut self, entity_id: EntityId) {
        let Some(source) = self
            .requests
            .iter()
            .find(|request| request.id() == entity_id)
        else {
            return;
        };
        let copied_name = match self.language {
            crate::i18n::Language::Chinese => format!("{} 副本", source.name),
            crate::i18n::Language::English => format!("{} copy", source.name),
        };
        let protocol = source.edited_protocol();
        let mut duplicate = WorkspaceRequest::new_protocol(copied_name, protocol.kind());
        duplicate.name.clone_from(&duplicate.definition.name);
        duplicate.request_case.protocol = protocol.clone();
        duplicate.request_case.assertions = source.request_case.assertions.clone();
        duplicate.request_case.extractors = source.request_case.extractors.clone();
        duplicate
            .request_case
            .pre_request_script
            .clone_from(&source.request_case.pre_request_script);
        duplicate
            .request_case
            .post_response_script
            .clone_from(&source.request_case.post_response_script);
        let id = duplicate.id();
        self.requests.push(duplicate);
        self.selected = self.requests.len() - 1;
        self.queue_request_save(self.selected, false, false);
        self.queue_action(crate::state::action::PendingAction::SelectRequest(id));
    }

    /// Delete a folder together with every request nested under it.
    pub(crate) fn delete_folder(&mut self, node_id: EntityId) {
        let Some(database) = self.database.clone() else {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return;
        };
        if !self.wait_storage() {
            return;
        }
        let definitions = match database.definitions_under(self.project.id, node_id) {
            Ok(definitions) => definitions,
            Err(error) => {
                self.toast(ToastKind::Error, error.to_string());
                return;
            }
        };
        for definition in &definitions {
            self.delete_request(*definition);
        }
        let parent = self
            .find_resource_node(node_id)
            .and_then(|node| node.parent_id);
        if let Err(error) = database.delete_project_node(self.project.id, node_id) {
            self.toast(ToastKind::Error, error.to_string());
            return;
        }
        self.expanded_resources.remove(&node_id);
        self.resource_pages.remove(&Some(node_id));
        self.reload_resource_page(parent);
        self.toast(
            ToastKind::Success,
            self.tr("文件夹已删除", "Folder deleted"),
        );
    }

    /// How many requests a folder deletion would take with it.
    pub(crate) fn folder_request_count(&self, node_id: EntityId) -> usize {
        self.database
            .as_ref()
            .and_then(|database| database.definitions_under(self.project.id, node_id).ok())
            .map(|definitions| definitions.len())
            .unwrap_or_default()
    }

    fn find_resource_node(&self, node_id: EntityId) -> Option<ProjectNode> {
        self.resource_pages
            .values()
            .flat_map(|page| page.items.iter())
            .find(|node| node.id == node_id)
            .cloned()
    }

    /// Whether `candidate` sits somewhere below `ancestor` in the loaded tree.
    fn is_descendant(&self, candidate: EntityId, ancestor: EntityId) -> bool {
        let mut current = Some(candidate);
        // Bounded by the loaded depth; a cycle would otherwise spin forever.
        for _ in 0..64 {
            let Some(id) = current else {
                return false;
            };
            if id == ancestor {
                return true;
            }
            current = self.find_resource_node(id).and_then(|node| node.parent_id);
        }
        false
    }
}
