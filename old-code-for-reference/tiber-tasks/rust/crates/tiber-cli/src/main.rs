use std::convert::Infallible;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{self, ExitCode};
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{error::ErrorKind, ArgGroup, Args, CommandFactory, Parser, Subcommand};

const ESCAPED_SUBTASK_TITLE_PREFIX: &str = "\0tiber-subtask-title\0";
const EXPLICIT_INSTALL_TARGET_PREFIX: &str = "\0tiber-install-target\0";
const BUILD_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "+source.",
    env!("TIBER_SOURCE_FINGERPRINT")
);

#[derive(Clone)]
struct CommaSeparatedValues(Vec<String>);

impl CommaSeparatedValues {
    fn into_values(self) -> Vec<String> {
        self.0
    }
}

impl FromStr for CommaSeparatedValues {
    type Err = Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect(),
        ))
    }
}

#[derive(Clone)]
struct SubtaskTitle(String);

impl SubtaskTitle {
    fn into_value(self) -> String {
        self.0
    }
}

impl FromStr for SubtaskTitle {
    type Err = Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(
            value
                .strip_prefix(ESCAPED_SUBTASK_TITLE_PREFIX)
                .unwrap_or(value)
                .to_string(),
        ))
    }
}

#[derive(Clone)]
struct InstallTargetDir(String);

impl InstallTargetDir {
    fn into_value(self) -> String {
        self.0
    }
}

impl FromStr for InstallTargetDir {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some(explicit) = value.strip_prefix(EXPLICIT_INSTALL_TARGET_PREFIX) {
            return Ok(Self(explicit.to_string()));
        }
        if matches!(value, "--dry-run" | "--apply") {
            return Err(format!(
                "{value} is an install mode; use --target-dir={value} for that literal path"
            ));
        }
        Ok(Self(value.to_string()))
    }
}

#[derive(Parser)]
#[command(
    name = "tiber",
    about = "Repository-local task board",
    version = BUILD_VERSION
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize the Tiber event store.
    Init,
    /// Synchronize local event state with origin/tiber.
    Sync,
    /// Show Codex sandbox approval guidance.
    CodexSandbox(CodexSandboxArgs),
    /// Run the dashboard with count-neutral backlog priority reordering.
    Dashboard(DashboardArgs),
    /// Run the MCP server.
    Mcp(McpArgs),
    /// Coordinate repository-wide pushed-CI failure recovery.
    CiRecovery(CiRecoveryArgs),
    /// Preview or install the bundled tiber launcher.
    InstallBin(InstallBinArgs),
    /// Create a backlog task.
    Create {
        /// Task title.
        #[arg(allow_hyphen_values = true)]
        title: String,
    },
    /// Show a task document.
    Show(TaskRefArgs),
    /// Show task metadata.
    Metadata(TaskRefArgs),
    /// List open tasks or tasks in one status.
    List(ListArgs),
    /// Search task titles and descriptions across all statuses.
    Search {
        /// Case-insensitive text to find.
        query: String,
    },
    /// Print the next available task.
    Next,
    /// Move a task to a status.
    Transition(TransitionArgs),
    /// Reorder a task before another task.
    Prioritize(PrioritizeArgs),
    /// Add a task dependency relation.
    Link(RelationArgs),
    /// Remove a task dependency relation.
    Unlink(RelationArgs),
    /// Edit task subtasks.
    Subtask(SubtaskArgs),
    /// Update structured task fields.
    Update(UpdateArgs),
    /// Edit task acceptance criteria.
    Acceptance(AcceptanceArgs),
    /// Append a dated task note.
    Note(NoteArgs),
    /// Validate and repair safe task-board issues.
    Validate(ValidateArgs),
    /// Close tasks from Git trailers.
    CloseFromTrailers,
    /// Scaffold repository integration.
    Scaffold(ScaffoldArgs),
}

#[derive(Args)]
struct CiRecoveryArgs {
    #[command(subcommand)]
    command: CiRecoveryCommand,
}

#[derive(Subcommand)]
enum CiRecoveryCommand {
    /// Claim a terminal failed pushed-CI run or join as a waiter.
    Claim(CiRecoveryClaimArgs),
    /// Verify that this session still owns the fenced recovery lease.
    AssertOwner(CiRecoveryOwnerArgs),
    /// Transfer recovery ownership to another session.
    Transfer(CiRecoveryTransferArgs),
    /// Take over an expired recovery lease.
    Takeover(CiRecoveryOwnerArgs),
    /// Assign bounded recovery work to a joined helper.
    Assign(CiRecoveryAssignArgs),
    /// Report the result of an assigned recovery task.
    Report(CiRecoveryReportArgs),
    /// Renew the active owner's recovery lease.
    Heartbeat(CiRecoveryOwnerArgs),
    /// Wait for an assignment, epoch change, resolution, or bounded timeout.
    Wait(CiRecoveryWaitArgs),
    /// Record the exact failed job, step, evidence, and diagnosis.
    Diagnose(CiRecoveryDiagnoseArgs),
    /// Select the sole permitted recovery action.
    ChooseAction(CiRecoveryActionArgs),
    /// Record the queued, running, or failed replacement run.
    RecordReplacement(CiRecoveryReplacementArgs),
    /// Record terminal-success proof and release the hold.
    Resolve(CiRecoveryResolveArgs),
    /// Read the authoritative repository-wide recovery state.
    Status,
}

#[derive(Args)]
struct CiRecoveryClaimArgs {
    #[arg(long)]
    run_id: String,
    #[arg(long)]
    run_url: String,
    #[arg(long)]
    failed_sha: String,
    #[arg(long)]
    workflow: String,
    #[arg(long = "ref")]
    git_ref: String,
}

#[derive(Args)]
struct CiRecoveryOwnerArgs {
    #[arg(long)]
    incident_id: String,
    #[arg(long)]
    epoch: u64,
}

#[derive(Args)]
struct CiRecoveryTransferArgs {
    #[arg(long)]
    incident_id: String,
    #[arg(long)]
    epoch: u64,
    #[arg(long)]
    to_host: String,
    #[arg(long)]
    to_session: String,
}

#[derive(Args)]
struct CiRecoveryAssignArgs {
    #[arg(long)]
    incident_id: String,
    #[arg(long)]
    epoch: u64,
    #[arg(long)]
    to_host: String,
    #[arg(long)]
    to_session: String,
    #[arg(long)]
    capabilities: String,
    #[arg(long)]
    scope: String,
}

#[derive(Args)]
struct CiRecoveryReportArgs {
    #[arg(long)]
    incident_id: String,
    #[arg(long)]
    assignment_id: String,
    #[arg(long)]
    summary: String,
    #[arg(long)]
    evidence: String,
}

#[derive(Args)]
struct CiRecoveryWaitArgs {
    #[arg(long)]
    incident_id: String,
    #[arg(long)]
    epoch: u64,
    #[arg(long)]
    timeout_seconds: u64,
}

#[derive(Args)]
struct CiRecoveryDiagnoseArgs {
    #[arg(long)]
    incident_id: String,
    #[arg(long)]
    epoch: u64,
    #[arg(long)]
    job: String,
    #[arg(long)]
    step: String,
    #[arg(long)]
    log_evidence: String,
    #[arg(long)]
    cause: String,
    #[arg(long)]
    classification: String,
}

#[derive(Args)]
struct CiRecoveryActionArgs {
    #[arg(long)]
    incident_id: String,
    #[arg(long)]
    epoch: u64,
    #[arg(long)]
    kind: String,
    #[arg(long)]
    description: String,
}

#[derive(Args)]
struct CiRecoveryReplacementArgs {
    #[arg(long)]
    incident_id: String,
    #[arg(long)]
    epoch: u64,
    #[arg(long)]
    run_id: String,
    #[arg(long)]
    run_url: String,
    #[arg(long)]
    sha: String,
    #[arg(long)]
    status: String,
}

#[derive(Args)]
struct CiRecoveryResolveArgs {
    #[arg(long)]
    incident_id: String,
    #[arg(long)]
    replacement_run_id: String,
    #[arg(long)]
    replacement_run_url: String,
    #[arg(long)]
    sha: String,
    #[arg(long)]
    terminal_status: String,
}

#[derive(Args)]
struct CodexSandboxArgs {
    /// Print a dry-run preview.
    #[arg(long, required = true)]
    dry_run: bool,
}

#[derive(Args)]
struct DashboardArgs {
    #[command(subcommand)]
    command: DashboardCommand,
}

#[derive(Subcommand)]
enum DashboardCommand {
    /// Serve the dashboard on localhost.
    Serve(DashboardServeArgs),
}

#[derive(Args)]
struct DashboardServeArgs {
    /// Bind a specific localhost port instead of selecting an available one.
    #[arg(long)]
    port: Option<u16>,
    /// Open a browser for a newly started dashboard.
    #[arg(long)]
    open: bool,
}

#[derive(Args)]
struct McpArgs {
    #[command(subcommand)]
    transport: McpTransport,
}

#[derive(Subcommand)]
enum McpTransport {
    /// Use stdio transport.
    Stdio,
}

#[derive(Args)]
#[command(group(ArgGroup::new("mode").required(true).args(["dry_run", "apply"])))]
struct InstallBinArgs {
    /// Directory where the launcher should be installed.
    #[arg(long, allow_hyphen_values = true)]
    target_dir: InstallTargetDir,
    /// Preview without writing.
    #[arg(long)]
    dry_run: bool,
    /// Install the launcher.
    #[arg(long)]
    apply: bool,
}

#[derive(Args)]
struct TaskRefArgs {
    /// Task id, nickname, or full stem.
    task_ref: String,
}

#[derive(Args)]
struct ListArgs {
    /// Limit results to backlog, in-progress, done, or abandoned.
    #[arg(long)]
    status: Option<String>,
}

#[derive(Args)]
struct TransitionArgs {
    /// Task id, nickname, or full stem.
    task_ref: String,
    /// Target status.
    status: String,
}

#[derive(Args)]
struct PrioritizeArgs {
    /// Task id, nickname, or full stem.
    task_ref: String,
    /// Place the task before this task.
    #[arg(long)]
    before: String,
}

#[derive(Args)]
struct RelationArgs {
    /// Source task id, nickname, or full stem.
    from_ref: String,
    /// Dependency relation.
    #[command(subcommand)]
    relation: RelationCommand,
}

#[derive(Subcommand)]
enum RelationCommand {
    /// The source task blocks another task.
    Blocks {
        /// Target task id, nickname, or full stem.
        to_ref: String,
    },
}

#[derive(Args)]
struct SubtaskArgs {
    #[command(subcommand)]
    command: SubtaskCommand,
}

#[derive(Subcommand)]
enum SubtaskCommand {
    /// Add a subtask.
    Add(SubtaskAddArgs),
    /// Mark a subtask complete.
    Check {
        /// Task id, nickname, or full stem.
        task_ref: String,
        /// 1-based subtask index.
        index: String,
    },
    /// Mark a subtask incomplete.
    Uncheck {
        /// Task id, nickname, or full stem.
        task_ref: String,
        /// 1-based subtask index.
        index: String,
    },
}

#[derive(Args)]
struct SubtaskAddArgs {
    /// Task id, nickname, or full stem.
    task_ref: String,
    /// Subtask title.
    #[arg(allow_hyphen_values = true)]
    title: SubtaskTitle,
    /// Comma-separated predecessor subtask refs.
    #[arg(long)]
    after: Option<CommaSeparatedValues>,
}

#[derive(Args)]
#[command(args_override_self = true)]
#[command(
    after_help = "To use a recognized option token as a literal field value, attach it with `=` (for example, `--summary=--tags`)."
)]
#[command(group(
    ArgGroup::new("field")
        .required(true)
        .multiple(true)
        .args(["title", "summary", "context", "tags", "pr_mr_url", "pr_mr_status"])
))]
struct UpdateArgs {
    /// Task id, nickname, or full stem.
    task_ref: String,
    #[arg(long, allow_hyphen_values = true)]
    title: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    summary: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    context: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    tags: Option<CommaSeparatedValues>,
    #[arg(long, allow_hyphen_values = true)]
    pr_mr_url: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pr_mr_status: Option<String>,
}

#[derive(Args)]
struct AcceptanceArgs {
    #[command(subcommand)]
    command: AcceptanceCommand,
}

#[derive(Subcommand)]
enum AcceptanceCommand {
    /// Add an acceptance criterion.
    Add {
        /// Task id, nickname, or full stem.
        task_ref: String,
        /// Criterion text.
        #[arg(allow_hyphen_values = true)]
        criterion: String,
    },
    /// Mark an acceptance criterion complete.
    Check {
        /// Task id, nickname, or full stem.
        task_ref: String,
        /// 1-based criterion index.
        index: String,
    },
    /// Mark an acceptance criterion incomplete.
    Uncheck {
        /// Task id, nickname, or full stem.
        task_ref: String,
        /// 1-based criterion index.
        index: String,
    },
    /// Remove an acceptance criterion.
    Remove {
        /// Task id, nickname, or full stem.
        task_ref: String,
        /// 1-based criterion index.
        index: String,
    },
}

#[derive(Args)]
struct NoteArgs {
    #[command(subcommand)]
    command: NoteCommand,
}

#[derive(Subcommand)]
enum NoteCommand {
    /// Add a dated note.
    Add {
        /// Task id, nickname, or full stem.
        task_ref: String,
        /// Note text.
        #[arg(allow_hyphen_values = true)]
        note: String,
    },
}

#[derive(Args)]
struct ValidateArgs {
    /// Apply safe repairs.
    #[arg(long, required = true)]
    fix: bool,
}

#[derive(Args)]
struct ScaffoldArgs {
    #[command(subcommand)]
    command: ScaffoldCommand,
}

#[derive(Subcommand)]
enum ScaffoldCommand {
    /// Scaffold repository integration.
    Repo(ScaffoldRepoArgs),
}

#[derive(Args)]
#[command(group(ArgGroup::new("mode").required(true).args(["dry_run", "apply"])))]
struct ScaffoldRepoArgs {
    /// Preview without writing.
    #[arg(long)]
    dry_run: bool,
    /// Apply scaffold changes.
    #[arg(long)]
    apply: bool,
    /// Replace integration files reported as conflicts.
    #[arg(long, requires = "apply")]
    replace_conflicts: bool,
}

fn main() -> ExitCode {
    let cli = match parse_cli_arguments(env::args_os()) {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_cli_arguments(arguments: impl IntoIterator<Item = OsString>) -> Result<Cli, clap::Error> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    if arguments.get(1).is_some_and(|value| value == "update")
        && !has_standalone_help(&arguments, is_bare_update_value_option)
    {
        if let Some(pair) = arguments
            .windows(2)
            .find(|pair| is_bare_update_value_option(&pair[0]) && is_update_option_token(&pair[1]))
        {
            let option = pair[0].to_string_lossy();
            let value = pair[1].to_string_lossy();
            return Err(command_error(
                &["update"],
                "tiber update",
                ErrorKind::InvalidValue,
                &format!("{option} requires a value; use {option}={value} for that literal value"),
            ));
        }
    }
    if arguments.get(1).is_some_and(|value| value == "subtask")
        && arguments.get(2).is_some_and(|value| value == "add")
        && arguments.len() >= 6
        && arguments
            .last()
            .is_some_and(|value| value == "--after" || value == "--after=")
    {
        return Err(command_error(
            &["subtask", "add"],
            "tiber subtask add",
            ErrorKind::InvalidValue,
            "--after requires a comma-separated predecessor value",
        ));
    }
    if arguments.get(1).is_some_and(|value| value == "install-bin")
        && !has_standalone_help(&arguments, is_bare_install_bin_value_option)
    {
        if let Some(pair) = arguments.windows(2).find(|pair| {
            is_bare_install_bin_value_option(&pair[0]) && is_install_bin_option_token(&pair[1])
        }) {
            let value = pair[1].to_string_lossy();
            return Err(command_error(
                &["install-bin"],
                "tiber install-bin",
                ErrorKind::InvalidValue,
                &format!(
                    "--target-dir requires a value; use --target-dir={value} for that literal path"
                ),
            ));
        }
    }

    Cli::try_parse_from(normalized_cli_arguments(arguments))
}

fn has_standalone_help(
    arguments: &[OsString],
    is_bare_value_option: fn(&OsString) -> bool,
) -> bool {
    arguments.iter().enumerate().any(|(index, value)| {
        is_help_request(value)
            && (index == 0 || !is_bare_value_option(&arguments[index.saturating_sub(1)]))
    })
}

fn is_help_request(value: &OsString) -> bool {
    value
        .to_str()
        .is_some_and(|value| value == "--help" || value.starts_with("-h"))
}

fn is_bare_update_value_option(value: &OsString) -> bool {
    value.to_str().is_some_and(is_update_value_option_name)
}

fn is_bare_install_bin_value_option(value: &OsString) -> bool {
    value == "--target-dir"
}

fn is_update_option_token(value: &OsString) -> bool {
    let Some(value) = value.to_str() else {
        return false;
    };
    if value.starts_with("-h") {
        return true;
    }
    let option = value.split_once('=').map_or(value, |(option, _)| option);
    option == "--help" || is_update_value_option_name(option)
}

fn is_update_value_option_name(option: &str) -> bool {
    matches!(
        option,
        "--title" | "--summary" | "--context" | "--tags" | "--pr-mr-url" | "--pr-mr-status"
    )
}

fn is_install_bin_option_token(value: &OsString) -> bool {
    let Some(value) = value.to_str() else {
        return false;
    };
    if value.starts_with("-h") {
        return true;
    }
    let option = value.split_once('=').map_or(value, |(option, _)| option);
    matches!(option, "--target-dir" | "--dry-run" | "--apply" | "--help")
}

fn command_error(
    path: &[&str],
    bin_name: &'static str,
    kind: ErrorKind,
    message: &str,
) -> clap::Error {
    let mut root = Cli::command();
    let mut command = &mut root;
    for segment in path {
        command = command
            .find_subcommand_mut(segment)
            .expect("parser error path is part of the CLI grammar");
    }
    let mut command = command.clone().bin_name(bin_name);
    command.error(kind, message)
}

fn normalized_cli_arguments(arguments: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut arguments = arguments.into_iter().collect::<Vec<_>>();

    // The legacy grammar always treated the first token after the task ref as
    // the title, even when it looked like the command-local --after option.
    if arguments.get(1).is_some_and(|value| value == "subtask")
        && arguments.get(2).is_some_and(|value| value == "add")
        && arguments
            .get(4)
            .and_then(|value| value.to_str())
            .is_some_and(|value| value == "--after" || value.starts_with("--after="))
    {
        let title = arguments[4].to_string_lossy();
        arguments[4] = OsString::from(format!("{ESCAPED_SUBTASK_TITLE_PREFIX}{title}"));
    }

    // Clap intentionally discards whether an option value used `=`. Retain
    // that distinction so mode-looking paths stay available only explicitly.
    if arguments.get(1).is_some_and(|value| value == "install-bin") {
        for argument in arguments.iter_mut().skip(2) {
            let Some(value) = argument
                .to_str()
                .and_then(|value| value.strip_prefix("--target-dir="))
            else {
                continue;
            };
            if matches!(value, "--dry-run" | "--apply") {
                *argument = OsString::from(format!(
                    "--target-dir={EXPLICIT_INSTALL_TARGET_PREFIX}{value}"
                ));
            }
        }
    }

    arguments
}

fn run(cli: Cli) -> Result<(), tiber_git::Error> {
    match cli.command {
        Command::Init => tiber_git::init_repository(),
        Command::Sync => tiber_git::sync_repository(),
        Command::CodexSandbox(_) => {
            print!("{}", tiber_mcp::codex_sandbox_setup());
            Ok(())
        }
        Command::Dashboard(DashboardArgs {
            command: DashboardCommand::Serve(DashboardServeArgs { port, open }),
        }) => {
            let requested_port = match port {
                Some(0) => None,
                Some(port) => Some(port),
                None => match env::var("TIBER_DASHBOARD_PORT") {
                    Ok(value) => Some(value.parse::<u16>().map_err(|_| {
                        tiber_git::Error::Parse(format!(
                            "dashboard_port_invalid source=TIBER_DASHBOARD_PORT value={value}"
                        ))
                    })?)
                    .filter(|port| *port != 0),
                    Err(env::VarError::NotPresent) => None,
                    Err(env::VarError::NotUnicode(_)) => {
                        return Err(tiber_git::Error::Parse(
                            "dashboard_port_invalid source=TIBER_DASHBOARD_PORT value=non-unicode"
                                .to_string(),
                        ));
                    }
                },
            };
            let startup_lock = tiber_git::acquire_dashboard_startup_lock()?;
            let runtime_dir = tiber_git::dashboard_runtime_dir()?;
            let state_path = runtime_dir.join("dashboard");
            if let Some(state) = read_dashboard_state(&state_path) {
                if dashboard_is_healthy(&state) {
                    if requested_port.is_some_and(|port| port != state.addr.port()) {
                        return Err(tiber_git::Error::Parse(format!(
                            "dashboard_port_conflict requested={} running={}",
                            requested_port.expect("requested port was checked"),
                            state.addr.port()
                        )));
                    }
                    println!("tiber dashboard already running on http://{}", state.addr);
                    return Ok(());
                }
            }
            let runtime = tokio::runtime::Runtime::new().map_err(tiber_git::Error::Io)?;
            runtime.block_on(async {
                let port = requested_port.unwrap_or(0);
                let addr = format!("127.0.0.1:{port}");
                let listener = tokio::net::TcpListener::bind(&addr)
                    .await
                    .map_err(|error| {
                        if let Some(requested) = requested_port {
                            tiber_git::Error::Parse(format!(
                                "dashboard_port_unavailable requested={requested} source={error}"
                            ))
                        } else {
                            tiber_git::Error::Io(error)
                        }
                    })?;
                let addr = listener.local_addr().map_err(tiber_git::Error::Io)?;
                let token = dashboard_token();
                let state = DashboardState {
                    addr,
                    token: token.clone(),
                };
                let server =
                    tokio::spawn(
                        async move { tiber_server::serve_with_token(listener, token).await },
                    );
                let ready = (0..30).any(|_| {
                    if dashboard_is_healthy(&state) {
                        true
                    } else {
                        std::thread::sleep(Duration::from_millis(10));
                        false
                    }
                });
                if !ready {
                    server.abort();
                    return Err(tiber_git::Error::Parse(
                        "dashboard_startup_health_timeout=true".to_string(),
                    ));
                }
                fs::create_dir_all(&runtime_dir)?;
                fs::write(&state_path, format!("{addr}\n{}\n", state.token))?;
                drop(startup_lock);
                let url = format!("http://{addr}");
                if open {
                    if let Err(error) = open_dashboard(&url) {
                        eprintln!(
                            "tiber.dashboard_browser_open_failed dashboard_continues=true {error}"
                        );
                    }
                }
                println!("tiber dashboard listening on {url}");
                server.await.map_err(|error| {
                    tiber_git::Error::Parse(format!("dashboard_server_join source={error}"))
                })?
            })
        }
        Command::Mcp(McpArgs {
            transport: McpTransport::Stdio,
        }) => {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            tiber_mcp::run_stdio(stdin.lock(), stdout.lock())
        }
        Command::CiRecovery(CiRecoveryArgs { command }) => match command {
            CiRecoveryCommand::Claim(CiRecoveryClaimArgs {
                run_id,
                run_url,
                failed_sha,
                workflow,
                git_ref,
            }) => {
                let claim = tiber_git::claim_ci_recovery(tiber_git::CiRecoveryTrigger {
                    run_id,
                    run_url,
                    failed_sha,
                    workflow,
                    git_ref,
                })?;
                print_ci_recovery_json(&claim)
            }
            CiRecoveryCommand::AssertOwner(CiRecoveryOwnerArgs { incident_id, epoch }) => {
                let assertion = tiber_git::assert_ci_recovery_owner(&incident_id, epoch)?;
                print_ci_recovery_json(&assertion)
            }
            CiRecoveryCommand::Transfer(CiRecoveryTransferArgs {
                incident_id,
                epoch,
                to_host,
                to_session,
            }) => {
                let transfer =
                    tiber_git::transfer_ci_recovery(&incident_id, epoch, &to_host, &to_session)?;
                print_ci_recovery_json(&transfer)
            }
            CiRecoveryCommand::Takeover(CiRecoveryOwnerArgs { incident_id, epoch }) => {
                let takeover = tiber_git::takeover_ci_recovery(&incident_id, epoch)?;
                print_ci_recovery_json(&takeover)
            }
            CiRecoveryCommand::Assign(CiRecoveryAssignArgs {
                incident_id,
                epoch,
                to_host,
                to_session,
                capabilities,
                scope,
            }) => {
                let assignment = tiber_git::assign_ci_recovery(
                    &incident_id,
                    epoch,
                    tiber_git::CiRecoveryAssignmentInput {
                        to_host,
                        to_session,
                        capabilities,
                        scope,
                    },
                )?;
                print_ci_recovery_json(&assignment)
            }
            CiRecoveryCommand::Report(CiRecoveryReportArgs {
                incident_id,
                assignment_id,
                summary,
                evidence,
            }) => {
                let report = tiber_git::report_ci_recovery(
                    &incident_id,
                    &assignment_id,
                    &summary,
                    &evidence,
                )?;
                print_ci_recovery_json(&report)
            }
            CiRecoveryCommand::Heartbeat(CiRecoveryOwnerArgs { incident_id, epoch }) => {
                let heartbeat = tiber_git::heartbeat_ci_recovery(&incident_id, epoch)?;
                print_ci_recovery_json(&heartbeat)
            }
            CiRecoveryCommand::Wait(CiRecoveryWaitArgs {
                incident_id,
                epoch,
                timeout_seconds,
            }) => {
                let wait = tiber_git::wait_for_ci_recovery(&incident_id, epoch, timeout_seconds)?;
                print_ci_recovery_json(&wait)
            }
            CiRecoveryCommand::Diagnose(CiRecoveryDiagnoseArgs {
                incident_id,
                epoch,
                job,
                step,
                log_evidence,
                cause,
                classification,
            }) => {
                let status = tiber_git::diagnose_ci_recovery(
                    &incident_id,
                    epoch,
                    tiber_git::CiRecoveryDiagnosisInput {
                        job,
                        step,
                        log_evidence,
                        cause,
                        classification,
                    },
                )?;
                print_ci_recovery_json(&status)
            }
            CiRecoveryCommand::ChooseAction(CiRecoveryActionArgs {
                incident_id,
                epoch,
                kind,
                description,
            }) => {
                let status =
                    tiber_git::choose_ci_recovery_action(&incident_id, epoch, &kind, &description)?;
                print_ci_recovery_json(&status)
            }
            CiRecoveryCommand::RecordReplacement(CiRecoveryReplacementArgs {
                incident_id,
                epoch,
                run_id,
                run_url,
                sha,
                status,
            }) => {
                let status = tiber_git::record_ci_recovery_replacement(
                    &incident_id,
                    epoch,
                    tiber_git::CiRecoveryReplacementInput {
                        run_id,
                        run_url,
                        sha,
                        status,
                    },
                )?;
                print_ci_recovery_json(&status)
            }
            CiRecoveryCommand::Resolve(CiRecoveryResolveArgs {
                incident_id,
                replacement_run_id,
                replacement_run_url,
                sha,
                terminal_status,
            }) => {
                let status = tiber_git::resolve_ci_recovery(
                    &incident_id,
                    tiber_git::CiRecoveryReleaseInput {
                        replacement_run_id,
                        replacement_run_url,
                        sha,
                        terminal_status,
                    },
                )?;
                print_ci_recovery_json(&status)
            }
            CiRecoveryCommand::Status => {
                let status = tiber_git::ci_recovery_status()?;
                print_ci_recovery_json(&status)
            }
        },
        Command::InstallBin(InstallBinArgs {
            target_dir, apply, ..
        }) => {
            let target_dir = target_dir.into_value();
            let installed = tiber_git::install_bin(&target_dir, apply)?;
            if apply {
                println!("installed {installed}");
            } else {
                println!("would install {installed}");
            }
            Ok(())
        }
        Command::Create { title } => {
            let created = tiber_git::create_task(&title)?;
            println!("created {}", created.path);
            Ok(())
        }
        Command::Show(TaskRefArgs { task_ref }) => {
            print!("{}", tiber_git::show_task(&task_ref)?);
            Ok(())
        }
        Command::Metadata(TaskRefArgs { task_ref }) => {
            let metadata = tiber_git::task_metadata(&task_ref)?;
            println!(
                "{}\t{}\tcommitted_at={}",
                metadata.path,
                metadata.title,
                metadata
                    .committed_at
                    .unwrap_or_else(|| "uncommitted".to_string())
            );
            Ok(())
        }
        Command::List(ListArgs { status }) => {
            let tasks = match status {
                Some(status) => tiber_git::list_tasks_by_status(&status)?,
                None => tiber_git::list_tasks()?,
            };
            for task in tasks {
                println!("{}\t{}", task.path, task.title);
            }
            Ok(())
        }
        Command::Search { query } => {
            let output = serde_json::to_string(&tiber_git::search_tasks(&query)?)
                .map_err(|error| tiber_git::Error::Parse(format!("search_json_invalid {error}")))?;
            println!("{output}");
            Ok(())
        }
        Command::Next => {
            if let Some(task) = tiber_git::next_task()? {
                println!("{}\t{}", task.path, task.title);
            }
            Ok(())
        }
        Command::Transition(TransitionArgs { task_ref, status }) => {
            tiber_git::transition_task(&task_ref, &status)?;
            Ok(())
        }
        Command::Prioritize(PrioritizeArgs { task_ref, before }) => {
            tiber_git::prioritize_before(&task_ref, &before)?;
            Ok(())
        }
        Command::Link(RelationArgs {
            from_ref,
            relation: RelationCommand::Blocks { to_ref },
        }) => {
            tiber_git::link_blocks(&from_ref, &to_ref)?;
            Ok(())
        }
        Command::Unlink(RelationArgs {
            from_ref,
            relation: RelationCommand::Blocks { to_ref },
        }) => {
            tiber_git::unlink_blocks(&from_ref, &to_ref)?;
            Ok(())
        }
        Command::Subtask(SubtaskArgs {
            command:
                SubtaskCommand::Add(SubtaskAddArgs {
                    task_ref,
                    title,
                    after,
                }),
        }) => {
            let title = title.into_value();
            let after = after
                .map(CommaSeparatedValues::into_values)
                .unwrap_or_default();
            tiber_git::add_subtask(&task_ref, &title, &after)?;
            Ok(())
        }
        Command::Subtask(SubtaskArgs {
            command: SubtaskCommand::Check { task_ref, index },
        }) => {
            tiber_git::set_subtask_checked(&task_ref, &index, true)?;
            Ok(())
        }
        Command::Subtask(SubtaskArgs {
            command: SubtaskCommand::Uncheck { task_ref, index },
        }) => {
            tiber_git::set_subtask_checked(&task_ref, &index, false)?;
            Ok(())
        }
        Command::Update(update) => {
            tiber_git::update_task(
                &update.task_ref,
                tiber_git::TaskUpdate {
                    title: update.title.as_deref(),
                    summary: update.summary.as_deref(),
                    context: update.context.as_deref(),
                    tags: update.tags.map(CommaSeparatedValues::into_values),
                    pr_mr_url: update.pr_mr_url.as_deref(),
                    pr_mr_status: update.pr_mr_status.as_deref(),
                },
            )?;
            Ok(())
        }
        Command::Acceptance(AcceptanceArgs {
            command:
                AcceptanceCommand::Add {
                    task_ref,
                    criterion,
                },
        }) => {
            tiber_git::add_acceptance(&task_ref, &criterion)?;
            Ok(())
        }
        Command::Acceptance(AcceptanceArgs {
            command: AcceptanceCommand::Check { task_ref, index },
        }) => {
            tiber_git::set_acceptance_checked(&task_ref, &index, true)?;
            Ok(())
        }
        Command::Acceptance(AcceptanceArgs {
            command: AcceptanceCommand::Uncheck { task_ref, index },
        }) => {
            tiber_git::set_acceptance_checked(&task_ref, &index, false)?;
            Ok(())
        }
        Command::Acceptance(AcceptanceArgs {
            command: AcceptanceCommand::Remove { task_ref, index },
        }) => {
            tiber_git::remove_acceptance(&task_ref, &index)?;
            Ok(())
        }
        Command::Note(NoteArgs {
            command: NoteCommand::Add { task_ref, note },
        }) => {
            tiber_git::add_note(&task_ref, &note)?;
            Ok(())
        }
        Command::Validate(_) => {
            for message in tiber_git::validate_fix()? {
                println!("{message}");
            }
            Ok(())
        }
        Command::CloseFromTrailers => {
            for closed in tiber_git::close_from_trailers()? {
                println!("closed {closed}");
            }
            Ok(())
        }
        Command::Scaffold(ScaffoldArgs {
            command:
                ScaffoldCommand::Repo(ScaffoldRepoArgs {
                    apply,
                    replace_conflicts,
                    ..
                }),
        }) => {
            for message in tiber_git::scaffold_repo(apply, replace_conflicts)? {
                println!("{message}");
            }
            Ok(())
        }
    }
}

fn print_ci_recovery_json(value: &impl serde::Serialize) -> Result<(), tiber_git::Error> {
    println!(
        "{}",
        serde_json::to_string(value).map_err(|error| {
            tiber_git::Error::Parse(format!("ci_recovery_output_json_invalid source={error}"))
        })?
    );
    Ok(())
}

struct DashboardState {
    addr: SocketAddr,
    token: String,
}

fn read_dashboard_state(path: &Path) -> Option<DashboardState> {
    let contents = fs::read_to_string(path).ok()?;
    let mut lines = contents.lines();
    let addr = lines.next()?.parse().ok()?;
    let token = lines.next()?.to_string();
    if token.is_empty() || lines.next().is_some() {
        return None;
    }
    Some(DashboardState { addr, token })
}

fn dashboard_is_healthy(state: &DashboardState) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&state.addr, Duration::from_millis(300)) else {
        return false;
    };
    stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .ok();
    stream
        .set_write_timeout(Some(Duration::from_millis(300)))
        .ok();
    if write!(
        stream,
        "GET /health HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        state.addr
    )
    .is_err()
    {
        return false;
    }
    let mut response = String::new();
    stream.read_to_string(&mut response).is_ok()
        && response.starts_with("HTTP/1.1 200")
        && response
            .split_once("\r\n\r\n")
            .is_some_and(|(_, body)| body == state.token)
}

fn dashboard_token() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{timestamp}", process::id())
}

fn open_dashboard(url: &str) -> Result<(), tiber_git::Error> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = process::Command::new(program)
        .arg(url)
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null())
        .spawn()
        .map_err(|error| {
            tiber_git::Error::Parse(format!(
                "dashboard_browser_open_failed program={program} source={error}"
            ))
        })?;
    std::thread::spawn(move || {
        match child.wait() {
            Ok(status) if status.success() => {}
            Ok(status) => eprintln!(
                "tiber.dashboard_browser_open_failed dashboard_continues=true program={program} status={status}"
            ),
            Err(error) => eprintln!(
                "tiber.dashboard_browser_open_failed dashboard_continues=true program={program} source={error}"
            ),
        }
    });
    Ok(())
}
