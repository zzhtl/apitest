use apitest_runtime::ScriptAssertion;

/// What the post-response script, assertions and extractors produced for one run.
#[derive(Debug, Clone, Default)]
pub(crate) struct VerificationOutcome {
    pub(crate) assertions: Vec<ScriptAssertion>,
    /// Variables the extractors pulled out, in declaration order.
    pub(crate) extracted: Vec<(String, String)>,
    /// A script or extractor failure, which is distinct from a failed assertion.
    pub(crate) error: Option<String>,
}

impl VerificationOutcome {
    pub(crate) fn passed(&self) -> bool {
        self.error.is_none() && self.assertions.iter().all(|assertion| assertion.passed)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.assertions.is_empty() && self.extracted.is_empty() && self.error.is_none()
    }

    pub(crate) fn failed_count(&self) -> usize {
        self.assertions
            .iter()
            .filter(|assertion| !assertion.passed)
            .count()
    }
}
