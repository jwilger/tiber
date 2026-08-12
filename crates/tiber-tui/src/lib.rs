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
    /// The active turn completed.
    TurnCompleted,
    /// The active turn ended with a typed failure.
    TurnFailed {
        code: String,
        message: String,
        retryable: bool,
    },
}

/// Current presentation phase; it carries no workflow authority.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PresentationPhase {
    /// The composer accepts owner input.
    #[default]
    Ready,
    /// One inference turn is producing observations.
    Streaming,
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
}

/// Owner intent emitted by the presentation for the application shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComposerIntent {
    /// No application action is requested.
    None,
    /// Submit one non-empty prompt.
    Submit(String),
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
    pub fn apply(&mut self, event: ProjectionEvent) {
        match event {
            ProjectionEvent::PromptSubmitted { text } => {
                self.push_entry(TranscriptEntry::User(sanitize_terminal_text(&text)));
                self.push_entry(TranscriptEntry::Assistant(String::new()));
                self.phase = PresentationPhase::Streaming;
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
            ProjectionEvent::TurnCompleted => self.phase = PresentationPhase::Ready,
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
        if self.phase != PresentationPhase::Ready {
            return ComposerIntent::None;
        }
        match key.code {
            KeyCode::Enter => {
                let prompt = self.composer.trim().to_owned();
                if prompt.is_empty() {
                    ComposerIntent::None
                } else {
                    ComposerIntent::Submit(prompt)
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
    };
    frame.render_widget(Paragraph::new(status).style(status_style), status_area);
    frame.render_widget(
        Paragraph::new(projection.composer.as_str()).block(Block::bordered().title(" Message ")),
        composer_area,
    );
    let footer = match projection.phase {
        PresentationPhase::Ready => "enter send \u{b7} ctrl+c quit",
        PresentationPhase::Streaming => "streaming \u{b7} ctrl+c quit \u{b7} tools are inert",
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
    let sanitized = text
        .chars()
        .flat_map(|character| match character {
            '\n' | '\t' => character.to_string().chars().collect::<Vec<_>>(),
            _ if character.is_control() => format!("\\u{{{:04X}}}", u32::from(character))
                .chars()
                .collect(),
            _ => vec![character],
        })
        .collect::<String>();
    truncate_utf8(&sanitized, MAX_FIELD_BYTES)
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
