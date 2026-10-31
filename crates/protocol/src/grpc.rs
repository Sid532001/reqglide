use std::time::{Duration, Instant};

use api_client_core::HeaderEntry;
use api_client_http::{HttpExecutionContext, ProxyConfiguration};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures_util::StreamExt;
use http::uri::PathAndQuery;
use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, MethodDescriptor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_stream::iter;
use tonic::{
    Request, Status,
    client::Grpc,
    codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder},
    metadata::{Ascii, Binary, KeyAndValueRef, MetadataKey, MetadataValue},
    transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity},
};
use tonic_reflection::pb::{v1, v1alpha};

const MAX_DESCRIPTOR_BYTES: u64 = 16 * 1024 * 1024;
const MAX_GRPC_MESSAGES: usize = 10_000;
const MAX_GRPC_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct GrpcReflectionInput {
    pub endpoint: String,
    pub service: String,
    pub metadata: Vec<HeaderEntry>,
    pub descriptor_set_path: String,
    pub use_reflection: bool,
    pub timeout_ms: u64,
    pub context: HttpExecutionContext,
    pub redactions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrpcMethodInfo {
    pub name: String,
    pub full_name: String,
    pub input_type: String,
    pub output_type: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrpcReflectionResult {
    pub services: Vec<String>,
    pub methods: Vec<GrpcMethodInfo>,
}

#[derive(Clone)]
pub struct GrpcInvokeInput {
    pub endpoint: String,
    pub service: String,
    pub method: String,
    pub message: String,
    pub metadata: Vec<HeaderEntry>,
    pub descriptor_set_path: String,
    pub use_reflection: bool,
    pub timeout_ms: u64,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub context: HttpExecutionContext,
    pub redactions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrpcInvokeResult {
    pub code: String,
    pub message: String,
    pub responses: Vec<Value>,
    pub metadata: Vec<HeaderEntry>,
    pub trailers: Vec<HeaderEntry>,
    pub duration_ms: u64,
    pub client_streaming: bool,
    pub server_streaming: bool,
}

pub async fn reflect_grpc_service(
    input: GrpcReflectionInput,
) -> Result<GrpcReflectionResult, String> {
    let redactions = input.redactions.clone();
    reflect_grpc_service_inner(input)
        .await
        .map_err(|error| redact(&error, &redactions))
}

async fn reflect_grpc_service_inner(
    input: GrpcReflectionInput,
) -> Result<GrpcReflectionResult, String> {
    validate_metadata(&input.metadata)?;
    if !input.use_reflection {
        let pool = load_descriptor_set(&input.descriptor_set_path).await?;
        if input.service.trim().is_empty() {
            let mut services = pool
                .services()
                .map(|service| service.full_name().to_owned())
                .collect::<Vec<_>>();
            services.sort();
            return Ok(GrpcReflectionResult {
                services,
                methods: Vec::new(),
            });
        }
        let service_name = input.service.trim();
        let service = pool
            .get_service_by_name(service_name)
            .ok_or_else(|| format!("gRPC service '{service_name}' was not found"))?;
        return Ok(GrpcReflectionResult {
            services: vec![service.full_name().to_owned()],
            methods: service.methods().map(method_info).collect(),
        });
    }
    let channel = connect_channel(&input.endpoint, input.timeout_ms, &input.context).await?;
    let timeout = Duration::from_millis(input.timeout_ms.clamp(1, 300_000));
    if input.service.trim().is_empty() {
        let services = tokio::time::timeout(timeout, list_services(channel, &input.metadata))
            .await
            .map_err(|_| "gRPC reflection timed out".to_owned())??;
        return Ok(GrpcReflectionResult {
            services,
            methods: Vec::new(),
        });
    }
    let service = input.service.trim().to_owned();
    let descriptor_bytes = tokio::time::timeout(
        timeout,
        descriptors_for_symbol(channel, &service, &input.metadata),
    )
    .await
    .map_err(|_| "gRPC reflection timed out".to_owned())??;
    let pool = descriptor_pool(descriptor_bytes)?;
    let service_descriptor = pool
        .get_service_by_name(&service)
        .ok_or_else(|| format!("gRPC reflection did not return service '{service}'"))?;
    let methods = service_descriptor
        .methods()
        .map(method_info)
        .collect::<Vec<_>>();
    Ok(GrpcReflectionResult {
        services: vec![service],
        methods,
    })
}

pub async fn invoke_grpc_unary(input: GrpcInvokeInput) -> Result<GrpcInvokeResult, String> {
    let redactions = input.redactions.clone();
    invoke_grpc_inner(input)
        .await
        .map_err(|error| redact(&error, &redactions))
}

async fn invoke_grpc_inner(input: GrpcInvokeInput) -> Result<GrpcInvokeResult, String> {
    if input.message.len() > input.max_request_bytes.min(MAX_GRPC_MESSAGE_BYTES) {
        return Err("gRPC JSON request exceeds the configured request limit".to_owned());
    }
    validate_metadata(&input.metadata)?;
    let channel = connect_channel(&input.endpoint, input.timeout_ms, &input.context).await?;
    let pool = if input.use_reflection {
        let bytes =
            descriptors_for_symbol(channel.clone(), &input.service, &input.metadata).await?;
        descriptor_pool(bytes)?
    } else {
        load_descriptor_set(&input.descriptor_set_path).await?
    };
    let service = pool
        .get_service_by_name(input.service.trim())
        .ok_or_else(|| format!("gRPC service '{}' was not found", input.service.trim()))?;
    let method = service
        .methods()
        .find(|method| method.name() == input.method.trim())
        .ok_or_else(|| {
            format!(
                "gRPC method '{}' was not found on service '{}'",
                input.method.trim(),
                service.full_name()
            )
        })?;
    let messages = parse_request_messages(&input.message, &method)?;
    let timeout = Duration::from_millis(input.timeout_ms.clamp(1, 300_000));
    let started = Instant::now();
    let invocation = tokio::time::timeout(
        timeout,
        invoke_method(
            channel,
            service.full_name(),
            &method,
            messages,
            &input.metadata,
            input.max_request_bytes.min(MAX_GRPC_MESSAGE_BYTES),
            input.max_response_bytes.min(MAX_GRPC_MESSAGE_BYTES),
        ),
    )
    .await;
    let duration_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    match invocation {
        Err(_) => Ok(GrpcInvokeResult {
            code: "DeadlineExceeded".to_owned(),
            message: "gRPC request exceeded its deadline".to_owned(),
            responses: Vec::new(),
            metadata: Vec::new(),
            trailers: Vec::new(),
            duration_ms,
            client_streaming: method.is_client_streaming(),
            server_streaming: method.is_server_streaming(),
        }),
        Ok(Err(status)) => Ok(GrpcInvokeResult {
            code: format!("{:?}", status.code()),
            message: status.message().to_owned(),
            responses: Vec::new(),
            metadata: metadata_entries(status.metadata()),
            trailers: Vec::new(),
            duration_ms,
            client_streaming: method.is_client_streaming(),
            server_streaming: method.is_server_streaming(),
        }),
        Ok(Ok(mut result)) => {
            result.duration_ms = duration_ms;
            Ok(result)
        }
    }
}

async fn invoke_method(
    channel: Channel,
    service_name: &str,
    method: &MethodDescriptor,
    mut messages: Vec<DynamicMessage>,
    metadata: &[HeaderEntry],
    max_request_bytes: usize,
    max_response_bytes: usize,
) -> Result<GrpcInvokeResult, Status> {
    let path = PathAndQuery::try_from(format!("/{service_name}/{}", method.name()))
        .map_err(|_| Status::invalid_argument("gRPC service or method path is invalid"))?;
    let codec = DynamicCodec::new(method.output());
    let mut grpc = Grpc::new(channel)
        .max_encoding_message_size(max_request_bytes)
        .max_decoding_message_size(max_response_bytes);
    grpc.ready()
        .await
        .map_err(|error| Status::unavailable(format!("gRPC service is not ready: {error}")))?;

    let (responses, response_metadata, trailers) =
        match (method.is_client_streaming(), method.is_server_streaming()) {
            (false, false) => {
                let message = messages
                    .pop()
                    .ok_or_else(|| Status::invalid_argument("gRPC unary request is empty"))?;
                let mut request = Request::new(message);
                append_metadata(&mut request, metadata)?;
                let response = grpc.unary(request, path, codec).await?;
                let response_metadata = metadata_entries(response.metadata());
                (
                    vec![dynamic_to_json(response.into_inner())?],
                    response_metadata,
                    Vec::new(),
                )
            }
            (false, true) => {
                let message = messages.pop().ok_or_else(|| {
                    Status::invalid_argument("gRPC server-streaming request is empty")
                })?;
                let mut request = Request::new(message);
                append_metadata(&mut request, metadata)?;
                let response = grpc.server_streaming(request, path, codec).await?;
                let response_metadata = metadata_entries(response.metadata());
                let mut stream = response.into_inner();
                let values = collect_stream(&mut stream).await?;
                let trailers = stream
                    .trailers()
                    .await?
                    .map_or_else(Vec::new, |value| metadata_entries(&value));
                (values, response_metadata, trailers)
            }
            (true, false) => {
                let mut request = Request::new(iter(messages));
                append_metadata(&mut request, metadata)?;
                let response = grpc.client_streaming(request, path, codec).await?;
                let response_metadata = metadata_entries(response.metadata());
                (
                    vec![dynamic_to_json(response.into_inner())?],
                    response_metadata,
                    Vec::new(),
                )
            }
            (true, true) => {
                let mut request = Request::new(iter(messages));
                append_metadata(&mut request, metadata)?;
                let response = grpc.streaming(request, path, codec).await?;
                let response_metadata = metadata_entries(response.metadata());
                let mut stream = response.into_inner();
                let values = collect_stream(&mut stream).await?;
                let trailers = stream
                    .trailers()
                    .await?
                    .map_or_else(Vec::new, |value| metadata_entries(&value));
                (values, response_metadata, trailers)
            }
        };
    Ok(GrpcInvokeResult {
        code: "Ok".to_owned(),
        message: String::new(),
        responses,
        metadata: response_metadata,
        trailers,
        duration_ms: 0,
        client_streaming: method.is_client_streaming(),
        server_streaming: method.is_server_streaming(),
    })
}

async fn collect_stream(
    stream: &mut tonic::codec::Streaming<DynamicMessage>,
) -> Result<Vec<Value>, Status> {
    let mut values = Vec::new();
    while let Some(message) = stream.next().await {
        if values.len() >= MAX_GRPC_MESSAGES {
            return Err(Status::resource_exhausted(format!(
                "gRPC stream exceeds {MAX_GRPC_MESSAGES} messages"
            )));
        }
        values.push(dynamic_to_json(message?)?);
    }
    Ok(values)
}

fn parse_request_messages(
    source: &str,
    method: &MethodDescriptor,
) -> Result<Vec<DynamicMessage>, String> {
    let document: Value = serde_json::from_str(source)
        .map_err(|error| format!("gRPC request body is not valid JSON: {error}"))?;
    let values = if method.is_client_streaming() {
        document.as_array().cloned().ok_or_else(|| {
            "Client-streaming and bidirectional gRPC methods require a JSON array of messages"
                .to_owned()
        })?
    } else {
        vec![document]
    };
    if values.is_empty() {
        return Err("gRPC request must contain at least one message".to_owned());
    }
    if values.len() > MAX_GRPC_MESSAGES {
        return Err(format!(
            "gRPC request contains more than {MAX_GRPC_MESSAGES} messages"
        ));
    }
    values
        .into_iter()
        .map(|value| dynamic_from_json(method.input(), value))
        .collect()
}

fn dynamic_from_json(
    descriptor: MessageDescriptor,
    value: Value,
) -> Result<DynamicMessage, String> {
    let source = serde_json::to_string(&value)
        .map_err(|error| format!("gRPC JSON message could not be serialized: {error}"))?;
    let mut deserializer = serde_json::Deserializer::from_str(&source);
    let message = DynamicMessage::deserialize(descriptor, &mut deserializer)
        .map_err(|error| format!("gRPC JSON does not match the input message: {error}"))?;
    deserializer
        .end()
        .map_err(|error| format!("gRPC JSON has trailing data: {error}"))?;
    Ok(message)
}

fn dynamic_to_json(message: DynamicMessage) -> Result<Value, Status> {
    serde_json::to_value(message)
        .map_err(|error| Status::internal(format!("gRPC response JSON encoding failed: {error}")))
}

async fn connect_channel(
    endpoint: &str,
    timeout_ms: u64,
    context: &HttpExecutionContext,
) -> Result<Channel, String> {
    if matches!(context.proxy, ProxyConfiguration::Custom { .. }) {
        return Err(
            "Custom proxy profiles are not supported for native gRPC connections yet".to_owned(),
        );
    }
    if context.accept_invalid_certificates {
        return Err(
            "Disabling certificate validation is not supported for gRPC; configure a custom CA instead"
                .to_owned(),
        );
    }
    let parsed =
        url::Url::parse(endpoint.trim()).map_err(|_| "gRPC endpoint is invalid".to_owned())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("gRPC endpoint must be HTTP(S) and cannot contain credentials".to_owned());
    }
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("gRPC endpoint must contain only a scheme, host, and optional port".to_owned());
    }
    let timeout = Duration::from_millis(timeout_ms.clamp(1, 300_000));
    let mut builder = Endpoint::from_shared(endpoint.trim().to_owned())
        .map_err(|_| "gRPC endpoint is invalid".to_owned())?
        .connect_timeout(timeout)
        .timeout(timeout)
        .tcp_nodelay(true);
    if parsed.scheme() == "https" {
        let mut tls = ClientTlsConfig::new();
        if let Some(host) = parsed.host_str() {
            tls = tls.domain_name(host.to_owned());
        }
        if !context.root_certificate_pem.is_empty() {
            tls = tls.ca_certificate(Certificate::from_pem(context.root_certificate_pem.clone()));
        }
        if !context.client_identity_pem.is_empty() {
            let (certificates, key) = split_identity_pem(&context.client_identity_pem)?;
            tls = tls.identity(Identity::from_pem(certificates, key));
        }
        builder = builder
            .tls_config(tls)
            .map_err(|error| format!("gRPC TLS configuration is invalid: {error}"))?;
    }
    builder
        .connect()
        .await
        .map_err(|error| format!("gRPC connection failed: {error}"))
}

fn split_identity_pem(contents: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let text = std::str::from_utf8(contents)
        .map_err(|_| "gRPC client identity PEM is not UTF-8".to_owned())?;
    let mut certificates = String::new();
    let mut key = String::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if !line.starts_with("-----BEGIN ") {
            continue;
        }
        let label = line
            .trim_start_matches("-----BEGIN ")
            .trim_end_matches("-----");
        let end = format!("-----END {label}-----");
        let mut block = format!("{line}\n");
        let mut complete = false;
        for value in lines.by_ref() {
            block.push_str(value);
            block.push('\n');
            if value == end {
                complete = true;
                break;
            }
        }
        if !complete {
            return Err("gRPC client identity contains an incomplete PEM block".to_owned());
        }
        if label == "CERTIFICATE" {
            certificates.push_str(&block);
        } else if label.ends_with("PRIVATE KEY") && key.is_empty() {
            key = block;
        }
    }
    if certificates.is_empty() || key.is_empty() {
        return Err(
            "gRPC client identity PEM must contain a certificate chain and private key".to_owned(),
        );
    }
    Ok((certificates.into_bytes(), key.into_bytes()))
}

async fn load_descriptor_set(path: &str) -> Result<DescriptorPool, String> {
    if path.trim().is_empty() {
        return Err("Choose a protobuf descriptor-set file or enable reflection".to_owned());
    }
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| "gRPC descriptor-set file could not be inspected".to_owned())?;
    if !metadata.is_file() || metadata.len() > MAX_DESCRIPTOR_BYTES {
        return Err("gRPC descriptor-set file is invalid or exceeds 16 MiB".to_owned());
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| "gRPC descriptor-set file could not be read".to_owned())?;
    let mut pool = DescriptorPool::new();
    pool.decode_file_descriptor_set(bytes.as_slice())
        .map_err(|error| format!("gRPC descriptor set is invalid: {error}"))?;
    Ok(pool)
}

async fn list_services(channel: Channel, metadata: &[HeaderEntry]) -> Result<Vec<String>, String> {
    match reflection_v1(channel.clone(), ReflectionRequest::ListServices, metadata).await {
        Ok(ReflectionResponse::Services(services)) => Ok(services),
        Ok(_) => Err("gRPC reflection returned an unexpected response".to_owned()),
        Err(v1_error) => {
            match reflection_v1alpha(channel, ReflectionRequest::ListServices, metadata).await {
                Ok(ReflectionResponse::Services(services)) => Ok(services),
                Ok(_) => Err("gRPC reflection returned an unexpected response".to_owned()),
                Err(alpha_error) => Err(format!(
                    "gRPC reflection failed (v1: {v1_error}; v1alpha: {alpha_error})"
                )),
            }
        }
    }
}

async fn descriptors_for_symbol(
    channel: Channel,
    symbol: &str,
    metadata: &[HeaderEntry],
) -> Result<Vec<Vec<u8>>, String> {
    match reflection_v1(
        channel.clone(),
        ReflectionRequest::FileContainingSymbol(symbol.to_owned()),
        metadata,
    )
    .await
    {
        Ok(ReflectionResponse::Descriptors(files)) => Ok(files),
        Ok(_) => Err("gRPC reflection returned an unexpected response".to_owned()),
        Err(v1_error) => match reflection_v1alpha(
            channel,
            ReflectionRequest::FileContainingSymbol(symbol.to_owned()),
            metadata,
        )
        .await
        {
            Ok(ReflectionResponse::Descriptors(files)) => Ok(files),
            Ok(_) => Err("gRPC reflection returned an unexpected response".to_owned()),
            Err(alpha_error) => Err(format!(
                "gRPC reflection failed (v1: {v1_error}; v1alpha: {alpha_error})"
            )),
        },
    }
}

enum ReflectionRequest {
    ListServices,
    FileContainingSymbol(String),
}

enum ReflectionResponse {
    Services(Vec<String>),
    Descriptors(Vec<Vec<u8>>),
}

async fn reflection_v1(
    channel: Channel,
    request: ReflectionRequest,
    metadata: &[HeaderEntry],
) -> Result<ReflectionResponse, String> {
    use v1::server_reflection_request::MessageRequest;
    use v1::server_reflection_response::MessageResponse;

    let message_request = match request {
        ReflectionRequest::ListServices => MessageRequest::ListServices(String::new()),
        ReflectionRequest::FileContainingSymbol(symbol) => {
            MessageRequest::FileContainingSymbol(symbol)
        }
    };
    let mut request = Request::new(iter([v1::ServerReflectionRequest {
        host: String::new(),
        message_request: Some(message_request),
    }]));
    append_metadata(&mut request, metadata).map_err(|error| error.to_string())?;
    let mut client = v1::server_reflection_client::ServerReflectionClient::new(channel);
    let mut stream = client
        .server_reflection_info(request)
        .await
        .map_err(|error| error.to_string())?
        .into_inner();
    let response = stream
        .message()
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "gRPC reflection returned no response".to_owned())?;
    match response.message_response {
        Some(MessageResponse::ListServicesResponse(response)) => Ok(ReflectionResponse::Services(
            response
                .service
                .into_iter()
                .map(|service| service.name)
                .collect(),
        )),
        Some(MessageResponse::FileDescriptorResponse(response)) => Ok(
            ReflectionResponse::Descriptors(response.file_descriptor_proto),
        ),
        Some(MessageResponse::ErrorResponse(error)) => {
            Err(format!("{} ({})", error.error_message, error.error_code))
        }
        _ => Err("gRPC reflection returned an unsupported response".to_owned()),
    }
}

async fn reflection_v1alpha(
    channel: Channel,
    request: ReflectionRequest,
    metadata: &[HeaderEntry],
) -> Result<ReflectionResponse, String> {
    use v1alpha::server_reflection_request::MessageRequest;
    use v1alpha::server_reflection_response::MessageResponse;

    let message_request = match request {
        ReflectionRequest::ListServices => MessageRequest::ListServices(String::new()),
        ReflectionRequest::FileContainingSymbol(symbol) => {
            MessageRequest::FileContainingSymbol(symbol)
        }
    };
    let mut request = Request::new(iter([v1alpha::ServerReflectionRequest {
        host: String::new(),
        message_request: Some(message_request),
    }]));
    append_metadata(&mut request, metadata).map_err(|error| error.to_string())?;
    let mut client = v1alpha::server_reflection_client::ServerReflectionClient::new(channel);
    let mut stream = client
        .server_reflection_info(request)
        .await
        .map_err(|error| error.to_string())?
        .into_inner();
    let response = stream
        .message()
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "gRPC reflection returned no response".to_owned())?;
    match response.message_response {
        Some(MessageResponse::ListServicesResponse(response)) => Ok(ReflectionResponse::Services(
            response
                .service
                .into_iter()
                .map(|service| service.name)
                .collect(),
        )),
        Some(MessageResponse::FileDescriptorResponse(response)) => Ok(
            ReflectionResponse::Descriptors(response.file_descriptor_proto),
        ),
        Some(MessageResponse::ErrorResponse(error)) => {
            Err(format!("{} ({})", error.error_message, error.error_code))
        }
        _ => Err("gRPC reflection returned an unsupported response".to_owned()),
    }
}

fn descriptor_pool(files: Vec<Vec<u8>>) -> Result<DescriptorPool, String> {
    let total = files.iter().map(Vec::len).sum::<usize>();
    if total > MAX_DESCRIPTOR_BYTES as usize {
        return Err("gRPC reflection descriptors exceed the 16 MiB limit".to_owned());
    }
    let protos = files
        .into_iter()
        .map(|bytes| {
            prost_reflect::prost_types::FileDescriptorProto::decode(bytes.as_slice())
                .map_err(|error| format!("gRPC reflection descriptor is invalid: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut pool = DescriptorPool::new();
    pool.add_file_descriptor_protos(protos)
        .map_err(|error| format!("gRPC reflection descriptors are incomplete: {error}"))?;
    Ok(pool)
}

fn method_info(method: MethodDescriptor) -> GrpcMethodInfo {
    GrpcMethodInfo {
        name: method.name().to_owned(),
        full_name: method.full_name().to_owned(),
        input_type: method.input().full_name().to_owned(),
        output_type: method.output().full_name().to_owned(),
        client_streaming: method.is_client_streaming(),
        server_streaming: method.is_server_streaming(),
    }
}

fn validate_metadata(metadata: &[HeaderEntry]) -> Result<(), String> {
    if metadata.len() > 4_096 {
        return Err("gRPC metadata contains more than 4,096 entries".to_owned());
    }
    for entry in metadata.iter().filter(|entry| entry.enabled) {
        if entry.name.trim().ends_with("-bin") {
            BASE64
                .decode(&entry.value)
                .map_err(|_| format!("Binary gRPC metadata '{}' must be base64", entry.name))?;
        } else {
            let _: MetadataKey<Ascii> = entry
                .name
                .trim()
                .parse()
                .map_err(|_| format!("gRPC metadata name '{}' is invalid", entry.name))?;
            let _: MetadataValue<Ascii> = entry
                .value
                .parse()
                .map_err(|_| format!("gRPC metadata '{}' has an invalid value", entry.name))?;
        }
    }
    Ok(())
}

fn append_metadata<T>(request: &mut Request<T>, metadata: &[HeaderEntry]) -> Result<(), Status> {
    for entry in metadata.iter().filter(|entry| entry.enabled) {
        if entry.name.trim().is_empty() && entry.value.is_empty() {
            continue;
        }
        if entry.name.trim().ends_with("-bin") {
            let key: MetadataKey<Binary> =
                entry.name.trim().parse().map_err(|_| {
                    Status::invalid_argument("gRPC binary metadata name is invalid")
                })?;
            let bytes = BASE64
                .decode(&entry.value)
                .map_err(|_| Status::invalid_argument("gRPC binary metadata is not base64"))?;
            request
                .metadata_mut()
                .append_bin(key, MetadataValue::from_bytes(&bytes));
        } else {
            let key: MetadataKey<Ascii> = entry
                .name
                .trim()
                .parse()
                .map_err(|_| Status::invalid_argument("gRPC metadata name is invalid"))?;
            let value: MetadataValue<Ascii> = entry
                .value
                .parse()
                .map_err(|_| Status::invalid_argument("gRPC metadata value is invalid"))?;
            request.metadata_mut().append(key, value);
        }
    }
    Ok(())
}

fn metadata_entries(metadata: &tonic::metadata::MetadataMap) -> Vec<HeaderEntry> {
    metadata
        .iter()
        .map(|entry| match entry {
            KeyAndValueRef::Ascii(key, value) => HeaderEntry {
                name: key.as_str().to_owned(),
                value: value.to_str().unwrap_or("[non-UTF-8]").to_owned(),
                enabled: true,
            },
            KeyAndValueRef::Binary(key, value) => HeaderEntry {
                name: key.as_str().to_owned(),
                value: BASE64.encode(value.as_encoded_bytes()),
                enabled: true,
            },
        })
        .collect()
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
    type Error = Status;

    fn encode(
        &mut self,
        item: Self::Item,
        destination: &mut EncodeBuf<'_>,
    ) -> Result<(), Self::Error> {
        destination.reserve(item.encoded_len());
        item.encode(destination)
            .map_err(|error| Status::internal(format!("protobuf encoding failed: {error}")))
    }
}

struct DynamicDecoder {
    descriptor: MessageDescriptor,
}

impl Decoder for DynamicDecoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn decode(&mut self, source: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        DynamicMessage::decode(self.descriptor.clone(), source)
            .map(Some)
            .map_err(|error| Status::internal(format!("protobuf decoding failed: {error}")))
    }
}

fn redact(value: &str, redactions: &[String]) -> String {
    redactions
        .iter()
        .filter(|secret| secret.len() >= 3)
        .fold(value.to_owned(), |redacted, secret| {
            redacted.replace(secret, "[REDACTED]")
        })
}

#[cfg(test)]
mod tests {
    use prost::Message as _;
    use prost_reflect::prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
        MethodDescriptorProto, ServiceDescriptorProto,
        field_descriptor_proto::{Label, Type},
    };

    use super::{descriptor_pool, dynamic_from_json, method_info, split_identity_pem};

    fn descriptor_bytes() -> Vec<Vec<u8>> {
        let message = DescriptorProto {
            name: Some("HelloRequest".to_owned()),
            field: vec![FieldDescriptorProto {
                name: Some("name".to_owned()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::String as i32),
                json_name: Some("name".to_owned()),
                ..FieldDescriptorProto::default()
            }],
            ..DescriptorProto::default()
        };
        let response = DescriptorProto {
            name: Some("HelloResponse".to_owned()),
            ..DescriptorProto::default()
        };
        let file = FileDescriptorProto {
            name: Some("hello.proto".to_owned()),
            package: Some("demo".to_owned()),
            syntax: Some("proto3".to_owned()),
            message_type: vec![message, response],
            service: vec![ServiceDescriptorProto {
                name: Some("Greeter".to_owned()),
                method: vec![MethodDescriptorProto {
                    name: Some("SayHello".to_owned()),
                    input_type: Some(".demo.HelloRequest".to_owned()),
                    output_type: Some(".demo.HelloResponse".to_owned()),
                    ..MethodDescriptorProto::default()
                }],
                ..ServiceDescriptorProto::default()
            }],
            ..FileDescriptorProto::default()
        };
        vec![file.encode_to_vec()]
    }

    #[test]
    fn builds_dynamic_messages_from_reflection_descriptors() {
        let pool = descriptor_pool(descriptor_bytes()).unwrap();
        let service = pool.get_service_by_name("demo.Greeter").unwrap();
        let method = service.methods().next().unwrap();
        let info = method_info(method.clone());
        assert_eq!(info.full_name, "demo.Greeter.SayHello");
        let message =
            dynamic_from_json(method.input(), serde_json::json!({ "name": "Ada" })).unwrap();
        assert!(!message.encode_to_vec().is_empty());
    }

    #[test]
    fn splits_a_combined_client_identity() {
        let pem = b"-----BEGIN CERTIFICATE-----\nY2VydA==\n-----END CERTIFICATE-----\n-----BEGIN PRIVATE KEY-----\na2V5\n-----END PRIVATE KEY-----\n";
        let (certificates, key) = split_identity_pem(pem).unwrap();
        assert!(
            String::from_utf8(certificates)
                .unwrap()
                .contains("CERTIFICATE")
        );
        assert!(String::from_utf8(key).unwrap().contains("PRIVATE KEY"));
    }

    #[test]
    fn descriptor_fixture_is_a_valid_file_descriptor_set_too() {
        let pool = descriptor_pool(descriptor_bytes()).unwrap();
        let set = FileDescriptorSet {
            file: pool.file_descriptor_protos().cloned().collect(),
        };
        assert!(!set.encode_to_vec().is_empty());
    }
}
