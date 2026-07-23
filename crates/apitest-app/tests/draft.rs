use std::path::PathBuf;

use apitest_app::{AuthDraft, AuthMode, BodyMode, EditablePair, ProxyDraft, RequestDraft};
use apitest_core::{
    ApiKeyLocation, AuthSpec, BodySpec, EntityId, FormField, HttpMethod, HttpProxy, HttpSpec,
    KeyValue, MultipartPart, OAuth2Grant, SecretRef,
};

#[test]
fn request_draft_preserves_enabled_and_disabled_fields() {
    let mut draft = RequestDraft {
        method: HttpMethod::Post,
        url: "https://example.com/users".into(),
        body_mode: BodyMode::Json,
        body: r#"{"name":"Ada"}"#.into(),
        ..RequestDraft::default()
    };
    draft.query.push(EditablePair::new("page", "2"));
    let mut disabled = EditablePair::new("debug", "true");
    disabled.enabled = false;
    draft.query.push(disabled);

    let spec = draft.to_http_spec();

    assert_eq!(spec.method, HttpMethod::Post);
    assert_eq!(spec.query.len(), 2);
    assert!(!spec.query[1].enabled);
    assert_eq!(spec.body, BodySpec::Json(r#"{"name":"Ada"}"#.into()));
}

#[test]
fn every_supported_body_round_trips_without_data_loss() {
    let bodies = [
        BodySpec::None,
        BodySpec::Json(r#"{"ok":true}"#.into()),
        BodySpec::Text("plain".into()),
        BodySpec::Xml("<ok/>".into()),
        BodySpec::FormUrlEncoded(vec![FormField {
            name: "name".into(),
            value: "Ada".into(),
            enabled: false,
        }]),
        BodySpec::Multipart(vec![
            MultipartPart::Text(FormField {
                name: "label".into(),
                value: "avatar".into(),
                enabled: true,
            }),
            MultipartPart::File {
                name: "file".into(),
                path: PathBuf::from("/tmp/avatar.png"),
                content_type: Some("image/png".into()),
                enabled: false,
            },
        ]),
        BodySpec::BinaryFile(PathBuf::from("/tmp/archive.bin")),
    ];

    for body in bodies {
        let mut spec = HttpSpec::new(HttpMethod::Post, "https://example.test/upload");
        spec.body = body.clone();
        let draft = RequestDraft::from(&spec);

        assert_eq!(draft.to_http_spec().body, body);
    }
}

#[test]
fn supported_authentication_round_trips_as_secret_references() {
    let auth = [
        AuthSpec::None,
        AuthSpec::Basic {
            username: "ada".into(),
            password: SecretRef::new("keyring://basic"),
        },
        AuthSpec::Bearer {
            token: SecretRef::new("keyring://bearer"),
        },
        AuthSpec::ApiKey {
            name: "x-api-key".into(),
            value: SecretRef::new("keyring://api-key"),
            location: ApiKeyLocation::Query,
        },
        AuthSpec::OAuth2 {
            grant: OAuth2Grant::Password,
            authorization_url: None,
            token_url: "https://identity.example.test/token".into(),
            client_id: "desktop".into(),
            client_secret: Some(SecretRef::new("keyring://oauth-client")),
            scopes: vec!["read".into(), "write".into()],
            username: Some("ada".into()),
            password: Some(SecretRef::new("keyring://oauth-password")),
            access_token: Some(SecretRef::new("keyring://oauth-access-token")),
        },
        AuthSpec::Digest {
            username: "ada".into(),
            password: SecretRef::new("keyring://digest"),
        },
        AuthSpec::AwsSigV4 {
            access_key: SecretRef::new("keyring://aws-access"),
            secret_key: SecretRef::new("keyring://aws-secret"),
            session_token: Some(SecretRef::new("keyring://aws-session")),
            region: "us-east-1".into(),
            service: "execute-api".into(),
        },
    ];

    for auth in auth {
        let mut spec = HttpSpec::new(HttpMethod::Get, "https://example.test");
        spec.auth = auth.clone();
        let draft = RequestDraft::from(&spec);

        assert_eq!(draft.to_http_spec().auth, auth);
        assert!(
            draft
                .auth
                .secrets()
                .into_iter()
                .all(|secret| secret.replacement.is_empty())
        );
    }
}

#[test]
fn optional_oauth_secrets_remain_absent_until_configured() {
    let auth = AuthSpec::OAuth2 {
        grant: OAuth2Grant::ClientCredentials,
        authorization_url: None,
        token_url: "https://identity.example.test/token".into(),
        client_id: "desktop".into(),
        client_secret: None,
        scopes: Vec::new(),
        username: None,
        password: None,
        access_token: None,
    };
    let mut spec = HttpSpec::new(HttpMethod::Get, "https://example.test");
    spec.auth = auth.clone();

    let draft = RequestDraft::from(&spec);

    assert_eq!(draft.auth.mode(), AuthMode::OAuth2);
    assert_eq!(draft.to_http_spec().auth, auth);
}

#[test]
fn pending_auth_and_proxy_secrets_never_enter_the_http_document() {
    let request_id = EntityId::new();
    let mut draft = RequestDraft {
        auth: AuthDraft::for_mode(AuthMode::AwsSigV4, request_id),
        ..RequestDraft::default()
    };
    if let AuthDraft::AwsSigV4 {
        access_key,
        secret_key,
        session_token,
        region,
        service,
    } = &mut draft.auth
    {
        access_key.replacement = "AKIA-PLAINTEXT".into();
        secret_key.replacement = "secret-plaintext".into();
        session_token.replacement = "session-plaintext".into();
        *region = "us-east-1".into();
        *service = "execute-api".into();
    }
    let mut proxy = ProxyDraft::new(request_id);
    proxy.url = "http://127.0.0.1:8080".into();
    proxy.username = "proxy-user".into();
    proxy.password.replacement = "proxy-plaintext".into();
    draft.proxy = Some(proxy);

    let document =
        serde_json::to_string(&draft.to_http_spec()).expect("HTTP specification should serialize");

    for plaintext in [
        "AKIA-PLAINTEXT",
        "secret-plaintext",
        "session-plaintext",
        "proxy-plaintext",
    ] {
        assert!(!document.contains(plaintext));
    }
    assert!(document.contains("keyring://request-"));
}

#[test]
fn proxy_secret_references_round_trip_without_loading_values() {
    let mut spec = HttpSpec::new(HttpMethod::Get, "https://example.test");
    spec.proxy = Some(HttpProxy {
        url: "socks5://127.0.0.1:1080".into(),
        username: Some("ada".into()),
        password: Some(SecretRef::new("keyring://proxy")),
    });

    let draft = RequestDraft::from(&spec);

    assert_eq!(draft.to_http_spec().proxy, spec.proxy);
    assert!(
        draft
            .proxy
            .as_ref()
            .is_some_and(|proxy| proxy.password.replacement.is_empty())
    );
}

#[test]
fn empty_placeholder_rows_are_not_persisted() {
    let mut draft = RequestDraft::default();
    draft
        .headers
        .push(EditablePair::new("accept", "application/json"));
    draft.query.push(EditablePair {
        enabled: false,
        name: "page".into(),
        value: "1".into(),
        description: "pagination".into(),
    });

    let spec = draft.to_http_spec();

    assert_eq!(
        spec.headers,
        vec![KeyValue::enabled("accept", "application/json")]
    );
    assert_eq!(spec.query.len(), 1);
    assert!(!spec.query[0].enabled);
}
