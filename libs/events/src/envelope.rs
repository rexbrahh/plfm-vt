//! Event envelope - the common wrapper for all events.

use chrono::{DateTime, Utc};
use plfm_id::{AggregateSeq, AppId, EnvId, EventId, OrgId, RequestId};
use serde::{Deserialize, Serialize};

/// Actor type for audit logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ActorType {
    /// A human user.
    User,
    /// A service principal (API key, service account).
    ServicePrincipal,
    /// The system itself (reconciliation, scheduler).
    #[default]
    System,
}

impl std::fmt::Display for ActorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActorType::User => write!(f, "user"),
            ActorType::ServicePrincipal => write!(f, "service_principal"),
            ActorType::System => write!(f, "system"),
        }
    }
}

/// Aggregate type for event routing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AggregateType {
    #[default]
    Org,
    Project,
    OrgMember,
    ServicePrincipal,
    App,
    Env,
    Release,
    Deploy,
    Route,
    SecretBundle,
    Volume,
    VolumeAttachment,
    Snapshot,
    RestoreJob,
    Instance,
    Node,
    ExecSession,
}

impl std::fmt::Display for AggregateType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            AggregateType::Org => "org",
            AggregateType::Project => "project",
            AggregateType::OrgMember => "org_member",
            AggregateType::ServicePrincipal => "service_principal",
            AggregateType::App => "app",
            AggregateType::Env => "env",
            AggregateType::Release => "release",
            AggregateType::Deploy => "deploy",
            AggregateType::Route => "route",
            AggregateType::SecretBundle => "secret_bundle",
            AggregateType::Volume => "volume",
            AggregateType::VolumeAttachment => "volume_attachment",
            AggregateType::Snapshot => "snapshot",
            AggregateType::RestoreJob => "restore_job",
            AggregateType::Instance => "instance",
            AggregateType::Node => "node",
            AggregateType::ExecSession => "exec_session",
        };
        write!(f, "{}", s)
    }
}

/// The event envelope - common metadata for all events.
///
/// This corresponds to the `api/schemas/event-envelope.json` schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope<P> {
    /// Globally monotonic event identifier.
    pub event_id: EventId,

    /// When the event occurred.
    pub occurred_at: DateTime<Utc>,

    /// The type of aggregate this event belongs to.
    pub aggregate_type: AggregateType,

    /// The ID of the aggregate instance.
    pub aggregate_id: String,

    /// Monotonic sequence within the aggregate.
    pub aggregate_seq: AggregateSeq,

    /// The event type (e.g., "org.created", "instance.started").
    pub event_type: String,

    /// Schema version for this event type.
    pub event_version: i32,

    /// Type of actor that triggered the event.
    pub actor_type: ActorType,

    /// Identifier of the actor.
    pub actor_id: String,

    /// Organization ID (required for tenant-scoped aggregates).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id: Option<OrgId>,

    /// Request correlation ID for tracing.
    pub request_id: RequestId,

    /// Client-provided idempotency key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,

    /// Application ID if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<AppId>,

    /// Environment ID if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_id: Option<EnvId>,

    /// Grouping ID for related events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,

    /// Event ID of the event that caused this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<EventId>,

    /// Event-specific payload.
    pub payload: P,
}

impl<P> EventEnvelope<P> {
    /// Creates a new event envelope builder.
    pub fn builder() -> EventEnvelopeBuilder<P> {
        EventEnvelopeBuilder::new()
    }
}

/// Builder for constructing event envelopes.
#[derive(Debug)]
pub struct EventEnvelopeBuilder<P> {
    event_id: Option<EventId>,
    occurred_at: Option<DateTime<Utc>>,
    aggregate_type: Option<AggregateType>,
    aggregate_id: Option<String>,
    aggregate_seq: Option<AggregateSeq>,
    event_type: Option<String>,
    event_version: i32,
    actor_type: Option<ActorType>,
    actor_id: Option<String>,
    org_id: Option<OrgId>,
    request_id: Option<RequestId>,
    idempotency_key: Option<String>,
    app_id: Option<AppId>,
    env_id: Option<EnvId>,
    correlation_id: Option<String>,
    causation_id: Option<EventId>,
    payload: Option<P>,
}

impl<P> EventEnvelopeBuilder<P> {
    pub fn new() -> Self {
        Self {
            event_id: None,
            occurred_at: None,
            aggregate_type: None,
            aggregate_id: None,
            aggregate_seq: None,
            event_type: None,
            event_version: 1,
            actor_type: None,
            actor_id: None,
            org_id: None,
            request_id: None,
            idempotency_key: None,
            app_id: None,
            env_id: None,
            correlation_id: None,
            causation_id: None,
            payload: None,
        }
    }

    pub fn event_id(mut self, id: EventId) -> Self {
        self.event_id = Some(id);
        self
    }

    pub fn occurred_at(mut self, ts: DateTime<Utc>) -> Self {
        self.occurred_at = Some(ts);
        self
    }

    pub fn aggregate(mut self, agg_type: AggregateType, agg_id: impl Into<String>) -> Self {
        self.aggregate_type = Some(agg_type);
        self.aggregate_id = Some(agg_id.into());
        self
    }

    pub fn aggregate_seq(mut self, seq: AggregateSeq) -> Self {
        self.aggregate_seq = Some(seq);
        self
    }

    pub fn event_type(mut self, event_type: impl Into<String>) -> Self {
        self.event_type = Some(event_type.into());
        self
    }

    pub fn event_version(mut self, version: i32) -> Self {
        self.event_version = version;
        self
    }

    pub fn actor(mut self, actor_type: ActorType, actor_id: impl Into<String>) -> Self {
        self.actor_type = Some(actor_type);
        self.actor_id = Some(actor_id.into());
        self
    }

    pub fn org_id(mut self, org_id: OrgId) -> Self {
        self.org_id = Some(org_id);
        self
    }

    pub fn request_id(mut self, request_id: RequestId) -> Self {
        self.request_id = Some(request_id);
        self
    }

    pub fn idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    pub fn app_id(mut self, app_id: AppId) -> Self {
        self.app_id = Some(app_id);
        self
    }

    pub fn env_id(mut self, env_id: EnvId) -> Self {
        self.env_id = Some(env_id);
        self
    }

    pub fn correlation_id(mut self, id: impl Into<String>) -> Self {
        self.correlation_id = Some(id.into());
        self
    }

    pub fn causation_id(mut self, id: EventId) -> Self {
        self.causation_id = Some(id);
        self
    }

    pub fn payload(mut self, payload: P) -> Self {
        self.payload = Some(payload);
        self
    }

    /// Builds the event envelope.
    ///
    /// # Panics
    ///
    /// Panics if required fields are not set.
    pub fn build(self) -> EventEnvelope<P> {
        EventEnvelope {
            event_id: self.event_id.expect("event_id is required"),
            occurred_at: self.occurred_at.unwrap_or_else(Utc::now),
            aggregate_type: self.aggregate_type.expect("aggregate_type is required"),
            aggregate_id: self.aggregate_id.expect("aggregate_id is required"),
            aggregate_seq: self.aggregate_seq.expect("aggregate_seq is required"),
            event_type: self.event_type.expect("event_type is required"),
            event_version: self.event_version,
            actor_type: self.actor_type.expect("actor_type is required"),
            actor_id: self.actor_id.expect("actor_id is required"),
            org_id: self.org_id,
            request_id: self.request_id.expect("request_id is required"),
            idempotency_key: self.idempotency_key,
            app_id: self.app_id,
            env_id: self.env_id,
            correlation_id: self.correlation_id,
            causation_id: self.causation_id,
            payload: self.payload.expect("payload is required"),
        }
    }
}

impl<P> Default for EventEnvelopeBuilder<P> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_actor_type_serialization() {
        assert_eq!(serde_json::to_string(&ActorType::User).unwrap(), "\"user\"");
        assert_eq!(
            serde_json::to_string(&ActorType::ServicePrincipal).unwrap(),
            "\"service_principal\""
        );
        assert_eq!(
            serde_json::to_string(&ActorType::System).unwrap(),
            "\"system\""
        );
    }

    #[test]
    fn test_aggregate_type_display() {
        assert_eq!(AggregateType::Org.to_string(), "org");
        assert_eq!(AggregateType::OrgMember.to_string(), "org_member");
        assert_eq!(AggregateType::SecretBundle.to_string(), "secret_bundle");
    }

    #[test]
    fn test_event_envelope_builder() {
        let envelope = EventEnvelope::<serde_json::Value>::builder()
            .event_id(EventId::new(1))
            .aggregate(AggregateType::Org, "org_01HV4Z2WQXKJNM8GPQY6VBKC3D")
            .aggregate_seq(AggregateSeq::FIRST)
            .event_type("org.created")
            .actor(ActorType::User, "user_123")
            .request_id(RequestId::new())
            .payload(serde_json::json!({"name": "Acme Corp"}))
            .build();

        assert_eq!(envelope.event_type, "org.created");
        assert_eq!(envelope.event_version, 1);
        assert_eq!(envelope.aggregate_type, AggregateType::Org);
    }
}
