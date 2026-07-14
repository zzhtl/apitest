use apitest_core::{BodySpec, HttpSpec};

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
        shell_quote(&spec.url)
    );
    for header in spec.headers.iter().filter(|header| header.enabled) {
        command.push_str(&format!(
            " \\\n+  --header {}",
            shell_quote(&format!("{}: {}", header.name, header.value))
        ));
    }
    if let Some(body) = text_body(&spec.body) {
        command.push_str(&format!(" \\\n+  --data {}", shell_quote(body)));
    }
    command
}

fn rust_reqwest(spec: &HttpSpec) -> String {
    let mut request = format!(
        "let client = reqwest::Client::new();\nlet response = client\n    .request(reqwest::Method::{}, {:?})",
        spec.method, spec.url
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
        spec.url
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
        spec.url
    )
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
