use apitest_core::{
    ApiContract, ApiParameterLocation, BodySpec, EntityId, MultipartPart, ProtocolSpec, SecretRef,
    Variable, VariableScope,
};
use apitest_interop::IMPORTED_AUTH_MARKER;
use apitest_storage::SecretStore;
use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::draft::{AuthDraft, BodyMode, MultipartValueMode, RequestDraft, SecretDraft};
use crate::state::workspace::{WorkspaceRequest, request_snapshot};

pub(crate) fn persist_request_draft_secrets(
    secrets: &dyn SecretStore,
    draft: &mut RequestDraft,
) -> Result<(), String> {
    let mut pending = draft
        .auth
        .secrets()
        .into_iter()
        .filter(|secret| secret.has_pending_value())
        .map(|secret| (secret.reference.clone(), secret.replacement.clone()))
        .collect::<Vec<_>>();
    if let Some(proxy) = &draft.proxy
        && proxy.password.has_pending_value()
    {
        pending.push((
            proxy.password.reference.clone(),
            proxy.password.replacement.clone(),
        ));
    }
    for (reference, value) in &pending {
        secrets
            .set(reference, value)
            .map_err(|error| error.to_string())?;
    }
    for secret in draft.auth.secrets_mut() {
        if secret.has_pending_value() {
            secret.mark_saved();
        }
    }
    if let Some(proxy) = draft.proxy.as_mut()
        && proxy.password.has_pending_value()
    {
        proxy.password.mark_saved();
    }
    Ok(())
}

pub(crate) fn secure_imported_request(
    request: &mut WorkspaceRequest,
    secrets: &dyn SecretStore,
) -> Result<Vec<SecretRef>, String> {
    let request_id = request.id();
    let mut pending = Vec::<(SecretRef, String)>::new();
    let mut protocol_changed = false;
    if let Some(index) = request
        .draft
        .headers
        .iter()
        .position(|header| header.enabled && header.name.eq_ignore_ascii_case("authorization"))
    {
        let value = request.draft.headers[index].value.trim().to_owned();
        if !is_variable_template(&value)
            && let Some((scheme, credential)) = value.split_once(' ')
        {
            if scheme.eq_ignore_ascii_case("bearer") && !credential.trim().is_empty() {
                let reference = imported_secret_ref(request_id, "bearer-token");
                pending.push((reference.clone(), credential.trim().to_owned()));
                request.draft.auth = AuthDraft::Bearer {
                    token: SecretDraft {
                        reference,
                        replacement: credential.trim().to_owned(),
                        configured: false,
                    },
                };
                request.draft.headers.remove(index);
            } else if scheme.eq_ignore_ascii_case("basic")
                && let Ok(decoded) = STANDARD.decode(credential.trim())
                && let Ok(decoded) = String::from_utf8(decoded)
                && let Some((username, password)) = decoded.split_once(':')
            {
                let reference = imported_secret_ref(request_id, "basic-password");
                pending.push((reference.clone(), password.to_owned()));
                request.draft.auth = AuthDraft::Basic {
                    username: username.to_owned(),
                    password: SecretDraft {
                        reference,
                        replacement: password.to_owned(),
                        configured: false,
                    },
                };
                request.draft.headers.remove(index);
            }
        }
    }

    let mut secret_index = 0usize;
    for pair in &mut request.draft.headers {
        if sensitive_name(&pair.name) || pair.description == IMPORTED_AUTH_MARKER {
            externalize_imported_value(
                request_id,
                &mut secret_index,
                &mut pair.value,
                &mut request.request_case.local_variables,
                &mut pending,
            );
            if pair.description == IMPORTED_AUTH_MARKER {
                pair.description.clear();
                protocol_changed = true;
            }
        }
    }
    for pair in &mut request.draft.query {
        if sensitive_name(&pair.name) || pair.description == IMPORTED_AUTH_MARKER {
            externalize_imported_value(
                request_id,
                &mut secret_index,
                &mut pair.value,
                &mut request.request_case.local_variables,
                &mut pending,
            );
            if pair.description == IMPORTED_AUTH_MARKER {
                pair.description.clear();
                protocol_changed = true;
            }
        }
    }
    for pair in &mut request.draft.cookies {
        externalize_imported_value(
            request_id,
            &mut secret_index,
            &mut pair.value,
            &mut request.request_case.local_variables,
            &mut pending,
        );
    }
    for pair in &mut request.draft.form_fields {
        if sensitive_name(&pair.name) {
            externalize_imported_value(
                request_id,
                &mut secret_index,
                &mut pair.value,
                &mut request.request_case.local_variables,
                &mut pending,
            );
        }
    }
    for part in &mut request.draft.multipart_parts {
        if part.mode == MultipartValueMode::Text && sensitive_name(&part.name) {
            externalize_imported_value(
                request_id,
                &mut secret_index,
                &mut part.value,
                &mut request.request_case.local_variables,
                &mut pending,
            );
        }
    }
    if request.draft.body_mode == BodyMode::Json
        && let Ok(mut body) = serde_json::from_str::<serde_json::Value>(&request.draft.body)
    {
        let pending_before = pending.len();
        externalize_imported_json(
            &mut body,
            request_id,
            &mut secret_index,
            &mut request.request_case.local_variables,
            &mut pending,
        );
        if pending.len() > pending_before {
            request.draft.body = serde_json::to_string_pretty(&body)
                .expect("sanitized imported JSON body should serialize");
        }
    }
    let secured_body = match request.edited_protocol() {
        ProtocolSpec::Http(spec) => spec.body,
        _ => BodySpec::None,
    };
    secure_contract_body_examples(
        &mut request.definition.contract,
        &secured_body,
        request_id,
        &mut secret_index,
        &mut request.request_case.local_variables,
        &mut pending,
    );

    let mut saved = Vec::new();
    for (reference, value) in &pending {
        if let Err(error) = secrets.set(reference, value) {
            delete_secret_refs(secrets, &saved);
            return Err(error.to_string());
        }
        saved.push(reference.clone());
    }
    for secret in request.draft.auth.secrets_mut() {
        if pending
            .iter()
            .any(|(reference, _)| reference == &secret.reference)
        {
            secret.mark_saved();
        }
    }
    if !pending.is_empty() || protocol_changed {
        let protocol = request.edited_protocol();
        sync_contract_examples(&mut request.definition.contract, &protocol);
        request.request_case.protocol = protocol;
        request.sync_contract = false;
        request.observed_snapshot = request_snapshot(&request.name, request.edited_protocol());
    }
    Ok(saved)
}

pub(crate) fn secure_contract_body_examples(
    contract: &mut ApiContract,
    body: &BodySpec,
    request_id: EntityId,
    secret_index: &mut usize,
    variables: &mut Vec<Variable>,
    pending: &mut Vec<(SecretRef, String)>,
) {
    if matches!(body, BodySpec::None) {
        return;
    }
    let ApiContract::Http(contract) = contract else {
        return;
    };
    let Some(content) = contract.request_body.as_mut() else {
        return;
    };
    let mut examples = content.examples.values_mut();
    if let Some(first) = examples.next() {
        *first = body_example_value(body);
    }
    for example in examples {
        externalize_imported_json(example, request_id, secret_index, variables, pending);
    }
}

pub(crate) fn body_example_value(body: &BodySpec) -> serde_json::Value {
    match body {
        BodySpec::None => serde_json::Value::Null,
        BodySpec::Json(value) => {
            serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::String(value.clone()))
        }
        BodySpec::Text(value) | BodySpec::Xml(value) => serde_json::Value::String(value.clone()),
        BodySpec::FormUrlEncoded(fields) => serde_json::Value::Object(
            fields
                .iter()
                .filter(|field| field.enabled)
                .map(|field| {
                    (
                        field.name.clone(),
                        serde_json::Value::String(field.value.clone()),
                    )
                })
                .collect(),
        ),
        BodySpec::Multipart(parts) => serde_json::Value::Object(
            parts
                .iter()
                .filter_map(|part| match part {
                    MultipartPart::Text(field) if field.enabled => Some((
                        field.name.clone(),
                        serde_json::Value::String(field.value.clone()),
                    )),
                    MultipartPart::File {
                        name,
                        path,
                        enabled: true,
                        ..
                    } => Some((
                        name.clone(),
                        serde_json::Value::String(path.display().to_string()),
                    )),
                    _ => None,
                })
                .collect(),
        ),
        BodySpec::BinaryFile(path) => serde_json::Value::String(path.display().to_string()),
    }
}

pub(crate) fn sync_contract_examples(contract: &mut ApiContract, protocol: &ProtocolSpec) {
    let (ApiContract::Http(contract), ProtocolSpec::Http(spec)) = (contract, protocol) else {
        return;
    };
    contract.parameters.retain_mut(|parameter| {
        let values = match parameter.location {
            ApiParameterLocation::Query => &spec.query,
            ApiParameterLocation::Header => &spec.headers,
            ApiParameterLocation::Cookie => &spec.cookies,
            ApiParameterLocation::Path => return true,
        };
        if let Some(value) = values
            .iter()
            .find(|value| value.name.eq_ignore_ascii_case(&parameter.name))
        {
            parameter.example = Some(serde_json::Value::String(value.value.clone()));
            parameter.enabled = value.enabled;
            true
        } else {
            !sensitive_name(&parameter.name) && parameter.description != IMPORTED_AUTH_MARKER
        }
    });
    contract.body_example = spec.body.clone();
    contract.default_auth = spec.auth.clone();
    contract.timeout_ms = spec.timeout_ms;
    contract.follow_redirects = spec.follow_redirects;
    contract.validate_tls = spec.validate_tls;
    contract.proxy = spec.proxy.clone();
    contract.client_certificate = spec.client_certificate.clone();
}

pub(crate) fn imported_secret_ref(request_id: EntityId, suffix: &str) -> SecretRef {
    SecretRef::new(format!("keyring://request-{request_id}-{suffix}"))
}

pub(crate) fn externalize_imported_value(
    request_id: EntityId,
    index: &mut usize,
    value: &mut String,
    variables: &mut Vec<Variable>,
    pending: &mut Vec<(SecretRef, String)>,
) {
    if value.is_empty() || is_variable_template(value) {
        return;
    }
    let name = format!("__import_secret_{index}");
    let reference = imported_secret_ref(request_id, &format!("import-{index}"));
    pending.push((reference.clone(), std::mem::take(value)));
    let mut variable = Variable::secret(&name, reference.0.clone());
    variable.scope = VariableScope::Local;
    variables.push(variable);
    *value = format!("{{{{{name}}}}}");
    *index += 1;
}

pub(crate) fn externalize_imported_json(
    value: &mut serde_json::Value,
    request_id: EntityId,
    index: &mut usize,
    variables: &mut Vec<Variable>,
    pending: &mut Vec<(SecretRef, String)>,
) {
    match value {
        serde_json::Value::Object(object) => {
            for (name, value) in object {
                if sensitive_name(name) {
                    if let serde_json::Value::String(secret) = value {
                        externalize_imported_value(request_id, index, secret, variables, pending);
                    }
                } else {
                    externalize_imported_json(value, request_id, index, variables, pending);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                externalize_imported_json(value, request_id, index, variables, pending);
            }
        }
        _ => {}
    }
}

pub(crate) fn is_variable_template(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("{{") && value.ends_with("}}")
}

pub(crate) fn delete_secret_refs(secrets: &dyn SecretStore, references: &[SecretRef]) {
    for reference in references {
        if let Err(error) = secrets.delete(reference) {
            tracing::warn!(%error, "failed to clean imported secret");
        }
    }
}

pub(crate) fn sensitive_name(name: &str) -> bool {
    let normalized = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    [
        "authorization",
        "credential",
        "password",
        "passwd",
        "secret",
        "token",
        "apikey",
        "cookie",
    ]
    .iter()
    .any(|candidate| normalized.contains(candidate))
}
