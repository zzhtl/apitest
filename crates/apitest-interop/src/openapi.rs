use std::collections::{HashMap, HashSet};

use apitest_core::{
    ApiComponents, ApiContent, ApiContract, ApiDefinition, ApiParameter, ApiParameterLocation,
    ApiResponse, ApiSchema, ApiServer, ApiStatus, BodySpec, FormField, HttpContract, HttpMethod,
    Project, SecurityRequirement,
};
use serde_json::{Map, Value, json};

use crate::InteropError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenApiImport {
    pub title: String,
    pub version: String,
    pub definitions: Vec<ApiDefinition>,
    pub components: ApiComponents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenApiIssueLevel {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenApiIssue {
    pub level: OpenApiIssueLevel,
    pub path: String,
    pub message: String,
}

pub fn import_openapi(source: &str) -> Result<Vec<ApiDefinition>, InteropError> {
    Ok(import_openapi_document(source)?.definitions)
}

pub fn import_openapi_document(source: &str) -> Result<OpenApiImport, InteropError> {
    let document: Value = serde_yaml::from_str(source)?;
    document
        .get("openapi")
        .and_then(Value::as_str)
        .filter(|version| version.starts_with("3."))
        .ok_or_else(|| {
            InteropError::Unsupported("document is not an OpenAPI 3.x definition".into())
        })?;
    let title = document
        .pointer("/info/title")
        .and_then(Value::as_str)
        .unwrap_or("Imported API")
        .to_owned();
    let api_version = document
        .pointer("/info/version")
        .and_then(Value::as_str)
        .unwrap_or("1.0.0")
        .to_owned();
    let paths = document
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| InteropError::Unsupported("OpenAPI document has no paths".into()))?;
    let root_servers = import_servers(document.get("servers"));
    let root_security = import_security(document.get("security"));

    let mut definitions = Vec::new();
    for (path, item) in paths {
        let Some(item) = resolve_object(&document, item) else {
            continue;
        };
        for (method_name, method) in methods() {
            let Some(operation) = item
                .get(method_name)
                .and_then(|value| resolve_object(&document, value))
            else {
                continue;
            };
            definitions.push(import_operation(
                &document,
                path,
                method,
                item,
                operation,
                &root_servers,
                &root_security,
            ));
        }
    }

    Ok(OpenApiImport {
        title,
        version: api_version,
        definitions,
        components: import_components(&document),
    })
}

pub fn export_openapi(
    project: &Project,
    definitions: &[ApiDefinition],
) -> Result<String, InteropError> {
    let mut paths = Map::new();
    for definition in definitions {
        let ApiContract::Http(contract) = &definition.contract else {
            continue;
        };
        let (path, inferred_server) = export_path_and_server(&contract.path);
        let path_item = paths
            .entry(path.clone())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("path entries are initialized as objects");
        let method = method_name(contract.method);
        if path_item.contains_key(method) {
            return Err(InteropError::Unsupported(format!(
                "duplicate operation {method} {path}"
            )));
        }
        path_item.insert(
            method.into(),
            export_operation(definition, contract, inferred_server),
        );
    }

    let mut info = Map::new();
    info.insert("title".into(), Value::String(project.name.clone()));
    info.insert("version".into(), Value::String("1.0.0".into()));
    if !project.description.trim().is_empty() {
        info.insert(
            "description".into(),
            Value::String(project.description.clone()),
        );
    }
    let document = Value::Object(Map::from_iter([
        ("openapi".into(), Value::String("3.1.0".into())),
        ("info".into(), Value::Object(info)),
        ("paths".into(), Value::Object(paths)),
        ("components".into(), export_components(&project.components)),
    ]));
    Ok(serde_yaml::to_string(&document)?)
}

pub fn validate_openapi(source: &str) -> Result<Vec<OpenApiIssue>, InteropError> {
    let document: Value = serde_yaml::from_str(source)?;
    let mut issues = Vec::new();
    if !document
        .get("openapi")
        .and_then(Value::as_str)
        .is_some_and(|version| version.starts_with("3.1."))
    {
        error(
            &mut issues,
            "/openapi",
            "the document must use OpenAPI 3.1.x",
        );
    }
    for (pointer, label) in [("/info/title", "title"), ("/info/version", "version")] {
        if !document
            .pointer(pointer)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
        {
            error(
                &mut issues,
                pointer,
                &format!("info.{label} must be a non-empty string"),
            );
        }
    }
    let Some(paths) = document.get("paths").and_then(Value::as_object) else {
        error(&mut issues, "/paths", "paths must be an object");
        return Ok(issues);
    };
    let mut operation_ids = HashMap::<String, String>::new();
    for (path, item) in paths {
        let path_pointer = format!("/paths/{}", pointer_escape(path));
        if !path.starts_with('/') {
            error(
                &mut issues,
                &path_pointer,
                "an OpenAPI path must start with `/`",
            );
        }
        let Some(item) = item.as_object() else {
            error(&mut issues, &path_pointer, "path item must be an object");
            continue;
        };
        for (method, _) in methods() {
            let Some(operation) = item.get(method).and_then(Value::as_object) else {
                continue;
            };
            let operation_pointer = format!("{path_pointer}/{method}");
            match operation.get("responses").and_then(Value::as_object) {
                Some(responses) if !responses.is_empty() => {}
                _ => error(
                    &mut issues,
                    &format!("{operation_pointer}/responses"),
                    "an operation must define at least one response",
                ),
            }
            if let Some(operation_id) = operation.get("operationId").and_then(Value::as_str) {
                if let Some(first) =
                    operation_ids.insert(operation_id.to_owned(), operation_pointer.clone())
                {
                    error(
                        &mut issues,
                        &format!("{operation_pointer}/operationId"),
                        &format!("duplicate operationId `{operation_id}`; first used at {first}"),
                    );
                }
            } else {
                warning(
                    &mut issues,
                    &operation_pointer,
                    "operationId is recommended for stable code generation",
                );
            }
            validate_path_parameters(
                &document,
                path,
                item,
                operation,
                &operation_pointer,
                &mut issues,
            );
        }
    }
    Ok(issues)
}

pub fn export_openapi_documentation(project: &Project, definitions: &[ApiDefinition]) -> String {
    let mut operations = String::new();
    for definition in definitions {
        let ApiContract::Http(contract) = &definition.contract else {
            continue;
        };
        let responses = contract
            .responses
            .iter()
            .map(|response| {
                format!(
                    "<li><code>{}</code> {}</li>",
                    escape_html(&response.status),
                    escape_html(&response.description)
                )
            })
            .collect::<String>();
        operations.push_str(&format!(
            "<article class=\"operation\"><div class=\"route\"><span class=\"method {}\">{}</span><code>{}</code></div><h2>{}</h2><p>{}</p><h3>Responses</h3><ul>{}</ul></article>",
            method_name(contract.method),
            contract.method,
            escape_html(&contract.path),
            escape_html(&definition.name),
            escape_html(&definition.description_markdown),
            responses,
        ));
    }
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>{}</style></head><body><main><header><h1>{}</h1><p>{}</p></header>{}</main></body></html>",
        escape_html(&project.name),
        DOCUMENTATION_CSS,
        escape_html(&project.name),
        escape_html(&project.description),
        operations,
    )
}

fn methods() -> [(&'static str, HttpMethod); 9] {
    [
        ("get", HttpMethod::Get),
        ("post", HttpMethod::Post),
        ("put", HttpMethod::Put),
        ("patch", HttpMethod::Patch),
        ("delete", HttpMethod::Delete),
        ("head", HttpMethod::Head),
        ("options", HttpMethod::Options),
        ("trace", HttpMethod::Trace),
        ("connect", HttpMethod::Connect),
    ]
}

fn method_name(method: HttpMethod) -> &'static str {
    methods()
        .into_iter()
        .find_map(|(name, value)| (value == method).then_some(name))
        .expect("all HTTP methods have an OpenAPI name")
}

fn import_operation(
    document: &Value,
    path: &str,
    method: HttpMethod,
    path_item: &Map<String, Value>,
    operation: &Map<String, Value>,
    root_servers: &[ApiServer],
    root_security: &[SecurityRequirement],
) -> ApiDefinition {
    let servers = import_servers(operation.get("servers"));
    let servers = if servers.is_empty() {
        let path_servers = import_servers(path_item.get("servers"));
        if path_servers.is_empty() {
            root_servers.to_vec()
        } else {
            path_servers
        }
    } else {
        servers
    };
    let parameters = import_parameters(document, path_item, operation);
    let request_body = operation
        .get("requestBody")
        .and_then(|value| resolve_object(document, value))
        .map(|body| import_content_map(document, body.get("content")))
        .unwrap_or_default()
        .into_iter()
        .next();
    let body_example = request_body
        .as_ref()
        .map(body_example_from_content)
        .unwrap_or(BodySpec::None);
    let responses = import_responses(document, operation.get("responses"));
    let security = operation
        .get("security")
        .map(|value| import_security(Some(value)))
        .unwrap_or_else(|| root_security.to_vec());
    let mut extensions = HttpContract::default().extensions;
    for (name, value) in operation.iter().filter(|(name, _)| name.starts_with("x-")) {
        extensions.insert(name.clone(), value.clone());
    }
    if let Some(operation_id) = operation.get("operationId") {
        extensions.insert("operationId".into(), operation_id.clone());
    }
    if let Some(deprecated) = operation.get("deprecated") {
        extensions.insert("deprecated".into(), deprecated.clone());
    }
    let contract = HttpContract {
        method,
        path: path.to_owned(),
        servers,
        parameters,
        request_body,
        body_example,
        responses,
        security,
        extensions,
        ..HttpContract::default()
    };
    let name = operation
        .get("summary")
        .or_else(|| operation.get("operationId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{method} {path}"));
    let mut definition = ApiDefinition::new(name, ApiContract::Http(contract));
    definition.description_markdown = operation
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    definition.tags = operation
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    if operation
        .get("deprecated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        definition.status = ApiStatus::Deprecated;
    }
    definition
}

fn import_components(document: &Value) -> ApiComponents {
    let mut components = ApiComponents::default();
    if let Some(schemas) = document
        .pointer("/components/schemas")
        .and_then(Value::as_object)
    {
        for (name, schema) in schemas {
            components
                .schemas
                .insert(name.clone(), schema_from_value(schema));
        }
    }
    if let Some(examples) = document
        .pointer("/components/examples")
        .and_then(Value::as_object)
    {
        for (name, example) in examples {
            components.examples.insert(
                name.clone(),
                example.get("value").unwrap_or(example).clone(),
            );
        }
    }
    if let Some(schemes) = document
        .pointer("/components/securitySchemes")
        .and_then(Value::as_object)
    {
        for (name, scheme) in schemes {
            components
                .security_schemes
                .insert(name.clone(), scheme.clone());
        }
    }
    components
}

fn import_servers(value: Option<&Value>) -> Vec<ApiServer> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|server| {
            let url = server.get("url")?.as_str()?;
            let mut imported = ApiServer::new(url);
            imported.description = server
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if let Some(variables) = server.get("variables").and_then(Value::as_object) {
                for (name, variable) in variables {
                    imported.variables.insert(
                        name.clone(),
                        variable.get("default").map(value_text).unwrap_or_default(),
                    );
                }
            }
            Some(imported)
        })
        .collect()
}

fn import_parameters(
    document: &Value,
    path_item: &Map<String, Value>,
    operation: &Map<String, Value>,
) -> Vec<ApiParameter> {
    let mut imported = Vec::<ApiParameter>::new();
    for value in path_item
        .get("parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            operation
                .get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
    {
        let Some(parameter) = resolve_object(document, value) else {
            continue;
        };
        let Some(name) = parameter.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(location) = parameter
            .get("in")
            .and_then(Value::as_str)
            .and_then(parameter_location)
        else {
            continue;
        };
        let schema = parameter
            .get("schema")
            .map(schema_from_value)
            .unwrap_or_default();
        let imported_parameter = ApiParameter {
            name: name.to_owned(),
            location,
            required: location == ApiParameterLocation::Path
                || parameter
                    .get("required")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            enabled: parameter
                .get("x-apitest-enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            description: parameter
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            schema,
            example: parameter
                .get("example")
                .or_else(|| {
                    parameter
                        .get("schema")
                        .and_then(|schema| schema.get("example"))
                })
                .or_else(|| {
                    parameter
                        .get("schema")
                        .and_then(|schema| schema.get("default"))
                })
                .cloned(),
        };
        if let Some(index) = imported.iter().position(|existing| {
            existing.name == imported_parameter.name
                && existing.location == imported_parameter.location
        }) {
            imported[index] = imported_parameter;
        } else {
            imported.push(imported_parameter);
        }
    }
    imported
}

fn import_responses(document: &Value, value: Option<&Value>) -> Vec<ApiResponse> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(status, response)| {
            let response = resolve_object(document, response)?;
            let mut imported = ApiResponse::new(
                status,
                response
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            if let Some(headers) = response.get("headers").and_then(Value::as_object) {
                for (name, header) in headers {
                    let Some(header) = resolve_object(document, header) else {
                        continue;
                    };
                    imported.headers.push(ApiParameter {
                        name: name.clone(),
                        location: ApiParameterLocation::Header,
                        required: header
                            .get("required")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        enabled: header
                            .get("x-apitest-enabled")
                            .and_then(Value::as_bool)
                            .unwrap_or(true),
                        description: header
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        schema: header
                            .get("schema")
                            .map(schema_from_value)
                            .unwrap_or_default(),
                        example: header.get("example").cloned(),
                    });
                }
            }
            imported.content = import_content_map(document, response.get("content"));
            Some(imported)
        })
        .collect()
}

fn import_content_map(document: &Value, value: Option<&Value>) -> Vec<ApiContent> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(media_type, media)| {
            let mut content = ApiContent {
                media_type: media_type.clone(),
                schema: media
                    .get("schema")
                    .map(schema_from_value)
                    .unwrap_or_default(),
                examples: Default::default(),
            };
            if let Some(example) = media.get("example") {
                content.examples.insert("default".into(), example.clone());
            }
            if let Some(examples) = media.get("examples").and_then(Value::as_object) {
                for (name, example) in examples {
                    let example = resolve_value(document, example);
                    content.examples.insert(
                        name.clone(),
                        example.get("value").unwrap_or(example).clone(),
                    );
                }
            }
            if content.examples.is_empty()
                && let Some(example) = media
                    .pointer("/schema/example")
                    .or_else(|| media.pointer("/schema/default"))
            {
                content.examples.insert("default".into(), example.clone());
            }
            content
        })
        .collect()
}

fn import_security(value: Option<&Value>) -> Vec<SecurityRequirement> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|requirement| requirement.iter())
        .map(|(scheme, scopes)| SecurityRequirement {
            scheme: scheme.clone(),
            scopes: scopes
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
        })
        .collect()
}

fn body_example_from_content(content: &ApiContent) -> BodySpec {
    let Some(example) = content.examples.values().next() else {
        return if content.media_type == "application/json" || content.media_type.ends_with("+json")
        {
            BodySpec::Json("{}".into())
        } else {
            BodySpec::None
        };
    };
    match content.media_type.as_str() {
        "application/json" => BodySpec::Json(pretty_value(example)),
        media_type if media_type.ends_with("+json") => BodySpec::Json(pretty_value(example)),
        "application/xml" | "text/xml" => BodySpec::Xml(value_text(example)),
        "application/x-www-form-urlencoded" => BodySpec::FormUrlEncoded(
            example
                .as_object()
                .into_iter()
                .flatten()
                .map(|(name, value)| FormField {
                    name: name.clone(),
                    value: value_text(value),
                    enabled: true,
                })
                .collect(),
        ),
        media_type if media_type.starts_with("text/") => BodySpec::Text(value_text(example)),
        _ => BodySpec::Text(value_text(example)),
    }
}

fn export_operation(
    definition: &ApiDefinition,
    contract: &HttpContract,
    inferred_server: Option<ApiServer>,
) -> Value {
    let mut operation = Map::new();
    operation.insert("summary".into(), Value::String(definition.name.clone()));
    if !definition.description_markdown.trim().is_empty() {
        operation.insert(
            "description".into(),
            Value::String(definition.description_markdown.clone()),
        );
    }
    let operation_id = contract
        .extensions
        .get("operationId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "{}_{}",
                method_name(contract.method),
                definition.id.to_string().replace('-', "")
            )
        });
    operation.insert("operationId".into(), Value::String(operation_id));
    if !definition.tags.is_empty() {
        operation.insert(
            "tags".into(),
            Value::Array(definition.tags.iter().cloned().map(Value::String).collect()),
        );
    }
    if definition.status == ApiStatus::Deprecated
        || contract
            .extensions
            .get("deprecated")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        operation.insert("deprecated".into(), Value::Bool(true));
    }
    if !contract.parameters.is_empty() {
        operation.insert(
            "parameters".into(),
            Value::Array(contract.parameters.iter().map(export_parameter).collect()),
        );
    }
    if let Some(content) = &contract.request_body {
        operation.insert(
            "requestBody".into(),
            json!({
                "content": {
                    content.media_type.clone(): export_content(content)
                }
            }),
        );
    }
    let responses = if contract.responses.is_empty() {
        vec![ApiResponse::new("200", "Successful response")]
    } else {
        contract.responses.clone()
    };
    operation.insert(
        "responses".into(),
        Value::Object(Map::from_iter(
            responses
                .iter()
                .map(|response| (response.status.clone(), export_response(response))),
        )),
    );
    if !contract.security.is_empty() {
        operation.insert(
            "security".into(),
            Value::Array(
                contract
                    .security
                    .iter()
                    .map(|requirement| json!({ requirement.scheme.clone(): requirement.scopes }))
                    .collect(),
            ),
        );
    }
    let mut servers = contract.servers.clone();
    if servers.is_empty()
        && let Some(server) = inferred_server
    {
        servers.push(server);
    }
    if !servers.is_empty() {
        operation.insert(
            "servers".into(),
            Value::Array(servers.iter().map(export_server).collect()),
        );
    }
    for (name, value) in &contract.extensions {
        if name.starts_with("x-") {
            operation.insert(name.clone(), value.clone());
        }
    }
    Value::Object(operation)
}

fn export_parameter(parameter: &ApiParameter) -> Value {
    let mut value = Map::from_iter([
        ("name".into(), Value::String(parameter.name.clone())),
        (
            "in".into(),
            Value::String(parameter_location_name(parameter.location).into()),
        ),
        ("required".into(), Value::Bool(parameter.required)),
        ("schema".into(), schema_to_value(&parameter.schema)),
    ]);
    if !parameter.description.trim().is_empty() {
        value.insert(
            "description".into(),
            Value::String(parameter.description.clone()),
        );
    }
    if let Some(example) = &parameter.example {
        value.insert("example".into(), example.clone());
    }
    if !parameter.enabled {
        value.insert("x-apitest-enabled".into(), Value::Bool(false));
    }
    Value::Object(value)
}

fn export_response(response: &ApiResponse) -> Value {
    let mut value = Map::new();
    value.insert(
        "description".into(),
        Value::String(if response.description.trim().is_empty() {
            "Response".into()
        } else {
            response.description.clone()
        }),
    );
    if !response.headers.is_empty() {
        value.insert(
            "headers".into(),
            Value::Object(Map::from_iter(response.headers.iter().map(|header| {
                let mut exported = export_parameter(header)
                    .as_object()
                    .expect("exported parameters are objects")
                    .clone();
                exported.remove("name");
                exported.remove("in");
                (header.name.clone(), Value::Object(exported))
            }))),
        );
    }
    if !response.content.is_empty() {
        value.insert(
            "content".into(),
            Value::Object(Map::from_iter(
                response
                    .content
                    .iter()
                    .map(|content| (content.media_type.clone(), export_content(content))),
            )),
        );
    }
    Value::Object(value)
}

fn export_content(content: &ApiContent) -> Value {
    let mut value = Map::new();
    value.insert("schema".into(), schema_to_value(&content.schema));
    if content.examples.len() == 1 && content.examples.contains_key("default") {
        value.insert("example".into(), content.examples["default"].clone());
    } else if !content.examples.is_empty() {
        value.insert(
            "examples".into(),
            Value::Object(Map::from_iter(
                content
                    .examples
                    .iter()
                    .map(|(name, value)| (name.clone(), json!({ "value": value }))),
            )),
        );
    }
    Value::Object(value)
}

fn export_server(server: &ApiServer) -> Value {
    let mut value = Map::new();
    value.insert("url".into(), Value::String(server.url.clone()));
    if !server.description.trim().is_empty() {
        value.insert(
            "description".into(),
            Value::String(server.description.clone()),
        );
    }
    if !server.variables.is_empty() {
        value.insert(
            "variables".into(),
            Value::Object(Map::from_iter(
                server
                    .variables
                    .iter()
                    .map(|(name, value)| (name.clone(), json!({ "default": value }))),
            )),
        );
    }
    Value::Object(value)
}

fn export_components(components: &ApiComponents) -> Value {
    let mut value = Map::new();
    if !components.schemas.is_empty() {
        value.insert(
            "schemas".into(),
            Value::Object(Map::from_iter(
                components
                    .schemas
                    .iter()
                    .map(|(name, schema)| (name.clone(), schema_to_value(schema))),
            )),
        );
    }
    if !components.examples.is_empty() {
        value.insert(
            "examples".into(),
            Value::Object(Map::from_iter(
                components
                    .examples
                    .iter()
                    .map(|(name, example)| (name.clone(), json!({ "value": example }))),
            )),
        );
    }
    if !components.security_schemes.is_empty() {
        value.insert(
            "securitySchemes".into(),
            Value::Object(Map::from_iter(
                components
                    .security_schemes
                    .iter()
                    .map(|(name, scheme)| (name.clone(), scheme.clone())),
            )),
        );
    }
    Value::Object(value)
}

fn export_path_and_server(path: &str) -> (String, Option<ApiServer>) {
    let Some(scheme_end) = path.find("://") else {
        return (ensure_openapi_path(path), None);
    };
    let authority_start = scheme_end + 3;
    let Some(relative_start) = path[authority_start..].find('/') else {
        return ("/".into(), Some(ApiServer::new(path)));
    };
    let relative_start = authority_start + relative_start;
    let server = &path[..relative_start];
    let relative = path[relative_start..]
        .split(['?', '#'])
        .next()
        .unwrap_or("/");
    (ensure_openapi_path(relative), Some(ApiServer::new(server)))
}

fn ensure_openapi_path(path: &str) -> String {
    let path = path
        .split(['?', '#'])
        .next()
        .filter(|path| !path.is_empty())
        .unwrap_or("/");
    if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    }
}

fn schema_from_value(value: &Value) -> ApiSchema {
    value
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix("#/components/schemas/"))
        .map(|name| ApiSchema::component(name.replace("~1", "/").replace("~0", "~")))
        .unwrap_or_else(|| ApiSchema::Inline(value.clone()))
}

fn schema_to_value(schema: &ApiSchema) -> Value {
    match schema {
        ApiSchema::Inline(value) => value.clone(),
        ApiSchema::Component(name) => {
            json!({ "$ref": format!("#/components/schemas/{}", pointer_escape(name)) })
        }
    }
}

fn parameter_location(value: &str) -> Option<ApiParameterLocation> {
    match value {
        "path" => Some(ApiParameterLocation::Path),
        "query" => Some(ApiParameterLocation::Query),
        "header" => Some(ApiParameterLocation::Header),
        "cookie" => Some(ApiParameterLocation::Cookie),
        _ => None,
    }
}

fn parameter_location_name(value: ApiParameterLocation) -> &'static str {
    match value {
        ApiParameterLocation::Path => "path",
        ApiParameterLocation::Query => "query",
        ApiParameterLocation::Header => "header",
        ApiParameterLocation::Cookie => "cookie",
    }
}

fn resolve_object<'a>(document: &'a Value, value: &'a Value) -> Option<&'a Map<String, Value>> {
    resolve_value(document, value).as_object()
}

fn resolve_value<'a>(document: &'a Value, value: &'a Value) -> &'a Value {
    value
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix('#'))
        .and_then(|pointer| document.pointer(pointer))
        .unwrap_or(value)
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn pretty_value(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn validate_path_parameters(
    document: &Value,
    path: &str,
    path_item: &Map<String, Value>,
    operation: &Map<String, Value>,
    operation_pointer: &str,
    issues: &mut Vec<OpenApiIssue>,
) {
    let required = path
        .split('{')
        .skip(1)
        .filter_map(|part| part.split_once('}').map(|(name, _)| name))
        .collect::<HashSet<_>>();
    if required.is_empty() {
        return;
    }
    let declared = path_item
        .get("parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            operation
                .get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
        .filter_map(|parameter| resolve_object(document, parameter))
        .filter(|parameter| parameter.get("in").and_then(Value::as_str) == Some("path"))
        .filter(|parameter| {
            parameter
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|parameter| parameter.get("name").and_then(Value::as_str))
        .collect::<HashSet<_>>();
    for name in required.difference(&declared) {
        error(
            issues,
            operation_pointer,
            &format!("path parameter `{{{name}}}` must be declared and required"),
        );
    }
}

fn error(issues: &mut Vec<OpenApiIssue>, path: &str, message: &str) {
    issues.push(OpenApiIssue {
        level: OpenApiIssueLevel::Error,
        path: path.into(),
        message: message.into(),
    });
}

fn warning(issues: &mut Vec<OpenApiIssue>, path: &str, message: &str) {
    issues.push(OpenApiIssue {
        level: OpenApiIssueLevel::Warning,
        path: path.into(),
        message: message.into(),
    });
}

fn pointer_escape(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const DOCUMENTATION_CSS: &str = "body{margin:0;background:#0f1115;color:#e8ecf2;font:15px system-ui,sans-serif}main{max-width:960px;margin:auto;padding:40px 24px}header{margin-bottom:32px}.operation{background:#171a21;border:1px solid #2b303b;border-radius:10px;padding:20px;margin:16px 0}.route{display:flex;align-items:center;gap:12px}.method{font-weight:700;color:#7dd3fc;text-transform:uppercase}.method.post{color:#86efac}.method.delete{color:#fda4af}code{font-family:ui-monospace,monospace}p{color:#aeb6c4;white-space:pre-wrap}li{margin:6px 0}";
