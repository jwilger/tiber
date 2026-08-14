use crate::protocol::{WorkerRequest, WorkerResponse, WorkerWritePrecondition};
use rustix::{
    fs::{
        self, AtFlags, FileType, FlockOperation, Mode, OFlags, RenameFlags, ResolveFlags, flock,
        openat2, renameat_with, unlinkat,
    },
    io::{Errno, Result as RustixResult},
    process::getpid,
};
use sha2::{Digest as _, Sha256};
use std::{
    env,
    fs::{File, read_link},
    io::{Read as _, Write as _, stdin, stdout},
    os::fd::OwnedFd,
};
use tiber_repository_core::{
    MAX_REPOSITORY_CONTENT_BYTES, RepositoryMutationFailureCode, Sha256Digest,
};

/// Fixed repository mount visible to the private worker.
const REPOSITORY_ROOT: &str = "/repo";
/// Safe resolution requirements applied to every repository-relative open.
const RESOLVE: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS);

/// Safely observed state needed before an exact namespace mutation.
struct RegularFileState {
    /// Digest proven through the safely opened regular-file descriptor.
    digest: Sha256Digest,
    /// Existing permission bits preserved by an exact replacement.
    mode: Mode,
}

/// Reads, validates, and executes exactly one closed worker request.
#[expect(
    clippy::implicit_return,
    clippy::pub_with_shorthand,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the private worker has one fail-closed entry point with direct error propagation"
)]
pub(crate) fn run() -> Result<(), ()> {
    let (request, content) = read_request()?;
    verify_isolation(&request)?;
    let response = match request {
        WorkerRequest::Write {
            content_digest,
            content_length,
            path,
            precondition,
            ..
        } => {
            let root = lock_repository_root()?;
            apply_write(
                &root,
                &path,
                content_length,
                &content_digest,
                precondition,
                &content,
            )
        }
        WorkerRequest::Delete {
            path, precondition, ..
        } => {
            let root = lock_repository_root()?;
            apply_delete(&root, &path, &precondition)
        }
        WorkerRequest::Reconcile { path, .. } => {
            let root = open_repository_root().map_err(|_error| ())?;
            reconcile(&root, &path)
        }
    };
    serde_json::to_writer(stdout().lock(), &response).map_err(|_error| ())
}

/// Reads one bounded JSON-header plus raw-content frame.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the single framing boundary propagates every malformed or failed read uniformly"
)]
fn read_request() -> Result<(WorkerRequest, Vec<u8>), ()> {
    let mut framed = Vec::new();
    stdin()
        .lock()
        .take(u64::try_from(MAX_REPOSITORY_CONTENT_BYTES + 16 * 1024 + 2).map_err(|_error| ())?)
        .read_to_end(&mut framed)
        .map_err(|_error| ())?;
    let Some(separator) = framed.iter().position(|byte| *byte == b'\n') else {
        return Err(());
    };
    if separator == 0 || separator > 16 * 1024 {
        return Err(());
    }
    let request =
        serde_json::from_slice(framed.get(..separator).ok_or(())?).map_err(|_error| ())?;
    let content_start = separator.checked_add(1).ok_or(())?;
    let content = framed.get(content_start..).ok_or(())?.to_vec();
    Ok((request, content))
}

/// Fails closed unless environment and network namespace isolation are active.
#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    clippy::single_call_fn,
    reason = "the closed request variants share borrowed namespace evidence and fail uniformly"
)]
fn verify_isolation(request: &WorkerRequest) -> Result<(), ()> {
    let inherited_environment_is_absent = env::vars().all(|(key, value)| match key.as_str() {
        "TIBER_SANDBOX" => value == "1",
        "PWD" => value == "/",
        _ => false,
    });
    if !inherited_environment_is_absent || env::var_os("HOME").is_some() {
        return Err(());
    }
    let parent = match request {
        WorkerRequest::Write {
            parent_network_namespace,
            ..
        }
        | WorkerRequest::Delete {
            parent_network_namespace,
            ..
        }
        | WorkerRequest::Reconcile {
            parent_network_namespace,
            ..
        } => parent_network_namespace,
    };
    let child = read_link("/proc/self/ns/net")
        .map_err(|_error| ())?
        .to_string_lossy()
        .into_owned();
    if parent.is_empty() || child == *parent {
        return Err(());
    }
    Ok(())
}

/// Checks and atomically installs one bounded write in its existing parent.
#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    reason = "the write interpreter borrows its precondition before consuming it at installation"
)]
fn apply_write(
    root: &OwnedFd,
    path: &str,
    content_length: usize,
    content_digest: &str,
    precondition: WorkerWritePrecondition,
    content: &[u8],
) -> WorkerResponse {
    if content_length > MAX_REPOSITORY_CONTENT_BYTES {
        return rejected(RepositoryMutationFailureCode::PreDispatchRejected);
    }
    let expected_content = match Sha256Digest::parse(content_digest) {
        Ok(digest) => digest,
        Err(_) => return rejected(RepositoryMutationFailureCode::PreDispatchRejected),
    };
    let (parent, name) = match open_parent(root, path) {
        Ok(parts) => parts,
        Err(_) => return rejected(RepositoryMutationFailureCode::DefinitelyNotApplied),
    };
    if content.len() != content_length || Sha256Digest::of(content) != expected_content {
        return rejected(RepositoryMutationFailureCode::PreDispatchRejected);
    }
    let existing_mode = match &precondition {
        WorkerWritePrecondition::Absent => None,
        WorkerWritePrecondition::ExactDigest(expected_hex) => {
            let Ok(expected_digest) = Sha256Digest::parse(expected_hex) else {
                return rejected(RepositoryMutationFailureCode::PreconditionNotMet);
            };
            let Some(state) = target_regular_state(&parent, &name) else {
                return rejected(RepositoryMutationFailureCode::PreconditionNotMet);
            };
            if state.digest != expected_digest {
                return rejected(RepositoryMutationFailureCode::PreconditionNotMet);
            }
            Some(state.mode)
        }
    };
    install_staged_write(&parent, &name, precondition, existing_mode, content)
}

/// Stages, atomically swaps, verifies, and finalizes one checked write.
#[expect(
    clippy::implicit_return,
    clippy::pattern_type_mismatch,
    clippy::single_call_fn,
    reason = "the staging interpreter matches a borrowed precondition before its final exact check"
)]
fn install_staged_write(
    parent: &OwnedFd,
    name: &str,
    precondition: WorkerWritePrecondition,
    existing_mode: Option<Mode>,
    content: &[u8],
) -> WorkerResponse {
    let temporary = format!(".tiber-write-{}", getpid().as_raw_pid());
    let staged_result = openat2(
        parent,
        temporary.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
        RESOLVE,
    );
    let Ok(staged_fd) = staged_result else {
        return rejected(RepositoryMutationFailureCode::DefinitelyNotApplied);
    };
    let mut staged_file = File::from(staged_fd);
    if staged_file.write_all(content).is_err()
        || existing_mode.is_some_and(|mode| fs::fchmod(&staged_file, mode).is_err())
        || staged_file.sync_all().is_err()
    {
        let _cleanup_result = unlinkat(parent, temporary.as_str(), AtFlags::empty());
        return rejected(RepositoryMutationFailureCode::DefinitelyNotApplied);
    }
    drop(staged_file);
    let renamed = match &precondition {
        WorkerWritePrecondition::Absent => renameat_with(
            parent,
            temporary.as_str(),
            parent,
            name,
            RenameFlags::NOREPLACE,
        ),
        WorkerWritePrecondition::ExactDigest(_) => renameat_with(
            parent,
            temporary.as_str(),
            parent,
            name,
            RenameFlags::EXCHANGE,
        ),
    };
    if let Err(error) = renamed {
        let _cleanup_result = unlinkat(parent, temporary.as_str(), AtFlags::empty());
        let precondition_failed = matches!(
            (&precondition, error),
            (WorkerWritePrecondition::Absent, Errno::EXIST)
                | (WorkerWritePrecondition::ExactDigest(_), Errno::NOENT)
        );
        return rejected(if precondition_failed {
            RepositoryMutationFailureCode::PreconditionNotMet
        } else {
            RepositoryMutationFailureCode::DefinitelyNotApplied
        });
    }
    if let WorkerWritePrecondition::ExactDigest(expected) = precondition {
        let exchanged_matches = Sha256Digest::parse(&expected)
            .ok()
            .is_some_and(|digest| target_digest(parent, &temporary) == Some(digest));
        if !exchanged_matches {
            return WorkerResponse::StillUnknown;
        }
        if unlinkat(parent, temporary.as_str(), AtFlags::empty()).is_err() {
            return WorkerResponse::StillUnknown;
        }
    }
    if fs::fsync(parent).is_err() {
        return WorkerResponse::StillUnknown;
    }
    WorkerResponse::Applied
}

/// Checks and removes one exact-digest regular file.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the closed delete interpreter returns its terminal or ambiguous worker response directly"
)]
fn apply_delete(root: &OwnedFd, path: &str, expected_hex: &str) -> WorkerResponse {
    let expected_digest = match Sha256Digest::parse(expected_hex) {
        Ok(digest) => digest,
        Err(_) => return rejected(RepositoryMutationFailureCode::PreDispatchRejected),
    };
    let (parent, name) = match open_parent(root, path) {
        Ok(parts) => parts,
        Err(_) => return rejected(RepositoryMutationFailureCode::DefinitelyNotApplied),
    };
    let Some(state) = target_regular_state(&parent, &name) else {
        return rejected(RepositoryMutationFailureCode::PreconditionNotMet);
    };
    if state.digest != expected_digest {
        return rejected(RepositoryMutationFailureCode::PreconditionNotMet);
    }
    let quarantine = format!(".tiber-delete-{}", getpid().as_raw_pid());
    if let Err(error) = renameat_with(
        &parent,
        name.as_str(),
        &parent,
        quarantine.as_str(),
        RenameFlags::NOREPLACE,
    ) {
        return rejected(if error == Errno::NOENT {
            RepositoryMutationFailureCode::PreconditionNotMet
        } else {
            RepositoryMutationFailureCode::DefinitelyNotApplied
        });
    }
    if target_digest(&parent, &quarantine) != Some(expected_digest) {
        if renameat_with(
            &parent,
            quarantine.as_str(),
            &parent,
            name.as_str(),
            RenameFlags::NOREPLACE,
        )
        .is_err()
        {
            return WorkerResponse::StillUnknown;
        }
        if fs::fsync(&parent).is_err() {
            return WorkerResponse::StillUnknown;
        }
        return rejected(RepositoryMutationFailureCode::PreconditionNotMet);
    }
    if unlinkat(&parent, quarantine.as_str(), AtFlags::empty()).is_err() {
        return WorkerResponse::StillUnknown;
    }
    if fs::fsync(&parent).is_err() {
        return WorkerResponse::StillUnknown;
    }
    WorkerResponse::Applied
}

/// Performs a read-only safe lookup and conservatively preserves ambiguity.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the one reconciliation projection deliberately discards lookup detail and stays unknown"
)]
fn reconcile(root: &OwnedFd, path: &str) -> WorkerResponse {
    let _lookup_result = open_parent(root, path).and_then(|(parent, name)| {
        openat2(
            parent,
            name,
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
            RESOLVE,
        )
    });
    WorkerResponse::StillUnknown
}

/// Streams the SHA-256 of one safely opened regular file.
#[expect(
    clippy::implicit_return,
    reason = "the digest projection directly maps the safely observed regular-file state"
)]
fn target_digest(parent: &OwnedFd, name: &str) -> Option<Sha256Digest> {
    target_regular_state(parent, name).map(|state| state.digest)
}

/// Streams the SHA-256 and captures permission bits of one safely opened regular file.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "any descriptor, stat, read, slice, or digest failure makes regular-file proof absent"
)]
fn target_regular_state(parent: &OwnedFd, name: &str) -> Option<RegularFileState> {
    let target = open_target(parent, name).ok()?;
    let stat = fs::fstat(&target).ok()?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return None;
    }
    let mut file = File::from(target);
    let mut hasher = Sha256::new();
    let mut buffer = [u8::default(); 8 * 1024];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(buffer.get(..read)?);
    }
    let digest = Sha256Digest::parse(&format!("{:x}", hasher.finalize())).ok()?;
    Some(RegularFileState {
        digest,
        mode: Mode::from_raw_mode(stat.st_mode),
    })
}

/// Opens one nonblocking target without following any link.
#[expect(
    clippy::implicit_return,
    clippy::single_call_fn,
    reason = "the safe target helper is the sole closed openat2 policy projection"
)]
fn open_target(parent: &OwnedFd, name: &str) -> RustixResult<OwnedFd> {
    openat2(
        parent,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        RESOLVE,
    )
}

/// Opens an existing parent beneath the fixed repository descriptor.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "the fd-relative parent lookup propagates the kernel resolution failure directly"
)]
fn open_parent(root: &OwnedFd, path: &str) -> RustixResult<(OwnedFd, String)> {
    let (parent_path, name) = path.rsplit_once('/').unwrap_or((".", path));
    if name.is_empty() {
        return Err(Errno::INVAL);
    }
    let parent = openat2(
        root,
        parent_path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        RESOLVE,
    )?;
    Ok((parent, name.to_owned()))
}

/// Opens the fixed root and holds one cooperative mutation lock on its inode.
#[expect(
    clippy::implicit_return,
    clippy::question_mark_used,
    reason = "root-open and advisory-lock failures share the worker's closed transport failure"
)]
fn lock_repository_root() -> Result<OwnedFd, ()> {
    let root = open_repository_root().map_err(|_error| ())?;
    flock(&root, FlockOperation::LockExclusive).map_err(|_error| ())?;
    Ok(root)
}

/// Opens the fixed repository root without following any configured link.
#[expect(
    clippy::implicit_return,
    reason = "the fixed root helper returns its one fail-closed open result directly"
)]
fn open_repository_root() -> RustixResult<OwnedFd> {
    fs::open(
        REPOSITORY_ROOT,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
}

/// Constructs one definitive no-application worker response.
#[expect(
    clippy::implicit_return,
    reason = "the terminal rejection helper directly wraps its stable failure code"
)]
fn rejected(code: RepositoryMutationFailureCode) -> WorkerResponse {
    WorkerResponse::Rejected { code }
}
