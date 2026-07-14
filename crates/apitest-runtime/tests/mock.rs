use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use apitest_core::{HttpMethod, KeyValue};
use apitest_runtime::{MockRoute, MockServer};

#[tokio::test]
async fn serves_configured_routes_and_returns_not_found_for_others() {
    let route = MockRoute {
        method: HttpMethod::Get,
        path: "/users/7".into(),
        status: 200,
        headers: vec![KeyValue::enabled("content-type", "application/json")],
        body: r#"{"id":7}"#.into(),
        delay_ms: 0,
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
