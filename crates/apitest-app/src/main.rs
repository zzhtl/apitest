use apitest_app::{ApiTestApp, native_options};

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "apitest=info".into()),
        )
        .compact()
        .init();
    eframe::run_native(
        "ApiTest",
        native_options(),
        Box::new(|context| Ok(Box::new(ApiTestApp::new(context)))),
    )
}
