//! Driving third-party agents over the Agent Client Protocol.
//!
//! The agent runs as a command in a terminal session, so it inherits that shell's state and shows
//! up as a block; its stdio is bridged to the in-process ACP client over a socket (see
//! [`bridge`]). The agent's `session/update` stream is translated into MAA client actions and
//! applied to the terminal's native conversation, so its work renders as an ordinary agent turn.
//!
//! The agent is launched in a local terminal session and bridged over a local socket, so the
//! `fs/*` methods act on this machine's file system, which is the agent's. If the bridge ever
//! spans a remote session they must go through the same session-aware file I/O as the native
//! `ReadFiles` / `RequestFileEdits` executors.
//!
//! For a scripted agent, put a `codex-acp` shim on `PATH` that runs
//! `node app/src/ai/acp/testdata/fake_agent.mjs`.
pub mod attachments;
pub mod bridge;
pub mod connection;
pub mod launch;
pub mod manifest;
pub mod mapping;
pub mod model_config;
pub mod policy;
pub mod protocol;
pub mod terminal;
pub mod turn_driver;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use async_trait::async_trait;
pub(crate) use bridge::run_bridge;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, future, pin_mut, select};
use instant::Instant;
use parking_lot::Mutex;
use serde_json::Value;
use uuid::Uuid;
use warp_cli::agent::Harness;
use warp_core::channel::ChannelState;
use warp_core::safe_warn;
use warp_errors::report_error;
use warp_multi_agent_api::response_event::StreamFinished;
use warp_multi_agent_api::{ClientAction, McpServerIdentity, ResponseEvent, response_event};
use warp_util::path::{EscapeChar, ShellFamily};
use warpui::r#async::Timer;
use warpui::r#async::executor::Background;
use warpui::{ModelContext, ModelDropped, ModelSpawner};

use self::attachments::AttachmentResolver;
use self::bridge::{BridgeListener, bridge_command};
use self::connection::{AcpConnection, RpcError};
use self::launch::AcpLaunchSpec;
use self::mapping::AcpTurnMapper;
use self::model_config::{HarnessModelConfig, configure_model_and_thinking_through_acp};
use self::policy::{PolicyDecision, PolicyRequest};
use self::protocol::{
    CancelParams, ClientCapabilities, ClientInfo, FsCapabilities, InitializeParams,
    InitializeResponse, McpServer, NewSessionParams, NewSessionResponse, PROTOCOL_VERSION,
    PermissionOptionKind, PermissionOutcome, ReadTextFileParams, ReadTextFileResponse,
    RequestPermissionParams, RequestPermissionResponse, WriteTextFileParams,
};
use self::terminal::{AcpTerminal, CommandHandle};
use self::turn_driver::{
    EXIT_CANCEL_DEADLINE, STEER_CANCEL_DEADLINE, STEER_SETTLE_DEADLINE, TurnCancellation,
    TurnDriver, TurnHost, TurnState, cancel_deadline,
};
use crate::ai::agent::CancellationReason;
use crate::ai::agent::api::mcp_server_identity;
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::attachment_utils::DownloadedAttachment;
use crate::ai::blocklist::{
    BlocklistAIController, ExternalHarnessCancel, ExternalHarnessPrompt, ExternalHarnessTurn,
    QueuedQueryId, ResponseStreamId,
};
use crate::ai::mcp::JSONTransportType;

/// Bound on each phase of the handshake: the agent connecting back through the bridge and
/// answering `initialize`, then opening its session, each counted from the phase's start.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);

/// Why an ACP session could not be set up or kept going.
#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    #[error("Failed to set up {harness}: {reason}")]
    SetupFailed { harness: String, reason: String },
    #[error("{harness} is not authenticated: {detail}")]
    AuthCheckFailed { harness: String, detail: String },
    #[error("'{program}' not found on your machine.")]
    HarnessNotInstalled { program: String },
    #[error("The terminal's shell exited")]
    ShellExited,
    #[error("The ACP session's terminal is in an invalid state")]
    InvalidRuntimeState,
}

impl From<ModelDropped> for AcpError {
    fn from(_: ModelDropped) -> Self {
        AcpError::InvalidRuntimeState
    }
}

/// An MCP server to hand to the agent.
pub struct AcpMcpServer {
    pub name: String,
    pub transport: JSONTransportType,
    /// The server's Warp identity, stamped on the agent's tool calls to it.
    pub warp_id: Option<String>,
}

/// What an [`AcpRunner`] runs.
pub struct AcpRunConfig {
    pub harness: Harness,
    pub launch: AcpLaunchSpec,
    pub prompt: String,
    /// Local files sent with the prompt. They are shaped for the agent only once the handshake
    /// has revealed what it accepts inline.
    pub initial_files: Vec<PathBuf>,
    pub working_dir: PathBuf,
    pub mcp_servers: Vec<AcpMcpServer>,
    pub model_config: Option<HarnessModelConfig>,
}

/// The protocol session with the agent running in the terminal.
struct RunningAgent {
    connection: Arc<AcpConnection>,
    session_id: Option<String>,
}

/// Why the handshake produced no session.
enum HandshakeError {
    /// The agent, or the connection to it, failed a handshake request.
    Protocol(anyhow::Error),
    /// The phase named here outran the handshake budget.
    TimedOut(&'static str),
}

impl From<anyhow::Error> for HandshakeError {
    fn from(error: anyhow::Error) -> Self {
        Self::Protocol(error)
    }
}

/// The handshake's deadline, counted from the start of its current phase. Restarting it at a
/// phase boundary gives the next phase the full budget.
#[derive(Clone)]
struct PhaseClock(Arc<Mutex<Phase>>);

struct Phase {
    started: Instant,
    name: &'static str,
}

impl PhaseClock {
    fn start(name: &'static str) -> Self {
        Self(Arc::new(Mutex::new(Phase {
            started: Instant::now(),
            name,
        })))
    }

    fn restart(&self, name: &'static str) {
        *self.0.lock() = Phase {
            started: Instant::now(),
            name,
        };
    }

    /// What the handshake is doing now, for the timeout's error.
    fn phase(&self) -> &'static str {
        self.0.lock().name
    }

    /// Resolves once the current phase has run for `budget` without a restart.
    async fn expire(&self, budget: Duration) {
        loop {
            let remaining = budget.saturating_sub(self.0.lock().started.elapsed());
            if remaining.is_zero() {
                return;
            }
            Timer::after(remaining).await;
        }
    }
}

/// Runs one agent over ACP in an [`AcpTerminal`].
pub struct AcpRunner {
    harness: Harness,
    launch: AcpLaunchSpec,
    prompt: String,
    initial_attachments: Vec<DownloadedAttachment>,
    working_dir: PathBuf,
    model_config: Option<HarnessModelConfig>,
    attachments: Arc<AttachmentResolver>,
    mcp_servers: Vec<McpServer>,
    /// Stamped on the agent's MCP tool calls, keyed by server name.
    mcp_server_identities: HashMap<String, McpServerIdentity>,
    agent: Mutex<Option<RunningAgent>>,
    /// Turn activity reported by the turn driver.
    turn_state: Arc<TurnState>,
}

impl AcpRunner {
    pub fn new(config: AcpRunConfig) -> Self {
        let AcpRunConfig {
            harness,
            launch,
            prompt,
            initial_files,
            working_dir,
            mcp_servers,
            model_config,
        } = config;
        Self {
            harness,
            launch,
            prompt,
            initial_attachments: initial_files.iter().map(|path| local_file(path)).collect(),
            working_dir,
            model_config,
            attachments: Arc::default(),
            // Pi reads its MCP servers from its own configuration rather than `session/new`.
            mcp_servers: mcp_servers
                .iter()
                .filter(|_| harness != Harness::Pi)
                .map(|server| mcp_server_for_acp(&server.name, &server.transport))
                .collect(),
            mcp_server_identities: mcp_servers
                .iter()
                .map(|server| {
                    let identity =
                        tool_call_server_identity(&server.name, server.warp_id.as_deref());
                    (server.name.clone(), identity)
                })
                .collect(),
            agent: Mutex::new(None),
            turn_state: Arc::default(),
        }
    }

    fn setup_failed(&self, reason: String) -> AcpError {
        AcpError::SetupFailed {
            harness: self.harness.to_string(),
            reason,
        }
    }

    /// The error for a handshake that produced no session.
    fn handshake_failed(&self, error: HandshakeError) -> AcpError {
        match error {
            HandshakeError::Protocol(error) => {
                // Code -32000 is the agent refusing the session for lack of credentials, which
                // is the user's configuration rather than a defect worth reporting.
                if let Some(rpc_error) = error
                    .downcast_ref::<RpcError>()
                    .filter(|error| error.code == -32000)
                {
                    return AcpError::AuthCheckFailed {
                        harness: self.harness.to_string(),
                        detail: rpc_error.message.clone(),
                    };
                }
                let reason = format!("{error:#}");
                report_error!(
                    error.context("ACP handshake failed"),
                    extra: { "harness" => %self.harness }
                );
                self.setup_failed(reason)
            }
            HandshakeError::TimedOut(phase) => self.setup_failed(format!(
                "the agent did not finish {phase} within {HANDSHAKE_TIMEOUT:?}"
            )),
        }
    }

    fn running_agent(&self) -> Option<(Arc<AcpConnection>, Option<String>)> {
        self.agent
            .lock()
            .as_ref()
            .map(|agent| (agent.connection.clone(), agent.session_id.clone()))
    }

    /// Launches the agent in the terminal, completes the handshake, and starts its first turn on
    /// the terminal's native conversation. The returned handle is the agent's block; the session
    /// stays serviceable until [`Self::exit`] or the agent leaves.
    pub async fn start(
        &self,
        foreground: &ModelSpawner<AcpTerminal>,
    ) -> Result<CommandHandle, AcpError> {
        let (background, shell_family): (Arc<Background>, ShellFamily) = foreground
            .spawn(|terminal, ctx| {
                let shell_family = terminal
                    .active_session_shell_type(ctx)
                    .map(ShellFamily::from)
                    .unwrap_or(ShellFamily::Posix);
                (ctx.background_executor(), shell_family)
            })
            .await?;

        let listener = BridgeListener::bind(&self.launch.program, &self.launch.args)
            .map_err(|error| self.setup_failed(format!("{error:#}")))?;
        let command = bridge_command(listener.launch_file(), shell_family)
            .map_err(|error| self.setup_failed(format!("{error:#}")))?;
        let command_handle = foreground
            .spawn(move |terminal, ctx| terminal.execute_command(&command, ctx))
            .await??
            .await?;

        let clock = PhaseClock::start("connecting and initializing");
        let (reader, writer) = match listener.accept(HANDSHAKE_TIMEOUT).await {
            Ok(halves) => halves,
            Err(error) => {
                interrupt_terminal(foreground).await;
                return Err(self.setup_failed(format!("{error:#}")));
            }
        };
        drop(listener);
        let request_context = Arc::new(AgentRequestContext {
            foreground: foreground.clone(),
            escape_char: shell_family.escape_char(),
        });
        let (connection, notifications) = AcpConnection::new(
            reader,
            writer,
            agent_request_handler(request_context),
            &background,
        );
        let connection = Arc::new(connection);
        *self.agent.lock() = Some(RunningAgent {
            connection: connection.clone(),
            session_id: None,
        });

        let handshake_connection = connection.clone();
        let handshake_clock = clock.clone();
        let handshake = async move {
            let connection = handshake_connection;
            let clock = handshake_clock;
            let initialize: InitializeResponse = connection
                .request(
                    protocol::METHOD_INITIALIZE,
                    InitializeParams {
                        protocol_version: PROTOCOL_VERSION,
                        client_capabilities: ClientCapabilities {
                            fs: FsCapabilities {
                                read_text_file: true,
                                write_text_file: true,
                            },
                            terminal: false,
                        },
                        client_info: ClientInfo {
                            name: "warp".to_owned(),
                            title: "Warp".to_owned(),
                            version: ChannelState::app_version()
                                .unwrap_or(env!("CARGO_PKG_VERSION"))
                                .to_owned(),
                        },
                    },
                )
                .await
                .context("initialize failed")?;
            let agent_description = initialize
                .agent_info
                .as_ref()
                .map(|info| format!("{} {}", info.name, info.version))
                .unwrap_or_else(|| "unknown".to_owned());
            // The agent answers with the highest version it supports that is no newer than
            // ours; anything else means the two sides would not agree on the wire format.
            if initialize.protocol_version != PROTOCOL_VERSION {
                return Err(anyhow!(
                    "agent {agent_description} speaks ACP protocol v{} but this client requires v{PROTOCOL_VERSION}",
                    initialize.protocol_version
                )
                .into());
            }
            let capabilities = &initialize.agent_capabilities;
            let extra_capabilities = capabilities.extra.keys().cloned().collect::<Vec<_>>();
            let steering_supported = initialize.supports_steering();
            log::info!(
                "ACP agent initialized: protocol v{} agent={agent_description} auth_methods={} \
                 load_session={} prompt_capabilities={:?} steering={steering_supported} \
                 other_capabilities={extra_capabilities:?}",
                initialize.protocol_version,
                initialize.auth_methods.len(),
                capabilities.load_session,
                capabilities.prompt_capabilities,
            );
            self.attachments
                .set_prompt_capabilities(capabilities.prompt_capabilities);
            clock.restart("opening its session");
            let mut session: NewSessionResponse = connection
                .request(
                    protocol::METHOD_SESSION_NEW,
                    NewSessionParams {
                        cwd: self.working_dir.display().to_string(),
                        mcp_servers: self.mcp_servers.clone(),
                    },
                )
                .await
                .context("session/new failed")?;
            configure_model_and_thinking_through_acp(
                &connection,
                &mut session,
                self.model_config.as_ref(),
            )
            .await?;
            let suppressed_startup_message = match self.harness {
                Harness::Pi => pi_startup_message(&session.meta),
                Harness::Claude
                | Harness::Codex
                | Harness::Gemini
                | Harness::Local
                | Harness::OpenCode
                | Harness::Unknown => None,
            };
            Ok::<_, HandshakeError>((
                session.session_id,
                suppressed_startup_message,
                steering_supported,
            ))
        }
        .fuse();
        pin_mut!(handshake);
        let deadline = clock.expire(HANDSHAKE_TIMEOUT).fuse();
        pin_mut!(deadline);
        let session = select! {
            result = handshake => result,
            _ = deadline => Err(HandshakeError::TimedOut(clock.phase())),
        };
        let (session_id, suppressed_startup_message, steering_supported) = match session {
            Ok(session) => session,
            Err(error) => {
                connection.close().await;
                interrupt_terminal(foreground).await;
                return Err(self.handshake_failed(error));
            }
        };
        log::info!("ACP session {session_id} created");
        if let Some(agent) = self.agent.lock().as_mut() {
            agent.session_id = Some(session_id.clone());
        }

        let conversation_id = with_ai_controller(foreground, |controller, ctx| {
            controller
                .native_prompt_conversation_id()
                .unwrap_or_else(|| controller.bind_native_prompt_conversation(None, ctx))
        })
        .await;
        let conversation_id = match conversation_id {
            Ok(conversation_id) => conversation_id,
            Err(error) => {
                connection.close().await;
                interrupt_terminal(foreground).await;
                return Err(error);
            }
        };

        let host = Arc::new(TerminalTurnHost {
            foreground: foreground.clone(),
            conversation_id,
        });
        let turn = match host.begin_turn().await {
            Ok(turn) => turn,
            Err(error) => {
                connection.close().await;
                interrupt_terminal(foreground).await;
                return Err(self.setup_failed(format!("{error:#}")));
            }
        };
        let (follow_up_tx, follow_up_rx) = async_channel::unbounded::<ExternalHarnessPrompt>();
        let (cancel_tx, cancel_rx) = async_channel::unbounded::<ExternalHarnessCancel>();
        with_ai_controller(foreground, move |controller, _| {
            controller.set_external_harness_prompt_sink(follow_up_tx);
            controller.set_external_harness_cancel_sink(cancel_tx);
        })
        .await?;

        let mut mapper = AcpTurnMapper::new(
            Uuid::new_v4().to_string(),
            turn.request_id.clone(),
            self.mcp_server_identities.clone(),
        );
        let opening_actions = mapper.initial_actions(&self.prompt);
        apply_actions(foreground, &turn.stream_id, opening_actions).await?;

        let turn_driver = TurnDriver {
            connection,
            notifications,
            follow_ups: Some(follow_up_rx),
            cancels: Some(cancel_rx),
            host,
            state: self.turn_state.clone(),
            turn,
            session_id,
            suppressed_startup_message,
            prompt_text: self.prompt.clone(),
            initial_attachments: self.initial_attachments.clone(),
            attachments: self.attachments.clone(),
            queued: VecDeque::new(),
            cancel_deadline: STEER_CANCEL_DEADLINE,
            steer_settle_deadline: STEER_SETTLE_DEADLINE,
            timer_factory: Arc::new(cancel_deadline),
            steering_supported,
            steering: None,
            awaiting_steer: VecDeque::new(),
            late_steers: Mutex::new(FuturesUnordered::new()),
        };
        background
            .spawn(async move { turn_driver.run(&mut mapper).await })
            .detach();

        Ok(command_handle)
    }

    /// Whether a turn is winding down or queued follow-ups are about to open a new one.
    pub fn is_settling_turn(&self) -> bool {
        self.turn_state.is_settling()
    }

    /// Cancels any turn in flight and waits up to [`EXIT_CANCEL_DEADLINE`] for the agent to
    /// answer it before closing the connection, so an agent that persists its session at the end
    /// of a turn gets to.
    pub async fn exit(&self) -> Result<()> {
        let Some((connection, session_id)) = self.running_agent() else {
            return Ok(());
        };
        if let Some(session_id) = session_id {
            connection
                .notify(
                    protocol::METHOD_SESSION_CANCEL,
                    CancelParams {
                        session_id: session_id.clone(),
                    },
                )
                .await?;
            connection
                .cancel_pending_permission_requests(&session_id)
                .await;
            let idle = self.turn_state.wait_until_idle().fuse();
            pin_mut!(idle);
            select! {
                _ = idle => {}
                _ = Timer::after(EXIT_CANCEL_DEADLINE).fuse() => log::warn!(
                    "ACP agent did not answer session/cancel within {EXIT_CANCEL_DEADLINE:?} \
                     on exit; closing anyway"
                ),
            }
        }
        connection.close().await;
        self.agent.lock().take();
        Ok(())
    }

    /// Interrupts the agent's block, for an agent still running once [`Self::exit`] has closed
    /// its connection.
    pub async fn interrupt(&self, foreground: &ModelSpawner<AcpTerminal>) {
        interrupt_terminal(foreground).await;
    }
}

/// A file already on disk, in the shape attachments are resolved from.
fn local_file(path: &Path) -> DownloadedAttachment {
    let file_path = path.to_string_lossy().into_owned();
    DownloadedAttachment {
        file_name: path.file_name().map_or_else(
            || file_path.clone(),
            |name| name.to_string_lossy().into_owned(),
        ),
        file_path,
    }
}

/// Text pi-acp announces in `session/new`'s metadata and then also streams as an agent message.
fn pi_startup_message(session_meta: &Value) -> Option<String> {
    session_meta
        .pointer("/piAcp/startupInfo")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Interrupts whatever the terminal session is running, which is the agent's block while the
/// agent is up.
async fn interrupt_terminal(foreground: &ModelSpawner<AcpTerminal>) {
    let _ = foreground
        .spawn(|terminal, ctx| terminal.send_interrupt_to_pty(ctx))
        .await;
}

/// Runs `f` against the terminal's AI controller on the foreground thread, which owns every
/// entity involved. The terminal view handle is cloned only for the synchronous callback (the
/// borrow of `ctx` through the model must end before the view can be updated) and is released
/// when it returns.
async fn with_ai_controller<T: Send + 'static>(
    foreground: &ModelSpawner<AcpTerminal>,
    f: impl FnOnce(&mut BlocklistAIController, &mut ModelContext<BlocklistAIController>) -> T
    + Send
    + 'static,
) -> Result<T, AcpError> {
    let result = foreground
        .spawn(move |terminal, ctx| {
            let terminal = terminal.terminal_view().clone();
            terminal.update(ctx, |terminal, ctx| terminal.ai_controller().update(ctx, f))
        })
        .await?;
    Ok(result)
}

/// The turn driver's view of the terminal: the bound native conversation, reached on the
/// foreground thread, and the session the agent runs in.
struct TerminalTurnHost {
    foreground: ModelSpawner<AcpTerminal>,
    conversation_id: AIConversationId,
}

#[async_trait]
impl TurnHost for TerminalTurnHost {
    async fn begin_turn(&self) -> Result<ExternalHarnessTurn> {
        with_ai_controller(&self.foreground, move |controller, ctx| {
            let conversation_id = controller
                .native_prompt_conversation_id()
                .unwrap_or_else(|| controller.bind_native_prompt_conversation(None, ctx));
            controller.begin_external_harness_turn(conversation_id, None, ctx)
        })
        .await
        .map_err(|error| anyhow!("{error}"))?
        .map_err(|error| anyhow!("could not open conversation turn: {error}"))
    }

    async fn apply_actions(
        &self,
        stream_id: &ResponseStreamId,
        actions: Vec<ClientAction>,
    ) -> Result<()> {
        apply_actions(&self.foreground, stream_id, actions)
            .await
            .map_err(|error| anyhow!("{error}"))
    }

    async fn finish_turn(
        &self,
        stream_id: ResponseStreamId,
        finished: StreamFinished,
    ) -> Result<()> {
        apply_event(
            &self.foreground,
            stream_id,
            response_event::Type::Finished(finished),
        )
        .await
        .map_err(|error| anyhow!("{error}"))
    }

    async fn cancel_turn(&self, stream_id: ResponseStreamId, by: TurnCancellation) {
        let reason = match by {
            TurnCancellation::FollowUp => CancellationReason::FollowUpSubmitted {
                is_for_same_conversation: true,
            },
            TurnCancellation::Interrupt => CancellationReason::ManuallyCancelled,
        };
        let cancelled = with_ai_controller(&self.foreground, move |controller, ctx| {
            controller.cancel_external_harness_turn(&stream_id, reason, ctx);
        })
        .await;
        if cancelled.is_err() {
            log::warn!("Terminal dropped before the cancelled ACP turn could be finished");
        }
    }

    async fn mark_conversation_cancelled(&self) {
        let conversation_id = self.conversation_id;
        let marked = with_ai_controller(&self.foreground, move |controller, ctx| {
            controller.mark_external_harness_conversation_cancelled(conversation_id, ctx);
        })
        .await;
        if marked.is_err() {
            log::warn!("Terminal dropped before the ACP conversation could be marked cancelled");
        }
    }

    async fn interrupt_terminal(&self) {
        interrupt_terminal(&self.foreground).await;
    }

    async fn show_queued_prompt(&self, prompt: &ExternalHarnessPrompt) -> Option<QueuedQueryId> {
        let conversation_id = self.conversation_id;
        let prompt = prompt.clone();
        with_ai_controller(&self.foreground, move |controller, ctx| {
            controller.show_external_harness_prompt_queued(conversation_id, &prompt, ctx)
        })
        .await
        .ok()
    }

    async fn hide_queued_prompt(&self, query_id: QueuedQueryId) {
        let conversation_id = self.conversation_id;
        let _ = with_ai_controller(&self.foreground, move |controller, ctx| {
            controller.hide_external_harness_prompt_queued(conversation_id, query_id, ctx);
        })
        .await;
    }

    async fn turn_driver_stopped(&self) {
        let _ = with_ai_controller(&self.foreground, |controller, _| {
            controller.release_external_harness_sinks();
        })
        .await;
    }
}

async fn apply_actions(
    foreground: &ModelSpawner<AcpTerminal>,
    stream_id: &ResponseStreamId,
    actions: Vec<ClientAction>,
) -> Result<(), AcpError> {
    if actions.is_empty() {
        return Ok(());
    }
    apply_event(
        foreground,
        stream_id.clone(),
        response_event::Type::ClientActions(response_event::ClientActions { actions }),
    )
    .await
}

async fn apply_event(
    foreground: &ModelSpawner<AcpTerminal>,
    stream_id: ResponseStreamId,
    event: response_event::Type,
) -> Result<(), AcpError> {
    with_ai_controller(foreground, move |controller, ctx| {
        controller.apply_external_harness_event(
            &stream_id,
            ResponseEvent {
                r#type: Some(event),
            },
            ctx,
        );
    })
    .await
}

/// What answering the agent's requests needs: a way onto the foreground thread to consult the
/// permission model, and the shell whose quoting rules the policy parses commands with.
struct AgentRequestContext {
    foreground: ModelSpawner<AcpTerminal>,
    escape_char: EscapeChar,
}

impl AgentRequestContext {
    async fn evaluate(&self, request: PolicyRequest) -> Result<PolicyDecision, RpcError> {
        let escape_char = self.escape_char;
        self.foreground
            .spawn(move |terminal, ctx| {
                let terminal = terminal.terminal_view().clone();
                let controller = terminal.as_ref(ctx).ai_controller().clone();
                request.evaluate(controller.as_ref(ctx), terminal.id(), escape_char, ctx)
            })
            .await
            .map_err(|_| RpcError::internal("the agent's terminal is gone"))
    }
}

/// Answers the requests an ACP agent may make of its client. Runs on the connection's read
/// loop (a background runtime thread), so file system work is moved to blocking threads to keep
/// protocol reads flowing.
fn agent_request_handler(context: Arc<AgentRequestContext>) -> connection::AgentRequestHandler {
    Arc::new(move |method, params| {
        let context = context.clone();
        match method {
            protocol::METHOD_REQUEST_PERMISSION => {
                async move { answer_permission_request(&context, params).await }.boxed()
            }
            protocol::METHOD_FS_READ_TEXT_FILE => blocking(move || read_text_file(params)).boxed(),
            protocol::METHOD_FS_WRITE_TEXT_FILE => {
                async move { write_text_file(&context, params).await }.boxed()
            }
            other => future::ready(Err(RpcError::method_not_found(other))).boxed(),
        }
    })
}

async fn blocking(
    work: impl FnOnce() -> Result<Value, RpcError> + Send + 'static,
) -> Result<Value, RpcError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(RpcError::internal)?
}

/// Grants the request with the broadest "allow" option unless Warp's permission policy refuses
/// it outright (see [`policy`]), in which case the narrowest "reject" option is chosen.
async fn answer_permission_request(
    context: &AgentRequestContext,
    params: Value,
) -> Result<Value, RpcError> {
    let params: RequestPermissionParams =
        serde_json::from_value(params).map_err(RpcError::internal)?;
    let decision = match PolicyRequest::for_tool_call(&params.tool_call) {
        Some(request) => context.evaluate(request).await?,
        None => PolicyDecision::Allow,
    };
    let preferred_kinds: &[PermissionOptionKind] = match &decision {
        PolicyDecision::Allow => &[
            PermissionOptionKind::AllowAlways,
            PermissionOptionKind::AllowOnce,
        ],
        PolicyDecision::Deny { .. } => &[
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ],
    };
    let chosen = preferred_kinds
        .iter()
        .find_map(|kind| params.options.iter().find(|option| option.kind == *kind));
    let outcome = match (&decision, chosen) {
        (PolicyDecision::Allow, Some(option)) => {
            log::info!(
                "Auto-approving ACP permission request with `{}`",
                option.name
            );
            PermissionOutcome::Selected {
                option_id: option.option_id.clone(),
            }
        }
        // Agents that offer no allow option are answered with whatever they did offer;
        // anything else would wedge the turn.
        (PolicyDecision::Allow, None) => match params.options.first() {
            Some(option) => PermissionOutcome::Selected {
                option_id: option.option_id.clone(),
            },
            None => PermissionOutcome::Cancelled,
        },
        (PolicyDecision::Deny { reason }, chosen) => {
            // The title is agent-authored and can quote commands or paths; keep it out of the
            // reported breadcrumb.
            safe_warn!(
                safe: ("Refusing ACP permission request: {reason}"),
                full: (
                    "Refusing ACP permission request for `{}`: {reason}",
                    params.tool_call.title.as_deref().unwrap_or("tool call")
                )
            );
            match chosen {
                Some(option) => PermissionOutcome::Selected {
                    option_id: option.option_id.clone(),
                },
                None => PermissionOutcome::Cancelled,
            }
        }
    };
    serde_json::to_value(RequestPermissionResponse { outcome }).map_err(RpcError::internal)
}

fn read_text_file(params: Value) -> Result<Value, RpcError> {
    let params: ReadTextFileParams = serde_json::from_value(params).map_err(RpcError::internal)?;
    let content = std::fs::read_to_string(&params.path)
        .with_context(|| format!("reading {}", params.path))
        .map_err(RpcError::internal)?;
    let content = match (params.line, params.limit) {
        (None, None) => content,
        (line, limit) => {
            let start = line.unwrap_or(1).saturating_sub(1) as usize;
            let lines = content.lines().skip(start);
            match limit {
                Some(limit) => lines.take(limit as usize).collect::<Vec<_>>().join("\n"),
                None => lines.collect::<Vec<_>>().join("\n"),
            }
        }
    };
    serde_json::to_value(ReadTextFileResponse { content }).map_err(RpcError::internal)
}

async fn write_text_file(context: &AgentRequestContext, params: Value) -> Result<Value, RpcError> {
    let params: WriteTextFileParams = serde_json::from_value(params).map_err(RpcError::internal)?;
    if let PolicyDecision::Deny { reason } =
        context.evaluate(PolicyRequest::write(&params.path)).await?
    {
        log::warn!("Refusing ACP fs/write_text_file: {reason}");
        return Err(RpcError::internal(format!(
            "writing {} is not permitted: {reason}",
            params.path
        )));
    }
    blocking(move || {
        if let Some(parent) = Path::new(&params.path).parent() {
            std::fs::create_dir_all(parent).map_err(RpcError::internal)?;
        }
        std::fs::write(&params.path, params.content)
            .with_context(|| format!("writing {}", params.path))
            .map_err(RpcError::internal)?;
        Ok(Value::Null)
    })
    .await
}

/// The identity of an MCP server as its tool calls carry it.
fn tool_call_server_identity(name: &str, warp_id: Option<&str>) -> McpServerIdentity {
    McpServerIdentity {
        display_name: name.to_owned(),
        ..warp_id.and_then(mcp_server_identity).unwrap_or_default()
    }
}

fn mcp_server_for_acp(name: &str, transport: &JSONTransportType) -> McpServer {
    match transport {
        JSONTransportType::CLIServer {
            command, args, env, ..
        } => McpServer::Stdio {
            name: name.to_owned(),
            command: command.clone(),
            args: args.clone(),
            env: env
                .iter()
                .map(|(name, value)| protocol::EnvVariable {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
        },
        JSONTransportType::SSEServer { url, headers } => McpServer::Remote {
            kind: "http",
            name: name.to_owned(),
            url: url.clone(),
            headers: headers
                .iter()
                .map(|(name, value)| protocol::HttpHeader {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
        },
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
