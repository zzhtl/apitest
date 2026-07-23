use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use apitest_core::{HttpMethod, KeyValue, MockRule};
use apitest_runtime::{MockRoute, MockServer};

#[tokio::test]
async fn serves_configured_routes_and_returns_not_found_for_others() {
    let route = MockRoute {
        priority: 0,
        method: HttpMethod::Get,
        path: "/users/7".into(),
        query: Vec::new(),
        request_headers: Vec::new(),
        body_contains: None,
        path_variables: Default::default(),
        status: 200,
        headers: vec![KeyValue::enabled("content-type", "application/json")],
        body: r#"{"id":7}"#.into(),
        delay_ms: 0,
        script: String::new(),
    };
    let server = MockServer::start(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        vec![route],
    )
    .await
    .expect("mock server should start");

    let response = reqwest::get(format!("{}/users/7", server.base_url()))
        .await
        .expect("request should complete");
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.text().await.expect("body should read"),
        r#"{"id":7}"#
    );

    let missing = reqwest::get(format!("{}/missing", server.base_url()))
        .await
        .expect("request should complete");
    assert_eq!(missing.status(), 404);

    server.shutdown().await;
}

#[tokio::test]
async fn preserves_duplicate_static_response_headers() {
    let mut rule = MockRule {
        name: "cookies".into(),
        path: "/cookies".into(),
        ..MockRule::default()
    };
    rule.response.headers = vec![
        KeyValue::enabled("set-cookie", "session=one; Path=/"),
        KeyValue::enabled("set-cookie", "theme=dark; Path=/"),
    ];
    let server = MockServer::start(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        vec![MockRoute::from(&rule)],
    )
    .await
    .expect("mock server should start");

    let response = reqwest::get(format!("{}/cookies", server.base_url()))
        .await
        .expect("request should complete");
    let cookies = response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().expect("cookie header should be valid"))
        .collect::<Vec<_>>();

    assert_eq!(cookies, ["session=one; Path=/", "theme=dark; Path=/"]);
    server.shutdown().await;
}

#[tokio::test]
async fn matches_templates_conditions_and_highest_priority() {
    let mut fallback = MockRule {
        name: "fallback".into(),
        path: "/users/{id}".into(),
        priority: 1,
        ..MockRule::default()
    };
    fallback.response.body = "fallback".into();
    let mut specific = MockRule {
        name: "specific".into(),
        method: HttpMethod::Post,
        path: "/users/{id}".into(),
        query: vec![KeyValue::enabled("draft", "true")],
        headers: vec![KeyValue::enabled("x-tenant", "dev")],
        body_contains: Some("Ada".into()),
        path_variables: indexmap::IndexMap::from([("id".into(), "7".into())]),
        priority: 10,
        ..MockRule::default()
    };
    specific.response.status = 201;
    specific.response.body = "specific".into();
    fallback.method = HttpMethod::Post;
    let server = MockServer::start(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        vec![MockRoute::from(&fallback), MockRoute::from(&specific)],
    )
    .await
    .expect("mock server should start");

    let response = reqwest::Client::new()
        .post(format!("{}/users/7?draft=true", server.base_url()))
        .header("x-tenant", "dev")
        .body(r#"{"name":"Ada"}"#)
        .send()
        .await
        .expect("request should complete");

    assert_eq!(response.status(), 201);
    assert_eq!(response.text().await.expect("body should read"), "specific");
    server.shutdown().await;
}

#[tokio::test]
async fn executes_dynamic_response_scripts() {
    let mut rule = MockRule {
        name: "dynamic".into(),
        path: "/users/{id}".into(),
        ..MockRule::default()
    };
    rule.response.body = "static".into();
    rule.response.script = r#"
        response.status = 202;
        response.headers["x-mock-script"] = "executed";
        response.body = JSON.stringify({ path: request.path });
    "#
    .into();
    let server = MockServer::start(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        vec![MockRoute::from(&rule)],
    )
    .await
    .expect("mock server should start");

    let response = reqwest::get(format!("{}/users/7", server.base_url()))
        .await
        .expect("request should complete");

    assert_eq!(response.status(), 202);
    assert_eq!(
        response
            .headers()
            .get("x-mock-script")
            .and_then(|value| value.to_str().ok()),
        Some("executed")
    );
    assert_eq!(
        response.text().await.expect("body should read"),
        r#"{"path":"/users/7"}"#
    );
    server.shutdown().await;
}
