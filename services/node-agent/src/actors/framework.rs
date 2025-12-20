//! Core actor framework types and traits.
//!
//! Provides the fundamental building blocks for the actor system:
//! - `Actor` trait for defining actor behavior
//! - `Supervisor` for managing actor lifecycles
//! - `ActorHandle` for sending messages to actors
//! - Backoff and restart policies

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};

use std::time::{Duration, Instant};

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

// =============================================================================
// Core Traits
// =============================================================================

/// Marker trait for actor messages.
pub trait Message: Send + Debug + 'static {}

impl<T: Send + Debug + 'static> Message for T {}

/// The Actor trait defines behavior for an actor.
///
/// Actors:
/// - Process messages one at a time (no internal concurrency)
/// - Own mutable state not shared with other actors
/// - Communicate only via message passing
#[async_trait]
pub trait Actor: Send + 'static {
    /// The message type this actor handles.
    type Message: Message;

    /// Actor name for logging and metrics.
    fn name(&self) -> &str;

    /// Handle a single message.
    ///
    /// Returns `Ok(true)` to continue, `Ok(false)` to stop, or `Err` on failure.
    async fn handle(&mut self, msg: Self::Message, ctx: &mut ActorContext) -> Result<bool, ActorError>;

    /// Called when the actor starts (or restarts).
    async fn on_start(&mut self, _ctx: &mut ActorContext) -> Result<(), ActorError> {
        Ok(())
    }

    /// Called when the actor is about to stop.
    async fn on_stop(&mut self, _ctx: &mut ActorContext) {
        // Default: no cleanup
    }

    /// Called after a crash, before restart.
    /// Return the actor state to use for the restart.
    fn on_crash(&mut self, _error: &ActorError) {
        // Default: keep current state
    }
}

/// Context provided to actors during message handling.
pub struct ActorContext {
    /// Actor's unique ID.
    pub actor_id: String,

    /// Shutdown signal receiver.
    pub shutdown: watch::Receiver<bool>,

    /// Message counter for metrics.
    pub messages_processed: u64,

    /// Last message processing time for metrics.
    pub last_message_at: Option<Instant>,

    /// Current actor state (for introspection).
    pub state: ActorState,
}

impl ActorContext {
    /// Create a new actor context.
    pub fn new(actor_id: String, shutdown: watch::Receiver<bool>) -> Self {
        Self {
            actor_id,
            shutdown,
            messages_processed: 0,
            last_message_at: None,
            state: ActorState::Starting,
        }
    }

    /// Check if shutdown has been signaled.
    pub fn is_shutdown(&self) -> bool {
        *self.shutdown.borrow()
    }
}

/// Actor lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorState {
    /// Actor is starting up.
    Starting,
    /// Actor is running and processing messages.
    Running,
    /// Actor is stopping.
    Stopping,
    /// Actor has stopped.
    Stopped,
    /// Actor has failed.
    Failed,
}

// =============================================================================
// Errors
// =============================================================================

/// Errors that can occur in actors.
#[derive(Debug, Error)]
pub enum ActorError {
    /// Transient error that should be retried.
    #[error("transient error: {0}")]
    Transient(String),

    /// Permanent error that should not be retried.
    #[error("permanent error: {0}")]
    Permanent(String),

    /// Actor mailbox is full.
    #[error("mailbox full")]
    MailboxFull,

    /// Actor has stopped.
    #[error("actor stopped")]
    ActorStopped,

    /// Internal error.
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

// =============================================================================
// Actor Handle
// =============================================================================

/// Handle for sending messages to an actor.
#[derive(Clone)]
pub struct ActorHandle<M: Message> {
    /// Sender for the actor's mailbox.
    tx: mpsc::Sender<M>,

    /// Actor ID for logging.
    actor_id: String,
}

impl<M: Message> ActorHandle<M> {
    /// Send a message to the actor.
    pub async fn send(&self, msg: M) -> Result<(), ActorError> {
        self.tx
            .send(msg)
            .await
            .map_err(|_| ActorError::ActorStopped)
    }

    /// Try to send a message without blocking.
    pub fn try_send(&self, msg: M) -> Result<(), ActorError> {
        self.tx.try_send(msg).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => ActorError::MailboxFull,
            mpsc::error::TrySendError::Closed(_) => ActorError::ActorStopped,
        })
    }

    /// Get the actor ID.
    pub fn actor_id(&self) -> &str {
        &self.actor_id
    }
}

/// Type-erased actor reference for supervision.
pub struct ActorRef {
    /// Actor ID.
    pub actor_id: String,

    /// Actor type name.
    pub actor_type: String,

    /// Task handle.
    task_handle: tokio::task::JoinHandle<()>,

    /// Shutdown sender.
    shutdown_tx: watch::Sender<bool>,
}

impl ActorRef {
    /// Signal the actor to stop.
    pub fn stop(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Check if the actor task is still running.
    pub fn is_running(&self) -> bool {
        !self.task_handle.is_finished()
    }

    /// Abort the actor task immediately.
    pub fn abort(&self) {
        self.task_handle.abort();
    }
}

// =============================================================================
// Backoff Policy
// =============================================================================

/// Exponential backoff configuration.
#[derive(Debug, Clone)]
pub struct BackoffPolicy {
    /// Base delay for first retry.
    pub base: Duration,

    /// Maximum delay.
    pub max: Duration,

    /// Jitter factor (0.0 to 1.0).
    pub jitter: f64,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            base: Duration::from_millis(100),
            max: Duration::from_secs(30),
            jitter: 0.25,
        }
    }
}

impl BackoffPolicy {
    /// Calculate delay for the given attempt number.
    pub fn delay(&self, attempt: u32) -> Duration {
        let delay = self.base.as_millis() as f64 * 2.0_f64.powi(attempt as i32);
        let delay = delay.min(self.max.as_millis() as f64);

        // Add jitter
        let jitter_range = delay * self.jitter;
        let jitter = rand_jitter(jitter_range);
        let final_delay = delay + jitter;

        Duration::from_millis(final_delay as u64)
    }
}

/// Simple jitter using a basic LCG (for no external deps).
fn rand_jitter(range: f64) -> f64 {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let random = (seed.wrapping_mul(6364136223846793005).wrapping_add(1)) as f64;
    let normalized = (random / u64::MAX as f64) * 2.0 - 1.0; // -1.0 to 1.0
    normalized * range
}

// =============================================================================
// Restart Policy
// =============================================================================

/// Actor restart policy.
#[derive(Debug, Clone)]
pub struct RestartPolicy {
    /// Maximum restart attempts within the window.
    pub max_restarts: u32,

    /// Time window for counting restarts.
    pub window: Duration,

    /// Backoff policy for restarts.
    pub backoff: BackoffPolicy,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 5,
            window: Duration::from_secs(300), // 5 minutes
            backoff: BackoffPolicy::default(),
        }
    }
}

// =============================================================================
// Supervisor
// =============================================================================

/// Supervisor for managing actor lifecycles.
pub struct Supervisor {
    /// Supervised actors.
    children: HashMap<String, SupervisedActor>,

    /// Restart policy.
    restart_policy: RestartPolicy,

    /// Global shutdown signal (reserved for future use in coordinated shutdown).
    #[allow(dead_code)]
    shutdown: watch::Receiver<bool>,
}

struct SupervisedActor {
    actor_ref: ActorRef,
    restart_count: u32,
    restart_timestamps: Vec<Instant>,
    state: SupervisedState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SupervisedState {
    Running,
    Restarting,
    Stopped,
    Degraded,
}

impl Supervisor {
    /// Create a new supervisor.
    pub fn new(restart_policy: RestartPolicy, shutdown: watch::Receiver<bool>) -> Self {
        Self {
            children: HashMap::new(),
            restart_policy,
            shutdown,
        }
    }

    /// Spawn and supervise an actor.
    pub fn spawn<A>(&mut self, actor: A, mailbox_size: usize) -> ActorHandle<A::Message>
    where
        A: Actor,
    {
        let actor_id = format!("{}_{}", actor.name(), generate_actor_id());
        let (tx, rx) = mpsc::channel(mailbox_size);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let actor_id_clone = actor_id.clone();
        let actor_type = actor.name().to_string();

        let task_handle = tokio::spawn(async move {
            run_actor_loop(actor, rx, shutdown_rx, actor_id_clone).await;
        });

        let actor_ref = ActorRef {
            actor_id: actor_id.clone(),
            actor_type: actor_type.clone(),
            task_handle,
            shutdown_tx,
        };

        self.children.insert(
            actor_id.clone(),
            SupervisedActor {
                actor_ref,
                restart_count: 0,
                restart_timestamps: Vec::new(),
                state: SupervisedState::Running,
            },
        );

        info!(actor_id = %actor_id, actor_type = %actor_type, "Spawned actor");

        ActorHandle { tx, actor_id }
    }

    /// Stop all supervised actors.
    pub async fn stop_all(&mut self) {
        info!(count = self.children.len(), "Stopping all actors");

        for (_, child) in &self.children {
            child.actor_ref.stop();
        }

        // Wait for all to finish with timeout
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let all_stopped = self.children.values().all(|c| !c.actor_ref.is_running());
            if all_stopped {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Force abort any remaining
        for (actor_id, child) in &self.children {
            if child.actor_ref.is_running() {
                warn!(actor_id = %actor_id, "Force aborting actor");
                child.actor_ref.abort();
            }
        }

        self.children.clear();
    }

    /// Check actor health and handle restarts.
    pub async fn check_and_restart(&mut self) {
        let mut to_restart = Vec::new();

        for (actor_id, child) in &mut self.children {
            if !child.actor_ref.is_running() && child.state == SupervisedState::Running {
                // Actor has stopped unexpectedly
                child.state = SupervisedState::Restarting;
                to_restart.push(actor_id.clone());
            }
        }

        for actor_id in to_restart {
            if let Some(child) = self.children.get_mut(&actor_id) {
                // Prune old timestamps outside the window
                let now = Instant::now();
                child
                    .restart_timestamps
                    .retain(|t| now.duration_since(*t) < self.restart_policy.window);

                if child.restart_timestamps.len() >= self.restart_policy.max_restarts as usize {
                    warn!(
                        actor_id = %actor_id,
                        restart_count = child.restart_count,
                        "Actor exceeded max restarts, marking as degraded"
                    );
                    child.state = SupervisedState::Degraded;
                    continue;
                }

                let delay = self
                    .restart_policy
                    .backoff
                    .delay(child.restart_timestamps.len() as u32);

                info!(
                    actor_id = %actor_id,
                    delay_ms = delay.as_millis(),
                    "Scheduling actor restart"
                );

                tokio::time::sleep(delay).await;

                child.restart_count += 1;
                child.restart_timestamps.push(Instant::now());

                // Note: actual restart would require recreating the actor
                // This is a simplified version - full implementation would
                // store actor factory functions
                warn!(
                    actor_id = %actor_id,
                    "Actor restart not fully implemented - requires actor factory"
                );
                child.state = SupervisedState::Stopped;
            }
        }
    }

    /// Get count of running actors.
    pub fn running_count(&self) -> usize {
        self.children
            .values()
            .filter(|c| c.actor_ref.is_running())
            .count()
    }

    /// Get count of degraded actors.
    pub fn degraded_count(&self) -> usize {
        self.children
            .values()
            .filter(|c| c.state == SupervisedState::Degraded)
            .count()
    }
}

// =============================================================================
// Actor Loop
// =============================================================================

/// Run the main actor loop.
async fn run_actor_loop<A: Actor>(
    mut actor: A,
    mut rx: mpsc::Receiver<A::Message>,
    mut shutdown: watch::Receiver<bool>,
    actor_id: String,
) {
    let mut ctx = ActorContext::new(actor_id.clone(), shutdown.clone());

    // Call on_start
    if let Err(e) = actor.on_start(&mut ctx).await {
        error!(actor_id = %actor_id, error = %e, "Actor failed to start");
        return;
    }

    ctx.state = ActorState::Running;
    debug!(actor_id = %actor_id, "Actor started");

    loop {
        tokio::select! {
            biased;

            // Check shutdown first
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!(actor_id = %actor_id, "Actor received shutdown signal");
                    break;
                }
            }

            // Process messages
            msg = rx.recv() => {
                match msg {
                    Some(msg) => {
                        ctx.messages_processed += 1;
                        ctx.last_message_at = Some(Instant::now());

                        match actor.handle(msg, &mut ctx).await {
                            Ok(true) => {
                                // Continue processing
                            }
                            Ok(false) => {
                                info!(actor_id = %actor_id, "Actor requested stop");
                                break;
                            }
                            Err(e) => {
                                error!(actor_id = %actor_id, error = %e, "Actor error");
                                actor.on_crash(&e);
                                // For transient errors, continue; for permanent, stop
                                if matches!(e, ActorError::Permanent(_)) {
                                    ctx.state = ActorState::Failed;
                                    break;
                                }
                            }
                        }
                    }
                    None => {
                        // Channel closed
                        debug!(actor_id = %actor_id, "Actor mailbox closed");
                        break;
                    }
                }
            }
        }
    }

    ctx.state = ActorState::Stopping;
    actor.on_stop(&mut ctx).await;
    ctx.state = ActorState::Stopped;

    info!(
        actor_id = %actor_id,
        messages_processed = ctx.messages_processed,
        "Actor stopped"
    );
}

// =============================================================================
// Helpers
// =============================================================================

static ACTOR_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

fn generate_actor_id() -> u64 {
    ACTOR_ID_COUNTER.fetch_add(1, Ordering::SeqCst)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct TestMessage(String);

    #[test]
    fn test_backoff_policy() {
        let policy = BackoffPolicy::default();

        let d0 = policy.delay(0);
        let d1 = policy.delay(1);
        let d2 = policy.delay(2);

        // Should increase exponentially (with some jitter variance)
        assert!(d0 < Duration::from_millis(200));
        assert!(d1 < Duration::from_millis(400));
        assert!(d2 < Duration::from_millis(800));
    }

    #[test]
    fn test_backoff_max() {
        let policy = BackoffPolicy {
            base: Duration::from_secs(1),
            max: Duration::from_secs(5),
            jitter: 0.0,
        };

        let d10 = policy.delay(10);
        assert!(d10 <= Duration::from_secs(6)); // max + some margin
    }

    #[tokio::test]
    async fn test_actor_handle_send() {
        let (tx, mut rx) = mpsc::channel::<TestMessage>(16);
        let handle = ActorHandle {
            tx,
            actor_id: "test".to_string(),
        };

        handle.send(TestMessage("hello".to_string())).await.unwrap();

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.0, "hello");
    }

    #[test]
    fn test_restart_policy_default() {
        let policy = RestartPolicy::default();
        assert_eq!(policy.max_restarts, 5);
        assert_eq!(policy.window, Duration::from_secs(300));
    }
}
