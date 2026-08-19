//! Authority-neutral Tiber terminal presentation.
//!
//! This crate is fork-derived from the interaction and layout conventions of
//! `OpenAI` Codex TUI at commit `d06dc73290729d2bcb464b955a4cfd9992abc35d`.
//! Tiber changed the presentation to accept typed projections and expose no
//! Codex runtime, configuration, tool, sandbox, workflow, or session authority.

#![forbid(unsafe_code)]
#![expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    clippy::implicit_return,
    clippy::missing_inline_in_public_items,
    clippy::single_call_fn,
    reason = "the closed projection vocabulary and small immediate-mode presentation keep event order explicit and follow Ratatui's frame/widget API"
)]

extern crate alloc;

use alloc::{string::String, vec::Vec};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph, Wrap},
};

/// Maximum retained transcript rows in the presentation projection.
const MAX_TRANSCRIPT_ENTRIES: usize = 256;
/// Maximum UTF-8 bytes retained for one assistant response.
const MAX_ASSISTANT_BYTES: usize = 256 * 1024;
/// Maximum retained bytes for any non-streaming presentation field.
const MAX_FIELD_BYTES: usize = 16 * 1024;
/// Maximum editable composer bytes.
const MAX_COMPOSER_BYTES: usize = 16 * 1024;

/// A presentation-only observation supplied by the application shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectionEvent {
    /// The owner submitted one prompt.
    PromptSubmitted { text: String },
    /// Streaming assistant text arrived.
    AssistantDelta { text: String },
    /// A tool proposal was rejected by the inference adapter and retained as data.
    InertToolRequested {
        arguments: serde_json::Value,
        call_id: String,
        tool: String,
    },
    /// A durably recorded, exact repository change awaits owner approval.
    RepositoryChangeProposed { diff: String, path: String },
    /// A durably prepared repository change was applied by the authorized adapter.
    RepositoryChangeApplied { path: String },
    /// The owner durably denied the exact repository proposal.
    RepositoryChangeDenied { path: String },
    /// The owner durably cancelled the exact repository proposal.
    RepositoryChangeCancelled { path: String },
    /// A repository mutation ended with one typed adapter failure.
    RepositoryChangeFailed {
        /// Stable repository adapter failure code.
        code: String,
        /// Root-relative repository path named by the receipt.
        path: String,
        /// Whether a safe retry may make progress.
        retryable: bool,
    },
    /// A read-only restart reconciliation reached one durable outcome.
    RepositoryChangeReconciled {
        /// Stable content-free reconciliation outcome.
        outcome: String,
        /// Root-relative repository path named by the receipt.
        path: String,
    },
    /// A repository mutation has an ambiguous durable outcome.
    RepositoryChangeUnknown {
        /// Root-relative repository path named by the receipt.
        path: String,
    },
    /// A process restart reconciliation restored a durable terminal receipt.
    ProcessReconciled { outcome: String },
    /// A process restart reconciliation remains uncertain and requires owner action.
    ProcessUnknown { next_action: String },
    /// One policy-admitted configured command is active under the application shell.
    ConfiguredCommandActive { command: String },
    /// One configured command reached a sanitized terminal status.
    ConfiguredCommandFinished { command: String, status: String },
    /// The active turn completed.
    TurnCompleted,
    /// The active turn ended with a typed failure.
    TurnFailed {
        code: String,
        message: String,
        retryable: bool,
    },
    /// A durable inference request has no terminal observation and requires reconciliation.
    ReconciliationRequired { code: String, message: String },
}

/// Current presentation phase; it carries no workflow authority.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PresentationPhase {
    /// The composer accepts owner input.
    #[default]
    Ready,
    /// One inference turn is producing observations.
    Streaming,
    /// One configured command is active and may be explicitly cancelled by the owner.
    ConfiguredCommandActive,
    /// One durable repository proposal awaits an explicit owner decision.
    Approval,
    /// The owner decided, but the inference turn has not durably completed.
    DecisionComplete,
    /// Durable workflow authority must be reconciled before another prompt.
    Reconcile,
}

/// One transcript row rendered by the presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
enum TranscriptEntry {
    /// Owner-authored transcript content.
    User(String),
    /// Model-authored transcript content.
    Assistant(String),
    /// A rejected model tool proposal retained for inspection.
    ToolProposal {
        /// Untrusted model arguments.
        arguments: String,
        /// App-server call identity.
        call_id: String,
        /// Declared tool name.
        tool: String,
    },
    /// One exact durable repository proposal awaiting owner approval.
    RepositoryProposal {
        /// Exact bounded diff displayed for owner review.
        diff: String,
        /// Root-relative repository path named by the proposal.
        path: String,
    },
    /// A typed inference failure retained with partial output.
    Failure {
        /// Stable failure classification.
        code: String,
        /// Sanitized owner-facing detail.
        message: String,
        /// Whether a fresh attempt may succeed.
        retryable: bool,
    },
}

/// Projection state rendered by the Tiber terminal UI.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConversationProjection {
    /// Current editable input.
    composer: String,
    /// Ordered visible transcript rows.
    entries: Vec<TranscriptEntry>,
    /// Current presentation-only phase.
    phase: PresentationPhase,
    /// Whether the active inference turn has emitted its completion observation.
    turn_completed: bool,
    /// Sanitized semantic identity of the active configured command, if any.
    active_command: Option<String>,
}

/// Owner intent emitted by the presentation for the application shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComposerIntent {
    /// No application action is requested.
    None,
    /// Submit one non-empty prompt.
    Submit(String),
    /// Approve the exact durable repository proposal currently displayed.
    Approve,
    /// Deny the exact durable repository proposal currently displayed.
    Deny,
    /// Cancel the exact durable repository proposal currently displayed.
    Cancel,
    /// Cancel the exact configured command currently displayed as active.
    CancelConfiguredCommand,
    /// Exit the terminal application.
    Quit,
}

impl ConversationProjection {
    /// Creates an empty, ready projection.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one application-owned observation without executing any effect.
    #[expect(
        clippy::too_many_lines,
        reason = "the closed presentation event fold keeps every authority-neutral state transition together and explicit"
    )]
    pub fn apply(&mut self, event: ProjectionEvent) {
        match event {
            ProjectionEvent::PromptSubmitted { text } => {
                self.push_entry(TranscriptEntry::User(sanitize_terminal_text(&text)));
                self.push_entry(TranscriptEntry::Assistant(String::new()));
                self.phase = PresentationPhase::Streaming;
                self.turn_completed = false;
                self.composer.clear();
            }
            ProjectionEvent::AssistantDelta { text } => {
                if let Some(&mut TranscriptEntry::Assistant(ref mut assistant)) = self
                    .entries
                    .iter_mut()
                    .rev()
                    .find(|entry| matches!(entry, TranscriptEntry::Assistant(_)))
                {
                    bounded_append(assistant, &sanitize_terminal_text(&text));
                }
            }
            ProjectionEvent::InertToolRequested {
                arguments,
                call_id,
                tool,
            } => self.push_entry(TranscriptEntry::ToolProposal {
                arguments: sanitize_terminal_text(&arguments.to_string()),
                call_id: sanitize_terminal_text(&call_id),
                tool: sanitize_terminal_text(&tool),
            }),
            ProjectionEvent::RepositoryChangeProposed { diff, path } => {
                self.push_entry(TranscriptEntry::RepositoryProposal {
                    diff: sanitize_repository_diff(&diff),
                    path: sanitize_terminal_text(&path),
                });
                self.phase = PresentationPhase::Approval;
                self.composer.clear();
            }
            ProjectionEvent::RepositoryChangeApplied { path } => {
                self.push_entry(TranscriptEntry::Assistant(format!(
                    "repository change applied: {path}"
                )));
                self.phase = if self.turn_completed {
                    PresentationPhase::Ready
                } else {
                    PresentationPhase::DecisionComplete
                };
                self.composer.clear();
            }
            ProjectionEvent::RepositoryChangeDenied { path } => {
                self.push_entry(TranscriptEntry::Assistant(format!(
                    "repository change denied: {path}"
                )));
                self.phase = if self.turn_completed {
                    PresentationPhase::Ready
                } else {
                    PresentationPhase::DecisionComplete
                };
                self.composer.clear();
            }
            ProjectionEvent::RepositoryChangeCancelled { path } => {
                self.push_entry(TranscriptEntry::Assistant(format!(
                    "repository change cancelled: {path}"
                )));
                self.phase = if self.turn_completed {
                    PresentationPhase::Ready
                } else {
                    PresentationPhase::DecisionComplete
                };
                self.composer.clear();
            }
            ProjectionEvent::RepositoryChangeFailed {
                code,
                path,
                retryable,
            } => {
                self.push_entry(TranscriptEntry::Failure {
                    code: sanitize_terminal_text(&code),
                    message: format!(
                        "repository change failed: {}",
                        sanitize_terminal_text(&path)
                    ),
                    retryable,
                });
                self.phase = PresentationPhase::Ready;
            }
            ProjectionEvent::RepositoryChangeReconciled { outcome, path } => {
                self.push_entry(TranscriptEntry::Assistant(format!(
                    "repository change reconciled: {} {}",
                    sanitize_terminal_text(&path),
                    sanitize_terminal_text(&outcome)
                )));
                self.phase = PresentationPhase::Ready;
            }
            ProjectionEvent::RepositoryChangeUnknown { path } => {
                self.push_entry(TranscriptEntry::Failure {
                    code: "repository_mutation_outcome_unknown".to_owned(),
                    message: format!(
                        "repository change unknown: {}",
                        sanitize_terminal_text(&path)
                    ),
                    retryable: true,
                });
                self.phase = PresentationPhase::Reconcile;
            }
            ProjectionEvent::TurnCompleted => {
                self.turn_completed = true;
                if self.phase != PresentationPhase::Approval
                    && self.phase != PresentationPhase::ConfiguredCommandActive
                {
                    self.phase = PresentationPhase::Ready;
                }
            }
            ProjectionEvent::TurnFailed {
                code,
                message,
                retryable,
            } => {
                self.push_entry(TranscriptEntry::Failure {
                    code: sanitize_terminal_text(&code),
                    message: sanitize_terminal_text(&message),
                    retryable,
                });
                self.phase = PresentationPhase::Ready;
            }
            ProjectionEvent::ProcessReconciled { outcome } => {
                self.push_entry(TranscriptEntry::Assistant(format!(
                    "process reconciled: {}",
                    sanitize_terminal_text(&outcome)
                )));
                self.phase = PresentationPhase::Ready;
            }
            ProjectionEvent::ProcessUnknown { next_action } => {
                self.push_entry(TranscriptEntry::Failure {
                    code: "process_outcome_unknown".to_owned(),
                    message: format!(
                        "process outcome unknown; next action: {}",
                        sanitize_terminal_text(&next_action)
                    ),
                    retryable: false,
                });
                self.phase = PresentationPhase::Ready;
            }
            ProjectionEvent::ConfiguredCommandActive { command } => {
                self.active_command = Some(sanitize_terminal_text(&command));
                self.phase = PresentationPhase::ConfiguredCommandActive;
                self.composer.clear();
            }
            ProjectionEvent::ConfiguredCommandFinished { command, status } => {
                self.push_entry(TranscriptEntry::Assistant(format!(
                    "configured command {}: {}",
                    sanitize_terminal_text(&status),
                    sanitize_terminal_text(&command)
                )));
                self.active_command = None;
                self.phase = if self.turn_completed {
                    PresentationPhase::Ready
                } else {
                    PresentationPhase::Streaming
                };
                self.composer.clear();
            }
            ProjectionEvent::ReconciliationRequired { code, message } => {
                self.push_entry(TranscriptEntry::Failure {
                    code: sanitize_terminal_text(&code),
                    message: sanitize_terminal_text(&message),
                    retryable: false,
                });
                self.phase = PresentationPhase::Reconcile;
            }
        }
    }

    /// Applies one terminal key to presentation state and returns a typed intent.
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "unhandled and future crossterm keys intentionally produce no presentation intent"
    )]
    pub fn handle_key(&mut self, key: KeyEvent) -> ComposerIntent {
        if key.kind != KeyEventKind::Press {
            return ComposerIntent::None;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return ComposerIntent::Quit;
        }
        if self.phase != PresentationPhase::Ready
            && self.phase != PresentationPhase::Approval
            && self.phase != PresentationPhase::ConfiguredCommandActive
        {
            return ComposerIntent::None;
        }
        match key.code {
            KeyCode::Enter => {
                if self.phase == PresentationPhase::ConfiguredCommandActive
                    && self.composer == "cancel"
                {
                    self.composer.clear();
                    ComposerIntent::CancelConfiguredCommand
                } else if self.phase == PresentationPhase::ConfiguredCommandActive {
                    ComposerIntent::None
                } else if self.phase == PresentationPhase::Approval && self.composer == "approve" {
                    self.composer.clear();
                    ComposerIntent::Approve
                } else if self.phase == PresentationPhase::Approval && self.composer == "deny" {
                    self.composer.clear();
                    ComposerIntent::Deny
                } else if self.phase == PresentationPhase::Approval && self.composer == "cancel" {
                    self.composer.clear();
                    ComposerIntent::Cancel
                } else if self.phase == PresentationPhase::Approval {
                    ComposerIntent::None
                } else {
                    let prompt = self.composer.trim().to_owned();
                    if prompt.is_empty() {
                        ComposerIntent::None
                    } else {
                        ComposerIntent::Submit(prompt)
                    }
                }
            }
            KeyCode::Backspace => {
                self.composer.pop();
                ComposerIntent::None
            }
            KeyCode::Char(character)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                if self.composer.len().saturating_add(character.len_utf8()) <= MAX_COMPOSER_BYTES {
                    self.composer.push(character);
                }
                ComposerIntent::None
            }
            _ => ComposerIntent::None,
        }
    }

    /// Retains a bounded tail of presentation rows.
    fn push_entry(&mut self, entry: TranscriptEntry) {
        if self.entries.len() >= MAX_TRANSCRIPT_ENTRIES {
            let active_assistant = (self.phase == PresentationPhase::Streaming)
                .then(|| {
                    self.entries
                        .iter()
                        .rposition(|candidate| matches!(candidate, TranscriptEntry::Assistant(_)))
                })
                .flatten();
            let eviction = usize::from(active_assistant == Some(0));
            self.entries.remove(eviction);
        }
        self.entries.push(entry);
    }
}

/// Renders one authority-neutral projection frame.
pub fn render(frame: &mut Frame<'_>, projection: &ConversationProjection) {
    let [transcript_area, status_area, composer_area, footer_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let transcript = transcript_text(projection);
    let inner_width = usize::from(transcript_area.width.saturating_sub(2)).max(1);
    let line_count = transcript
        .lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(inner_width))
        .sum::<usize>();
    let transcript_widget = Paragraph::new(transcript)
        .block(Block::bordered().title(" Tiber "))
        .wrap(Wrap { trim: false });
    let inner_height = usize::from(transcript_area.height.saturating_sub(2));
    let scroll = line_count.saturating_sub(inner_height);
    frame.render_widget(
        transcript_widget.scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0)),
        transcript_area,
    );
    let (status, status_style) = match projection.phase {
        PresentationPhase::Ready => ("ready", Style::default().fg(Color::Green)),
        PresentationPhase::Streaming => (
            "streaming \u{b7} model effects remain inert",
            Style::default().fg(Color::Cyan),
        ),
        PresentationPhase::ConfiguredCommandActive => (
            "configured command active",
            Style::default().fg(Color::Yellow),
        ),
        PresentationPhase::Approval => ("approval required", Style::default().fg(Color::Yellow)),
        PresentationPhase::DecisionComplete => (
            "waiting for turn completion",
            Style::default().fg(Color::Cyan),
        ),
        PresentationPhase::Reconcile => ("reconcile required", Style::default().fg(Color::Yellow)),
    };
    let status_text = projection.active_command.as_ref().map_or_else(
        || status.to_owned(),
        |command| format!("{status} \u{b7} {command}"),
    );
    frame.render_widget(Paragraph::new(status_text).style(status_style), status_area);
    frame.render_widget(
        Paragraph::new(projection.composer.as_str()).block(Block::bordered().title(" Message ")),
        composer_area,
    );
    let footer = match projection.phase {
        PresentationPhase::Ready => "enter send \u{b7} ctrl+c quit",
        PresentationPhase::Streaming => "streaming \u{b7} ctrl+c quit \u{b7} tools are inert",
        PresentationPhase::ConfiguredCommandActive => "type cancel \u{b7} ctrl+c quit",
        PresentationPhase::Approval => "type approve, deny, or cancel \u{b7} ctrl+c quit",
        PresentationPhase::DecisionComplete => "waiting for turn completion \u{b7} ctrl+c quit",
        PresentationPhase::Reconcile => "reconcile required \u{b7} ctrl+c quit",
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
        footer_area,
    );
}

/// Converts transcript entries into styled Ratatui text.
#[expect(
    clippy::pattern_type_mismatch,
    reason = "matching borrowed transcript variants avoids cloning owned presentation content"
)]
fn transcript_text(projection: &ConversationProjection) -> Text<'static> {
    let mut lines = Vec::new();
    let active_assistant = projection
        .entries
        .iter()
        .rposition(|entry| matches!(entry, TranscriptEntry::Assistant(_)));
    for (index, entry) in projection.entries.iter().enumerate() {
        match entry {
            TranscriptEntry::User(text) => {
                lines.push(Line::styled(
                    "you",
                    Style::default().add_modifier(Modifier::BOLD),
                ));
                lines.push(Line::from(format!("  {text}")));
            }
            TranscriptEntry::Assistant(text) => {
                let label = if projection.phase == PresentationPhase::Streaming
                    && active_assistant == Some(index)
                {
                    "tiber \u{b7} streaming"
                } else {
                    "tiber"
                };
                lines.push(Line::styled(label, Style::default().fg(Color::Cyan)));
                lines.push(Line::from(format!("  {text}")));
            }
            TranscriptEntry::ToolProposal {
                arguments,
                call_id,
                tool,
            } => {
                lines.push(Line::styled(
                    "tool proposal \u{b7} not executed",
                    Style::default().fg(Color::Yellow),
                ));
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(tool.clone(), Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(format!(" \u{b7} {call_id}")),
                ]));
                lines.push(Line::from(format!("  {arguments}")));
            }
            TranscriptEntry::RepositoryProposal { diff, path } => {
                lines.push(Line::styled(
                    format!("repository change proposed \u{b7} {path}"),
                    Style::default().fg(Color::Yellow),
                ));
                for line in diff.lines() {
                    lines.push(Line::from(format!("  {line}")));
                }
            }
            TranscriptEntry::Failure {
                code,
                message,
                retryable,
            } => {
                let retry = if *retryable { " \u{b7} retryable" } else { "" };
                lines.push(Line::styled(
                    format!("{code}{retry}"),
                    Style::default().fg(Color::Red),
                ));
                lines.push(Line::from(format!("  {message}")));
            }
        }
        lines.push(Line::default());
    }
    Text::from(lines)
}

/// Escapes terminal control characters while preserving ordinary layout whitespace.
fn sanitize_terminal_text(text: &str) -> String {
    truncate_utf8(&sanitize_terminal_controls(text), MAX_FIELD_BYTES)
}

/// Escapes controls in a bounded repository diff without hiding an approvable suffix.
fn sanitize_repository_diff(text: &str) -> String {
    sanitize_terminal_controls(text)
}

/// Escapes terminal controls while preserving ordinary layout whitespace.
fn sanitize_terminal_controls(text: &str) -> String {
    text.chars()
        .flat_map(|character| match character {
            '\n' | '\t' => character.to_string().chars().collect::<Vec<_>>(),
            _ if character.is_control() => format!("\\u{{{:04X}}}", u32::from(character))
                .chars()
                .collect(),
            _ => vec![character],
        })
        .collect::<String>()
}

/// Returns a UTF-8-safe prefix within one byte budget.
fn truncate_utf8(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let boundary = text
        .char_indices()
        .map(|(index, _character)| index)
        .take_while(|index| *index <= limit)
        .last()
        .unwrap_or(0);
    text.get(..boundary).unwrap_or_default().to_owned()
}

/// Appends sanitized model text without allowing unbounded projection growth.
fn bounded_append(target: &mut String, delta: &str) {
    let remaining = MAX_ASSISTANT_BYTES.saturating_sub(target.len());
    if remaining == 0 {
        return;
    }
    let boundary = delta
        .char_indices()
        .map(|(index, _character)| index)
        .take_while(|index| *index <= remaining)
        .last()
        .unwrap_or(0);
    if delta.len() <= remaining {
        target.push_str(delta);
        return;
    }
    if let Some(prefix) = delta.get(..boundary) {
        target.push_str(prefix);
    }
}
