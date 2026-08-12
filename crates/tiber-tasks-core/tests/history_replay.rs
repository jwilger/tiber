#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::implicit_return,
    clippy::missing_trait_methods,
    clippy::single_call_fn,
    clippy::unnecessary_literal_bound,
    clippy::unwrap_in_result,
    reason = "the black-box persisted-history fixture uses fail-fast assertions and an intentionally minimal EventCore projector without entering shipping library code"
)]
mod tests {
    extern crate alloc;

    use alloc::{collections::BTreeSet, sync::Arc};
    use core::convert::Infallible;
    use std::sync::Mutex;

    use eventcore::{Event as _, ProjectionConfig, Projector, StreamPosition, run_projection};
    use eventcore_memory::InMemoryEventStore;
    use eventcore_types::{EventStore as _, StreamVersion, StreamWrites};
    use futures::executor::block_on;
    use tiber_tasks_core::{TaskEvent, TaskStatus};

    const EXPECTED_EVENT_KINDS: [&str; 15] = [
        "board_reordered",
        "repository_initialized",
        "task_acceptance_added",
        "task_acceptance_checked",
        "task_acceptance_removed",
        "task_created",
        "task_details_updated",
        "task_links_changed",
        "task_note_added",
        "task_priority_changed",
        "task_state_published",
        "task_subtask_added",
        "task_subtask_checked",
        "task_transitioned",
        "tasks_closed_from_commit_trailers",
    ];
    // Scrubbed and synthetic, but shaped from one retained `origin/tiber` fact
    // for every event tag found at the standalone-Tiber cutover.
    const HISTORY_FIXTURE: &str = include_str!("fixtures/origin-tiber-task-history-v1.json");

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct ReplayedFact {
        kind: String,
        stream: String,
    }

    struct TaskHistoryCompatibilityProjector {
        facts: Arc<Mutex<Vec<ReplayedFact>>>,
    }

    impl Projector for TaskHistoryCompatibilityProjector {
        type Context = ();
        type Error = Infallible;
        type Event = TaskEvent;

        fn apply(
            &mut self,
            event: Self::Event,
            _position: StreamPosition,
            _context: &mut Self::Context,
        ) -> Result<(), Self::Error> {
            self.facts
                .lock()
                .expect("fixture projection storage is available")
                .push(ReplayedFact {
                    kind: event_kind(&event).to_owned(),
                    stream: event.stream_id().as_ref().to_owned(),
                });
            Ok(())
        }

        fn name(&self) -> &str {
            "tiber-tasks-core-history-replay"
        }
    }

    fn event_kind(event: &TaskEvent) -> &'static str {
        match *event {
            TaskEvent::BoardReordered(_) => "board_reordered",
            TaskEvent::HistoricalTaskClaimChanged(_) => "task_claim_changed",
            TaskEvent::HistoricalTaskClosedFromTrailer(_) => "task_closed_from_trailer",
            TaskEvent::HistoricalTaskRemoved(_) => "task_removed",
            TaskEvent::HistoricalTaskStatePublished(_) => "task_state_published",
            TaskEvent::RepositoryInitialized(_) => "repository_initialized",
            TaskEvent::TaskAcceptanceAdded(_) => "task_acceptance_added",
            TaskEvent::TaskAcceptanceChecked(_) => "task_acceptance_checked",
            TaskEvent::TaskAcceptanceRemoved(_) => "task_acceptance_removed",
            TaskEvent::TaskCreated(_) => "task_created",
            TaskEvent::TaskDetailsUpdated(_) => "task_details_updated",
            TaskEvent::TaskLinksChanged(_) => "task_links_changed",
            TaskEvent::TaskNoteAdded(_) => "task_note_added",
            TaskEvent::TaskPriorityChanged(_) => "task_priority_changed",
            TaskEvent::TaskPullRequestChanged(_) => "task_pull_request_changed",
            TaskEvent::TaskSubtaskAdded(_) => "task_subtask_added",
            TaskEvent::TaskSubtaskChecked(_) => "task_subtask_checked",
            TaskEvent::TaskTransitioned(_) => "task_transitioned",
            TaskEvent::TaskValidationRepaired(_) => "task_validation_repaired",
            TaskEvent::TasksClosedFromCommitTrailers(_) => "tasks_closed_from_commit_trailers",
            _ => "unrecognized",
        }
    }

    fn fixture_events() -> Vec<TaskEvent> {
        serde_json::from_str(HISTORY_FIXTURE).expect("task-history fixture deserializes")
    }

    fn replayed_fact(event: &TaskEvent) -> ReplayedFact {
        ReplayedFact {
            kind: event_kind(event).to_owned(),
            stream: event.stream_id().as_ref().to_owned(),
        }
    }

    fn seed_fixture(store: &InMemoryEventStore, events: &[TaskEvent]) {
        let mut streams = Vec::new();
        for event in events {
            let stream = event.stream_id().clone();
            if !streams.contains(&stream) {
                streams.push(stream);
            }
        }

        let mut writes = StreamWrites::new();
        for stream in streams {
            writes = writes
                .register_stream(stream, StreamVersion::new(0))
                .expect("fixture streams are registered exactly once");
        }
        for event in events {
            writes = writes
                .append(event.clone())
                .expect("fixture event targets a registered stream");
        }

        block_on(store.append_events(writes)).expect("fixture append succeeds");
    }

    #[test]
    fn retained_task_history_fixture_round_trips_every_observed_wire_kind() {
        let events = fixture_events();
        let original: serde_json::Value =
            serde_json::from_str(HISTORY_FIXTURE).expect("fixture is valid JSON");
        let reserialized = serde_json::to_value(&events).expect("native facts serialize");
        let actual_kinds = events.iter().map(event_kind).collect::<BTreeSet<_>>();
        let expected_kinds = EXPECTED_EVENT_KINDS.into_iter().collect::<BTreeSet<_>>();

        assert_eq!(events.len(), EXPECTED_EVENT_KINDS.len());
        assert_eq!(actual_kinds, expected_kinds);
        assert_eq!(TaskEvent::event_type_name(), "tiber.domain_event");
        assert_eq!(reserialized, original);
        assert!(matches!(
            events.get(1).expect("fixture contains task creation"),
            TaskEvent::TaskCreated(created)
                if created.task.status == TaskStatus::Backlog
                    && created.task.title.as_str() == "Replay retained Tiber task history"
        ));
        assert!(matches!(
            events.get(4).expect("fixture contains task transition"),
            TaskEvent::TaskTransitioned(transition)
                if transition.status == TaskStatus::InProgress
                    && transition.claim.is_some()
        ));
    }

    #[test]
    fn native_eventcore_projection_replays_every_retained_task_fact() {
        let events = fixture_events();
        let expected = events.iter().map(replayed_fact).collect::<Vec<_>>();
        let facts = Arc::new(Mutex::new(Vec::new()));
        let projector = TaskHistoryCompatibilityProjector {
            facts: Arc::clone(&facts),
        };
        let store = InMemoryEventStore::new();

        seed_fixture(&store, &events);
        block_on(run_projection(
            projector,
            &store,
            ProjectionConfig::default(),
        ))
        .expect("native EventCore projector replays the fixture");

        assert_eq!(
            facts
                .lock()
                .expect("fixture projection storage is available")
                .as_slice(),
            expected.as_slice()
        );
    }
}
