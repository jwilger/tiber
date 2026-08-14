#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(test)]
#[expect(
    clippy::absolute_paths,
    clippy::expect_used,
    clippy::implicit_return,
    clippy::panic,
    clippy::std_instead_of_core,
    reason = "black-box filesystem fixtures and failure assertions use fail-fast test ergonomics without entering shipping adapter code"
)]
mod tests {

    use alloc::sync::Arc;
    use rustix::fs::{FlockOperation, flock, inotify};
    use std::{
        future::Future,
        mem::MaybeUninit,
        os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink},
        path::PathBuf,
        pin::Pin,
        sync::{
            Barrier,
            atomic::{AtomicBool, Ordering},
        },
        task::{Context, Poll, Waker},
        time::{Duration, Instant},
    };
    use tempfile::TempDir;
    use tiber_repository_core::{
        ComponentScope, OwnerApprovalId, RepositoryAssignmentContext, RepositoryCapability,
        RepositoryContent, RepositoryDispatchOutcome, RepositoryError, RepositoryId,
        RepositoryMutationApproval, RepositoryMutationFailureCode, RepositoryMutationPolicy,
        RepositoryMutationProposal, RepositoryMutationProvenance, RepositoryPath,
        RepositoryReconciliationOutcome, RepositoryReconciliationState, RepositoryService as _,
        WritePrecondition, authorize_mutation,
    };
    use tiber_repository_linux::{
        LinuxRepositoryConfigurationError, LinuxRepositoryService, LinuxRepositoryServiceConfig,
        RepositoryCancellation,
    };
    use tiber_workflow_core::{
        AgentId, AssignmentEpoch, AssignmentId, AssignmentScope, AttemptNumber, ContextReceiptId,
        DeadlineMilliseconds, EffectId, HarnessError, IdempotencyKey, PolicyDecisionId, SessionId,
        WorkflowId,
    };

    #[test]
    fn absent_write_applies_exact_bytes_through_the_public_repository_service() {
        let repository = TempDir::new().expect("test repository should be created");
        std::fs::create_dir_all(repository.path().join("src"))
            .expect("test parent directory should be created");
        let service = service(repository.path());
        let mutation = authorized_write(
            "src/lib.rs",
            b"public behavior\n",
            WritePrecondition::Absent,
            1_000,
        );

        let outcome = block_on_ready(service.dispatch(mutation));

        assert!(
            matches!(outcome, Ok(RepositoryDispatchOutcome::Applied(_))),
            "unexpected dispatch outcome: {outcome:?}"
        );
        assert_eq!(
            std::fs::read(repository.path().join("src/lib.rs"))
                .expect("applied write should be readable"),
            b"public behavior\n"
        );
    }

    #[test]
    fn launch_thread_outlives_bubblewrap_parent_death_contract() {
        let repository = repository_with_src();
        let mutation = authorized_write(
            "src/launch-guard.txt",
            b"worker stayed alive\n",
            WritePrecondition::Absent,
            1_000,
        );

        let outcome = block_on_ready(service(repository.path()).dispatch(mutation));

        assert!(
            matches!(outcome, Ok(RepositoryDispatchOutcome::Applied(_))),
            "the actual spawning thread must remain alive through worker completion"
        );
    }

    #[test]
    fn exact_write_replaces_only_the_expected_regular_file() {
        let repository = repository_with_src();
        let original = b"original\n";
        std::fs::write(repository.path().join("src/lib.rs"), original)
            .expect("test file should be written");
        let mutation = authorized_write(
            "src/lib.rs",
            b"replacement\n",
            WritePrecondition::ExactDigest(tiber_repository_core::Sha256Digest::of(original)),
            1_000,
        );

        let outcome = block_on_ready(service(repository.path()).dispatch(mutation));

        assert!(
            matches!(outcome, Ok(RepositoryDispatchOutcome::Applied(_))),
            "exact write should apply: {outcome:?}"
        );
        assert_eq!(
            std::fs::read(repository.path().join("src/lib.rs")).expect("target should be readable"),
            b"replacement\n"
        );
        assert_no_worker_artifacts(repository.path().join("src"));
    }

    #[test]
    fn exact_write_preserves_existing_executable_mode() {
        let repository = repository_with_src();
        let target = repository.path().join("src/tool");
        let original = b"#!/bin/false\n";
        std::fs::write(&target, original).expect("test executable should be written");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
            .expect("test executable mode should be set");
        let mutation = authorized_write(
            "src/tool",
            b"#!/bin/true\n",
            WritePrecondition::ExactDigest(tiber_repository_core::Sha256Digest::of(original)),
            1_000,
        );

        let outcome = block_on_ready(service(repository.path()).dispatch(mutation));

        assert!(
            matches!(outcome, Ok(RepositoryDispatchOutcome::Applied(_))),
            "exact executable write should apply"
        );
        let mode = std::fs::metadata(target)
            .expect("replacement should have metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755, "exact write must preserve executable mode");
    }

    #[test]
    fn exact_delete_removes_only_the_expected_regular_file() {
        let repository = repository_with_src();
        let original = b"delete me\n";
        std::fs::write(repository.path().join("src/lib.rs"), original)
            .expect("test file should be written");
        let mutation = authorized_delete(
            "src/lib.rs",
            tiber_repository_core::Sha256Digest::of(original),
            1_000,
        );

        let outcome = block_on_ready(service(repository.path()).dispatch(mutation));

        assert!(
            matches!(outcome, Ok(RepositoryDispatchOutcome::Applied(_))),
            "exact delete should apply"
        );
        assert!(
            !repository.path().join("src/lib.rs").exists(),
            "deleted target should be absent"
        );
        assert_no_worker_artifacts(repository.path().join("src"));
    }

    #[test]
    fn delete_rollback_never_overwrites_a_newly_appearing_target() {
        const LARGE_BYTES: usize = 64_000_000;

        let repository = repository_with_src();
        let target = repository.path().join("src/raced-delete.bin");
        let original = vec![b'd'; LARGE_BYTES];
        std::fs::write(&target, &original).expect("large delete target should be written");
        let parent = repository.path().join("src");
        let interloper_target = target.clone();
        let ready = Arc::new(Barrier::new(2));
        let interloper_ready = Arc::clone(&ready);
        let interloper = std::thread::spawn(move || {
            let notifications =
                inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK)
                    .expect("inotify fixture should initialize");
            inotify::add_watch(
                &notifications,
                &parent,
                inotify::WatchFlags::CREATE | inotify::WatchFlags::MOVED_TO,
            )
            .expect("worker directory should be watchable");
            interloper_ready.wait();
            let quarantine = wait_for_worker_artifact(notifications, &parent, ".tiber-delete-");
            std::fs::write(&interloper_target, b"newly appeared\n")
                .expect("interloper target should be created");
            std::fs::write(quarantine, b"changed while quarantined\n")
                .expect("quarantine should be changed to force rollback");
        });
        ready.wait();
        let mutation = authorized_delete(
            "src/raced-delete.bin",
            tiber_repository_core::Sha256Digest::of(&original),
            30_000,
        );

        let outcome = block_on_ready(service(repository.path()).dispatch(mutation));

        interloper.join().expect("interloper should finish");
        assert!(
            matches!(outcome, Ok(RepositoryDispatchOutcome::OutcomeUnknown(_))),
            "post-mutation conflict must preserve ambiguity: {outcome:?}"
        );
        assert_eq!(
            std::fs::read(&target).expect("new target should remain readable"),
            b"newly appeared\n",
            "rollback must not overwrite a target that appeared after quarantine"
        );
    }

    #[test]
    fn precondition_mismatches_are_terminal_and_preserve_existing_bytes() {
        let repository = repository_with_src();
        let original = b"unchanged\n";
        std::fs::write(repository.path().join("src/lib.rs"), original)
            .expect("test file should be written");
        let absent_write = authorized_write(
            "src/lib.rs",
            b"must not replace\n",
            WritePrecondition::Absent,
            1_000,
        );

        let absent_outcome = block_on_ready(service(repository.path()).dispatch(absent_write));

        assert_failure(
            absent_outcome,
            RepositoryMutationFailureCode::PreconditionNotMet,
        );
        assert_eq!(
            std::fs::read(repository.path().join("src/lib.rs")).expect("target should remain"),
            original
        );
        assert_no_worker_artifacts(repository.path().join("src"));

        let wrong = tiber_repository_core::Sha256Digest::of(b"different\n");
        let exact_write = authorized_write(
            "src/lib.rs",
            b"must not replace\n",
            WritePrecondition::ExactDigest(wrong),
            1_000,
        );
        assert_failure(
            block_on_ready(service(repository.path()).dispatch(exact_write)),
            RepositoryMutationFailureCode::PreconditionNotMet,
        );
        let exact_delete = authorized_delete("src/lib.rs", wrong, 1_000);
        assert_failure(
            block_on_ready(service(repository.path()).dispatch(exact_delete)),
            RepositoryMutationFailureCode::PreconditionNotMet,
        );
        assert_eq!(
            std::fs::read(repository.path().join("src/lib.rs")).expect("target should remain"),
            original
        );
        assert_no_worker_artifacts(repository.path().join("src"));
    }

    #[test]
    fn stale_exact_targets_never_expose_write_or_delete_namespace_mutation() {
        const LARGE_BYTES: usize = 16_000_000;

        let repository = repository_with_src();
        let target = repository.path().join("src/large.bin");
        let original = vec![b'x'; LARGE_BYTES];
        std::fs::write(&target, &original).expect("large stale target should be written");
        let wrong = tiber_repository_core::Sha256Digest::of(b"not-the-large-file");

        let write_observed = Arc::new(AtomicBool::new(false));
        let write_stop = Arc::new(AtomicBool::new(false));
        let write_watcher = watch_target_length(
            target.clone(),
            u64::try_from(LARGE_BYTES).expect("fixture length should fit u64"),
            Arc::clone(&write_observed),
            Arc::clone(&write_stop),
        );
        let wrong_write = authorized_write(
            "src/large.bin",
            b"replacement",
            WritePrecondition::ExactDigest(wrong),
            10_000,
        );
        assert_failure(
            block_on_ready(service(repository.path()).dispatch(wrong_write)),
            RepositoryMutationFailureCode::PreconditionNotMet,
        );
        write_stop.store(true, Ordering::Release);
        write_watcher.join().expect("write watcher should stop");
        assert!(
            !write_observed.load(Ordering::Acquire),
            "stale exact write must not expose staged bytes at the target"
        );

        let delete_observed = Arc::new(AtomicBool::new(false));
        let delete_stop = Arc::new(AtomicBool::new(false));
        let delete_watcher = watch_target_length(
            target.clone(),
            u64::try_from(LARGE_BYTES).expect("fixture length should fit u64"),
            Arc::clone(&delete_observed),
            Arc::clone(&delete_stop),
        );
        let wrong_delete = authorized_delete("src/large.bin", wrong, 10_000);
        assert_failure(
            block_on_ready(service(repository.path()).dispatch(wrong_delete)),
            RepositoryMutationFailureCode::PreconditionNotMet,
        );
        delete_stop.store(true, Ordering::Release);
        delete_watcher.join().expect("delete watcher should stop");
        assert!(
            !delete_observed.load(Ordering::Acquire),
            "stale exact delete must not make the target disappear"
        );
        assert_eq!(
            std::fs::metadata(&target)
                .expect("stale target must remain")
                .len(),
            u64::try_from(LARGE_BYTES).expect("fixture length should fit u64")
        );
    }

    #[test]
    fn exact_operations_reject_special_targets_without_namespace_mutation() {
        let repository = repository_with_src();
        let target = repository.path().join("src/special");
        std::fs::create_dir_all(&target).expect("special directory target should be created");
        let digest = tiber_repository_core::Sha256Digest::of(b"not-a-directory");

        assert_failure(
            block_on_ready(service(repository.path()).dispatch(authorized_write(
                "src/special",
                b"replacement",
                WritePrecondition::ExactDigest(digest),
                1_000,
            ))),
            RepositoryMutationFailureCode::PreconditionNotMet,
        );
        assert_failure(
            block_on_ready(service(repository.path()).dispatch(authorized_delete(
                "src/special",
                digest,
                1_000,
            ))),
            RepositoryMutationFailureCode::PreconditionNotMet,
        );
        assert!(target.is_dir(), "special target must remain a directory");
    }

    #[test]
    fn fd_relative_resolution_rejects_symlink_escape_and_missing_parents() {
        let repository = TempDir::new().expect("test repository should be created");
        let outside = TempDir::new().expect("outside directory should be created");
        symlink(outside.path(), repository.path().join("escape"))
            .expect("test symlink should be created");
        let escape = authorized_write(
            "escape/outside.txt",
            b"must stay contained\n",
            WritePrecondition::Absent,
            1_000,
        );

        assert_failure(
            block_on_ready(service(repository.path()).dispatch(escape)),
            RepositoryMutationFailureCode::DefinitelyNotApplied,
        );
        assert!(
            !outside.path().join("outside.txt").exists(),
            "symlink escape must not create an outside file"
        );

        let missing = authorized_write(
            "missing/parent/file.txt",
            b"must not create parents\n",
            WritePrecondition::Absent,
            1_000,
        );
        assert_failure(
            block_on_ready(service(repository.path()).dispatch(missing)),
            RepositoryMutationFailureCode::DefinitelyNotApplied,
        );
        assert!(
            !repository.path().join("missing").exists(),
            "the worker must not synthesize missing parents"
        );
    }

    #[test]
    fn relative_configuration_is_rejected_before_any_process_launch() {
        let result = LinuxRepositoryServiceConfig::new(
            repository_value(RepositoryId::parse, "repo-1"),
            PathBuf::from("relative-repository"),
            test_bubblewrap(),
            PathBuf::from(env!("CARGO_BIN_EXE_tiber-repository-worker")),
        );

        assert!(matches!(
            result,
            Err(LinuxRepositoryConfigurationError::PathNotAbsolute)
        ));
    }

    #[test]
    fn service_rejects_authority_for_a_different_repository_before_launch() {
        let repository = repository_with_src();
        let marker = repository.path().join("bwrap-launched");
        let fake_bwrap = marker_script(repository.path(), &marker);
        let mutation = authorized_write_for_repository(
            "repo-2",
            "src/lib.rs",
            b"wrong repository\n",
            WritePrecondition::Absent,
            1_000,
        );

        let service = LinuxRepositoryService::new(config_with_paths(
            "repo-1",
            repository.path(),
            fake_bwrap,
            PathBuf::from(env!("CARGO_BIN_EXE_tiber-repository-worker")),
        ));
        assert_failure(
            block_on_ready(service.dispatch(mutation)),
            RepositoryMutationFailureCode::PreDispatchRejected,
        );
        assert!(
            !repository.path().join("src/lib.rs").exists(),
            "repository identity mismatch must be zero-touch"
        );
        assert!(
            !marker.exists(),
            "repository mismatch must not launch bwrap"
        );
    }

    #[test]
    fn prelaunch_cancellation_is_definitive_and_does_not_touch_the_repository() {
        let repository = repository_with_src();
        let cancellation = RepositoryCancellation::default();
        cancellation.cancel();
        let service =
            LinuxRepositoryService::with_cancellation(config(repository.path()), cancellation);
        let mutation = authorized_write(
            "src/lib.rs",
            b"cancelled\n",
            WritePrecondition::Absent,
            1_000,
        );

        assert_failure(
            block_on_ready(service.dispatch(mutation)),
            RepositoryMutationFailureCode::PreDispatchRejected,
        );
        assert!(
            !repository.path().join("src/lib.rs").exists(),
            "prelaunch cancellation must not touch the target"
        );
    }

    #[test]
    fn dispatch_budget_bounds_lock_wait_and_keeps_second_target_untouched() {
        let repository = repository_with_src();
        let stalled_worker = stalled_worker_script(repository.path());
        let ready_marker = repository.path().join("descendant.ready");
        let status = repository.path().join("bwrap-status.jsonl");
        let status_bwrap = status_bwrap_script(repository.path(), "status-bwrap", &status);
        let service = Arc::new(LinuxRepositoryService::new(config_with_paths(
            "repo-1",
            repository.path(),
            status_bwrap,
            stalled_worker,
        )));
        let first = authorized_write(
            "src/first.txt",
            &vec![b'x'; tiber_repository_core::MAX_REPOSITORY_CONTENT_BYTES],
            WritePrecondition::Absent,
            500,
        );
        let first_service = Arc::clone(&service);
        let first_thread = std::thread::spawn(move || {
            matches!(
                block_on_ready(first_service.dispatch(first)),
                Ok(RepositoryDispatchOutcome::OutcomeUnknown(_))
            )
        });
        let sandbox_processes = capture_sandbox_processes(&status, &ready_marker);
        let second = authorized_write(
            "src/blocked.txt",
            b"must not wait past budget",
            WritePrecondition::Absent,
            10,
        );
        let started = Instant::now();

        let second_outcome = block_on_ready(service.dispatch(second));

        assert_failure(
            second_outcome,
            RepositoryMutationFailureCode::PreDispatchRejected,
        );
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "lock wait must obey the second dispatch budget"
        );
        assert!(
            !repository.path().join("src/blocked.txt").exists(),
            "budget expiry before launch must be zero-touch"
        );
        let first_was_unknown = first_thread.join().expect("first dispatch should finish");
        assert!(
            first_was_unknown,
            "launched stalled dispatch should preserve ambiguity"
        );
        assert_processes_reaped(&sandbox_processes);
    }

    #[test]
    fn separate_services_serialize_exact_write_and_delete_with_the_root_lock() {
        let repository = repository_with_src();
        let target = repository.path().join("src/shared.txt");
        let original = b"shared original\n";
        std::fs::write(&target, original).expect("shared target should be written");
        let root_lock = std::fs::File::open(repository.path())
            .expect("test repository root should be openable");
        flock(&root_lock, FlockOperation::LockExclusive)
            .expect("test should hold the repository mutation lock");

        let status_one = repository.path().join("bwrap-one.jsonl");
        let status_two = repository.path().join("bwrap-two.jsonl");
        let service_one = service_with_bwrap(
            repository.path(),
            status_bwrap_script(repository.path(), "status-bwrap-one", &status_one),
        );
        let service_two = service_with_bwrap(
            repository.path(),
            status_bwrap_script(repository.path(), "status-bwrap-two", &status_two),
        );
        let start = Arc::new(Barrier::new(3));
        let start_one = Arc::clone(&start);
        let first = authorized_write(
            "src/shared.txt",
            b"first replacement\n",
            WritePrecondition::ExactDigest(tiber_repository_core::Sha256Digest::of(original)),
            5_000,
        );
        let first_thread = std::thread::spawn(move || {
            start_one.wait();
            classify_dispatch(block_on_ready(service_one.dispatch(first)))
        });
        let start_two = Arc::clone(&start);
        let second = authorized_delete(
            "src/shared.txt",
            tiber_repository_core::Sha256Digest::of(original),
            5_000,
        );
        let second_thread = std::thread::spawn(move || {
            start_two.wait();
            classify_dispatch(block_on_ready(service_two.dispatch(second)))
        });
        start.wait();

        wait_for_flock_waiters(repository.path(), 2);
        assert_eq!(
            std::fs::read(&target).expect("locked target should be readable"),
            original,
            "neither service may precheck or mutate while the root lock is held"
        );
        flock(&root_lock, FlockOperation::Unlock).expect("test root lock should release");

        let outcomes = [
            first_thread.join().expect("first service should finish"),
            second_thread.join().expect("second service should finish"),
        ];
        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_none()).count(),
            1,
            "exact mutations must have one linearized application: {outcomes:?}"
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| {
                    **outcome == Some(RepositoryMutationFailureCode::PreconditionNotMet)
                })
                .count(),
            1,
            "the later exact mutation must observe the committed digest: {outcomes:?}"
        );
    }

    #[test]
    fn deadline_while_waiting_for_root_lock_is_unknown_untouched_and_reaped() {
        let repository = repository_with_src();
        let target = repository.path().join("src/lock-timeout.txt");
        let root_lock = std::fs::File::open(repository.path())
            .expect("test repository root should be openable");
        flock(&root_lock, FlockOperation::LockExclusive)
            .expect("test should hold the repository mutation lock");
        let status = repository.path().join("lock-timeout-bwrap.jsonl");
        let service = service_with_bwrap(
            repository.path(),
            status_bwrap_script(repository.path(), "lock-timeout-bwrap", &status),
        );
        let capture_repository = repository.path().to_path_buf();
        let capture_status = status.clone();
        let process_capture = std::thread::spawn(move || {
            wait_for_flock_waiters(&capture_repository, 1);
            capture_sandbox_process_tree(&capture_status, 2)
        });
        let mutation = authorized_write(
            "src/lock-timeout.txt",
            b"must not apply while locked\n",
            WritePrecondition::Absent,
            500,
        );
        let started = Instant::now();

        let outcome = block_on_ready(service.dispatch(mutation));

        let sandbox_processes = process_capture
            .join()
            .expect("lock waiter process capture should finish");
        assert!(matches!(
            outcome,
            Ok(RepositoryDispatchOutcome::OutcomeUnknown(_))
        ));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "root-lock wait must remain within the dispatch budget"
        );
        assert!(!target.exists(), "blocked worker must not touch its target");
        assert_processes_reaped(&sandbox_processes);
        flock(&root_lock, FlockOperation::Unlock).expect("test root lock should release");
    }

    #[test]
    fn reaped_bwrap_leader_leaves_no_contained_descendant_without_stale_pid_kill() {
        let repository = repository_with_src();
        let status = repository.path().join("exiting-worker-bwrap.jsonl");
        let ready = repository.path().join("descendant.ready");
        let release = repository.path().join("release-worker");
        let worker = exiting_worker_script(repository.path());
        let service = LinuxRepositoryService::new(config_with_paths(
            "repo-1",
            repository.path(),
            status_bwrap_script(repository.path(), "exiting-worker-bwrap", &status),
            worker,
        ));
        let mutation = authorized_write(
            "src/never-applied.txt",
            &vec![b'x'; tiber_repository_core::MAX_REPOSITORY_CONTENT_BYTES],
            WritePrecondition::Absent,
            2_000,
        );
        let dispatch = std::thread::spawn(move || {
            matches!(
                block_on_ready(service.dispatch(mutation)),
                Ok(RepositoryDispatchOutcome::OutcomeUnknown(_))
            )
        });
        let sandbox_processes = capture_sandbox_processes(&status, &ready);
        std::fs::write(release, b"release\n").expect("test worker should be released");

        assert!(
            dispatch
                .join()
                .expect("exiting worker dispatch should finish"),
            "partial transfer followed by worker exit must preserve ambiguity"
        );
        assert_processes_reaped(&sandbox_processes);
        assert!(
            !repository.path().join("src/never-applied.txt").exists(),
            "private test worker must not mutate the requested target"
        );
    }

    #[test]
    fn postlaunch_deadline_is_bounded_unknown_and_reaps_the_worker_group() {
        let repository = repository_with_src();
        let stalled_worker = stalled_worker_script(repository.path());
        let ready_marker = repository.path().join("descendant.ready");
        let status = repository.path().join("bwrap-status.jsonl");
        let status_bwrap = status_bwrap_script(repository.path(), "status-bwrap", &status);
        let capture_status = status.clone();
        let process_capture =
            std::thread::spawn(move || capture_sandbox_processes(&capture_status, &ready_marker));
        let mutation = authorized_write(
            "src/lib.rs",
            &vec![b'x'; tiber_repository_core::MAX_REPOSITORY_CONTENT_BYTES],
            WritePrecondition::Absent,
            500,
        );
        let service = LinuxRepositoryService::new(config_with_paths(
            "repo-1",
            repository.path(),
            status_bwrap,
            stalled_worker,
        ));
        let started = Instant::now();

        let outcome = block_on_ready(service.dispatch(mutation));

        assert!(matches!(
            outcome,
            Ok(RepositoryDispatchOutcome::OutcomeUnknown(_))
        ));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "deadline must bound blocked stdin transfer"
        );
        let sandbox_processes = process_capture
            .join()
            .expect("host process capture should finish");
        assert_processes_reaped(&sandbox_processes);
        assert!(
            !repository.path().join("src/lib.rs").exists(),
            "stalled worker must not touch the target"
        );
    }

    #[test]
    fn postlaunch_cancellation_returns_unknown_and_reaps_the_worker_group() {
        let repository = repository_with_src();
        let stalled_worker = stalled_worker_script(repository.path());
        let ready_marker = repository.path().join("descendant.ready");
        let status = repository.path().join("bwrap-status.jsonl");
        let status_bwrap = status_bwrap_script(repository.path(), "status-bwrap", &status);
        let cancellation = RepositoryCancellation::default();
        let canceller = cancellation.clone();
        let service = Arc::new(LinuxRepositoryService::with_cancellation(
            config_with_paths("repo-1", repository.path(), status_bwrap, stalled_worker),
            cancellation,
        ));
        let mutation = authorized_write(
            "src/lib.rs",
            &vec![b'x'; tiber_repository_core::MAX_REPOSITORY_CONTENT_BYTES],
            WritePrecondition::Absent,
            2_000,
        );
        let dispatched = Arc::clone(&service);
        let dispatch_thread = std::thread::spawn(move || {
            matches!(
                block_on_ready(dispatched.dispatch(mutation)),
                Ok(RepositoryDispatchOutcome::OutcomeUnknown(_))
            )
        });
        let sandbox_processes = capture_sandbox_processes(&status, &ready_marker);
        let started = Instant::now();
        canceller.cancel();

        let outcome_was_unknown = dispatch_thread
            .join()
            .expect("dispatch thread should finish");

        assert!(outcome_was_unknown);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "cancellation must unblock worker transfer promptly"
        );
        assert_processes_reaped(&sandbox_processes);
    }

    #[test]
    fn repository_mismatched_reconciliation_fails_before_bwrap_launch() {
        let repository = repository_with_src();
        let marker = repository.path().join("reconcile-bwrap-launched");
        let fake_bwrap = marker_script(repository.path(), &marker);
        let reconciliation = match authorized_write_for_repository(
            "repo-2",
            "src/lib.rs",
            b"never dispatched",
            WritePrecondition::Absent,
            1_000,
        )
        .into_ambiguity()
        {
            RepositoryDispatchOutcome::OutcomeUnknown(reconciliation) => reconciliation,
            RepositoryDispatchOutcome::Applied(_) => {
                panic!("direct ambiguity conversion cannot produce applied")
            }
        };
        let service = LinuxRepositoryService::new(config_with_paths(
            "repo-1",
            repository.path(),
            fake_bwrap,
            PathBuf::from(env!("CARGO_BIN_EXE_tiber-repository-worker")),
        ));

        let outcome = block_on_ready(service.reconcile(reconciliation));

        assert!(outcome.is_err(), "wrong-repository reconcile must fail");
        assert!(
            !marker.exists(),
            "wrong-repository reconcile must not launch"
        );
    }

    #[test]
    fn matching_repository_reconciliation_is_read_only_and_still_unknown() {
        let repository = repository_with_src();
        let reconciliation = ambiguity_for_repository("repo-1");
        let before = std::fs::read_dir(repository.path().join("src"))
            .expect("repository should be readable")
            .count();

        let outcome = block_on_ready(service(repository.path()).reconcile(reconciliation));

        assert!(matches!(
            outcome,
            Ok(RepositoryReconciliationOutcome::StillUnknown(receipt))
                if receipt.state() == RepositoryReconciliationState::StillUnknown
        ));
        let after = std::fs::read_dir(repository.path().join("src"))
            .expect("repository should remain readable")
            .count();
        assert_eq!(before, after, "read-only reconciliation must not mutate");
    }

    fn repository_with_src() -> TempDir {
        let repository = TempDir::new().expect("test repository should be created");
        std::fs::create_dir_all(repository.path().join("src"))
            .expect("test parent directory should be created");
        repository
    }

    fn config(repository: &std::path::Path) -> LinuxRepositoryServiceConfig {
        config_with_paths(
            "repo-1",
            repository,
            test_bubblewrap(),
            PathBuf::from(env!("CARGO_BIN_EXE_tiber-repository-worker")),
        )
    }

    fn config_with_paths(
        repository_id: &str,
        repository: &std::path::Path,
        bubblewrap: PathBuf,
        worker: PathBuf,
    ) -> LinuxRepositoryServiceConfig {
        LinuxRepositoryServiceConfig::new(
            repository_value(RepositoryId::parse, repository_id),
            repository.to_path_buf(),
            bubblewrap,
            worker,
        )
        .expect("absolute adapter paths should parse")
    }

    fn service(repository: &std::path::Path) -> LinuxRepositoryService {
        LinuxRepositoryService::new(config(repository))
    }

    fn test_bubblewrap() -> PathBuf {
        std::env::var_os("TIBER_TEST_BWRAP")
            .map(PathBuf::from)
            .expect("pinned Nix shell should export TIBER_TEST_BWRAP")
    }

    fn test_bash() -> PathBuf {
        std::env::var_os("TIBER_TEST_BASH")
            .map(PathBuf::from)
            .expect("pinned Nix shell should export TIBER_TEST_BASH")
    }

    fn executable_script(directory: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = directory.join(name);
        let source = format!("#!{}\n{body}\n", test_bash().display());
        std::fs::write(&path, source).expect("test executable script should be written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .expect("test executable script mode should be set");
        path
    }

    fn marker_script(directory: &std::path::Path, marker: &std::path::Path) -> PathBuf {
        executable_script(
            directory,
            "fake-bwrap",
            &format!("printf launched > '{}'", marker.display()),
        )
    }

    fn stalled_worker_script(directory: &std::path::Path) -> PathBuf {
        let bash = test_bash();
        executable_script(
            directory,
            "stalled-worker",
            &format!(
                "'{bash}' -c 'while :; do :; done' &\nprintf ready > /repo/descendant.ready\nwhile :; do :; done",
                bash = bash.display()
            ),
        )
    }

    #[expect(
        clippy::single_call_fn,
        reason = "the reaped-leader regression has one synchronized private worker fixture"
    )]
    fn exiting_worker_script(directory: &std::path::Path) -> PathBuf {
        let bash = test_bash();
        executable_script(
            directory,
            "exiting-worker",
            &format!(
                "'{bash}' -c 'while :; do :; done' &\nprintf ready > /repo/descendant.ready\nwhile [[ ! -e /repo/release-worker ]]; do :; done",
                bash = bash.display()
            ),
        )
    }

    fn status_bwrap_script(
        directory: &std::path::Path,
        name: &str,
        status: &std::path::Path,
    ) -> PathBuf {
        executable_script(
            directory,
            name,
            &format!(
                "exec 3>'{status}'\nexec '{bwrap}' --json-status-fd 3 \"$@\"",
                status = status.display(),
                bwrap = test_bubblewrap().display()
            ),
        )
    }

    fn service_with_bwrap(
        repository: &std::path::Path,
        bubblewrap: PathBuf,
    ) -> LinuxRepositoryService {
        LinuxRepositoryService::new(config_with_paths(
            "repo-1",
            repository,
            bubblewrap,
            PathBuf::from(env!("CARGO_BIN_EXE_tiber-repository-worker")),
        ))
    }

    fn wait_for_flock_waiters(repository: &std::path::Path, expected: usize) {
        let inode = std::fs::metadata(repository)
            .expect("repository root metadata should be readable")
            .ino();
        let suffix = format!(":{inode}");
        let started = Instant::now();
        loop {
            let locks = std::fs::read_to_string("/proc/locks")
                .expect("kernel lock table should be readable");
            let waiters = locks
                .lines()
                .filter(|line| line.contains("FLOCK") && line.contains(" -> "))
                .filter(|line| {
                    line.split_whitespace()
                        .any(|field| field.ends_with(&suffix))
                })
                .count();
            if waiters >= expected {
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "expected {expected} root-lock waiters, observed {waiters}: {locks}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[expect(
        clippy::single_call_fn,
        reason = "the rollback regression observes the private quarantine namespace boundary once"
    )]
    fn wait_for_worker_artifact(
        notifications: std::os::fd::OwnedFd,
        directory: &std::path::Path,
        prefix: &str,
    ) -> PathBuf {
        let started = Instant::now();
        let mut buffer = [MaybeUninit::uninit(); 512];
        let mut events = inotify::Reader::new(notifications, &mut buffer);
        loop {
            match events.next() {
                Ok(event) => {
                    let Some(name) = event.file_name() else {
                        continue;
                    };
                    if name.to_string_lossy().starts_with(prefix) {
                        return directory.join(name.to_string_lossy().as_ref());
                    }
                }
                Err(rustix::io::Errno::AGAIN) => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("inotify artifact observation failed: {error}"),
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "worker artifact with prefix {prefix} should appear"
            );
            std::thread::yield_now();
        }
    }

    fn wait_for_path(path: &std::path::Path) {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(2))
            .expect("test wait deadline should fit");
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "synchronized worker marker should appear"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn capture_sandbox_processes(
        status: &std::path::Path,
        ready: &std::path::Path,
    ) -> Vec<PathBuf> {
        wait_for_path(ready);
        capture_sandbox_process_tree(status, 3)
    }

    fn capture_sandbox_process_tree(status: &std::path::Path, minimum: usize) -> Vec<PathBuf> {
        wait_for_path(status);
        let status_line = std::fs::read_to_string(status)
            .expect("bwrap host status should be readable")
            .lines()
            .next()
            .expect("bwrap should report its sandbox child")
            .to_owned();
        let parsed_status: serde_json::Value =
            serde_json::from_str(&status_line).expect("bwrap status should be valid JSON");
        let root = parsed_status
            .get("child-pid")
            .and_then(serde_json::Value::as_u64)
            .expect("bwrap should report a host-visible child-pid");
        let mut processes = vec![PathBuf::from(format!("/proc/{root}"))];
        let mut index: usize = 0;
        while let Some(process) = processes.get(index) {
            let pid = process
                .file_name()
                .expect("process path should have a pid")
                .to_string_lossy();
            let children =
                std::fs::read_to_string(process.join("task").join(&*pid).join("children"))
                    .expect("sandbox process children should be readable");
            for child_pid in children.split_whitespace() {
                let child_path = PathBuf::from(format!("/proc/{child_pid}"));
                if !processes.contains(&child_path) {
                    processes.push(child_path);
                }
            }
            index = index.checked_add(1).expect("process index should fit");
        }
        assert!(
            processes.len() >= minimum,
            "host proof must capture at least {minimum} sandbox processes: {processes:?}"
        );
        processes
    }

    fn assert_processes_reaped(processes: &[PathBuf]) {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(1))
            .expect("test reap deadline should fit");
        while processes.iter().any(|process| process.exists()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            processes.iter().all(|process| !process.exists()),
            "host-visible sandbox tree must be reaped: {:?}",
            processes
                .iter()
                .filter(|process| process.exists())
                .collect::<Vec<_>>()
        );
    }

    fn watch_target_length(
        target: PathBuf,
        expected_length: u64,
        observed: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<()> {
        let ready = Arc::new(AtomicBool::new(false));
        let watcher_ready = Arc::clone(&ready);
        let watcher = std::thread::spawn(move || {
            watcher_ready.store(true, Ordering::Release);
            while !stop.load(Ordering::Acquire) {
                let changed = std::fs::metadata(&target)
                    .map_or(true, |metadata| metadata.len() != expected_length);
                if changed {
                    observed.store(true, Ordering::Release);
                    break;
                }
                std::thread::yield_now();
            }
        });
        while !ready.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        watcher
    }

    #[expect(
        clippy::single_call_fn,
        reason = "the matching-repository reconciliation fixture keeps its opaque conversion readable"
    )]
    fn ambiguity_for_repository(
        repository_id: &str,
    ) -> tiber_repository_core::RepositoryReconciliation {
        match authorized_write_for_repository(
            repository_id,
            "src/lib.rs",
            b"ambiguous",
            WritePrecondition::Absent,
            1_000,
        )
        .into_ambiguity()
        {
            RepositoryDispatchOutcome::OutcomeUnknown(reconciliation) => reconciliation,
            RepositoryDispatchOutcome::Applied(_) => {
                panic!("direct ambiguity conversion cannot produce applied")
            }
        }
    }

    fn assert_failure(
        outcome: Result<
            RepositoryDispatchOutcome,
            tiber_repository_core::RepositoryMutationFailure,
        >,
        expected: RepositoryMutationFailureCode,
    ) {
        match outcome {
            Err(failure) => assert_eq!(failure.error(), expected),
            Ok(other) => panic!("expected definitive failure, got {other:?}"),
        }
    }

    fn classify_dispatch(
        outcome: Result<
            RepositoryDispatchOutcome,
            tiber_repository_core::RepositoryMutationFailure,
        >,
    ) -> Option<RepositoryMutationFailureCode> {
        match outcome {
            Ok(RepositoryDispatchOutcome::Applied(_)) => None,
            Err(failure) => Some(failure.error()),
            Ok(RepositoryDispatchOutcome::OutcomeUnknown(_)) => {
                panic!("cooperative exact mutation should have a terminal outcome")
            }
        }
    }

    fn assert_no_worker_artifacts(directory: PathBuf) {
        let artifacts = std::fs::read_dir(directory)
            .expect("test directory should be readable")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".tiber-"))
            .count();
        assert_eq!(
            artifacts, 0,
            "completed operations must clean staging files"
        );
    }

    fn authorized_write(
        path: &str,
        content: &[u8],
        precondition: WritePrecondition,
        deadline_milliseconds: u64,
    ) -> tiber_repository_core::AuthorizedRepositoryMutation {
        authorized_write_for_repository(
            "repo-1",
            path,
            content,
            precondition,
            deadline_milliseconds,
        )
    }

    fn authorized_write_for_repository(
        repository_id: &str,
        path: &str,
        content: &[u8],
        precondition: WritePrecondition,
        deadline_milliseconds: u64,
    ) -> tiber_repository_core::AuthorizedRepositoryMutation {
        let provenance = provenance(deadline_milliseconds);
        let repository = repository_value(RepositoryId::parse, repository_id);
        let assignment = RepositoryAssignmentContext::new(
            provenance.clone(),
            repository.clone(),
            ComponentScope::repository_root(),
        );
        let policy = RepositoryMutationPolicy::new(
            assignment.clone(),
            [RepositoryCapability::MutateRepository],
        );
        let proposal = RepositoryMutationProposal::write(
            provenance,
            repository,
            repository_value(RepositoryPath::parse, path),
            RepositoryContent::from_bytes(content).expect("bounded test content should parse"),
            precondition,
        );
        let approval = RepositoryMutationApproval::issue(
            repository_value(OwnerApprovalId::parse, "approval-1"),
            &proposal,
            &policy,
        );
        authorize_mutation(proposal, &assignment, &policy, Some(approval))
            .expect("test mutation should authorize")
    }

    fn authorized_delete(
        path: &str,
        precondition: tiber_repository_core::Sha256Digest,
        deadline_milliseconds: u64,
    ) -> tiber_repository_core::AuthorizedRepositoryMutation {
        let provenance = provenance(deadline_milliseconds);
        let repository = repository_value(RepositoryId::parse, "repo-1");
        let assignment = RepositoryAssignmentContext::new(
            provenance.clone(),
            repository.clone(),
            ComponentScope::repository_root(),
        );
        let policy = RepositoryMutationPolicy::new(
            assignment.clone(),
            [RepositoryCapability::MutateRepository],
        );
        let proposal = RepositoryMutationProposal::delete(
            provenance,
            repository,
            repository_value(RepositoryPath::parse, path),
            precondition,
        );
        let approval = RepositoryMutationApproval::issue(
            repository_value(OwnerApprovalId::parse, "approval-delete"),
            &proposal,
            &policy,
        );
        authorize_mutation(proposal, &assignment, &policy, Some(approval))
            .expect("test deletion should authorize")
    }

    fn provenance(deadline_milliseconds: u64) -> RepositoryMutationProvenance {
        RepositoryMutationProvenance::new(
            workflow_value(SessionId::parse, "session-1"),
            workflow_value(AgentId::parse, "agent-1"),
            workflow_value(WorkflowId::parse, "workflow-1"),
            workflow_value(AssignmentId::parse, "assignment-1"),
            workflow_value(AssignmentScope::parse, "repository:src"),
            AssignmentEpoch::FIRST,
            AttemptNumber::FIRST,
            workflow_value(ContextReceiptId::parse, "context-1"),
            workflow_value(PolicyDecisionId::parse, "policy-1"),
            workflow_value(EffectId::parse, "effect-1"),
            workflow_value(IdempotencyKey::parse, "idem-1"),
            DeadlineMilliseconds::parse(deadline_milliseconds).expect("test deadline should parse"),
        )
    }

    fn workflow_value<T>(parse: impl FnOnce(&str) -> Result<T, HarnessError>, value: &str) -> T {
        parse(value).expect("workflow test fixture should parse")
    }

    fn repository_value<T>(
        parse: impl FnOnce(&str) -> Result<T, RepositoryError>,
        value: &str,
    ) -> T {
        parse(value).expect("repository test fixture should parse")
    }

    fn block_on_ready<T>(mut future: Pin<Box<dyn Future<Output = T> + Send + '_>>) -> T {
        let mut context = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("Linux adapter futures should complete synchronously"),
        }
    }
}
