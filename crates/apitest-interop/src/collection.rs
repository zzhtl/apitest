use std::{collections::HashMap, str::FromStr};

use apitest_core::{
    ApiDefinition, BodySpec, FormField, HttpMethod, HttpSpec, KeyValue, MultipartPart, Project,
    ProtocolSpec,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};
use url::Url;

use crate::{CodeLanguage, InteropError, generate_code};

pub const IMPORTED_AUTH_MARKER: &str = "apitest.imported_auth";

pub fn import_curl(source: &str) -> Result<ApiDefinition, InteropError> {
    let tokens = shell_words(source)?;
    if tokens.first().map(String::as_str) != Some("curl") {
        return Err(InteropError::Unsupported(
            "cURL command must start with `curl`".into(),
        ));
    }
    let mut method = None;
    let mut url = None;
    let mut headers = Vec::new();
    let mut body = None;
    let mut form = Vec::new();
    let mut validate_tls = true;
    let mut timeout_ms = 30_000;
    let mut index = 1;
    while index < tokens.len() {
        let token = &tokens[index];
        let mut take_value = |option: &str| -> Result<String, InteropError> {
            index += 1;
            tokens.get(index).cloned().ok_or_else(|| {
                InteropError::Unsupported(format!("cURL option `{option}` requires a value"))
            })
        };
        match token.as_str() {
            "-X" | "--request" => {
                let value = take_value(token)?;
                method = Some(
                    HttpMethod::from_str(&value)
                        .map_err(|error| InteropError::Unsupported(error.to_string()))?,
                );
            }
            "--url" => url = Some(take_value(token)?),
            "-H" | "--header" => {
                let value = take_value(token)?;
                let (name, value) = value.split_once(':').ok_or_else(|| {
                    InteropError::Unsupported(format!("invalid cURL header `{value}`"))
                })?;
                headers.push(KeyValue::enabled(name.trim(), value.trim()));
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" => {
                body = Some(take_value(token)?);
            }
            "-F" | "--form" => {
                let value = take_value(token)?;
                let (name, value) = value.split_once('=').ok_or_else(|| {
                    InteropError::Unsupported(format!("invalid cURL form part `{value}`"))
                })?;
                if let Some(path) = value.strip_prefix('@') {
                    form.push(MultipartPart::File {
                        name: name.into(),
                        path: path.into(),
                        content_type: None,
                        enabled: true,
                    });
                } else {
                    form.push(MultipartPart::Text(FormField {
                        name: name.into(),
                        value: value.into(),
                        enabled: true,
                    }));
                }
            }
            "-u" | "--user" => {
                let credentials = take_value(token)?;
                headers.push(KeyValue::enabled(
                    "authorization",
                    format!("Basic {}", STANDARD.encode(credentials)),
                ));
            }
            "-b" | "--cookie" => {
                headers.push(KeyValue::enabled("cookie", take_value(token)?));
            }
            "-k" | "--insecure" => validate_tls = false,
            "-m" | "--max-time" => {
                let seconds = take_value(token)?.parse::<f64>().map_err(|error| {
                    InteropError::Unsupported(format!("invalid cURL timeout: {error}"))
                })?;
                timeout_ms = (seconds.max(0.001) * 1_000.0) as u64;
            }
            value if value.starts_with('-') => {
                return Err(InteropError::Unsupported(format!(
                    "unsupported cURL option `{value}`"
                )));
            }
            value => {
                if url.is_none() {
                    url = Some(value.into());
                } else {
                    return Err(InteropError::Unsupported(format!(
                        "unexpected cURL argument `{value}`"
                    )));
                }
            }
        }
        index += 1;
    }

    let raw_url = url.ok_or_else(|| InteropError::Unsupported("cURL URL is missing".into()))?;
    let inferred_method = if body.is_some() || !form.is_empty() {
        HttpMethod::Post
    } else {
        HttpMethod::Get
    };
    let mut spec = HttpSpec::new(method.unwrap_or(inferred_method), "");
    set_url_and_query(&mut spec, &raw_url);
    spec.headers = headers;
    spec.validate_tls = validate_tls;
    spec.timeout_ms = timeout_ms;
    spec.body = if !form.is_empty() {
        BodySpec::Multipart(form)
    } else if let Some(body) = body {
        let content_type = spec
            .headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case("content-type"))
            .map(|header| header.value.to_ascii_lowercase());
        match content_type.as_deref() {
            Some(value) if value.contains("json") => BodySpec::Json(body),
            Some(value) if value.contains("xml") => BodySpec::Xml(body),
            _ => BodySpec::Text(body),
        }
    } else {
        BodySpec::None
    };
    let name = format!("{} {}", spec.method, display_path(&spec.url));
    Ok(ApiDefinition::new(name, ProtocolSpec::Http(spec)))
}

pub fn export_curl(definition: &ApiDefinition) -> Result<String, InteropError> {
    let ProtocolSpec::Http(spec) = definition.contract.example_protocol() else {
        return Err(InteropError::Unsupported(
            "only HTTP definitions can be exported as cURL".into(),
        ));
    };
    Ok(generate_code(&spec, CodeLanguage::Curl))
}

pub fn import_postman(source: &str) -> Result<Vec<ApiDefinition>, InteropError> {
    let document: Value = serde_json::from_str(source)?;
    let schema = document
        .pointer("/info/schema")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !schema.contains("collection/v2.") && document.get("item").is_none() {
        return Err(InteropError::Unsupported(
            "document is not a Postman collection".into(),
        ));
    }
    let mut definitions = Vec::new();
    walk_postman_items(
        document
            .get("item")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new()),
        &[],
        effective_postman_auth(document.get("auth"), None),
        &mut definitions,
    )?;
    Ok(definitions)
}

pub fn export_postman(
    project: &Project,
    definitions: &[ApiDefinition],
) -> Result<String, InteropError> {
    let items = definitions
        .iter()
        .filter_map(|definition| {
            let ProtocolSpec::Http(spec) = definition.contract.example_protocol() else {
                return None;
            };
            let mut headers = spec
                .headers
                .iter()
                .map(|header| {
                    json!({
                        "key": header.name,
                        "value": header.value,
                        "description": header.description,
                        "disabled": !header.enabled,
                    })
                })
                .collect::<Vec<_>>();
            for cookie in &spec.cookies {
                headers.push(json!({
                    "key": "Cookie",
                    "value": format!("{}={}", cookie.name, cookie.value),
                    "description": cookie.description,
                    "disabled": !cookie.enabled,
                }));
            }
            Some(json!({
                "name": definition.name,
                "request": {
                    "method": spec.method.to_string(),
                    "header": headers,
                    "url": {
                        "raw": url_with_query(&spec),
                        "query": spec.query.iter().map(|query| json!({
                            "key": query.name,
                            "value": query.value,
                            "description": query.description,
                            "disabled": !query.enabled,
                        })).collect::<Vec<_>>(),
                    },
                    "description": definition.description_markdown,
                    "body": export_postman_body(&spec.body),
                }
            }))
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string_pretty(&json!({
        "info": {
            "name": project.name,
            "description": project.description,
            "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
        },
        "item": items
    }))?)
}

pub fn import_har(source: &str) -> Result<Vec<ApiDefinition>, InteropError> {
    let document: Value = serde_json::from_str(source)?;
    let entries = document
        .pointer("/log/entries")
        .and_then(Value::as_array)
        .ok_or_else(|| InteropError::Unsupported("document is not an HTTP Archive".into()))?;
    let mut definitions = Vec::new();
    for entry in entries {
        let Some(request) = entry.get("request").and_then(Value::as_object) else {
            continue;
        };
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .and_then(|value| HttpMethod::from_str(value).ok())
            .unwrap_or(HttpMethod::Get);
        let raw_url = request
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut spec = HttpSpec::new(method, "");
        set_url_and_query(&mut spec, raw_url);
        spec.headers = import_key_values(request.get("headers"), "name", "value", "comment");
        spec.cookies = import_key_values(request.get("cookies"), "name", "value", "comment");
        if request.get("queryString").is_some() {
            spec.query = import_key_values(request.get("queryString"), "name", "value", "comment");
        }
        spec.body = import_har_body(request.get("postData"));
        let name = format!("{} {}", method, display_path(&spec.url));
        definitions.push(ApiDefinition::new(name, ProtocolSpec::Http(spec)));
    }
    Ok(definitions)
}

pub fn export_har(definitions: &[ApiDefinition]) -> Result<String, InteropError> {
    let entries = definitions
        .iter()
        .filter_map(|definition| {
            let ProtocolSpec::Http(spec) = definition.contract.example_protocol() else {
                return None;
            };
            Some(json!({
                "startedDateTime": definition.updated_at.to_rfc3339(),
                "time": 0,
                "request": {
                    "method": spec.method.to_string(),
                    "url": url_with_query(&spec),
                    "httpVersion": "HTTP/1.1",
                    "headers": spec.headers.iter().filter(|header| header.enabled).map(|header| json!({
                        "name": header.name,
                        "value": header.value,
                        "comment": header.description,
                    })).collect::<Vec<_>>(),
                    "queryString": spec.query.iter().filter(|parameter| parameter.enabled).map(|parameter| json!({
                        "name": parameter.name,
                        "value": parameter.value,
                        "comment": parameter.description,
                    })).collect::<Vec<_>>(),
                    "cookies": spec.cookies.iter().filter(|cookie| cookie.enabled).map(|cookie| json!({
                        "name": cookie.name,
                        "value": cookie.value,
                        "comment": cookie.description,
                    })).collect::<Vec<_>>(),
                    "headersSize": -1,
                    "bodySize": body_size(&spec.body),
                    "postData": export_har_body(&spec.body),
                },
                "response": {
                    "status": 0,
                    "statusText": "",
                    "httpVersion": "HTTP/1.1",
                    "headers": [],
                    "cookies": [],
                    "content": { "size": 0, "mimeType": "application/octet-stream" },
                    "redirectURL": "",
                    "headersSize": -1,
                    "bodySize": -1
                },
                "cache": {},
                "timings": { "send": 0, "wait": 0, "receive": 0 }
            }))
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string_pretty(&json!({
        "log": {
            "version": "1.2",
            "creator": { "name": "ApiTest", "version": env!("CARGO_PKG_VERSION") },
            "entries": entries
        }
    }))?)
}

fn walk_postman_items(
    items: &[Value],
    folders: &[String],
    inherited_auth: Option<&Value>,
    definitions: &mut Vec<ApiDefinition>,
) -> Result<(), InteropError> {
    for item in items {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Imported request")
            .to_owned();
        if let Some(children) = item.get("item").and_then(Value::as_array) {
            let mut nested = folders.to_vec();
            nested.push(name);
            let folder_auth = effective_postman_auth(item.get("auth"), inherited_auth);
            walk_postman_items(children, &nested, folder_auth, definitions)?;
            continue;
        }
        let Some(request) = item.get("request") else {
            continue;
        };
        let request = if let Some(url) = request.as_str() {
            json!({ "method": "GET", "url": url })
        } else {
            request.clone()
        };
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .and_then(|value| HttpMethod::from_str(value).ok())
            .unwrap_or(HttpMethod::Get);
        let raw_url = postman_url(request.get("url"));
        let mut spec = HttpSpec::new(method, "");
        set_url_and_query(&mut spec, &raw_url);
        spec.headers = import_key_values(request.get("header"), "key", "value", "description");
        spec.cookies = take_cookie_headers(&mut spec.headers);
        if let Some(query) = request.pointer("/url/query") {
            spec.query = import_key_values(Some(query), "key", "value", "description");
        }
        let item_auth = effective_postman_auth(item.get("auth"), inherited_auth);
        let request_auth = effective_postman_auth(request.get("auth"), item_auth);
        import_postman_auth(request_auth, &mut spec);
        spec.body = import_postman_body(request.get("body"));
        let mut definition = ApiDefinition::new(name, ProtocolSpec::Http(spec));
        definition.description_markdown = request
            .get("description")
            .and_then(postman_description)
            .unwrap_or_default();
        definition.tags = folders.to_vec();
        definitions.push(definition);
    }
    Ok(())
}

fn import_postman_body(value: Option<&Value>) -> BodySpec {
    let Some(body) = value.and_then(Value::as_object) else {
        return BodySpec::None;
    };
    match body.get("mode").and_then(Value::as_str) {
        Some("raw") => {
            let raw = body.get("raw").and_then(Value::as_str).unwrap_or_default();
            let language = body
                .get("options")
                .and_then(|options| options.get("raw"))
                .and_then(|raw| raw.get("language"))
                .and_then(Value::as_str);
            match language {
                Some("json") => BodySpec::Json(raw.into()),
                Some("xml") => BodySpec::Xml(raw.into()),
                _ => BodySpec::Text(raw.into()),
            }
        }
        Some("urlencoded") => BodySpec::FormUrlEncoded(
            body.get("urlencoded")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|field| FormField {
                    name: field
                        .get("key")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    value: field
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    enabled: !field
                        .get("disabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
                .collect(),
        ),
        Some("formdata") => BodySpec::Multipart(
            body.get("formdata")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|part| {
                    let name = part
                        .get("key")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let enabled = !part
                        .get("disabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if part.get("type").and_then(Value::as_str) == Some("file") {
                        MultipartPart::File {
                            name,
                            path: part
                                .get("src")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .into(),
                            content_type: part
                                .get("contentType")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            enabled,
                        }
                    } else {
                        MultipartPart::Text(FormField {
                            name,
                            value: part
                                .get("value")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .into(),
                            enabled,
                        })
                    }
                })
                .collect(),
        ),
        Some("file") => body
            .get("file")
            .and_then(|file| file.get("src"))
            .and_then(Value::as_str)
            .map(|path| BodySpec::BinaryFile(path.into()))
            .unwrap_or(BodySpec::None),
        _ => BodySpec::None,
    }
}

fn export_postman_body(body: &BodySpec) -> Value {
    match body {
        BodySpec::None => Value::Null,
        BodySpec::Json(value) => json!({
            "mode": "raw",
            "raw": value,
            "options": { "raw": { "language": "json" } }
        }),
        BodySpec::Xml(value) => json!({
            "mode": "raw",
            "raw": value,
            "options": { "raw": { "language": "xml" } }
        }),
        BodySpec::Text(value) => json!({ "mode": "raw", "raw": value }),
        BodySpec::FormUrlEncoded(fields) => json!({
            "mode": "urlencoded",
            "urlencoded": fields.iter().map(|field| json!({
                "key": field.name,
                "value": field.value,
                "disabled": !field.enabled,
            })).collect::<Vec<_>>()
        }),
        BodySpec::Multipart(parts) => json!({
            "mode": "formdata",
            "formdata": parts.iter().map(|part| match part {
                MultipartPart::Text(field) => json!({
                    "key": field.name,
                    "value": field.value,
                    "type": "text",
                    "disabled": !field.enabled,
                }),
                MultipartPart::File { name, path, content_type, enabled } => json!({
                    "key": name,
                    "src": path,
                    "type": "file",
                    "contentType": content_type,
                    "disabled": !enabled,
                }),
            }).collect::<Vec<_>>()
        }),
        BodySpec::BinaryFile(path) => json!({ "mode": "file", "file": { "src": path } }),
    }
}

fn import_postman_auth(auth: Option<&Value>, spec: &mut HttpSpec) {
    let Some(auth) = auth.and_then(Value::as_object) else {
        return;
    };
    let kind = auth.get("type").and_then(Value::as_str).unwrap_or_default();
    let values = auth
        .get(kind)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some((
                entry.get("key")?.as_str()?,
                entry.get("value").map(value_text).unwrap_or_default(),
            ))
        })
        .collect::<HashMap<_, _>>();
    match kind {
        "bearer" => {
            if let Some(token) = values.get("token") {
                spec.headers.push(imported_auth_value(
                    "authorization",
                    format!("Bearer {token}"),
                ));
            }
        }
        "basic" => {
            let username = values.get("username").cloned().unwrap_or_default();
            let password = values.get("password").cloned().unwrap_or_default();
            spec.headers.push(imported_auth_value(
                "authorization",
                format!(
                    "Basic {}",
                    STANDARD.encode(format!("{username}:{password}"))
                ),
            ));
        }
        "apikey" => {
            let name = values.get("key").cloned().unwrap_or_default();
            let value = values.get("value").cloned().unwrap_or_default();
            if values.get("in").is_some_and(|location| location == "query") {
                spec.query.push(imported_auth_value(name, value));
            } else {
                spec.headers.push(imported_auth_value(name, value));
            }
        }
        "oauth2" => {
            if let Some(token) = values.get("accessToken") {
                spec.headers.push(imported_auth_value(
                    "authorization",
                    format!("Bearer {token}"),
                ));
            }
        }
        _ => {}
    }
}

fn imported_auth_value(name: impl Into<String>, value: impl Into<String>) -> KeyValue {
    KeyValue {
        name: name.into(),
        value: value.into(),
        enabled: true,
        description: IMPORTED_AUTH_MARKER.into(),
    }
}

fn import_har_body(value: Option<&Value>) -> BodySpec {
    let Some(body) = value.and_then(Value::as_object) else {
        return BodySpec::None;
    };
    let mime_type = body
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let text = body.get("text").and_then(Value::as_str).unwrap_or_default();
    if body.get("encoding").and_then(Value::as_str) == Some("file-path") {
        return if text.is_empty() {
            BodySpec::None
        } else {
            BodySpec::BinaryFile(text.into())
        };
    }
    if mime_type.contains("application/x-www-form-urlencoded") {
        return BodySpec::FormUrlEncoded(
            body.get("params")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|field| FormField {
                    name: field
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    value: field
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    enabled: true,
                })
                .collect(),
        );
    }
    if mime_type.contains("multipart/form-data") {
        return BodySpec::Multipart(
            body.get("params")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|part| {
                    let name = part
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    if let Some(path) = part.get("fileName").and_then(Value::as_str) {
                        MultipartPart::File {
                            name,
                            path: path.into(),
                            content_type: part
                                .get("contentType")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            enabled: true,
                        }
                    } else {
                        MultipartPart::Text(FormField {
                            name,
                            value: part
                                .get("value")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            enabled: true,
                        })
                    }
                })
                .collect(),
        );
    }
    if mime_type.contains("json") {
        BodySpec::Json(text.into())
    } else if mime_type.contains("xml") {
        BodySpec::Xml(text.into())
    } else if text.is_empty() {
        BodySpec::None
    } else {
        BodySpec::Text(text.into())
    }
}

fn export_har_body(body: &BodySpec) -> Value {
    match body {
        BodySpec::None => Value::Null,
        BodySpec::Json(text) => json!({ "mimeType": "application/json", "text": text }),
        BodySpec::Xml(text) => json!({ "mimeType": "application/xml", "text": text }),
        BodySpec::Text(text) => json!({ "mimeType": "text/plain", "text": text }),
        BodySpec::FormUrlEncoded(fields) => json!({
            "mimeType": "application/x-www-form-urlencoded",
            "params": fields.iter().filter(|field| field.enabled).map(|field| json!({
                "name": field.name,
                "value": field.value,
            })).collect::<Vec<_>>()
        }),
        BodySpec::Multipart(parts) => json!({
            "mimeType": "multipart/form-data",
            "params": parts.iter().filter_map(|part| match part {
                MultipartPart::Text(field) if field.enabled => Some(json!({
                    "name": field.name,
                    "value": field.value,
                })),
                MultipartPart::File { name, path, content_type, enabled: true } => Some(json!({
                    "name": name,
                    "fileName": path,
                    "contentType": content_type,
                })),
                _ => None,
            }).collect::<Vec<_>>()
        }),
        BodySpec::BinaryFile(path) => json!({
            "mimeType": "application/octet-stream",
            "text": path.display().to_string(),
            "encoding": "file-path"
        }),
    }
}

fn body_size(body: &BodySpec) -> i64 {
    match body {
        BodySpec::Json(value) | BodySpec::Text(value) | BodySpec::Xml(value) => value.len() as i64,
        BodySpec::None => 0,
        BodySpec::FormUrlEncoded(_) | BodySpec::Multipart(_) | BodySpec::BinaryFile(_) => -1,
    }
}

fn import_key_values(
    value: Option<&Value>,
    name_key: &str,
    value_key: &str,
    description_key: &str,
) -> Vec<KeyValue> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let name = entry.get(name_key)?.as_str()?;
            let mut pair = KeyValue::enabled(
                name,
                entry.get(value_key).map(value_text).unwrap_or_default(),
            );
            pair.enabled = !entry
                .get("disabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            pair.description = entry
                .get(description_key)
                .and_then(postman_description)
                .unwrap_or_default();
            Some(pair)
        })
        .collect()
}

fn set_url_and_query(spec: &mut HttpSpec, raw_url: &str) {
    let (url_without_query, query) = split_url_query(raw_url);
    spec.query.extend(
        query
            .into_iter()
            .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
            .map(|(name, value)| KeyValue::enabled(name.into_owned(), value.into_owned())),
    );
    let Ok(url) = Url::parse(&url_without_query) else {
        spec.url = url_without_query;
        return;
    };
    spec.url = url.into();
}

fn url_with_query(spec: &HttpSpec) -> String {
    let Ok(mut url) = Url::parse(&spec.url) else {
        let encoded = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(
                spec.query
                    .iter()
                    .filter(|pair| pair.enabled)
                    .map(|pair| (pair.name.as_str(), pair.value.as_str())),
            )
            .finish();
        return append_query(&spec.url, &encoded);
    };
    {
        let mut query = url.query_pairs_mut();
        for pair in spec.query.iter().filter(|pair| pair.enabled) {
            query.append_pair(&pair.name, &pair.value);
        }
    }
    url.into()
}

fn split_url_query(raw_url: &str) -> (String, Option<&str>) {
    let Some((base, query_and_fragment)) = raw_url.split_once('?') else {
        return (raw_url.to_owned(), None);
    };
    let (query, fragment) = query_and_fragment
        .split_once('#')
        .map_or((query_and_fragment, None), |(query, fragment)| {
            (query, Some(fragment))
        });
    let url = fragment.map_or_else(|| base.to_owned(), |fragment| format!("{base}#{fragment}"));
    (url, Some(query))
}

fn append_query(url: &str, query: &str) -> String {
    if query.is_empty() {
        return url.to_owned();
    }
    let (base, fragment) = url
        .split_once('#')
        .map_or((url, None), |(base, fragment)| (base, Some(fragment)));
    let separator = if base.contains('?') { '&' } else { '?' };
    let mut value = format!("{base}{separator}{query}");
    if let Some(fragment) = fragment {
        value.push('#');
        value.push_str(fragment);
    }
    value
}

fn effective_postman_auth<'a>(
    own: Option<&'a Value>,
    inherited: Option<&'a Value>,
) -> Option<&'a Value> {
    match own
        .and_then(Value::as_object)
        .and_then(|auth| auth.get("type"))
        .and_then(Value::as_str)
    {
        Some("inherit") => inherited,
        Some("noauth") => None,
        Some(_) => own,
        None => inherited,
    }
}

fn take_cookie_headers(headers: &mut Vec<KeyValue>) -> Vec<KeyValue> {
    let mut cookies = Vec::new();
    headers.retain(|header| {
        if !header.name.eq_ignore_ascii_case("cookie") {
            return true;
        }
        cookies.extend(header.value.split(';').filter_map(|cookie| {
            let (name, value) = cookie.trim().split_once('=')?;
            let mut pair = KeyValue::enabled(name.trim(), value.trim());
            pair.enabled = header.enabled;
            pair.description = header.description.clone();
            Some(pair)
        }));
        false
    });
    cookies
}

fn postman_url(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Object(value)) => value
            .get("raw")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| build_postman_url(value)),
        _ => String::new(),
    }
}

fn build_postman_url(value: &Map<String, Value>) -> String {
    let protocol = value
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or("https");
    let host = value
        .get("host")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(".");
    let path = value
        .get("path")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("/");
    format!("{protocol}://{host}/{path}")
}

fn postman_description(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned).or_else(|| {
        value
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
}

fn display_path(url: &str) -> String {
    Url::parse(url)
        .ok()
        .map(|url| url.path().to_owned())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| url.to_owned())
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn shell_words(source: &str) -> Result<Vec<String>, InteropError> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in source.chars() {
        if escaped {
            if character != '\n' && character != '\r' {
                current.push(character);
            }
            escaped = false;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if character == active {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            value if value.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            value => current.push(value),
        }
    }
    if quote.is_some() {
        return Err(InteropError::Unsupported(
            "unterminated quote in cURL command".into(),
        ));
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}
