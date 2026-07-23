use std::{collections::HashMap, sync::Arc};

use apitest_core::{
    ExecutionError, ExecutionHandle, ExecutionRequest, ProtocolExecutor, ProtocolKind,
};

pub struct ExecutorRegistry {
    executors: HashMap<ProtocolKind, Arc<dyn ProtocolExecutor>>,
}

impl ExecutorRegistry {
    pub fn new() -> Self {
        Self {
            executors: HashMap::new(),
        }
    }

    pub fn register(&mut self, kind: ProtocolKind, executor: Arc<dyn ProtocolExecutor>) {
        self.executors.insert(kind, executor);
    }

    pub fn contains(&self, kind: ProtocolKind) -> bool {
        self.executors.contains_key(&kind)
    }

    pub fn executor(&self, kind: ProtocolKind) -> Option<Arc<dyn ProtocolExecutor>> {
        self.executors.get(&kind).cloned()
    }

    pub fn start(&self, request: ExecutionRequest) -> Result<ExecutionHandle, ExecutionError> {
        let kind = request.protocol.kind();
        let executor = self
            .executors
            .get(&kind)
            .ok_or(ExecutionError::UnsupportedProtocol(kind))?;
        Ok(executor.start(request))
    }
}

impl Default for ExecutorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
