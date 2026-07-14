use apitest_app::{BodyMode, EditablePair, RequestDraft};
use apitest_core::{BodySpec, HttpMethod};

#[test]
fn request_draft_builds_enabled_fields_only() {
    let mut draft = RequestDraft {
        method: HttpMethod::Post,
        url: "https://example.com/users".into(),
        body_mode: BodyMode::Json,
        body: r#"{"name":"Ada"}"#.into(),
        ..RequestDraft::default()
    };
    draft.query.push(EditablePair::new("page", "2"));
    let mut ignored = EditablePair::new("debug", "true");
    ignored.enabled = false;
    draft.query.push(ignored);

    let spec = draft.to_http_spec();
    assert_eq!(spec.method, HttpMethod::Post);
    assert_eq!(spec.query.len(), 1);
    assert_eq!(spec.body, BodySpec::Json(r#"{"name":"Ada"}"#.into()));
}
