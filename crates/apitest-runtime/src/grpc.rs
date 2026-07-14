use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use apitest_core::{
    Environment, ExecutionError, ExecutionEvent, ExecutionMetrics, ExecutionRequest,
    ExecutionStream, GrpcCallKind, GrpcSpec, ProtocolExecutor, ProtocolKind, ProtocolSpec,
    ResponseHead, Variable,
};
use apitest_storage::SecretStore;
use async_stream::try_stream;
use bytes::Bytes;
use chrono::Utc;
use http::uri::PathAndQuery;
use indexmap::IndexMap;
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, MethodDescriptor};
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tonic::{
    Request,
    codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder},
    metadata::{Ascii, KeyAndValueRef, MetadataKey, MetadataMap, MetadataValue},
    transport::{Channel, Endpoint},
};

#[derive(Debug, Error)]
pub enum GrpcDescriptorError {
    #[error("failed to compile protobuf definitions: {0}")]
    Compile(#[from] protox::Error),
    #[error("invalid protobuf descriptor set: {0}")]
    Descriptor(#[from] prost_reflect::DescriptorError),
    #[error("service `{0}` was not found")]
    ServiceNotFound(String),
    #[error("method `{method}` was not found in service `{service}`")]
    MethodNotFound { service: String, method: String },
    #[error("invalid protobuf JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone)]
pub struct GrpcDescriptor {
    pool: DescriptorPool,
}

impl GrpcDescriptor {
    pub fn from_proto_files(
        files: &[PathBuf],
        includes: &[PathBuf],
    ) -> Result<Self, GrpcDescriptorError> {
        let descriptor_set = protox::compile(files, includes)?;
        let pool = DescriptorPool::from_file_descriptor_set(descriptor_set)?;
        Ok(Self { pool })
    }

    pub fn from_descriptor_set(bytes: &[u8]) -> Result<Self, GrpcDescriptorError> {
        let pool = DescriptorPool::decode(bytes)?;
        Ok(Self { pool })
    }

    pub fn from_descriptor_set_file(
        path: impl AsRef<Path>,
    ) -> Result<Self, GrpcDescriptorFileError> {
        let bytes = std::fs::read(path)?;
        Ok(Self::from_descriptor_set(&bytes)?)
    }

    pub fn method(&self, service: &str, method: &str) -> Result<GrpcMethod, GrpcDescriptorError> {
        let service_descriptor = self
            .pool
            .get_service_by_name(service)
            .ok_or_else(|| GrpcDescriptorError::ServiceNotFound(service.to_owned()))?;
        let descriptor = service_descriptor
            .methods()
            .find(|candidate| candidate.name() == method)
            .ok_or_else(|| GrpcDescriptorError::MethodNotFound {
                service: service.to_owned(),
                method: method.to_owned(),
            })?;
        Ok(GrpcMethod { descriptor })
    }

    fn from_pool(pool: DescriptorPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Error)]
pub enum GrpcDescriptorFileError {
    #[error("failed to read protobuf descriptor set: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Descriptor(#[from] GrpcDescriptorError),
}

#[derive(Clone)]
pub struct GrpcMethod {
    descriptor: MethodDescriptor,
}

impl GrpcMethod {
    pub fn call_kind(&self) -> GrpcCallKind {
        match (
            self.descriptor.is_client_streaming(),
            self.descriptor.is_server_streaming(),
        ) {
            (false, false) => GrpcCallKind::Unary,
            (false, true) => GrpcCallKind::ServerStreaming,
            (true, false) => GrpcCallKind::ClientStreaming,
            (true, true) => GrpcCallKind::BidirectionalStreaming,
        }
    }

    pub fn input_from_json(&self, json: &str) -> Result<DynamicMessage, GrpcDescriptorError> {
        let mut deserializer = serde_json::Deserializer::from_str(json);
        let message = DynamicMessage::deserialize(self.descriptor.input(), &mut deserializer)?;
        deserializer.end()?;
        Ok(message)
    }

    pub fn message_to_json(&self, message: &DynamicMessage) -> Result<Value, GrpcDescriptorError> {
        Ok(serde_json::to_value(message)?)
    }

    pub fn descriptor(&self) -> &MethodDescriptor {
        &self.descriptor
    }
}

#[derive(Clone)]
pub struct GrpcExecutor {
    secrets: Arc<dyn SecretStore>,
}

impl GrpcExecutor {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self { secrets }
    }

    fn materialize_environment(
        &self,
        environment: &Environment,
        local_variables: &[Variable],
    ) -> Result<(Environment, Vec<Variable>), ExecutionError> {
        let materialize = |variable: &Variable| -> Result<Variable, ExecutionError> {
            let Some(reference) = &variable.secret_ref else {
                return Ok(variable.clone());
            };
            let value = self
                .secrets
                .get(reference)
                .map_err(|error| ExecutionError::Authentication(error.to_string()))?
                .ok_or_else(|| {
                    ExecutionError::Authentication(format!(
                        "secret `{}` is unavailable",
                        reference.as_str()
                    ))
                })?;
            Ok(variable.with_materialized_secret(value))
        };
        let mut environment = environment.clone();
        environment.variables = environment
            .variables
            .iter()
            .map(materialize)
            .collect::<Result<_, _>>()?;
        let locals = local_variables
            .iter()
            .map(materialize)
            .collect::<Result<_, _>>()?;
        Ok((environment, locals))
    }
}

impl ProtocolExecutor for GrpcExecutor {
    fn kind(&self) -> ProtocolKind {
        ProtocolKind::Grpc
    }

    fn execute(
        &self,
        request: ExecutionRequest,
        cancellation: CancellationToken,
    ) -> ExecutionStream {
        let executor = self.clone();
        Box::pin(try_stream! {
            let ProtocolSpec::Grpc(spec) = request.protocol else {
                Err(ExecutionError::InvalidRequest("gRPC executor requires a gRPC request".into()))?
            };
            if cancellation.is_cancelled() {
                Err(ExecutionError::Cancelled)?;
            }
            if !spec.validate_tls && spec.endpoint.starts_with("https://") {
                Err(ExecutionError::Tls(
                    "disabling TLS certificate verification is not supported for gRPC".into(),
                ))?;
            }
            let started = Instant::now();
            yield ExecutionEvent::Started { id: request.id, at: Utc::now() };
            let (environment, locals) = executor.materialize_environment(
                &request.environment,
                &request.local_variables,
            )?;
            let channel = connect(&spec, &cancellation).await?;
            let descriptor = load_descriptor(&spec, channel.clone()).await?;
            let method = descriptor
                .method(&spec.service, &spec.method)
                .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
            if method.call_kind() != spec.call_kind {
                Err(ExecutionError::InvalidRequest(format!(
                    "configured gRPC call kind {:?} does not match descriptor {:?}",
                    spec.call_kind,
                    method.call_kind()
                )))?;
            }
            let input = method
                .input_from_json(&spec.message_json)
                .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
            let sent_bytes = input.encoded_len() as u64;
            let path = PathAndQuery::try_from(format!("/{}/{}", spec.service, spec.method))
                .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
            let codec = DynamicCodec::new(method.descriptor().output());
            let mut client = tonic::client::Grpc::new(channel);
            let mut received_bytes = 0_u64;

            match method.call_kind() {
                GrpcCallKind::Unary => {
                    let request = grpc_request(input, &spec, &environment, &locals)?;
                    let response = tokio::select! {
                        _ = cancellation.cancelled() => Err(ExecutionError::Cancelled),
                        result = client.unary(request, path, codec) => result.map_err(map_status),
                    }?;
                    yield ExecutionEvent::ResponseHead(response_head(response.metadata()));
                    let data = json_bytes(&method, response.into_inner())?;
                    received_bytes = data.len() as u64;
                    yield ExecutionEvent::Data(data);
                }
                GrpcCallKind::ServerStreaming => {
                    let request = grpc_request(input, &spec, &environment, &locals)?;
                    let response = tokio::select! {
                        _ = cancellation.cancelled() => Err(ExecutionError::Cancelled),
                        result = client.server_streaming(request, path, codec) => result.map_err(map_status),
                    }?;
                    yield ExecutionEvent::ResponseHead(response_head(response.metadata()));
                    let mut stream = response.into_inner();
                    loop {
                        let message = tokio::select! {
                            _ = cancellation.cancelled() => Err(ExecutionError::Cancelled),
                            result = stream.message() => result.map_err(map_status),
                        }?;
                        let Some(message) = message else { break };
                        let data = json_bytes(&method, message)?;
                        received_bytes = received_bytes.saturating_add(data.len() as u64);
                        yield ExecutionEvent::Message {
                            outgoing: false,
                            media_type: Some("application/grpc+json".into()),
                            data,
                            at: Utc::now(),
                        };
                    }
                }
                GrpcCallKind::ClientStreaming => {
                    let stream = tokio_stream::once(input);
                    let request = grpc_request(stream, &spec, &environment, &locals)?;
                    let response = tokio::select! {
                        _ = cancellation.cancelled() => Err(ExecutionError::Cancelled),
                        result = client.client_streaming(request, path, codec) => result.map_err(map_status),
                    }?;
                    yield ExecutionEvent::ResponseHead(response_head(response.metadata()));
                    let data = json_bytes(&method, response.into_inner())?;
                    received_bytes = data.len() as u64;
                    yield ExecutionEvent::Data(data);
                }
                GrpcCallKind::BidirectionalStreaming => {
                    let stream = tokio_stream::once(input);
                    let request = grpc_request(stream, &spec, &environment, &locals)?;
                    let response = tokio::select! {
                        _ = cancellation.cancelled() => Err(ExecutionError::Cancelled),
                        result = client.streaming(request, path, codec) => result.map_err(map_status),
                    }?;
                    yield ExecutionEvent::ResponseHead(response_head(response.metadata()));
                    let mut stream = response.into_inner();
                    loop {
                        let message = tokio::select! {
                            _ = cancellation.cancelled() => Err(ExecutionError::Cancelled),
                            result = stream.message() => result.map_err(map_status),
                        }?;
                        let Some(message) = message else { break };
                        let data = json_bytes(&method, message)?;
                        received_bytes = received_bytes.saturating_add(data.len() as u64);
                        yield ExecutionEvent::Message {
                            outgoing: false,
                            media_type: Some("application/grpc+json".into()),
                            data,
                            at: Utc::now(),
                        };
                    }
                }
            }

            let metrics = ExecutionMetrics {
                elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                received_bytes,
                sent_bytes,
            };
            yield ExecutionEvent::Metrics(metrics);
            yield ExecutionEvent::Completed(metrics);
        })
    }
}

async fn connect(
    spec: &GrpcSpec,
    cancellation: &CancellationToken,
) -> Result<Channel, ExecutionError> {
    let endpoint = Endpoint::from_shared(spec.endpoint.clone())
        .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?
        .connect_timeout(Duration::from_millis(spec.timeout_ms))
        .timeout(Duration::from_millis(spec.timeout_ms));
    tokio::select! {
        _ = cancellation.cancelled() => Err(ExecutionError::Cancelled),
        result = endpoint.connect() => result.map_err(|error| ExecutionError::Network(error.to_string())),
    }
}

async fn load_descriptor(
    spec: &GrpcSpec,
    channel: Channel,
) -> Result<GrpcDescriptor, ExecutionError> {
    if let Some(path) = &spec.descriptor_set {
        return GrpcDescriptor::from_descriptor_set_file(path)
            .map_err(|error| ExecutionError::InvalidRequest(error.to_string()));
    }
    if !spec.proto_files.is_empty() {
        return GrpcDescriptor::from_proto_files(&spec.proto_files, &spec.import_paths)
            .map_err(|error| ExecutionError::InvalidRequest(error.to_string()));
    }
    if spec.use_reflection {
        return descriptor_from_reflection(channel, &spec.service).await;
    }
    Err(ExecutionError::InvalidRequest(
        "gRPC requires proto files, a descriptor set, or server reflection".into(),
    ))
}

async fn descriptor_from_reflection(
    channel: Channel,
    service: &str,
) -> Result<GrpcDescriptor, ExecutionError> {
    use tonic_reflection::pb::v1::{
        ServerReflectionRequest, server_reflection_client::ServerReflectionClient,
        server_reflection_request::MessageRequest, server_reflection_response::MessageResponse,
    };

    let request = ServerReflectionRequest {
        host: String::new(),
        message_request: Some(MessageRequest::FileContainingSymbol(service.to_owned())),
    };
    let mut client = ServerReflectionClient::new(channel);
    let mut response = client
        .server_reflection_info(tokio_stream::once(request))
        .await
        .map_err(map_status)?
        .into_inner();
    let response = response
        .message()
        .await
        .map_err(map_status)?
        .ok_or_else(|| ExecutionError::Protocol("reflection returned no descriptor".into()))?;
    let files = match response.message_response {
        Some(MessageResponse::FileDescriptorResponse(response)) => response.file_descriptor_proto,
        Some(MessageResponse::ErrorResponse(error)) => {
            return Err(ExecutionError::Protocol(format!(
                "reflection error {}: {}",
                error.error_code, error.error_message
            )));
        }
        _ => {
            return Err(ExecutionError::Protocol(
                "reflection returned an unexpected response".into(),
            ));
        }
    };
    let protos = files
        .into_iter()
        .map(|bytes| {
            prost_types::FileDescriptorProto::decode(bytes.as_slice())
                .map_err(|error| ExecutionError::Protocol(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let pool =
        DescriptorPool::from_file_descriptor_set(prost_types::FileDescriptorSet { file: protos })
            .map_err(|error| ExecutionError::Protocol(error.to_string()))?;
    Ok(GrpcDescriptor::from_pool(pool))
}

fn grpc_request<T>(
    message: T,
    spec: &GrpcSpec,
    environment: &Environment,
    locals: &[Variable],
) -> Result<Request<T>, ExecutionError> {
    let mut request = Request::new(message);
    for entry in spec.metadata.iter().filter(|entry| entry.enabled) {
        let name = environment
            .resolve(&entry.name, locals)
            .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?
            .into_value();
        let value = environment
            .resolve(&entry.value, locals)
            .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?
            .into_value();
        let key = name
            .parse::<MetadataKey<Ascii>>()
            .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
        let value = value
            .parse::<MetadataValue<Ascii>>()
            .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
        request.metadata_mut().insert(key, value);
    }
    Ok(request)
}

fn response_head(metadata: &MetadataMap) -> ResponseHead {
    let mut headers = IndexMap::<String, Vec<String>>::new();
    for entry in metadata.iter() {
        if let KeyAndValueRef::Ascii(key, value) = entry
            && let Ok(value) = value.to_str()
        {
            headers
                .entry(key.as_str().to_owned())
                .or_default()
                .push(value.to_owned());
        }
    }
    ResponseHead {
        status: Some(200),
        version: Some("HTTP/2".into()),
        headers,
    }
}

fn json_bytes(method: &GrpcMethod, message: DynamicMessage) -> Result<Bytes, ExecutionError> {
    let value = method
        .message_to_json(&message)
        .map_err(|error| ExecutionError::Protocol(error.to_string()))?;
    serde_json::to_vec_pretty(&value)
        .map(Bytes::from)
        .map_err(|error| ExecutionError::Protocol(error.to_string()))
}

fn map_status(status: tonic::Status) -> ExecutionError {
    ExecutionError::Protocol(format!("{}: {}", status.code(), status.message()))
}

struct DynamicCodec {
    output: MessageDescriptor,
}

impl DynamicCodec {
    fn new(output: MessageDescriptor) -> Self {
        Self { output }
    }
}

impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynamicEncoder;
    type Decoder = DynamicDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        DynamicEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        DynamicDecoder {
            descriptor: self.output.clone(),
        }
    }
}

struct DynamicEncoder;

impl Encoder for DynamicEncoder {
    type Item = DynamicMessage;
    type Error = tonic::Status;

    fn encode(
        &mut self,
        item: Self::Item,
        destination: &mut EncodeBuf<'_>,
    ) -> Result<(), Self::Error> {
        item.encode(destination)
            .map_err(|error| tonic::Status::internal(error.to_string()))
    }
}

struct DynamicDecoder {
    descriptor: MessageDescriptor,
}

impl Decoder for DynamicDecoder {
    type Item = DynamicMessage;
    type Error = tonic::Status;

    fn decode(&mut self, source: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        DynamicMessage::decode(self.descriptor.clone(), source)
            .map(Some)
            .map_err(|error| tonic::Status::internal(error.to_string()))
    }
}
