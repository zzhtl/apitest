use std::time::Duration;

use apitest_core::{SecretRef, Variable};
use egui_kittest::Harness;

use super::support::test_app;
use crate::draft::{AuthDraft, AuthMode, ProxyDraft};
use crate::environment::EnvironmentDraft;
use crate::persistence::StorageEvent;

#[test]
fn failed_database_delete_does_not_remove_request_secrets() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let request_id = harness.state().requests[0].id();
    let reference = SecretRef::new(format!("keyring://request-{request_id}-local"));
    harness
        .state()
        .secrets
        .set(&reference, "keep-me")
        .expect("request secret should save");
    harness.state_mut().requests[0]
        .request_case
        .local_variables
        .push(Variable::secret("token", reference.0.clone()));
    harness.state_mut().database = None;

    harness.state_mut().delete_request(request_id);

    assert_eq!(harness.state().requests.len(), 1);
    assert_eq!(
        harness
            .state()
            .secrets
            .get(&reference)
            .expect("secret lookup should succeed")
            .as_deref(),
        Some("keep-me")
    );
}

#[test]
fn stale_environment_receipts_keep_secrets_used_by_the_current_draft() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let environment_id = harness.state().environments[0].id();
    let reference = SecretRef::new(format!(
        "keyring://environment-{environment_id}-variable-shared"
    ));
    harness
        .state()
        .secrets
        .set(&reference, "keep-current")
        .expect("environment secret should save");
    let mut saved = harness.state().environments[0].saved.clone();
    saved.variables = vec![Variable::secret("token", reference.0.clone())];
    harness.state_mut().environments[0] = EnvironmentDraft::from_environment(saved);
    {
        let variable = &mut harness.state_mut().environments[0].variables[0];
        variable.secret = false;
        variable.value = "plain".into();
        variable.reference = None;
        variable.configured = false;
    }
    assert!(harness.state_mut().queue_environment_save(0, true, false));
    {
        let variable = &mut harness.state_mut().environments[0].variables[0];
        variable.secret = true;
        variable.value.clear();
        variable.reference = Some(reference.clone());
        variable.configured = true;
    }

    assert!(harness.state_mut().settle_storage());

    assert_eq!(
        harness
            .state()
            .secrets
            .get(&reference)
            .expect("secret lookup should succeed")
            .as_deref(),
        Some("keep-current")
    );
}

#[test]
fn discarding_a_queued_environment_edit_preserves_restored_secrets() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let environment_id = harness.state().environments[0].id();
    let reference = SecretRef::new(format!(
        "keyring://environment-{environment_id}-variable-restored"
    ));
    harness
        .state()
        .secrets
        .set(&reference, "restore-me")
        .expect("environment secret should save");
    let mut saved = harness.state().environments[0].saved.clone();
    saved.variables = vec![Variable::secret("token", reference.0.clone())];
    harness.state_mut().environments[0] = EnvironmentDraft::from_environment(saved);
    {
        let variable = &mut harness.state_mut().environments[0].variables[0];
        variable.secret = false;
        variable.value = "plain".into();
        variable.reference = None;
        variable.configured = false;
    }
    assert!(harness.state_mut().queue_environment_save(0, true, false));

    assert!(harness.state_mut().discard_environment(0));

    assert_eq!(
        harness
            .state()
            .secrets
            .get(&reference)
            .expect("secret lookup should succeed")
            .as_deref(),
        Some("restore-me")
    );
    assert_eq!(
        harness.state().environments[0].saved.variables[0]
            .secret_ref
            .as_ref(),
        Some(&reference)
    );
}

#[test]
fn auth_and_proxy_plaintext_is_moved_to_the_secret_store_before_save() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let request_id = harness.state().requests[0].id();
    let references = {
        let request = &mut harness.state_mut().requests[0];
        request.draft.auth = AuthDraft::for_mode(AuthMode::AwsSigV4, request_id);
        let AuthDraft::AwsSigV4 {
            access_key,
            secret_key,
            session_token,
            region,
            service,
        } = &mut request.draft.auth
        else {
            panic!("AWS authentication draft should be created");
        };
        access_key.replacement = "AKIA-LOCAL".into();
        secret_key.replacement = "aws-secret-local".into();
        session_token.replacement = "aws-session-local".into();
        *region = "us-east-1".into();
        *service = "execute-api".into();
        let mut proxy = ProxyDraft::new(request_id);
        proxy.url = "http://127.0.0.1:8080".into();
        proxy.username = "proxy-user".into();
        proxy.password.replacement = "proxy-secret-local".into();
        let references = vec![
            (access_key.reference.clone(), "AKIA-LOCAL"),
            (secret_key.reference.clone(), "aws-secret-local"),
            (session_token.reference.clone(), "aws-session-local"),
            (proxy.password.reference.clone(), "proxy-secret-local"),
        ];
        request.draft.proxy = Some(proxy);
        references
    };

    assert!(harness.state_mut().save_current_request());
    let event = harness
        .state()
        .storage_worker
        .as_ref()
        .expect("storage worker should exist")
        .recv_timeout(Duration::from_secs(1))
        .expect("save should complete");
    let StorageEvent::RequestSaved { request_case, .. } = event else {
        panic!("request save should be acknowledged");
    };
    let document =
        serde_json::to_string(&request_case).expect("saved request case should serialize");

    for (reference, plaintext) in references {
        assert_eq!(
            harness
                .state()
                .secrets
                .get(&reference)
                .expect("secret store should be readable")
                .as_deref(),
            Some(plaintext)
        );
        assert!(!document.contains(plaintext));
    }
    assert!(!harness.state().requests[0].draft.has_pending_secret());
}
