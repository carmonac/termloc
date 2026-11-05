//! The terminal session an ACP agent runs in.
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::channel::oneshot;
use warp_core::command::ExitCode;
use warpui::{AppContext, Entity, ModelContext, ModelHandle, ViewHandle};

use super::AcpError;
use crate::terminal::TerminalView;
use crate::terminal::model::block::BlockId;
use crate::terminal::shell::ShellType;
use crate::terminal::view::Event;

struct WaitingCommand {
    exit_tx: oneshot::Sender<Result<ExitCode, AcpError>>,
    started_block_id: Option<BlockId>,
}

/// Runs commands in an existing terminal view and reports when their blocks complete.
pub struct AcpTerminal {
    terminal_view: ViewHandle<TerminalView>,
    /// Resolves the in-flight command's exit status.
    waiting_command: Option<WaitingCommand>,
    /// The command text expected to start executing, and where to send its block ID.
    pending_command_start: Option<(String, oneshot::Sender<Result<BlockId, AcpError>>)>,
    /// True once the shell process backing this session has exited, after which no command can
    /// run.
    shell_exited: bool,
}

impl Entity for AcpTerminal {
    type Event = ();
}

impl AcpTerminal {
    /// Wraps an already-created terminal view in a new model.
    pub fn create_from_existing_view(
        terminal_view: ViewHandle<TerminalView>,
        ctx: &mut AppContext,
    ) -> ModelHandle<Self> {
        ctx.add_model(|ctx| Self::new(terminal_view, ctx))
    }

    fn new(terminal_view: ViewHandle<TerminalView>, ctx: &mut ModelContext<Self>) -> Self {
        ctx.subscribe_to_view(&terminal_view, |me, _, event, ctx| {
            me.handle_terminal_view_event(event, ctx);
        });
        Self {
            terminal_view,
            waiting_command: None,
            pending_command_start: None,
            shell_exited: false,
        }
    }

    pub fn terminal_view(&self) -> &ViewHandle<TerminalView> {
        &self.terminal_view
    }

    /// Sends a raw Ctrl-C (`\x03`) to the PTY, interrupting the foreground process group.
    pub fn send_interrupt_to_pty(&self, ctx: &mut ModelContext<Self>) {
        self.terminal_view.update(ctx, |terminal, ctx| {
            terminal.write_to_pty(b"\x03".to_vec(), ctx);
        });
    }

    /// Executes a command in the terminal and returns a future that resolves to a
    /// [`CommandHandle`] once the command starts executing.
    pub fn execute_command(
        &mut self,
        command: &str,
        ctx: &mut ModelContext<Self>,
    ) -> Result<impl Future<Output = Result<CommandHandle, AcpError>> + use<>, AcpError> {
        if self.shell_exited {
            return Err(AcpError::ShellExited);
        }
        if self.waiting_command.is_some() || self.pending_command_start.is_some() {
            return Err(AcpError::InvalidRuntimeState);
        }

        let (exit_tx, exit_rx) = oneshot::channel();
        let (start_tx, start_rx) = oneshot::channel();
        self.waiting_command = Some(WaitingCommand {
            exit_tx,
            started_block_id: None,
        });
        self.pending_command_start = Some((command.to_owned(), start_tx));
        self.terminal_view.update(ctx, |terminal, ctx| {
            terminal.execute_command_or_set_pending(command, ctx);
        });

        Ok(async move {
            let block_id = start_rx
                .await
                .map_err(|_| AcpError::InvalidRuntimeState)??;
            Ok(CommandHandle {
                exit_status_rx: exit_rx,
                block_id,
            })
        })
    }

    /// Returns the shell type of the active terminal session, if known.
    pub fn active_session_shell_type(&self, ctx: &AppContext) -> Option<ShellType> {
        self.terminal_view
            .read(ctx, |terminal, app| terminal.active_session_shell_type(app))
    }

    fn handle_terminal_view_event(&mut self, event: &Event, ctx: &mut ModelContext<Self>) {
        match event {
            Event::Exited => {
                self.shell_exited = true;
                if let Some((_, sender)) = self.pending_command_start.take() {
                    let _ = sender.send(Err(AcpError::ShellExited));
                }
                if let Some(command) = self.waiting_command.take() {
                    let _ = command.exit_tx.send(Err(AcpError::ShellExited));
                }
            }
            Event::ExecuteCommand(event) => {
                if let Some((_, sender)) = self
                    .pending_command_start
                    .take_if(|(command, _)| *command == event.command)
                {
                    let block_id = self.terminal_view.read(ctx, |terminal, _| {
                        terminal.model.lock().block_list().active_block_id().clone()
                    });
                    if let Some(command) = self.waiting_command.as_mut() {
                        command.started_block_id = Some(block_id.clone());
                    }
                    let _ = sender.send(Ok(block_id));
                }
            }
            Event::BlockCompleted { block, .. } => {
                // Missing preexec hooks leave did_execute false even for commands that ran.
                if let Some(command) = self
                    .waiting_command
                    .take_if(|command| command.started_block_id.as_ref() == Some(&block.id))
                {
                    let _ = command.exit_tx.send(Ok(block.exit_code));
                }
            }
            _ => (),
        }
    }
}

/// A running terminal command, which resolves to its [`ExitCode`] when its block completes.
pub struct CommandHandle {
    exit_status_rx: oneshot::Receiver<Result<ExitCode, AcpError>>,
    block_id: BlockId,
}

impl CommandHandle {
    pub fn block_id(&self) -> &BlockId {
        &self.block_id
    }
}

impl Future for CommandHandle {
    type Output = Result<ExitCode, AcpError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.exit_status_rx)
            .poll(cx)
            .map(|result| result.unwrap_or(Err(AcpError::InvalidRuntimeState)))
    }
}
