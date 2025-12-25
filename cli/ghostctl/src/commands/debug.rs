use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};
use plfm_proto::FILE_DESCRIPTOR_SET;
use prost_reflect::prost::Message;
use prost_reflect::{
    DescriptorPool, DeserializeOptions, DynamicMessage, MessageDescriptor, MethodDescriptor, Value,
};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::str::FromStr;
use tokio_stream::StreamExt;
use tonic::codec::{BufferSettings, Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::codegen::http::uri::PathAndQuery;
use tonic::metadata::MetadataValue;
use tonic::transport::Endpoint;
use tonic::{Request, Status};

use super::CommandContext;
use crate::output::print_single;
use crate::resolve;

#[derive(Debug, Args)]
pub struct DebugCommand {
    #[command(subcommand)]
    command: DebugSubcommand,
}

#[derive(Debug, Subcommand)]
enum DebugSubcommand {
    DecodeEvent(DecodeEventArgs),
    GrpcCall(GrpcCallArgs),
}

#[derive(Debug, Args)]
struct DecodeEventArgs {
    event_id: String,

    #[arg(long, help = "Show raw protobuf bytes as hex")]
    raw: bool,

    #[arg(long, help = "Path to proto type registry")]
    registry: Option<String>,

    #[arg(long, help = "gRPC endpoint URL for event envelope retrieval")]
    endpoint: Option<String>,
}

#[derive(Debug, Args)]
struct GrpcCallArgs {
    service: String,

    method: String,

    #[arg(long, help = "Request body as JSON")]
    data: Option<String>,

    #[arg(long, help = "Read request body from file")]
    data_file: Option<String>,

    #[arg(long, help = "gRPC endpoint URL")]
    endpoint: Option<String>,

    #[arg(long, help = "Use insecure connection (no TLS)")]
    insecure: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct EventRecord {
    event_id: i64,
    occurred_at: String,
    event_type: String,
    #[serde(default)]
    aggregate_type: Option<String>,
    #[serde(default)]
    aggregate_id: Option<String>,
    #[serde(default)]
    aggregate_seq: Option<i64>,
    #[serde(default)]
    actor_id: Option<String>,
    #[serde(default)]
    payload: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct EventsResponse {
    items: Vec<EventRecord>,
    next_after_event_id: i64,
}

#[derive(Debug, Serialize)]
struct DecodeEventHttpOutput {
    source: &'static str,
    event: EventRecord,
}

#[derive(Debug, Serialize)]
struct DecodeEventGrpcOutput {
    source: &'static str,
    envelope: serde_json::Value,
    payload_type_url: Option<String>,
    payload: Option<serde_json::Value>,
    raw_payload_hex: Option<String>,
}

#[derive(Clone)]
struct DynamicCodec {
    output: MessageDescriptor,
    buffer_settings: BufferSettings,
}

#[derive(Clone, Default)]
struct DynamicEncoder {
    buffer_settings: BufferSettings,
}

#[derive(Clone)]
struct DynamicDecoder {
    output: MessageDescriptor,
    buffer_settings: BufferSettings,
}

impl DynamicCodec {
    fn new(output: MessageDescriptor) -> Self {
        Self {
            output,
            buffer_settings: BufferSettings::default(),
        }
    }
}

impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynamicEncoder;
    type Decoder = DynamicDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        DynamicEncoder {
            buffer_settings: self.buffer_settings,
        }
    }

    fn decoder(&mut self) -> Self::Decoder {
        DynamicDecoder {
            output: self.output.clone(),
            buffer_settings: self.buffer_settings,
        }
    }
}

impl Encoder for DynamicEncoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn encode(&mut self, item: Self::Item, buf: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        item.encode(buf)
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(())
    }

    fn buffer_settings(&self) -> BufferSettings {
        self.buffer_settings
    }
}

impl Decoder for DynamicDecoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn decode(&mut self, buf: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        let message = DynamicMessage::decode(self.output.clone(), buf)
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Some(message))
    }

    fn buffer_settings(&self) -> BufferSettings {
        self.buffer_settings
    }
}

impl DebugCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            DebugSubcommand::DecodeEvent(args) => decode_event(ctx, args).await,
            DebugSubcommand::GrpcCall(args) => grpc_call(ctx, args).await,
        }
    }
}

async fn decode_event(ctx: CommandContext, args: DecodeEventArgs) -> Result<()> {
    let pool = load_descriptor_pool(args.registry.as_deref())?;

    if let Some(endpoint) = args.endpoint.as_deref() {
        let output = decode_event_grpc(&ctx, &pool, endpoint, &args.event_id, args.raw).await?;
        print_single(&output, ctx.format);
        return Ok(());
    }

    if args.raw {
        return Err(anyhow!("--raw requires --endpoint to fetch payload bytes"));
    }

    if args.event_id.parse::<i64>().is_err() {
        return Err(anyhow!(
            "Non-numeric event IDs require --endpoint for gRPC lookup"
        ));
    }

    let output = decode_event_http(&ctx, &args.event_id).await?;
    print_single(&output, ctx.format);
    Ok(())
}

async fn decode_event_http(ctx: &CommandContext, event_id: &str) -> Result<DecodeEventHttpOutput> {
    let event_id_num: i64 = event_id
        .parse()
        .with_context(|| "event_id must be numeric for HTTP lookup")?;
    let client = ctx.client()?;
    let org_id = resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let after = if event_id_num > 0 {
        event_id_num - 1
    } else {
        0
    };
    let path = format!(
        "/v1/orgs/{}/events?after_event_id={}&limit=1",
        org_id, after
    );

    let response: EventsResponse = client.get(&path).await?;
    let event = response
        .items
        .into_iter()
        .find(|item| item.event_id == event_id_num)
        .ok_or_else(|| anyhow!("Event {} not found", event_id))?;

    Ok(DecodeEventHttpOutput {
        source: "http",
        event,
    })
}

async fn decode_event_grpc(
    ctx: &CommandContext,
    pool: &DescriptorPool,
    endpoint: &str,
    event_id: &str,
    raw: bool,
) -> Result<DecodeEventGrpcOutput> {
    let method = resolve_method(pool, "plfm.controlplane.v1.EventsApi", "GetEvent")?;
    let input_desc = method.input();
    let mut request_message = DynamicMessage::new(input_desc.clone());
    let field = input_desc
        .get_field_by_name("event_id")
        .ok_or_else(|| anyhow!("GetEventRequest missing event_id field"))?;
    request_message.set_field(&field, Value::String(event_id.to_string()));

    let response_message = grpc_unary(ctx, endpoint, false, &method, request_message).await?;
    let event_value = response_message
        .get_field_by_name("event")
        .ok_or_else(|| anyhow!("GetEventResponse missing event field"))?;
    let event_message = match event_value.as_ref() {
        Value::Message(message) => message,
        _ => return Err(anyhow!("GetEventResponse event is not a message")),
    };

    let payload_type_url = match event_message.get_field_by_name("payload_type_url") {
        Some(value) => match value.as_ref() {
            Value::String(value) if !value.is_empty() => Some(value.clone()),
            _ => None,
        },
        None => None,
    };

    let payload_bytes = match event_message.get_field_by_name("payload") {
        Some(value) => match value.as_ref() {
            Value::Bytes(bytes) if !bytes.is_empty() => Some(bytes.to_vec()),
            _ => None,
        },
        None => None,
    };

    let payload = match (payload_type_url.as_deref(), payload_bytes.as_deref()) {
        (Some(type_url), Some(bytes)) => {
            let message = decode_payload(pool, type_url, bytes)?;
            Some(serde_json::to_value(&message)?)
        }
        _ => None,
    };

    let raw_payload_hex = if raw {
        payload_bytes.as_deref().map(hex::encode)
    } else {
        None
    };

    Ok(DecodeEventGrpcOutput {
        source: "grpc",
        envelope: serde_json::to_value(event_message)?,
        payload_type_url,
        payload,
        raw_payload_hex,
    })
}

async fn grpc_call(ctx: CommandContext, args: GrpcCallArgs) -> Result<()> {
    let endpoint = args
        .endpoint
        .as_deref()
        .ok_or_else(|| anyhow!("--endpoint is required for grpc-call"))?;
    let pool = load_descriptor_pool(None)?;
    let method = resolve_method(&pool, &args.service, &args.method)?;

    let is_client_streaming = method.is_client_streaming();
    let is_server_streaming = method.is_server_streaming();

    match (is_client_streaming, is_server_streaming) {
        (false, false) => {
            // Unary
            let request_message = parse_single_request(&args, method.input())?;
            let response =
                grpc_unary(&ctx, endpoint, args.insecure, &method, request_message).await?;
            print_single(&serde_json::to_value(&response)?, ctx.format);
        }
        (false, true) => {
            // Server streaming
            let request_message = parse_single_request(&args, method.input())?;
            let mut stream =
                grpc_server_streaming(&ctx, endpoint, args.insecure, &method, request_message)
                    .await?;
            while let Some(message) = stream.message().await? {
                print_single(&serde_json::to_value(&message)?, ctx.format);
            }
        }
        (true, false) => {
            // Client streaming
            let messages = parse_ndjson_requests(&args, method.input())?;
            let response =
                grpc_client_streaming(&ctx, endpoint, args.insecure, &method, messages).await?;
            print_single(&serde_json::to_value(&response)?, ctx.format);
        }
        (true, true) => {
            // Bidirectional streaming
            let messages = parse_ndjson_requests(&args, method.input())?;
            let mut stream =
                grpc_bidi_streaming(&ctx, endpoint, args.insecure, &method, messages).await?;
            while let Some(message) = stream.next().await {
                let message = message?;
                print_single(&serde_json::to_value(&message)?, ctx.format);
            }
        }
    }

    Ok(())
}

fn parse_single_request(args: &GrpcCallArgs, desc: MessageDescriptor) -> Result<DynamicMessage> {
    let request_data = match (&args.data, &args.data_file) {
        (Some(data), None) => data.clone(),
        (None, Some(file)) => std::fs::read_to_string(file)?,
        (Some(_), Some(_)) => {
            return Err(anyhow!("Cannot specify both --data and --data-file"));
        }
        (None, None) => "{}".to_string(),
    };
    parse_json_message(desc, &request_data)
}

fn parse_ndjson_requests(
    args: &GrpcCallArgs,
    desc: MessageDescriptor,
) -> Result<Vec<DynamicMessage>> {
    let content = match (&args.data, &args.data_file) {
        (Some(data), None) => data.clone(),
        (None, Some(file)) => std::fs::read_to_string(file)?,
        (Some(_), Some(_)) => {
            return Err(anyhow!("Cannot specify both --data and --data-file"));
        }
        (None, None) => {
            // Read from stdin for streaming
            let stdin = std::io::stdin();
            let reader = BufReader::new(stdin.lock());
            let mut messages = Vec::new();
            for line in reader.lines() {
                let line = line.context("Failed to read from stdin")?;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                messages.push(parse_json_message(desc.clone(), trimmed)?);
            }
            return Ok(messages);
        }
    };

    let mut messages = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        messages.push(parse_json_message(desc.clone(), trimmed)?);
    }

    if messages.is_empty() {
        // Fall back to treating entire content as single message
        messages.push(parse_json_message(desc, &content)?);
    }

    Ok(messages)
}

fn load_descriptor_pool(registry: Option<&str>) -> Result<DescriptorPool> {
    match registry {
        Some(path) => {
            let bytes = std::fs::read(path)
                .with_context(|| format!("Failed to read registry file at {}", path))?;
            DescriptorPool::decode(bytes.as_slice()).context("Failed to decode registry file")
        }
        None => {
            DescriptorPool::decode(FILE_DESCRIPTOR_SET).context("Failed to load descriptor set")
        }
    }
}

fn resolve_method(pool: &DescriptorPool, service: &str, method: &str) -> Result<MethodDescriptor> {
    let service_desc = pool
        .get_service_by_name(service)
        .ok_or_else(|| anyhow!("Service not found: {}", service))?;
    let method_desc = service_desc
        .methods()
        .find(|candidate| candidate.name() == method);
    method_desc.ok_or_else(|| anyhow!("Method not found: {}.{}", service, method))
}

fn parse_json_message(desc: MessageDescriptor, data: &str) -> Result<DynamicMessage> {
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return Ok(DynamicMessage::new(desc));
    }

    let mut deserializer = serde_json::Deserializer::from_str(trimmed);
    let options = DeserializeOptions::new().deny_unknown_fields(false);
    let message = DynamicMessage::deserialize_with_options(desc, &mut deserializer, &options)
        .map_err(|e| anyhow!("Invalid JSON: {}", e))?;
    deserializer
        .end()
        .map_err(|e| anyhow!("Invalid JSON: {}", e))?;
    Ok(message)
}

fn decode_payload(pool: &DescriptorPool, type_url: &str, bytes: &[u8]) -> Result<DynamicMessage> {
    let message_name = type_url.rsplit('/').next().unwrap_or(type_url);
    let descriptor = pool
        .get_message_by_name(message_name)
        .ok_or_else(|| anyhow!("Message not found in registry: {}", message_name))?;
    DynamicMessage::decode(descriptor, bytes)
        .map_err(|e| anyhow!("Failed to decode payload: {}", e))
}

async fn grpc_unary(
    ctx: &CommandContext,
    endpoint: &str,
    insecure: bool,
    method: &MethodDescriptor,
    request_message: DynamicMessage,
) -> Result<DynamicMessage> {
    let channel = connect_endpoint(endpoint, insecure).await?;
    let mut grpc = tonic::client::Grpc::new(channel);
    let path = PathAndQuery::from_str(&format!(
        "/{}/{}",
        method.parent_service().full_name(),
        method.name()
    ))
    .context("Invalid gRPC method path")?;

    let mut request = Request::new(request_message);
    if let Some(creds) = ctx.credentials.as_ref() {
        let value = MetadataValue::try_from(format!("Bearer {}", creds.token))?;
        request.metadata_mut().insert("authorization", value);
    }

    let codec = DynamicCodec::new(method.output());
    let response = grpc.unary(request, path, codec).await?;
    Ok(response.into_inner())
}

async fn grpc_server_streaming(
    ctx: &CommandContext,
    endpoint: &str,
    insecure: bool,
    method: &MethodDescriptor,
    request_message: DynamicMessage,
) -> Result<tonic::codec::Streaming<DynamicMessage>> {
    let channel = connect_endpoint(endpoint, insecure).await?;
    let mut grpc = tonic::client::Grpc::new(channel);
    let path = PathAndQuery::from_str(&format!(
        "/{}/{}",
        method.parent_service().full_name(),
        method.name()
    ))
    .context("Invalid gRPC method path")?;

    let mut request = Request::new(request_message);
    if let Some(creds) = ctx.credentials.as_ref() {
        let value = MetadataValue::try_from(format!("Bearer {}", creds.token))?;
        request.metadata_mut().insert("authorization", value);
    }

    let codec = DynamicCodec::new(method.output());
    let response = grpc.server_streaming(request, path, codec).await?;
    Ok(response.into_inner())
}

async fn connect_endpoint(endpoint: &str, insecure: bool) -> Result<tonic::transport::Channel> {
    let normalized = normalize_endpoint(endpoint, insecure);
    Endpoint::from_shared(normalized)
        .context("Invalid gRPC endpoint")?
        .connect()
        .await
        .context("Failed to connect to gRPC endpoint")
}

fn normalize_endpoint(endpoint: &str, insecure: bool) -> String {
    let mut out = if endpoint.contains("://") {
        endpoint.to_string()
    } else if insecure {
        format!("http://{}", endpoint)
    } else {
        format!("https://{}", endpoint)
    };

    if insecure && out.starts_with("https://") {
        out = out.replacen("https://", "http://", 1);
    }

    out
}

async fn grpc_client_streaming(
    ctx: &CommandContext,
    endpoint: &str,
    insecure: bool,
    method: &MethodDescriptor,
    messages: Vec<DynamicMessage>,
) -> Result<DynamicMessage> {
    let channel = connect_endpoint(endpoint, insecure).await?;
    let mut grpc = tonic::client::Grpc::new(channel);
    let path = PathAndQuery::from_str(&format!(
        "/{}/{}",
        method.parent_service().full_name(),
        method.name()
    ))
    .context("Invalid gRPC method path")?;

    let stream = tokio_stream::iter(messages);
    let mut request = Request::new(stream);
    if let Some(creds) = ctx.credentials.as_ref() {
        let value = MetadataValue::try_from(format!("Bearer {}", creds.token))?;
        request.metadata_mut().insert("authorization", value);
    }

    let codec = DynamicCodec::new(method.output());
    let response = grpc.client_streaming(request, path, codec).await?;
    Ok(response.into_inner())
}

async fn grpc_bidi_streaming(
    ctx: &CommandContext,
    endpoint: &str,
    insecure: bool,
    method: &MethodDescriptor,
    messages: Vec<DynamicMessage>,
) -> Result<tonic::codec::Streaming<DynamicMessage>> {
    let channel = connect_endpoint(endpoint, insecure).await?;
    let mut grpc = tonic::client::Grpc::new(channel);
    let path = PathAndQuery::from_str(&format!(
        "/{}/{}",
        method.parent_service().full_name(),
        method.name()
    ))
    .context("Invalid gRPC method path")?;

    let stream = tokio_stream::iter(messages);
    let mut request = Request::new(stream);
    if let Some(creds) = ctx.credentials.as_ref() {
        let value = MetadataValue::try_from(format!("Bearer {}", creds.token))?;
        request.metadata_mut().insert("authorization", value);
    }

    let codec = DynamicCodec::new(method.output());
    let response = grpc.streaming(request, path, codec).await?;
    Ok(response.into_inner())
}
