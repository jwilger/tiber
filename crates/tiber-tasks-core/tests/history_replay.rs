#[cfg(test)]
mod tests {
    extern crate alloc;

    use alloc::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    };
    use core::convert::Infallible;
    use std::sync::Mutex;

    use eventcore::{Event as _, ProjectionConfig, Projector, StreamPosition, run_projection};
    use eventcore_memory::InMemoryEventStore;
    use eventcore_types::{EventStore as _, StreamVersion, StreamWrites};
    use futures::executor::block_on;
    use serde::Deserialize;
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
    // The trace records every retained fact's chronological event tag, stream
    // partition, stream version, transaction boundary, task relationship, and
    // lifecycle state. Its directly deserializable payloads are anonymized.
    const FULL_HISTORY_FIXTURE: &str =
        include_str!("fixtures/origin-tiber-task-history-v1-mixed-stream.jsonl");

    #[derive(Debug, Deserialize)]
    struct FullHistoryHeader {
        source: FullHistorySource,
    }

    #[derive(Debug, Deserialize)]
    struct FullHistorySource {
        branch: String,
        commit: String,
        facts: usize,
        streams: usize,
        transactions: usize,
    }

    #[derive(Clone, Debug, Deserialize)]
    struct FullHistoryFact {
        #[serde(rename = "d")]
        data: serde_json::Value,
        #[serde(rename = "e")]
        event: String,
        #[serde(rename = "z")]
        status: Option<String>,
        #[serde(rename = "s")]
        stream: u8,
        #[serde(rename = "v")]
        stream_version: usize,
        #[serde(rename = "q")]
        subject: Option<u8>,
        #[serde(rename = "x")]
        transaction: usize,
    }

    struct FullHistoryFixture {
        facts: Vec<FullHistoryFact>,
        source: FullHistorySource,
    }

    struct HistoryRelationships {
        stream_names_by_partition: BTreeMap<u8, BTreeSet<String>>,
        subject_lifecycle_states: BTreeMap<u8, BTreeSet<String>>,
        subject_streams: BTreeMap<u8, BTreeSet<u8>>,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct ReplayedFact {
        kind: String,
        stream: String,
    }

    struct TaskHistoryCompatibilityProjector {
        facts: Arc<Mutex<Vec<ReplayedFact>>>,
    }

    #[expect(
        clippy::missing_trait_methods,
        reason = "the compatibility fixture only records successful events and deliberately retains EventCore's default error hook"
    )]
    impl Projector for TaskHistoryCompatibilityProjector {
        type Context = ();
        type Error = Infallible;
        type Event = TaskEvent;

        #[expect(
            clippy::expect_used,
            clippy::implicit_return,
            clippy::unwrap_in_result,
            reason = "a poisoned fixture-only projection buffer cannot be represented by the deliberately infallible compatibility projector"
        )]
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

        #[expect(
            clippy::implicit_return,
            clippy::unnecessary_literal_bound,
            reason = "the EventCore trait requires a borrowed name while this fixture has one static projector identity"
        )]
        fn name(&self) -> &str {
            "tiber-tasks-core-history-replay"
        }
    }

    #[expect(
        clippy::implicit_return,
        reason = "the exhaustive fixture wire-kind mapping returns static labels directly"
    )]
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
            TaskEvent::TaskSubtaskOccurrenceChecked(_) => "task_subtask_occurrence_checked",
            TaskEvent::TaskSubtaskIdCorrected(_) => "task_subtask_id_corrected",
            TaskEvent::TaskTransitioned(_) => "task_transitioned",
            TaskEvent::TaskValidationRepaired(_) => "task_validation_repaired",
            TaskEvent::TasksClosedFromCommitTrailers(_) => "tasks_closed_from_commit_trailers",
            _ => "unrecognized",
        }
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        reason = "the static retained-history fixture must deserialize before its compatibility assertions can run"
    )]
    fn fixture_events() -> Vec<TaskEvent> {
        serde_json::from_str(HISTORY_FIXTURE).expect("task-history fixture deserializes")
    }

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the one full-history scenario keeps compact trace decoding isolated and fails fast on an invalid retained fixture"
    )]
    fn full_history_fixture() -> FullHistoryFixture {
        let mut lines = FULL_HISTORY_FIXTURE
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'));
        let header: FullHistoryHeader = lines
            .next()
            .map(serde_json::from_str)
            .transpose()
            .expect("full-history source metadata deserializes")
            .expect("full-history source metadata is present");
        let facts = lines
            .map(serde_json::from_str)
            .collect::<Result<Vec<_>, _>>()
            .expect("every retained full-history fact trace deserializes");

        FullHistoryFixture {
            facts,
            source: header.source,
        }
    }

    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the full-history relationship assertion uses one dedicated compact-wire task identity decoder"
    )]
    fn wire_task_subject(payload: &serde_json::Value) -> Option<u8> {
        let task_stem = match payload.get("task") {
            Some(task) => task.get("stem"),
            None => None,
        };
        let closure_stem = match payload.get("stems") {
            Some(stems) => match stems.as_array() {
                Some(closure_ids) => closure_ids.first(),
                None => None,
            },
            _ => None,
        };
        let stem = payload.get("stem").or(task_stem).or(closure_stem)?;
        let task_id = stem.as_str()?;
        let subject = task_id.strip_prefix("fixture-task-")?;

        subject.parse::<u8>().ok()
    }

    #[expect(
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the retained full-history scenario derives all cross-stream relationships in one named assertion helper"
    )]
    fn history_relationships(
        facts: &[FullHistoryFact],
        events: &[TaskEvent],
    ) -> HistoryRelationships {
        let mut stream_names_by_partition = BTreeMap::<u8, BTreeSet<String>>::new();
        let mut subject_lifecycle_states = BTreeMap::<u8, BTreeSet<String>>::new();
        let mut subject_streams = BTreeMap::<u8, BTreeSet<u8>>::new();

        for (fact, event) in facts.iter().zip(events) {
            assert_eq!(
                wire_task_subject(&fact.data),
                fact.subject,
                "the compact task identity agrees with the scrubbed wire payload"
            );
            stream_names_by_partition
                .entry(fact.stream)
                .or_default()
                .insert(event.stream_id().as_ref().to_owned());
            if let Some(subject) = fact.subject {
                subject_streams
                    .entry(subject)
                    .or_default()
                    .insert(fact.stream);
                if let Some(status) = fact.status.as_deref() {
                    subject_lifecycle_states
                        .entry(subject)
                        .or_default()
                        .insert(status.to_owned());
                }
            }
        }

        HistoryRelationships {
            stream_names_by_partition,
            subject_lifecycle_states,
            subject_streams,
        }
    }

    #[expect(
        clippy::implicit_return,
        reason = "the replay comparison maps a fixture event directly to its observable identity"
    )]
    fn replayed_fact(event: &TaskEvent) -> ReplayedFact {
        ReplayedFact {
            kind: event_kind(event).to_owned(),
            stream: event.stream_id().as_ref().to_owned(),
        }
    }

    #[expect(
        clippy::expect_used,
        clippy::single_call_fn,
        reason = "the small EventCore replay scenario fails fast while preparing its only in-memory fixture"
    )]
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

    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        clippy::single_call_fn,
        reason = "the retained full-history scenario fails fast if its recorded transaction boundaries cannot seed EventCore"
    )]
    fn seed_full_history(
        store: &InMemoryEventStore,
        facts: &[FullHistoryFact],
        events: &[TaskEvent],
    ) {
        let mut observed_versions = BTreeMap::<u8, usize>::new();
        let mut current_transaction = None;
        let mut registered_streams = BTreeSet::new();
        let mut writes = StreamWrites::new();

        for (fact, event) in facts.iter().zip(events) {
            if current_transaction.is_some_and(|transaction| transaction != fact.transaction) {
                block_on(store.append_events(writes))
                    .expect("transactional retained-history append succeeds");
                writes = StreamWrites::new();
                registered_streams.clear();
            }
            current_transaction = Some(fact.transaction);

            let actual_previous_version = observed_versions.entry(fact.stream).or_insert(0);
            let next_version = actual_previous_version
                .checked_add(1)
                .expect("fixture stream versions do not overflow");
            assert_eq!(
                fact.stream_version, next_version,
                "fixture preserves each stream's durable version sequence"
            );
            if registered_streams.insert(fact.stream) {
                writes = writes
                    .register_stream(
                        event.stream_id().clone(),
                        StreamVersion::new(*actual_previous_version),
                    )
                    .expect("transaction registers every participating stream once");
            }
            writes = writes
                .append(event.clone())
                .expect("transaction appends each retained fact to its registered stream");
            *actual_previous_version = fact.stream_version;
        }
        block_on(store.append_events(writes))
            .expect("final transactional retained-history append succeeds");
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the fixture's static wire payload must parse and reserialize before its public task-event contract can be asserted"
    )]
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
    #[expect(
        clippy::expect_used,
        reason = "the in-memory EventCore replay scenario stops at the exact fixture or projection failure preventing compatibility evidence"
    )]
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

    #[test]
    #[expect(
        clippy::expect_used,
        clippy::implicit_return,
        clippy::too_many_lines,
        reason = "one complete retained-history replay scenario keeps its transaction, stream, lifecycle, and wire-round-trip assertions beside the single replay it proves"
    )]
    fn scrubbed_full_history_preserves_retained_mixed_stream_replay() {
        let history = full_history_fixture();
        let events = history
            .facts
            .iter()
            .map(|fact| serde_json::from_value::<TaskEvent>(fact.data.clone()))
            .collect::<Result<Vec<_>, _>>()
            .expect(
                "every scrubbed retained full-history fact directly deserializes through TaskEvent",
            );
        let expected = events.iter().map(replayed_fact).collect::<Vec<_>>();
        let expected_event_kinds = history
            .facts
            .iter()
            .map(|fact| fact.event.as_str())
            .collect::<Vec<_>>();
        let actual_event_kinds = events.iter().map(event_kind).collect::<Vec<_>>();
        let event_kinds = history
            .facts
            .iter()
            .map(|fact| fact.event.as_str())
            .collect::<BTreeSet<_>>();
        let stream_partitions = history
            .facts
            .iter()
            .map(|fact| fact.stream)
            .collect::<BTreeSet<_>>();
        let transaction_ordinals = history
            .facts
            .iter()
            .map(|fact| fact.transaction)
            .collect::<BTreeSet<_>>();
        let lifecycle_states = history
            .facts
            .iter()
            .filter_map(|fact| fact.status.as_deref())
            .collect::<BTreeSet<_>>();
        let relationships = history_relationships(&history.facts, &events);
        let facts = Arc::new(Mutex::new(Vec::new()));
        let projector = TaskHistoryCompatibilityProjector {
            facts: Arc::clone(&facts),
        };
        let store = InMemoryEventStore::new();

        assert_eq!(history.source.branch, "origin/tiber");
        assert_eq!(
            history.source.commit,
            "513736a8e639c281a6fd42f812cdd931a2e2e033"
        );
        assert_eq!(history.source.facts, 251);
        assert_eq!(history.source.streams, 9);
        assert_eq!(history.source.transactions, 163);
        assert_eq!(history.facts.len(), history.source.facts);
        assert_eq!(events.len(), history.source.facts);
        assert_eq!(actual_event_kinds, expected_event_kinds);
        for (fact, event) in history.facts.iter().zip(&events) {
            assert_eq!(
                serde_json::to_value(event).expect("deserialized task fact reserializes"),
                fact.data,
                "every retained fact round-trips its scrubbed wire payload"
            );
        }
        assert_eq!(
            event_kinds,
            EXPECTED_EVENT_KINDS.into_iter().collect::<BTreeSet<_>>()
        );
        assert_eq!(stream_partitions.len(), history.source.streams);
        assert_eq!(
            relationships.stream_names_by_partition.len(),
            history.source.streams
        );
        assert!(
            relationships
                .stream_names_by_partition
                .values()
                .all(|stream_names| stream_names.len() == 1),
            "each retained stream partition maps to exactly one event stream"
        );
        assert_eq!(
            relationships
                .stream_names_by_partition
                .values()
                .flatten()
                .collect::<BTreeSet<_>>()
                .len(),
            history.source.streams,
            "the retained partitions map to distinct event streams"
        );
        assert_eq!(
            transaction_ordinals,
            (1..=history.source.transactions).collect::<BTreeSet<_>>(),
            "the retained trace has every transaction boundary in chronological order"
        );
        assert!(
            history.facts.windows(2).all(|window| {
                window
                    .first()
                    .zip(window.last())
                    .is_some_and(|(first, last)| first.transaction <= last.transaction)
            }),
            "the retained transaction boundaries remain chronological"
        );
        assert_eq!(
            lifecycle_states,
            BTreeSet::from(["backlog", "done", "in-progress"])
        );
        assert!(
            history.facts.windows(2).any(|window| {
                window
                    .first()
                    .zip(window.last())
                    .is_some_and(|(first, second)| {
                        first.transaction == second.transaction && first.stream != second.stream
                    })
            }),
            "the retained history has cross-stream EventCore transactions"
        );
        assert!(
            relationships.subject_streams.values().any(|streams| {
                streams.contains(&1) && streams.iter().any(|stream| *stream >= 3)
            }),
            "the retained history evolves a task from a task stream through the board stream"
        );
        assert!(
            relationships
                .subject_lifecycle_states
                .values()
                .any(|states| {
                    states.contains("backlog")
                        && states.contains("in-progress")
                        && states.contains("done")
                }),
            "the retained history preserves a task lifecycle across multiple event types"
        );

        seed_full_history(&store, &history.facts, &events);
        block_on(run_projection(
            projector,
            &store,
            ProjectionConfig::default(),
        ))
        .expect("native EventCore projector replays every scrubbed retained fact");

        assert_eq!(
            facts
                .lock()
                .expect("fixture projection storage is available")
                .as_slice(),
            expected.as_slice()
        );
    }
}
