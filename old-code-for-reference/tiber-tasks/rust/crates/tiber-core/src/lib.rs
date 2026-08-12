use std::fmt;

pub mod events;
pub mod task;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskTitle(String);

impl TaskTitle {
    pub fn parse(input: &str) -> Result<Self, CoreError> {
        let title = input.trim();
        if title.is_empty() {
            return Err(CoreError::EmptyTitle);
        }
        if title.chars().any(char::is_control) {
            return Err(CoreError::InvalidTitle);
        }
        Ok(Self(title.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn file_stem(&self) -> String {
        let mut slug = String::new();
        let mut previous_was_separator = true;

        for character in self.0.chars().flat_map(char::to_lowercase) {
            if character.is_ascii_alphanumeric() {
                slug.push(character);
                previous_was_separator = false;
            } else if !previous_was_separator {
                slug.push('-');
                previous_was_separator = true;
            }
        }

        if slug.ends_with('-') {
            slug.pop();
        }

        if slug.is_empty() {
            "task".to_string()
        } else {
            slug
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskSnapshot {
    path: String,
    title: String,
}

impl TaskSnapshot {
    pub fn new(path: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            title: title.into(),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn title(&self) -> &str {
        &self.title
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoardSnapshot {
    ordered_tasks: Vec<TaskSnapshot>,
}

impl BoardSnapshot {
    pub fn from_ordered_tasks(ordered_tasks: Vec<TaskSnapshot>) -> Self {
        Self { ordered_tasks }
    }

    pub fn ordered_tasks(&self) -> &[TaskSnapshot] {
        &self.ordered_tasks
    }

    pub fn next_task(&self) -> Option<&TaskSnapshot> {
        self.ordered_tasks.first()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskDependencies {
    task_ref: String,
    blocks: Vec<String>,
}

impl TaskDependencies {
    pub fn new(task_ref: impl Into<String>, blocks: Vec<impl Into<String>>) -> Self {
        Self {
            task_ref: task_ref.into(),
            blocks: blocks.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependencyGraph {
    tasks: Vec<TaskDependencies>,
}

impl DependencyGraph {
    pub fn from_tasks(tasks: Vec<TaskDependencies>) -> Self {
        Self { tasks }
    }

    pub fn cycle_messages(&self) -> Vec<String> {
        self.cycle_messages_with_label("dependency")
    }

    pub fn cycle_messages_with_label(&self, label: &str) -> Vec<String> {
        let task_refs = self
            .tasks
            .iter()
            .map(|task| task.task_ref.clone())
            .collect::<Vec<_>>();
        let mut reported = Vec::new();
        for task_ref in &task_refs {
            let mut path = Vec::new();
            self.find_cycles(task_ref, task_ref, &task_refs, &mut path, &mut reported);
        }
        reported.sort();
        reported.dedup();
        reported
            .into_iter()
            .map(|cycle| format!("cycle {label} {}", cycle.join(" -> ")))
            .collect()
    }

    fn find_cycles(
        &self,
        start: &str,
        current: &str,
        task_refs: &[String],
        path: &mut Vec<String>,
        reported: &mut Vec<Vec<String>>,
    ) {
        if path.iter().any(|task_ref| task_ref == current) {
            return;
        }
        path.push(current.to_string());
        for blocked_ref in self.blocks_for(current) {
            if !task_refs.contains(blocked_ref) {
                continue;
            }
            if blocked_ref == start {
                let mut cycle = path.clone();
                cycle.push(start.to_string());
                if path.first().is_some_and(|first| first == start) {
                    if let Some(canonical) = canonical_cycle(&cycle) {
                        reported.push(canonical);
                    }
                }
            } else {
                self.find_cycles(start, blocked_ref, task_refs, path, reported);
            }
        }
        path.pop();
    }

    fn blocks_for(&self, task_ref: &str) -> &[String] {
        self.tasks
            .iter()
            .find(|task| task.task_ref == task_ref)
            .map(|task| task.blocks.as_slice())
            .unwrap_or_default()
    }
}

fn canonical_cycle(cycle: &[String]) -> Option<Vec<String>> {
    let nodes = cycle.split_last()?.1;
    let start_index = nodes
        .iter()
        .enumerate()
        .min_by(|(_left_index, left), (_right_index, right)| left.cmp(right))?
        .0;
    let mut canonical = nodes[start_index..].to_vec();
    canonical.extend_from_slice(&nodes[..start_index]);
    canonical.push(canonical.first()?.clone());
    Some(canonical)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderReconciliation {
    entries: Vec<String>,
    messages: Vec<String>,
}

impl OrderReconciliation {
    pub fn reconcile(
        existing_entries: Vec<impl Into<String>>,
        task_refs: Vec<impl Into<String>>,
    ) -> Self {
        let existing_entries = existing_entries
            .into_iter()
            .map(Into::into)
            .collect::<Vec<_>>();
        let task_refs = task_refs.into_iter().map(Into::into).collect::<Vec<_>>();
        let mut entries = Vec::new();
        let mut messages = Vec::new();
        for task_ref in &existing_entries {
            if task_refs.contains(task_ref) {
                entries.push(task_ref.clone());
            } else {
                messages.push(format!("fixed order stale {task_ref}"));
            }
        }
        for task_ref in &task_refs {
            if !entries.contains(task_ref) {
                messages.push(format!("fixed order missing {task_ref}"));
                entries.push(task_ref.clone());
            }
        }
        Self { entries, messages }
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    pub fn messages(&self) -> &[String] {
        &self.messages
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum CoreError {
    EmptyTitle,
    InvalidTitle,
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTitle => write!(formatter, "tiber.empty_title"),
            Self::InvalidTitle => write!(formatter, "tiber.invalid_title"),
        }
    }
}

impl std::error::Error for CoreError {}

#[cfg(test)]
mod tests {
    use crate::events::{
        CiRecoveryClaimedEvent, CiRecoveryJoinedEvent, CiRecoveryParticipant, CiRecoveryTrigger,
        TiberEvent,
    };
    use crate::task::{ChecklistItem, Claim, Note, Subtask, Task};
    use eventcore_types::Event;
    use eventcore_types::StreamId;

    use super::{
        BoardSnapshot, CoreError, DependencyGraph, OrderReconciliation, TaskDependencies,
        TaskSnapshot, TaskTitle,
    };

    #[test]
    fn domain_event_type_name_is_stable() {
        assert_eq!(TiberEvent::event_type_name(), "tiber.domain_event");
    }

    #[test]
    fn ci_recovery_claim_serializes_only_the_opening_facts() {
        let event = TiberEvent::CiRecoveryClaimed(CiRecoveryClaimedEvent {
            stream_id: StreamId::try_new("tiber:ci-recovery".to_string()).expect("stream"),
            schema_version: 1,
            incident_id: "ci-123".into(),
            trigger: CiRecoveryTrigger {
                run_id: "123".into(),
                run_url: "https://example.invalid/runs/123".into(),
                failed_sha: "abcdef".into(),
                workflow: "CI".into(),
                git_ref: "refs/heads/main".into(),
            },
            owner: CiRecoveryParticipant {
                host: "host".into(),
                session: "session".into(),
            },
            lease_expires_at: 1,
        });
        let encoded = serde_json::to_value(&event).expect("serialize");
        assert_eq!(encoded["event"], "ci_recovery_claimed");
        assert_eq!(encoded["incident_id"], "ci-123");
        assert!(encoded.get("state").is_none());
        assert!(matches!(
            serde_json::from_value::<TiberEvent>(encoded).expect("deserialize"),
            TiberEvent::CiRecoveryClaimed(CiRecoveryClaimedEvent { stream_id, .. })
                if stream_id.as_ref() == "tiber:ci-recovery"
        ));
    }

    #[test]
    fn ci_recovery_join_serializes_only_new_contributed_facts() {
        let event = TiberEvent::CiRecoveryJoined(CiRecoveryJoinedEvent {
            stream_id: StreamId::try_new("tiber:ci-recovery".to_string()).expect("stream"),
            trigger: None,
            participant: Some(CiRecoveryParticipant {
                host: "helper".into(),
                session: "session-2".into(),
            }),
        });
        let encoded = serde_json::to_value(&event).expect("serialize");
        assert_eq!(encoded["event"], "ci_recovery_joined");
        assert_eq!(encoded["participant"]["session"], "session-2");
        assert!(encoded.get("state").is_none());
    }

    #[test]
    fn legacy_task_state_publication_remains_readable_but_is_explicitly_legacy() {
        let encoded = serde_json::json!({
            "event": "task_state_published",
            "stream_id": "tiber:board"
        });

        assert!(matches!(
            serde_json::from_value::<TiberEvent>(encoded).expect("deserialize legacy event"),
            TiberEvent::LegacyTaskStatePublished(_)
        ));
    }

    #[test]
    fn task_markdown_renderer_preserves_the_public_document_contract() {
        let mut task = Task::new(
            "20260805-abcd-event-source-tiber".into(),
            "Event-source Tiber".into(),
            "2026-08-05T20:00:00Z".into(),
        );
        task.blocked_by = vec!["20260805-aaaa-prerequisite".into()];
        task.blocks = vec!["20260805-eeee-follow-up".into()];
        task.tags = vec!["architecture".into(), "eventcore".into()];
        task.pr_mr_url = Some("https://example.invalid/pull/1".into());
        task.pr_mr_status = Some("open".into());
        task.claim = Some(Claim {
            host: "workstation".into(),
            session: "session-1".into(),
        });
        task.summary = "  Replace file-backed state.  ".into();
        task.context = "  Preserve existing behavior.  ".into();
        task.acceptance = vec![ChecklistItem {
            checked: true,
            text: "Events are authoritative".into(),
        }];
        task.subtasks = vec![Subtask {
            id: "model".into(),
            checked: false,
            title: "Model commands".into(),
            after: vec!["store".into()],
        }];
        task.notes = vec![Note {
            date: "2026-08-05".into(),
            text: "Implementation verified".into(),
        }];

        assert_eq!(
            task.render_markdown(),
            concat!(
                "---\n",
                "title: Event-source Tiber\n",
                "blocked_by: [20260805-aaaa-prerequisite]\n",
                "blocks: [20260805-eeee-follow-up]\n",
                "tags: [architecture, eventcore]\n",
                "pr_mr_url: https://example.invalid/pull/1\n",
                "pr_mr_status: open\n",
                "claim:\n",
                "  host: workstation\n",
                "  session: session-1\n",
                "---\n\n",
                "## Summary\n\n",
                "Replace file-backed state.\n\n",
                "## Context / Why\n\n",
                "Preserve existing behavior.\n\n",
                "## Acceptance criteria\n",
                "\n- [x] Events are authoritative",
                "\n## Subtasks\n",
                "\n- [ ] (model) Model commands — after: store",
                "\n## Notes / Log\n",
                "\n- 2026-08-05: Implementation verified\n",
            )
        );
    }

    #[test]
    fn task_title_parse_trims_input_and_rejects_empty_titles() {
        let title = TaskTitle::parse("  Ship tiber  ").expect("title should parse");

        assert_eq!(title.as_str(), "Ship tiber");
        assert_eq!(TaskTitle::parse(" \t\n"), Err(CoreError::EmptyTitle));
        assert_eq!(
            TaskTitle::parse("Ship\nTiber"),
            Err(CoreError::InvalidTitle)
        );
    }

    #[test]
    fn task_title_file_stem_slugifies_ascii_words() {
        let title = TaskTitle::parse(" Fix: API + UI handoff! ").expect("title should parse");

        assert_eq!(title.file_stem(), "fix-api-ui-handoff");
    }

    #[test]
    fn task_title_file_stem_falls_back_when_title_has_no_ascii_slug() {
        let title = TaskTitle::parse("✓✓✓").expect("title should parse");

        assert_eq!(title.file_stem(), "task");
    }

    #[test]
    fn core_error_display_is_stable_for_cli_errors() {
        assert_eq!(CoreError::EmptyTitle.to_string(), "tiber.empty_title");
        assert_eq!(CoreError::InvalidTitle.to_string(), "tiber.invalid_title");
    }

    #[test]
    fn board_snapshot_preserves_ordered_task_summaries_and_next_task() {
        let snapshot = BoardSnapshot::from_ordered_tasks(vec![
            TaskSnapshot::new("20260706-abcd-write-docs", "Write docs"),
            TaskSnapshot::new("20260706-efgh-review-docs", "Review docs"),
        ]);

        assert_eq!(
            snapshot.ordered_tasks()[0].path(),
            "20260706-abcd-write-docs"
        );
        assert_eq!(snapshot.ordered_tasks()[0].title(), "Write docs");

        assert_eq!(
            snapshot.ordered_tasks(),
            [
                TaskSnapshot::new("20260706-abcd-write-docs", "Write docs"),
                TaskSnapshot::new("20260706-efgh-review-docs", "Review docs"),
            ]
        );
        assert_eq!(
            snapshot.next_task(),
            Some(&TaskSnapshot::new("20260706-abcd-write-docs", "Write docs"))
        );
    }

    #[test]
    fn dependency_graph_reports_canonical_cycles_once() {
        let graph = DependencyGraph::from_tasks(vec![
            TaskDependencies::new("20260706-bbbb-cycle-b", vec!["20260706-aaaa-cycle-a"]),
            TaskDependencies::new("20260706-aaaa-cycle-a", vec!["20260706-bbbb-cycle-b"]),
        ]);

        assert_eq!(
            graph.cycle_messages(),
            ["cycle dependency 20260706-aaaa-cycle-a -> 20260706-bbbb-cycle-b -> 20260706-aaaa-cycle-a"]
        );
    }

    #[test]
    fn order_reconciliation_reports_stale_entries_and_appends_missing_tasks() {
        let reconciliation = OrderReconciliation::reconcile(
            vec!["20260706-aaaa-build-api", "20260706-bbbb-stale"],
            vec!["20260706-aaaa-build-api", "20260706-cccc-build-ui"],
        );

        assert_eq!(
            reconciliation.entries(),
            ["20260706-aaaa-build-api", "20260706-cccc-build-ui"]
        );
        assert_eq!(
            reconciliation.messages(),
            [
                "fixed order stale 20260706-bbbb-stale",
                "fixed order missing 20260706-cccc-build-ui"
            ]
        );
    }
}
