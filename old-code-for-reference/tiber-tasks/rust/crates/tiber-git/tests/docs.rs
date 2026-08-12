use std::fs;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn docs_are_listed_read_and_constrained_to_repo_docs_tree() {
    let repo = TempRepo::initialized();
    fs::create_dir_all(repo.path.join("docs/guides")).expect("create docs directory");
    fs::write(repo.path.join("docs/guides/tiber.md"), "# Tiber guide\n").expect("write doc");
    fs::write(repo.path.join("docs/notes.txt"), "not markdown\n").expect("write text file");

    let _cwd = CurrentDirGuard::enter(&repo.path);

    let docs = tiber_git::list_docs().expect("list docs");
    assert_eq!(docs, vec!["docs/guides/tiber.md"]);

    let doc = tiber_git::read_doc("docs/guides/tiber.md").expect("read doc");
    assert_eq!(doc, "# Tiber guide\n");

    let traversal = tiber_git::read_doc("docs/../README.md").expect_err("reject traversal");
    assert!(traversal.to_string().contains("invalid_doc_ref"));
}

struct CurrentDirGuard {
    previous: std::path::PathBuf,
}

impl CurrentDirGuard {
    fn enter(path: &std::path::Path) -> Self {
        let previous = std::env::current_dir().expect("current dir");
        std::env::set_current_dir(path).expect("enter temp repo");
        Self { previous }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.previous).expect("restore current dir");
    }
}

struct TempRepo {
    path: std::path::PathBuf,
}

impl TempRepo {
    fn initialized() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "tiber-git-docs-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create temp repo");
        let repo = Self { path };
        repo.git(["init", "-b", "main"]);
        repo.git(["config", "commit.gpgsign", "false"]);
        fs::write(repo.path.join("README.md"), "# test repo\n").expect("write readme");
        repo
    }

    fn git<I, S>(&self, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        assert_success(
            Command::new("git")
                .args(args)
                .current_dir(&self.path)
                .output()
                .expect("run git"),
        );
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "command failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
