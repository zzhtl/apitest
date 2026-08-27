use apitest_core::{BodySpec, GrpcSpec, HttpSpec, WebSocketSpec};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeLanguage {
    Curl,
    RustReqwest,
    JavaScriptFetch,
    PythonRequests,
}

pub fn generate_code(spec: &HttpSpec, language: CodeLanguage) -> String {
    match language {
        CodeLanguage::Curl => curl(spec),
        CodeLanguage::RustReqwest => rust_reqwest(spec),
        CodeLanguage::JavaScriptFetch => javascript_fetch(spec),
        CodeLanguage::PythonRequests => python_requests(spec),
    }
}

fn curl(spec: &HttpSpec) -> String {
    let mut command = format!(
        "curl --request {} --url {}",
        spec.method,
        shell_quote(&render_url(spec))
    );
    for header in spec.headers.iter().filter(|header| header.enabled) {
        command.push_str(&format!(
            " \\\n  --header {}",
            shell_quote(&format!("{}: {}", header.name, header.value))
        ));
    }
    if let Some(body) = text_body(&spec.body) {
        command.push_str(&format!(" \\\n  --data {}", shell_quote(body)));
    }
    command
}

fn rust_reqwest(spec: &HttpSpec) -> String {
    let mut request = format!(
        "let client = reqwest::Client::new();\nlet response = client\n    .request(reqwest::Method::{}, {:?})",
        spec.method,
        render_url(spec)
    );
    for header in spec.headers.iter().filter(|header| header.enabled) {
        request.push_str(&format!(
            "\n    .header({:?}, {:?})",
            header.name, header.value
        ));
    }
    match &spec.body {
        BodySpec::Json(body) => {
            let json = serde_json::from_str::<serde_json::Value>(body)
                .map(|value| value.to_string())
                .unwrap_or_else(|_| "null".into());
            request.push_str(&format!("\n    .json(&serde_json::json!({json}))"));
        }
        body if text_body(body).is_some() => {
            request.push_str(&format!("\n    .body({:?})", text_body(body).unwrap()));
        }
        _ => {}
    }
    request.push_str("\n    .send()\n    .await?;");
    request
}

fn javascript_fetch(spec: &HttpSpec) -> String {
    let headers = spec
        .headers
        .iter()
        .filter(|header| header.enabled)
        .map(|header| (header.name.as_str(), header.value.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut options = format!(
        "{{\n  method: {:?},\n  headers: {}",
        spec.method.to_string(),
        serde_json::to_string_pretty(&headers).unwrap_or_else(|_| "{}".into())
    );
    if let Some(body) = text_body(&spec.body) {
        options.push_str(&format!(",\n  body: {:?}", body));
    }
    options.push_str("\n}");
    format!(
        "const response = await fetch({:?}, {options});\nconst data = await response.json();",
        render_url(spec)
    )
}

fn python_requests(spec: &HttpSpec) -> String {
    let headers = spec
        .headers
        .iter()
        .filter(|header| header.enabled)
        .map(|header| (header.name.as_str(), header.value.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let body = text_body(&spec.body)
        .map(|body| format!(",\n    data={body:?}"))
        .unwrap_or_default();
    format!(
        "import requests\n\nresponse = requests.request(\n    {:?},\n    {:?},\n    headers={headers:?}{body}\n)\nprint(response.text)",
        spec.method.to_string(),
        render_url(spec)
    )
}

/// Snippets for a WebSocket session. `Curl` maps to a `websocat` command
/// (the closest command-line equivalent); other languages have no meaningful
/// WebSocket form and return `None` so the UI can hide them.
pub fn generate_websocket_code(spec: &WebSocketSpec, language: CodeLanguage) -> Option<String> {
    match language {
        CodeLanguage::Curl => Some(websocat(spec)),
        CodeLanguage::JavaScriptFetch => Some(websocket_javascript(spec)),
        CodeLanguage::RustReqwest | CodeLanguage::PythonRequests => None,
    }
}

/// Snippets for a gRPC call: only a `grpcurl` command line is generated.
pub fn generate_grpc_code(spec: &GrpcSpec, language: CodeLanguage) -> Option<String> {
    match language {
        CodeLanguage::Curl => Some(grpcurl(spec)),
        _ => None,
    }
}

fn websocat(spec: &WebSocketSpec) -> String {
    let mut command = format!("websocat {}", shell_quote(&render_websocket_url(spec)));
    for header in spec.headers.iter().filter(|header| header.enabled) {
        command.push_str(&format!(
            " \\\n  -H {}",
            shell_quote(&format!("{}: {}", header.name, header.value))
        ));
    }
    if !spec.subprotocols.is_empty() {
        command.push_str(&format!(
            " \\\n  --protocol {}",
            shell_quote(&spec.subprotocols.join(","))
        ));
    }
    command
}

fn websocket_javascript(spec: &WebSocketSpec) -> String {
    let url = render_websocket_url(spec);
    let mut script = String::new();
    if spec.headers.iter().any(|header| header.enabled) {
        script.push_str(
            "// Browsers cannot set custom handshake headers; use the websocat snippet for those.\n",
        );
    }
    if spec.subprotocols.is_empty() {
        script.push_str(&format!("const socket = new WebSocket({url:?});\n"));
    } else {
        script.push_str(&format!(
            "const socket = new WebSocket({url:?}, {subprotocols});\n",
            subprotocols = serde_json::to_string(&spec.subprotocols).unwrap_or_default()
        ));
    }
    script.push_str(
        "socket.addEventListener(\"open\", () => socket.send(\"hello\"));\n\
         socket.addEventListener(\"message\", (event) => console.log(event.data));\n\
         socket.addEventListener(\"close\", (event) => console.log(\"closed\", event.code));",
    );
    script
}

fn grpcurl(spec: &GrpcSpec) -> String {
    let endpoint = spec
        .endpoint
        .strip_prefix("http://")
        .or_else(|| spec.endpoint.strip_prefix("https://"))
        .unwrap_or(&spec.endpoint)
        .trim_end_matches('/');
    let plaintext = spec.endpoint.starts_with("http://") || !spec.validate_tls;
    let mut command = "grpcurl".to_owned();
    if plaintext {
        command.push_str(" -plaintext");
    }
    for entry in spec.metadata.iter().filter(|entry| entry.enabled) {
        command.push_str(&format!(
            " \\\n  -H {}",
            shell_quote(&format!("{}: {}", entry.name, entry.value))
        ));
    }
    if !spec.message_json.trim().is_empty() {
        command.push_str(&format!(" \\\n  -d {}", shell_quote(&spec.message_json)));
    }
    if !spec.use_reflection {
        for path in &spec.import_paths {
            command.push_str(&format!(
                " \\\n  -import-path {}",
                shell_quote(&path.display().to_string())
            ));
        }
        for path in &spec.proto_files {
            command.push_str(&format!(
                " \\\n  -proto {}",
                shell_quote(&path.display().to_string())
            ));
        }
        if let Some(path) = &spec.descriptor_set {
            command.push_str(&format!(
                " \\\n  -protoset {}",
                shell_quote(&path.display().to_string())
            ));
        }
    }
    command.push_str(&format!(
        " \\\n  {} {}/{}",
        shell_quote(endpoint),
        spec.service,
        spec.method
    ));
    command
}

fn render_websocket_url(spec: &WebSocketSpec) -> String {
    let Ok(mut url) = Url::parse(&spec.url) else {
        return spec.url.clone();
    };
    {
        let mut query = url.query_pairs_mut();
        for parameter in spec.query.iter().filter(|parameter| parameter.enabled) {
            query.append_pair(&parameter.name, &parameter.value);
        }
    }
    url.into()
}

fn text_body(body: &BodySpec) -> Option<&str> {
    match body {
        BodySpec::Json(value) | BodySpec::Text(value) | BodySpec::Xml(value) => Some(value),
        _ => None,
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn render_url(spec: &HttpSpec) -> String {
    let Ok(mut url) = Url::parse(&spec.url) else {
        return spec.url.clone();
    };
    {
        let mut query = url.query_pairs_mut();
        for parameter in spec.query.iter().filter(|parameter| parameter.enabled) {
            query.append_pair(&parameter.name, &parameter.value);
        }
    }
    url.into()
}
