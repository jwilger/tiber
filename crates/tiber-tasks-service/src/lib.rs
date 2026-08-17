//! Native query projections and narrow task-command decisions.
//!
//! This crate folds the preserved `tiber.domain_event` task vocabulary into a
//! query model and exposes command-specific pure decisions. Broad
//! [`TaskBoardProjection`] state remains query-only; every write decision owns
//! only the facts required for its business rule.

#![forbid(unsafe_code)]

extern crate alloc;

/// Pure native task-command decisions.
pub mod command;

use alloc::{collections::BTreeMap, vec, vec::Vec};
use core::{error::Error, fmt};

use eventcore_types::StreamId;
use tiber_tasks_core::{
    Task, TaskAcceptanceAdded, TaskAcceptanceChecked, TaskAcceptanceRemoved, TaskCreated,
    TaskDetailsUpdated, TaskEvent, TaskId, TaskLinksChanged, TaskOrder, TaskPullRequestChanged,
    TaskStatus, TaskSubtaskAdded, TaskSubtaskChecked, TaskSubtaskIdCorrected,
    TaskSubtaskOccurrenceChecked, TaskTransitioned,
};

/// Opaque publication for one modeled reciprocal blocked-by dependency.
#[derive(Debug, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the opaque token groups the consistency fence before the target and blocker facts in command-flow order"
)]
pub struct DependencyLinkPublication {
    /// Exact board and both endpoint streams read by the decision.
    consistency_streams: [StreamId; 3],
    /// Complete replacement links for the task that is blocked.
    target_fact: TaskLinksChanged,
    /// Complete replacement links for the task that blocks it.
    blocker_fact: TaskLinksChanged,
}

/// Opaque publication for one modeled strict board-priority movement.
#[derive(Debug, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the opaque token stores the modeled fact before the consistency fence used to publish it"
)]
pub struct TaskPriorityPublication {
    /// Sole complete strict-order fact authorized for publication.
    fact: TaskOrder,
    /// Exact board and both addressed-task streams read by the decision.
    consistency_streams: [StreamId; 3],
}

/// Opaque modeled abandonment batch and exact consistency fence.
pub struct TaskAbandonmentPublication {
    /// Exact board and addressed-task streams read by the modeled decision.
    consistency_streams: [StreamId; 2],
    /// Complete strict board order authorized for publication.
    order: TaskOrder,
    /// Terminal task transition authorized for publication.
    transition: TaskTransitioned,
}
impl TaskAbandonmentPublication {
    /// Closes modeled abandonment facts and their exact consistency fence into
    /// one opaque publication token.
    ///
    /// # Errors
    ///
    /// Returns a typed command error when the modeled transition, resulting
    /// order, or consistency streams do not describe the same abandonment.
    #[expect(
        clippy::needless_pass_by_value,
        clippy::needless_return,
        clippy::question_mark_used,
        clippy::single_call_fn,
        reason = "the sole modeled abandonment boundary consumes its closed fact values, validates their semantic identity with typed propagation, and follows the project explicit-return policy"
    )]
    pub(crate) fn from_modeled_facts(
        task: TaskId,
        transition: TaskTransitioned,
        order: TaskOrder,
        consistency_streams: [StreamId; 2],
    ) -> Result<Self, command::TaskCommandError> {
        let expected = command::acceptance_consistency_streams(&task)?;
        if expected != consistency_streams
            || transition.stream_id != consistency_streams[1]
            || transition.stem != task
            || transition.status != TaskStatus::Abandoned
            || transition.claim.is_some()
            || order.stream_id != consistency_streams[0]
            || order.order.contains(&task)
        {
            return Err(command::TaskCommandError::InvalidModeledTaskAbandonmentPublication);
        }
        return Ok(Self {
            consistency_streams,
            order,
            transition,
        });
    }
    /// Transfers the closed modeled batch and its exact consistency fence to
    /// the publication adapter.
    #[must_use]
    #[inline]
    #[expect(
        clippy::needless_return,
        reason = "the ownership-transfer boundary follows the project explicit-return policy"
    )]
    pub fn into_events_and_consistency_streams(self) -> (Vec<TaskEvent>, [StreamId; 2]) {
        return (
            vec![
                TaskEvent::TaskTransitioned(self.transition),
                TaskEvent::TaskPriorityChanged(self.order),
            ],
            self.consistency_streams,
        );
    }
}

impl TaskPriorityPublication {
    /// Creates the closed priority token from one modeled board fact.
    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the sole command-local constructor validates modeled provenance before returning the closed token"
    )]
    pub(crate) fn from_modeled_fact(
        fact: TaskOrder,
        consistency_streams: [StreamId; 3],
    ) -> Result<Self, command::TaskCommandError> {
        if fact.stream_id != consistency_streams[0] {
            return Err(command::TaskCommandError::InvalidModeledTaskPriorityPublication);
        }
        Ok(Self {
            fact,
            consistency_streams,
        })
    }

    /// Transfers the modeled fact and its exact three-stream fence.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the one-shot opaque transfer returns its complete event and fence directly"
    )]
    pub fn into_event_and_consistency_streams(self) -> (TaskEvent, [StreamId; 3]) {
        (
            TaskEvent::TaskPriorityChanged(self.fact),
            self.consistency_streams,
        )
    }
}

impl DependencyLinkPublication {
    /// Creates the closed reciprocal dependency token from modeled facts.
    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the sole command-local constructor validates reciprocal modeled provenance before returning the closed token"
    )]
    pub(crate) fn from_modeled_facts(
        target_fact: TaskLinksChanged,
        blocker_fact: TaskLinksChanged,
        consistency_streams: [StreamId; 3],
    ) -> Result<Self, command::TaskCommandError> {
        let board = &consistency_streams[0];
        if &target_fact.stream_id != board
            || &blocker_fact.stream_id != board
            || !target_fact.blocked_by.contains(&blocker_fact.stem)
            || !blocker_fact.blocks.contains(&target_fact.stem)
        {
            return Err(command::TaskCommandError::InvalidModeledDependencyLinkPublication);
        }
        Ok(Self {
            consistency_streams,
            target_fact,
            blocker_fact,
        })
    }

    /// Transfers the modeled reciprocal facts and their exact consistency fence.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the one-shot transfer returns its complete reciprocal batch and fence directly"
    )]
    pub fn into_events_and_consistency_streams(self) -> (Vec<TaskEvent>, [StreamId; 3]) {
        (
            vec![
                TaskEvent::TaskLinksChanged(self.target_fact),
                TaskEvent::TaskLinksChanged(self.blocker_fact),
            ],
            self.consistency_streams,
        )
    }
}

/// Opaque publication for one modeled unchecked acceptance addition.
#[derive(Debug, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the opaque token stores the modeled fact before the consistency fence used to publish it"
)]
pub struct AcceptanceAddPublication {
    /// Sole modeled unchecked acceptance fact authorized for publication.
    fact: TaskAcceptanceAdded,
    /// Exact board and addressed-task streams read by the addition decision.
    consistency_streams: [StreamId; 2],
}

impl AcceptanceAddPublication {
    /// Creates the closed acceptance-add token from one modeled unchecked fact.
    ///
    /// # Errors
    ///
    /// Returns a typed command failure when the fact is checked, is not on the
    /// addressed task stream, or differs from the exact consistency fence.
    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the closed-token constructor validates modeled provenance before returning the exact two-stream fence"
    )]
    pub(crate) fn from_modeled_fact(
        fact: TaskAcceptanceAdded,
        consistency_streams: [StreamId; 2],
    ) -> Result<Self, command::TaskCommandError> {
        if fact.stream_id != consistency_streams[1]
            || fact.item.checked
            || !fact.stream_id.as_ref().ends_with(fact.stem.as_str())
        {
            return Err(command::TaskCommandError::InvalidModeledAcceptanceAddPublication);
        }
        Ok(Self {
            fact,
            consistency_streams,
        })
    }

    /// Transfers the modeled fact and its exact board-and-task stream fences.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the one-shot opaque transfer returns its complete event and fence directly"
    )]
    pub fn into_event_and_consistency_streams(self) -> (TaskEvent, [StreamId; 2]) {
        (
            TaskEvent::TaskAcceptanceAdded(self.fact),
            self.consistency_streams,
        )
    }
}

/// Opaque publication for one modeled task-details replacement.
#[derive(Debug, Eq, PartialEq)]
pub struct TaskDetailsPublication {
    /// Exact board and addressed-task streams read by the details decision.
    consistency_streams: [StreamId; 2],
    /// Sole modeled task-details fact authorized for publication.
    fact: TaskDetailsUpdated,
}

impl TaskDetailsPublication {
    /// Creates the closed details token from one modeled fact and its task-stream authority.
    ///
    /// # Errors
    ///
    /// Returns a typed command failure when the modeled fact is not on the
    /// addressed task stream or the required board stream cannot be derived.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        clippy::single_call_fn,
        reason = "the closed-token constructor validates modeled provenance before returning the exact two-stream fence"
    )]
    #[inline]
    pub(crate) fn from_modeled_fact(
        fact: TaskDetailsUpdated,
        consistency_stream: StreamId,
    ) -> Result<Self, command::TaskCommandError> {
        let expected = consistency_stream
            .as_ref()
            .strip_prefix("tiber:task:")
            .unwrap_or_default();
        if fact.stream_id != consistency_stream || fact.stem.as_str() != expected {
            return Err(command::TaskCommandError::InvalidModeledTaskDetailsPublication);
        }
        let board_stream = StreamId::try_new("tiber:board".to_owned())
            .map_err(|_source| command::TaskCommandError::InvalidTaskStream)?;
        Ok(Self {
            consistency_streams: [board_stream, consistency_stream],
            fact,
        })
    }

    /// Transfers the modeled fact and its exact board-and-task stream fences.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the one-shot opaque transfer is the complete final value"
    )]
    pub fn into_event_and_consistency_streams(self) -> (TaskEvent, [StreamId; 2]) {
        (
            TaskEvent::TaskDetailsUpdated(self.fact),
            self.consistency_streams,
        )
    }
}

/// The only publication input for creating one backlog task.
///
/// This opaque token carries the modeled creation fact and resulting strict
/// board order on the single board-authority stream. It cannot encode an
/// arbitrary task mutation or caller-selected event batch.
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct TaskCreationPublication {
    /// Exact board stream-version fence read by the creation command.
    consistency_stream: StreamId,
    /// The sole modeled backlog-creation fact authorized for publication.
    created_fact: TaskCreated,
    /// The modeled strict board order that accompanies the creation fact.
    order_fact: TaskOrder,
}

impl TaskCreationPublication {
    /// Creates the closed creation token from the modeled creation facts.
    ///
    /// # Errors
    ///
    /// Returns a typed command error when the modeled facts do not form the
    /// exact backlog creation batch with its required board consistency fence.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the only command-local construction point validates the modeled creation facts before the closed transfer token is created"
    )]
    pub(crate) fn from_modeled_facts(
        created_fact: TaskCreated,
        order_fact: TaskOrder,
        consistency_stream: StreamId,
    ) -> Result<Self, command::TaskCommandError> {
        if created_fact.stream_id != consistency_stream
            || order_fact.stream_id != consistency_stream
            || created_fact.task.status != TaskStatus::Backlog
            || created_fact.task.claim.is_some()
            || order_fact.order.last() != Some(&created_fact.task.stem)
        {
            return Err(command::TaskCommandError::InvalidModeledTaskCreationPublication);
        }
        Ok(Self {
            consistency_stream,
            created_fact,
            order_fact,
        })
    }

    /// Transfers the closed two-fact creation batch to the Git adapter.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the opaque transfer returns its fixed fact batch and fence directly"
    )]
    pub fn into_events_and_consistency_streams(self) -> (Vec<TaskEvent>, [StreamId; 1]) {
        (
            vec![
                TaskEvent::TaskCreated(self.created_fact),
                TaskEvent::BoardReordered(self.order_fact),
            ],
            [self.consistency_stream],
        )
    }

    /// Returns the durable identity selected by the modeled creation decision.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the opaque token exposes only its modeled durable identity"
    )]
    pub const fn task_id(&self) -> &TaskId {
        &self.created_fact.task.stem
    }
}

/// The only publication input for activating one strict-next backlog task.
///
/// This opaque token carries exactly one unclaimed `InProgress` transition and
/// the board plus addressed-task streams whose versions fenced the pure start
/// decision. It is a named activation, not a generic lifecycle transition.
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the opaque activation token fields follow activation fact then consistency-fence transfer flow rather than alphabetical order"
)]
pub struct TaskActivationPublication {
    /// The sole unclaimed in-progress fact allowed through this closed boundary.
    transitioned_fact: TaskTransitioned,
    /// Exact stream-version fence read by the activation command.
    consistency_streams: [StreamId; 2],
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the opaque activation token presents validation, inspection, then one-shot transfer in data-flow order"
)]
impl TaskActivationPublication {
    /// Creates the closed activation token from one modeled command fact.
    ///
    /// # Errors
    ///
    /// Returns a typed command error when the modeled fact is not the exact
    /// unclaimed activation on its required board/task consistency fence.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        clippy::single_call_fn,
        reason = "the only command-local construction point validates the modeled activation fact before the closed transfer token is created"
    )]
    pub(crate) fn from_modeled_fact(
        transitioned_fact: TaskTransitioned,
        consistency_streams: [StreamId; 2],
    ) -> Result<Self, command::TaskCommandError> {
        let expected_streams = command::acceptance_consistency_streams(&transitioned_fact.stem)?;
        if transitioned_fact.stream_id != consistency_streams[0]
            || expected_streams != consistency_streams
            || transitioned_fact.status != TaskStatus::InProgress
            || transitioned_fact.claim.is_some()
        {
            return Err(command::TaskCommandError::InvalidModeledTaskActivationPublication);
        }
        Ok(Self {
            transitioned_fact,
            consistency_streams,
        })
    }

    /// Returns the sole unclaimed in-progress fact authorized for publication.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the opaque token exposes its modeled activation fact only for adapter-boundary inspection"
    )]
    pub const fn transitioned_fact(&self) -> &TaskTransitioned {
        &self.transitioned_fact
    }

    /// Returns the exact board/task stream fence read by the command.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the fixed two-stream activation fence is clearest as a borrowed token accessor"
    )]
    pub const fn consistency_streams(&self) -> &[StreamId; 2] {
        &self.consistency_streams
    }

    /// Transfers the closed activation event and exact stream fence to its adapter.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the one-shot adapter transfer preserves the modeled activation fact and its fence together"
    )]
    pub fn into_event_and_consistency_streams(self) -> (TaskEvent, [StreamId; 2]) {
        (
            TaskEvent::TaskTransitioned(self.transitioned_fact),
            self.consistency_streams,
        )
    }
}

/// The only publication input the initial native task-write slice may emit.
///
/// This opaque token carries exactly one checked acceptance fact and the board
/// plus addressed-task streams whose versions fenced the command decision.
/// Future task writes introduce their own closed publication types rather than
/// widening this boundary to arbitrary events or stream lists.
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct AcceptanceCheckPublication {
    /// The sole fact allowed through the first native task-write boundary.
    checked_fact: TaskAcceptanceChecked,
    /// Exact stream-version fence read by the acceptance-check command.
    consistency_streams: [StreamId; 2],
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the opaque publication token presents construction, inspection, then one-shot transfer in data-flow order"
)]
impl AcceptanceCheckPublication {
    /// Creates the single check-only publication token from one modeled command projection.
    ///
    /// # Errors
    ///
    /// Returns [`command::TaskCommandError::InvalidModeledAcceptancePublication`]
    /// when the modeled fact would not produce the one allowed durable task
    /// acceptance event with its exact two-stream consistency boundary.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        clippy::single_call_fn,
        reason = "the only command-local construction point consumes the checked model's single internal fact before constructing the single named durable fact"
    )]
    pub(crate) fn from_modeled_fact(
        checked_fact: TaskAcceptanceChecked,
        consistency_streams: [StreamId; 2],
    ) -> Result<Self, command::TaskCommandError> {
        let expected_streams = command::acceptance_consistency_streams(&checked_fact.stem)?;
        if !checked_fact.checked
            || checked_fact.stream_id != consistency_streams[0]
            || expected_streams != consistency_streams
        {
            return Err(command::TaskCommandError::InvalidModeledAcceptancePublication);
        }
        Ok(Self {
            checked_fact,
            consistency_streams,
        })
    }

    /// Returns the sole checked acceptance fact authorized for publication.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the opaque token exposes its modeled fact only for inspection at an adapter boundary"
    )]
    pub const fn checked_fact(&self) -> &TaskAcceptanceChecked {
        &self.checked_fact
    }

    /// Returns the exact board/task stream fence read by the command.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the fixed two-stream command fence is clearest as a borrowed token accessor"
    )]
    pub const fn consistency_streams(&self) -> &[StreamId; 2] {
        &self.consistency_streams
    }

    /// Transfers the closed event and its exact stream fence to the publication adapter.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the one-shot adapter transfer preserves the token's modeled event and fixed fence together"
    )]
    pub fn into_event_and_consistency_streams(self) -> (TaskEvent, [StreamId; 2]) {
        (
            TaskEvent::TaskAcceptanceChecked(self.checked_fact),
            self.consistency_streams,
        )
    }
}

/// The only publication input for correcting one duplicate legacy subtask identity.
///
/// This opaque token carries exactly one preconditioned correction fact and
/// the board plus addressed-task streams whose versions fenced its pure command
/// decision. It does not authorize general task mutation.
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the opaque correction token fields follow fact-then-fence transfer flow rather than alphabetical order"
)]
pub struct SubtaskIdCorrectionPublication {
    /// The sole correction fact allowed through this closed boundary.
    corrected_fact: TaskSubtaskIdCorrected,
    /// Exact stream-version fence read by the correction command.
    consistency_streams: [StreamId; 2],
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the opaque correction token presents validation, inspection, then one-shot transfer in data-flow order"
)]
impl SubtaskIdCorrectionPublication {
    /// Creates a closed correction token from one modeled command fact.
    ///
    /// # Errors
    ///
    /// Returns a typed command error when the modeled fact is not an exact
    /// board-side correction with the required board/task consistency fence.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        clippy::single_call_fn,
        reason = "the only command-local construction point validates the modeled fact before the closed transfer token is created"
    )]
    pub(crate) fn from_modeled_fact(
        corrected_fact: TaskSubtaskIdCorrected,
        consistency_streams: [StreamId; 2],
    ) -> Result<Self, command::TaskCommandError> {
        let expected_streams = command::acceptance_consistency_streams(&corrected_fact.stem)?;
        if corrected_fact.stream_id != consistency_streams[0]
            || expected_streams != consistency_streams
            || corrected_fact.expected.id == corrected_fact.replacement_id
            || corrected_fact.replacement_id.trim().is_empty()
        {
            return Err(command::TaskCommandError::InvalidModeledSubtaskCorrectionPublication);
        }
        Ok(Self {
            corrected_fact,
            consistency_streams,
        })
    }

    /// Returns the sole preconditioned correction fact authorized for publication.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the opaque token exposes its modeled correction only for adapter-boundary inspection"
    )]
    pub const fn corrected_fact(&self) -> &TaskSubtaskIdCorrected {
        &self.corrected_fact
    }

    /// Returns the exact board/task stream fence read by the correction command.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the fixed two-stream correction fence is clearest as a borrowed token accessor"
    )]
    pub const fn consistency_streams(&self) -> &[StreamId; 2] {
        &self.consistency_streams
    }

    /// Transfers the closed correction event and exact stream fence to the publication adapter.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the one-shot adapter transfer preserves the modeled correction and its fence together"
    )]
    pub fn into_event_and_consistency_streams(self) -> (TaskEvent, [StreamId; 2]) {
        (
            TaskEvent::TaskSubtaskIdCorrected(self.corrected_fact),
            self.consistency_streams,
        )
    }
}

/// The only publication input for checking one exact subtask occurrence.
///
/// This opaque token carries one preconditioned occurrence fact and the board
/// plus addressed-task streams whose versions fenced its pure decision. It is
/// intentionally not an identifier-based or generic subtask mutation.
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct SubtaskOccurrenceCheckPublication {
    /// The sole exact occurrence fact allowed through this closed boundary.
    checked_fact: TaskSubtaskOccurrenceChecked,
    /// Exact stream-version fence read by the occurrence-check command.
    consistency_streams: [StreamId; 2],
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the opaque occurrence-check token presents validation, inspection, then one-shot transfer in data-flow order"
)]
impl SubtaskOccurrenceCheckPublication {
    /// Creates a closed occurrence-check token from one modeled command fact.
    ///
    /// # Errors
    ///
    /// Returns a typed command error when the modeled fact is not an exact
    /// unchecked occurrence on the required board/task consistency fence.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        clippy::single_call_fn,
        reason = "the only command-local construction point validates the modeled fact before the closed transfer token is created"
    )]
    pub(crate) fn from_modeled_fact(
        checked_fact: TaskSubtaskOccurrenceChecked,
        consistency_streams: [StreamId; 2],
    ) -> Result<Self, command::TaskCommandError> {
        let expected_streams = command::acceptance_consistency_streams(&checked_fact.stem)?;
        if checked_fact.expected.checked
            || checked_fact.stream_id != consistency_streams[0]
            || expected_streams != consistency_streams
        {
            return Err(command::TaskCommandError::InvalidModeledSubtaskOccurrencePublication);
        }
        Ok(Self {
            checked_fact,
            consistency_streams,
        })
    }

    /// Returns the sole exact occurrence fact authorized for publication.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the opaque token exposes its modeled fact only for adapter-boundary inspection"
    )]
    pub const fn checked_fact(&self) -> &TaskSubtaskOccurrenceChecked {
        &self.checked_fact
    }

    /// Returns the exact board/task stream fence read by the command.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the fixed two-stream occurrence-check fence is clearest as a borrowed token accessor"
    )]
    pub const fn consistency_streams(&self) -> &[StreamId; 2] {
        &self.consistency_streams
    }

    /// Transfers the closed event and exact stream fence to the publication adapter.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the one-shot adapter transfer preserves the modeled occurrence fact and its fence together"
    )]
    pub fn into_event_and_consistency_streams(self) -> (TaskEvent, [StreamId; 2]) {
        (
            TaskEvent::TaskSubtaskOccurrenceChecked(self.checked_fact),
            self.consistency_streams,
        )
    }
}

/// The only publication input for completing one current task.
///
/// A first completion carries the terminal lifecycle transition plus one
/// strict board-order fact. A retry after a completed task whose old order
/// still names it carries only the order repair. No caller can construct a
/// broad task-mutation batch through this opaque boundary.
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the opaque completion token fields follow addressed task, optional transition, order repair, then consistency fence"
)]
pub struct TaskCompletionPublication {
    /// Exact task addressed by the completion decision.
    task: TaskId,
    /// Present only when the task still needs its terminal lifecycle transition.
    transitioned_fact: Option<TaskTransitioned>,
    /// Complete strict open-task order after removing every stale target entry.
    reordered_fact: TaskOrder,
    /// Exact board/task stream fence read by the completion command.
    consistency_streams: [StreamId; 2],
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the opaque completion token presents validation, inspection, then one-shot batch transfer in data-flow order"
)]
impl TaskCompletionPublication {
    /// Creates a closed completion token from modeled terminal and board facts.
    ///
    /// # Errors
    ///
    /// Returns a typed command error when the modeled batch is not exactly one
    /// order repair with an optional `Done` transition on its board/task fence.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        clippy::single_call_fn,
        reason = "the only command-local construction point validates the optional terminal fact and mandatory strict order before creating the closed token"
    )]
    pub(crate) fn from_modeled_facts(
        task: TaskId,
        transitioned_fact: Option<TaskTransitioned>,
        reordered_fact: TaskOrder,
        consistency_streams: [StreamId; 2],
    ) -> Result<Self, command::TaskCommandError> {
        let expected_streams = command::acceptance_consistency_streams(&task)?;
        let transition_is_valid = transitioned_fact.as_ref().is_none_or(|transitioned| {
            transitioned.stream_id == consistency_streams[0]
                && transitioned.stem == task
                && transitioned.status == TaskStatus::Done
                && transitioned.claim.is_none()
        });
        if expected_streams != consistency_streams
            || !transition_is_valid
            || reordered_fact.stream_id != consistency_streams[0]
            || reordered_fact.order.contains(&task)
        {
            return Err(command::TaskCommandError::InvalidModeledTaskCompletionPublication);
        }
        Ok(Self {
            task,
            transitioned_fact,
            reordered_fact,
            consistency_streams,
        })
    }

    /// Returns the addressed durable task identity.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed addressed identity is clearest as the opaque token's direct accessor"
    )]
    pub const fn task(&self) -> &TaskId {
        &self.task
    }

    /// Returns the terminal fact when this publication performs the first completion.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the optional terminal fact is exposed only for adapter-boundary inspection"
    )]
    pub const fn transitioned_fact(&self) -> Option<&TaskTransitioned> {
        self.transitioned_fact.as_ref()
    }

    /// Returns the mandatory strict board-order fact.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the mandatory strict order repair is exposed only for adapter-boundary inspection"
    )]
    pub const fn reordered_fact(&self) -> &TaskOrder {
        &self.reordered_fact
    }

    /// Returns the exact board/task stream fence read by the command.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the fixed two-stream completion fence is clearest as a borrowed token accessor"
    )]
    pub const fn consistency_streams(&self) -> &[StreamId; 2] {
        &self.consistency_streams
    }

    /// Transfers the closed one-or-two-event batch and exact fence to its adapter.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the one-shot batch transfer preserves the optional terminal fact, mandatory order repair, and fence together"
    )]
    pub fn into_events_and_consistency_streams(self) -> (Vec<TaskEvent>, [StreamId; 2]) {
        let mut events = Vec::new();
        if let Some(transitioned_fact) = self.transitioned_fact {
            events.push(TaskEvent::TaskTransitioned(transitioned_fact));
        }
        events.push(TaskEvent::BoardReordered(self.reordered_fact));
        (events, self.consistency_streams)
    }
}

/// An immutable, globally ordered set of durable task facts.
///
/// The source adapter must preserve commit order across all included streams.
/// Each [`TaskEvent`] carries its own stream identity, so this boundary does
/// not enumerate or assume task streams: preserved historical
/// `tiber:task:*` streams are accepted whenever their facts appear here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskHistory {
    /// Source facts retained in their global durable order.
    ordered_events: Vec<TaskEvent>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the history API presents construction before inspection, which follows the query boundary's data flow rather than alphabetical ordering"
)]
impl TaskHistory {
    /// Captures task facts in their globally durable order.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the constructed immutable history is clearest as the function's final expression"
    )]
    pub fn from_ordered_events<Events>(events: Events) -> Self
    where
        Events: IntoIterator<Item = TaskEvent>,
    {
        Self {
            ordered_events: events.into_iter().collect(),
        }
    }

    /// Returns the immutable globally ordered source facts.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed immutable fact slice is clearest as a final expression"
    )]
    pub fn events(&self) -> &[TaskEvent] {
        &self.ordered_events
    }
}

/// A normalized user task reference accepted by task queries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskReference(String);

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the reference API documents parsing before inspection to keep the user-input boundary legible"
)]
impl TaskReference {
    /// Parses a task reference at a user-input boundary.
    ///
    /// # Errors
    ///
    /// Returns [`TaskProjectionError::InvalidTaskReference`] when the input is
    /// empty, path-like, control-containing, or names a Markdown file.
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the successful normalized reference and predicate closures are idiomatic final expressions"
    )]
    pub fn parse(input: &str) -> Result<Self, TaskProjectionError> {
        let value = input.trim();
        if value.is_empty()
            || value
                .get(value.len().saturating_sub(3)..)
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".md"))
            || value.contains(['/', '\\'])
            || value.chars().any(char::is_control)
        {
            return Err(TaskProjectionError::InvalidTaskReference);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the normalized reference text.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed normalized reference text is clearest as a final expression"
    )]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable failures emitted while replaying or querying task history.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    clippy::exhaustive_enums,
    reason = "the closed stable-code vocabulary is grouped by input, replay, and workflow failure semantics rather than alphabetically, and callers may exhaustively handle its durable cases"
)]
pub enum TaskProjectionError {
    /// A task reference did not meet the task-query input grammar.
    InvalidTaskReference,
    /// A user reference matched no current task.
    TaskReferenceMissing {
        /// The normalized unresolved reference.
        reference: TaskReference,
    },
    /// A user reference matched more than one current task.
    TaskReferenceAmbiguous {
        /// The normalized ambiguous reference.
        reference: TaskReference,
        /// Current task identities sharing that reference.
        matches: Vec<TaskId>,
    },
    /// A history repeated a task's creation fact.
    DuplicateTaskCreation {
        /// The duplicated task identity.
        task: TaskId,
    },
    /// A task mutation appeared before its task creation fact.
    TaskMissing {
        /// The unknown task identity.
        task: TaskId,
    },
    /// A subtask mutation referenced an absent subtask.
    SubtaskMissing {
        /// The owning task identity.
        task: TaskId,
        /// The absent subtask identity.
        subtask: String,
    },
    /// A correction referenced an absent immutable subtask occurrence.
    SubtaskOccurrenceMissing {
        /// The owning task identity.
        task: TaskId,
        /// The zero-based absent occurrence position.
        index: usize,
    },
    /// A correction's recorded preimage did not match the current occurrence.
    SubtaskCorrectionPreimageMismatch {
        /// The owning task identity.
        task: TaskId,
        /// The zero-based corrected occurrence position.
        index: usize,
    },
    /// An exact occurrence check's recorded preimage did not match the current occurrence.
    SubtaskOccurrenceCheckPreimageMismatch {
        /// The owning task identity.
        task: TaskId,
        /// The zero-based checked occurrence position.
        index: usize,
    },
    /// A correction attempted to rename an identity that was not duplicated.
    SubtaskCorrectionIdNotDuplicate {
        /// The owning task identity.
        task: TaskId,
        /// The zero-based corrected occurrence position.
        index: usize,
    },
    /// A correction would introduce another duplicate subtask identity.
    SubtaskCorrectionDuplicateId {
        /// The owning task identity.
        task: TaskId,
        /// The already-present replacement identity.
        subtask: String,
    },
    /// An acceptance mutation referenced an absent list position.
    AcceptanceItemMissing {
        /// The owning task identity.
        task: TaskId,
        /// The zero-based absent list position.
        index: usize,
    },
    /// Historical facts contain more than one active task.
    MultipleActiveTasks {
        /// Active task identities in stable identity order.
        active_tasks: Vec<TaskId>,
    },
    /// A newer task-fact vocabulary variant is not modeled by this projection.
    UnsupportedTaskEvent,
}

impl TaskProjectionError {
    /// Returns the stable machine-readable failure code.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "the closed borrowed error-code mapping is clearest as a concise final match"
    )]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidTaskReference => "tasks_invalid_task_reference",
            Self::TaskReferenceMissing { .. } => "tasks_task_reference_missing",
            Self::TaskReferenceAmbiguous { .. } => "tasks_task_reference_ambiguous",
            Self::DuplicateTaskCreation { .. } => "tasks_projection_duplicate_task_creation",
            Self::TaskMissing { .. } => "tasks_projection_task_missing",
            Self::SubtaskMissing { .. } => "tasks_projection_subtask_missing",
            Self::SubtaskOccurrenceMissing { .. } => "tasks_projection_subtask_occurrence_missing",
            Self::SubtaskCorrectionPreimageMismatch { .. } => {
                "tasks_projection_subtask_correction_preimage_mismatch"
            }
            Self::SubtaskOccurrenceCheckPreimageMismatch { .. } => {
                "tasks_projection_subtask_occurrence_check_preimage_mismatch"
            }
            Self::SubtaskCorrectionIdNotDuplicate { .. } => {
                "tasks_projection_subtask_correction_id_not_duplicate"
            }
            Self::SubtaskCorrectionDuplicateId { .. } => {
                "tasks_projection_subtask_correction_duplicate_id"
            }
            Self::AcceptanceItemMissing { .. } => "tasks_projection_acceptance_item_missing",
            Self::MultipleActiveTasks { .. } => "tasks_projection_multiple_active_tasks",
            Self::UnsupportedTaskEvent => "tasks_projection_unsupported_task_event",
        }
    }
}

impl fmt::Display for TaskProjectionError {
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "formatting directly delegates to the stable code as its final expression"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[expect(
    clippy::missing_trait_methods,
    reason = "this leaf projection error has no cause, source, provider, or deprecated description beyond its stable display code"
)]
impl Error for TaskProjectionError {}

/// The complete query-side state folded from native task facts.
///
/// This is not an aggregate or a write authority. It contains broad task-board
/// state only because read queries need it; modeled task commands will fold
/// only the bounded facts required by their individual decisions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the projection fields follow replay state flow—initialization, task index, then board order—rather than alphabetical names"
)]
pub struct TaskBoardProjection {
    /// Whether the repository initialization fact has been observed.
    initialized: bool,
    /// Current task state indexed by durable identity.
    tasks: BTreeMap<TaskId, Task>,
    /// Strict open-board priority sequence from the latest order fact.
    priority_order: Vec<TaskId>,
}

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "the public query methods and private fold helpers follow replay and query flow rather than alphabetical ordering"
)]
impl TaskBoardProjection {
    /// Replays one globally ordered history into a fresh query projection.
    ///
    /// # Errors
    ///
    /// Returns the first malformed-history error encountered while folding the
    /// supplied durable facts in their source order.
    #[expect(
        clippy::implicit_return,
        clippy::missing_inline_in_public_items,
        clippy::question_mark_used,
        reason = "replay is a nontrivial public fold where typed propagation and an expression final result are clearer than forced inlining or manual matches"
    )]
    pub fn replay(history: &TaskHistory) -> Result<Self, TaskProjectionError> {
        let mut projection = Self::default();
        for event in history.events() {
            projection.apply(event)?;
        }
        Ok(projection)
    }

    /// Folds one next durable fact into this query projection.
    ///
    /// Callers use this only with a source that preserves global fact order.
    /// It does not issue facts, make a task decision, or change the source
    /// event store.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the next fact is invalid relative to the
    /// state already folded into this read projection.
    #[expect(
        clippy::implicit_return,
        clippy::missing_inline_in_public_items,
        clippy::pattern_type_mismatch,
        clippy::question_mark_used,
        reason = "the central ordered replay dispatcher needs typed propagation and borrowed event patterns; forcing inline expansion or manual matches would obscure its bounded read-model behavior"
    )]
    pub fn apply(&mut self, event: &TaskEvent) -> Result<(), TaskProjectionError> {
        match event {
            TaskEvent::RepositoryInitialized(_) => self.initialized = true,
            TaskEvent::TaskCreated(created) => self.apply_task_created(created)?,
            TaskEvent::TaskTransitioned(transitioned) => self.apply_transition(transitioned)?,
            TaskEvent::TaskPriorityChanged(order) | TaskEvent::BoardReordered(order) => {
                self.apply_order(order);
            }
            TaskEvent::TaskLinksChanged(changed) => self.apply_links_changed(changed)?,
            TaskEvent::TaskSubtaskAdded(added) => self.apply_subtask_added(added)?,
            TaskEvent::TaskSubtaskChecked(checked) => self.apply_subtask_checked(checked)?,
            TaskEvent::TaskSubtaskOccurrenceChecked(checked) => {
                self.apply_subtask_occurrence_checked(checked)?;
            }
            TaskEvent::TaskSubtaskIdCorrected(corrected) => {
                self.apply_subtask_id_corrected(corrected)?;
            }
            TaskEvent::TaskDetailsUpdated(updated) => self.apply_details_updated(updated)?,
            TaskEvent::HistoricalTaskClaimChanged(changed) => {
                self.task_mut(&changed.stem)?
                    .claim
                    .clone_from(&changed.claim);
            }
            TaskEvent::TaskPullRequestChanged(changed) => {
                self.apply_pull_request_changed(changed)?;
            }
            TaskEvent::TaskAcceptanceAdded(added) => self.apply_acceptance_added(added)?,
            TaskEvent::TaskAcceptanceChecked(checked) => self.apply_acceptance_checked(checked)?,
            TaskEvent::TaskAcceptanceRemoved(removed) => self.apply_acceptance_removed(removed)?,
            TaskEvent::TaskNoteAdded(added) => {
                self.task_mut(&added.stem)?.notes.push(added.note.clone());
            }
            TaskEvent::TaskValidationRepaired(repaired) => {
                for changed in &repaired.link_changes {
                    self.apply_links_changed(changed)?;
                }
                if let Some(order) = &repaired.order_change {
                    self.apply_order(order);
                }
            }
            TaskEvent::TasksClosedFromCommitTrailers(closed) => {
                for task_id in &closed.stems {
                    let task = self.task_mut(task_id)?;
                    task.status = TaskStatus::Done;
                    task.claim = None;
                }
                self.priority_order.clone_from(&closed.order);
            }
            TaskEvent::HistoricalTaskClosedFromTrailer(closed) => {
                let task = self.task_mut(&closed.stem)?;
                task.status = TaskStatus::Done;
                task.claim = None;
            }
            TaskEvent::HistoricalTaskRemoved(removed) => {
                let _: Option<Task> = self.tasks.remove(&removed.stem);
            }
            // Historical publication notifications carry no query-side state.
            TaskEvent::HistoricalTaskStatePublished(_) => {}
            // `TaskEvent` is non-exhaustive so newer durable facts must never
            // silently produce an incomplete task-board view.
            _ => return Err(TaskProjectionError::UnsupportedTaskEvent),
        }
        Ok(())
    }

    /// Returns whether the repository initialization fact has been observed.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the initialization predicate is clearest as a final field expression"
    )]
    pub const fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Returns all current tasks in stable task-identity order.
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the ordered map iterator is clearest as a final borrowed expression"
    )]
    pub fn tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values()
    }

    /// Returns one current task by its durable identity.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the map lookup is clearest as a final borrowed expression"
    )]
    pub fn task(&self, task_id: &TaskId) -> Option<&Task> {
        self.tasks.get(task_id)
    }

    /// Returns the durable board priority sequence, including any stale IDs
    /// retained in historical facts.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the immutable priority slice is clearest as a final borrowed expression"
    )]
    pub fn priority_order(&self) -> &[TaskId] {
        &self.priority_order
    }

    /// Returns current open tasks in strict board priority order.
    ///
    /// A stale historical board ID or a task later moved to a terminal state is
    /// omitted from this query view rather than being turned into an open task.
    /// Future modeled validation commands own repair of durable board-order
    /// inconsistency.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the ordered query pipeline and its predicates are clearer as expression returns"
    )]
    pub fn ordered_tasks(&self) -> Vec<&Task> {
        self.priority_order
            .iter()
            .filter_map(|task_id| self.tasks.get(task_id))
            .filter(|task| matches!(task.status, TaskStatus::Backlog | TaskStatus::InProgress))
            .collect()
    }

    /// Returns all current active tasks in stable identity order.
    ///
    /// The projection reports rather than hides multiple historical active
    /// tasks. Future task-transition commands will be their authority fence.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the stable active-task query pipeline is clearer as an expression return"
    )]
    pub fn active_tasks(&self) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|task| task.status == TaskStatus::InProgress)
            .collect()
    }

    /// Returns the first backlog task in board order whose blockers are all
    /// known and done.
    ///
    /// It intentionally does not select an active task or treat a missing
    /// blocker as done. The caller can separately inspect [`Self::active_tasks`]
    /// to apply the one-active-task workflow policy.
    #[must_use]
    #[inline]
    #[expect(
        clippy::implicit_return,
        reason = "the prioritized eligibility query and its predicates are clearer as expression returns"
    )]
    pub fn next_eligible_task(&self) -> Option<&Task> {
        self.priority_order
            .iter()
            .filter_map(|task_id| self.tasks.get(task_id))
            .find(|task| task.status == TaskStatus::Backlog && self.blockers_are_done(task))
    }

    /// Returns the task to continue under the one-active-task workflow policy.
    ///
    /// A sole in-progress task always takes precedence over a new eligible
    /// backlog task. Corrupt historical state with multiple active tasks is
    /// surfaced as a stable typed error rather than hidden by selection.
    ///
    /// # Errors
    ///
    /// Returns [`TaskProjectionError::MultipleActiveTasks`] when history has
    /// more than one active task.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "the closed active-task selection remains clearest as a borrowed slice match"
    )]
    pub fn next_actionable_task(&self) -> Result<Option<&Task>, TaskProjectionError> {
        let active_tasks = self.active_tasks();
        match active_tasks.as_slice() {
            [] => Ok(self.next_eligible_task()),
            [task] => Ok(Some(*task)),
            _ => Err(TaskProjectionError::MultipleActiveTasks {
                active_tasks: active_tasks
                    .into_iter()
                    .map(|task| task.stem.clone())
                    .collect(),
            }),
        }
    }

    /// Resolves a normalized task reference to one durable task identity.
    ///
    /// # Errors
    ///
    /// Returns a stable missing or ambiguity error when the normalized
    /// reference does not identify exactly one current task.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::pattern_type_mismatch,
        reason = "the exact-match query keeps its predicate pipeline and borrowed slice match explicit"
    )]
    pub fn resolve_task_reference(
        &self,
        reference: &TaskReference,
    ) -> Result<TaskId, TaskProjectionError> {
        let matches = self
            .tasks
            .keys()
            .filter(|task_id| task_reference_matches(task_id.as_str(), reference.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [task_id] => Ok(task_id.clone()),
            [] => Err(TaskProjectionError::TaskReferenceMissing {
                reference: reference.clone(),
            }),
            _ => Err(TaskProjectionError::TaskReferenceAmbiguous {
                reference: reference.clone(),
                matches,
            }),
        }
    }

    /// Parses and resolves a user-provided full stem, short ID, or nickname.
    ///
    /// # Errors
    ///
    /// Returns a parsing, missing, or ambiguity error when the input does not
    /// identify exactly one current task.
    #[inline]
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the composed input boundary keeps its typed parse propagation explicit"
    )]
    pub fn resolve_task_ref(&self, input: &str) -> Result<TaskId, TaskProjectionError> {
        let reference = TaskReference::parse(input)?;
        self.resolve_task_reference(&reference)
    }

    /// Folds a creation fact after rejecting a duplicate durable identity.
    #[expect(
        clippy::implicit_return,
        reason = "the successful duplicate-checked insertion ends with the idiomatic unit result"
    )]
    fn apply_task_created(&mut self, created: &TaskCreated) -> Result<(), TaskProjectionError> {
        let task = (*created.task).clone();
        let task_id = task.stem.clone();
        if self.tasks.contains_key(&task_id) {
            return Err(TaskProjectionError::DuplicateTaskCreation { task: task_id });
        }
        let _: Option<Task> = self.tasks.insert(task_id, task);
        Ok(())
    }

    /// Folds a lifecycle/claim transition into an existing task.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and an idiomatic final unit result"
    )]
    fn apply_transition(
        &mut self,
        transitioned: &TaskTransitioned,
    ) -> Result<(), TaskProjectionError> {
        let task = self.task_mut(&transitioned.stem)?;
        task.status = transitioned.status;
        task.claim.clone_from(&transitioned.claim);
        Ok(())
    }

    /// Replaces the query-side strict priority sequence.
    fn apply_order(&mut self, order: &TaskOrder) {
        self.priority_order.clone_from(&order.order);
    }

    /// Replaces one task's dependency-link fields.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and an idiomatic final unit result"
    )]
    fn apply_links_changed(
        &mut self,
        changed: &TaskLinksChanged,
    ) -> Result<(), TaskProjectionError> {
        let task = self.task_mut(&changed.stem)?;
        task.blocks.clone_from(&changed.blocks);
        task.blocked_by.clone_from(&changed.blocked_by);
        Ok(())
    }

    /// Appends a durable subtask to its owning task.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and an idiomatic final unit result"
    )]
    fn apply_subtask_added(&mut self, added: &TaskSubtaskAdded) -> Result<(), TaskProjectionError> {
        self.task_mut(&added.stem)?
            .subtasks
            .push(added.subtask.clone());
        Ok(())
    }

    /// Changes the check state of an already-created subtask.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and compact option-to-error conversion"
    )]
    fn apply_subtask_checked(
        &mut self,
        checked: &TaskSubtaskChecked,
    ) -> Result<(), TaskProjectionError> {
        let task = self.task_mut(&checked.stem)?;
        let subtask = task
            .subtasks
            .iter_mut()
            .find(|subtask| subtask.id == checked.subtask_id)
            .ok_or_else(|| TaskProjectionError::SubtaskMissing {
                task: checked.stem.clone(),
                subtask: checked.subtask_id.clone(),
            })?;
        subtask.checked = checked.checked;
        Ok(())
    }

    /// Checks one exact current subtask occurrence after verifying its complete preimage.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the narrow exact-occurrence fold uses typed propagation and validates its immutable preimage before changing only the check state"
    )]
    fn apply_subtask_occurrence_checked(
        &mut self,
        checked: &TaskSubtaskOccurrenceChecked,
    ) -> Result<(), TaskProjectionError> {
        let task = self.task_mut(&checked.stem)?;
        let current = task.subtasks.get(checked.index).ok_or_else(|| {
            TaskProjectionError::SubtaskOccurrenceMissing {
                task: checked.stem.clone(),
                index: checked.index,
            }
        })?;
        if current != &checked.expected || checked.expected.checked {
            return Err(
                TaskProjectionError::SubtaskOccurrenceCheckPreimageMismatch {
                    task: checked.stem.clone(),
                    index: checked.index,
                },
            );
        }
        let target = task.subtasks.get_mut(checked.index).ok_or_else(|| {
            TaskProjectionError::SubtaskOccurrenceMissing {
                task: checked.stem.clone(),
                index: checked.index,
            }
        })?;
        target.checked = true;
        Ok(())
    }

    /// Corrects precisely one malformed legacy subtask identity after checking its preimage.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the narrow append-only correction validates one indexed preimage and preserves typed projection failures"
    )]
    fn apply_subtask_id_corrected(
        &mut self,
        corrected: &TaskSubtaskIdCorrected,
    ) -> Result<(), TaskProjectionError> {
        let task = self.task_mut(&corrected.stem)?;
        let current = task.subtasks.get(corrected.index).ok_or_else(|| {
            TaskProjectionError::SubtaskOccurrenceMissing {
                task: corrected.stem.clone(),
                index: corrected.index,
            }
        })?;
        if current != &corrected.expected {
            return Err(TaskProjectionError::SubtaskCorrectionPreimageMismatch {
                task: corrected.stem.clone(),
                index: corrected.index,
            });
        }
        let duplicate_count = task
            .subtasks
            .iter()
            .filter(|subtask| subtask.id == corrected.expected.id)
            .count();
        if duplicate_count < 2 {
            return Err(TaskProjectionError::SubtaskCorrectionIdNotDuplicate {
                task: corrected.stem.clone(),
                index: corrected.index,
            });
        }
        if task.subtasks.iter().enumerate().any(|(index, subtask)| {
            index != corrected.index && subtask.id == corrected.replacement_id
        }) {
            return Err(TaskProjectionError::SubtaskCorrectionDuplicateId {
                task: corrected.stem.clone(),
                subtask: corrected.replacement_id.clone(),
            });
        }
        let Some(target) = task.subtasks.get_mut(corrected.index) else {
            return Err(TaskProjectionError::SubtaskOccurrenceMissing {
                task: corrected.stem.clone(),
                index: corrected.index,
            });
        };
        target.id.clone_from(&corrected.replacement_id);
        Ok(())
    }

    /// Replaces mutable display and rationale fields for one task.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and an idiomatic final unit result"
    )]
    fn apply_details_updated(
        &mut self,
        updated: &TaskDetailsUpdated,
    ) -> Result<(), TaskProjectionError> {
        let task = self.task_mut(&updated.stem)?;
        task.title.clone_from(&updated.title);
        task.tags.clone_from(&updated.tags);
        task.summary.clone_from(&updated.summary);
        task.context.clone_from(&updated.context);
        Ok(())
    }

    /// Replaces the pull-request metadata for one task.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and an idiomatic final unit result"
    )]
    fn apply_pull_request_changed(
        &mut self,
        changed: &TaskPullRequestChanged,
    ) -> Result<(), TaskProjectionError> {
        let task = self.task_mut(&changed.stem)?;
        task.pr_mr_url.clone_from(&changed.url);
        task.pr_mr_status.clone_from(&changed.status);
        Ok(())
    }

    /// Appends a durable acceptance criterion to its owning task.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and an idiomatic final unit result"
    )]
    fn apply_acceptance_added(
        &mut self,
        added: &TaskAcceptanceAdded,
    ) -> Result<(), TaskProjectionError> {
        self.task_mut(&added.stem)?
            .acceptance
            .push(added.item.clone());
        Ok(())
    }

    /// Changes the check state of one already-created acceptance criterion.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and compact option-to-error conversion"
    )]
    fn apply_acceptance_checked(
        &mut self,
        checked: &TaskAcceptanceChecked,
    ) -> Result<(), TaskProjectionError> {
        let item = self
            .task_mut(&checked.stem)?
            .acceptance
            .get_mut(checked.index)
            .ok_or_else(|| TaskProjectionError::AcceptanceItemMissing {
                task: checked.stem.clone(),
                index: checked.index,
            })?;
        item.checked = checked.checked;
        Ok(())
    }

    /// Removes one already-created acceptance criterion by durable position.
    #[expect(
        clippy::implicit_return,
        clippy::question_mark_used,
        reason = "the small fold uses typed propagation and an idiomatic final unit result"
    )]
    fn apply_acceptance_removed(
        &mut self,
        removed: &TaskAcceptanceRemoved,
    ) -> Result<(), TaskProjectionError> {
        let task = self.task_mut(&removed.stem)?;
        if removed.index >= task.acceptance.len() {
            return Err(TaskProjectionError::AcceptanceItemMissing {
                task: removed.stem.clone(),
                index: removed.index,
            });
        }
        let _: tiber_tasks_core::ChecklistItem = task.acceptance.remove(removed.index);
        Ok(())
    }

    /// Borrows an existing task or reports the malformed historical sequence.
    #[expect(
        clippy::implicit_return,
        reason = "the borrowed lookup and typed missing-task conversion are clearest as expressions"
    )]
    fn task_mut(&mut self, task_id: &TaskId) -> Result<&mut Task, TaskProjectionError> {
        self.tasks
            .get_mut(task_id)
            .ok_or_else(|| TaskProjectionError::TaskMissing {
                task: task_id.clone(),
            })
    }

    /// Reports whether every dependency is currently present and done.
    #[expect(
        clippy::implicit_return,
        reason = "the all-blockers predicate is clearest as a final iterator expression"
    )]
    fn blockers_are_done(&self, task: &Task) -> bool {
        task.blocked_by.iter().all(|blocker_id| {
            self.tasks
                .get(blocker_id)
                .is_some_and(|blocker| blocker.status == TaskStatus::Done)
        })
    }
}

/// Reports whether a full stem supplies the requested full, short, or nickname reference.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "this named matcher keeps the three accepted reference grammars isolated from the projection query pipeline"
)]
fn task_reference_matches(task_id: &str, reference: &str) -> bool {
    if task_id == reference {
        return true;
    }
    let Some((date, after_date)) = task_id.split_once('-') else {
        return false;
    };
    let Some((code, nickname)) = after_date.split_once('-') else {
        return false;
    };
    format!("{date}-{code}") == reference || nickname == reference
}
