use super::*;
use std::fs;

fn sh(dir: &Path, args: &[&str]) {
    let out = git_cmd().arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn temp_repo(name: &str) -> (PathBuf, Repo) {
    let d = std::env::temp_dir().join(format!("brisk-git-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    sh(&d, &["init", "-q", "-b", "main"]);
    sh(&d, &["config", "user.name", "Test"]);
    sh(&d, &["config", "user.email", "test@example.com"]);
    sh(&d, &["config", "core.autocrlf", "false"]);
    let repo = Repo::discover(&d).expect("discover");
    (d, repo)
}

#[test]
fn status_commit_log_branches() {
    let (d, repo) = temp_repo("flow");
    fs::write(d.join("a.py"), "x = 1\n").unwrap();
    fs::write(d.join("b.py"), "y = 2\n").unwrap();

    let st = repo.status().unwrap();
    assert_eq!(st.branch.as_deref(), Some("main"));
    assert_eq!(st.untracked().count(), 2);
    assert!(repo.log(10).unwrap().is_empty(), "no commits yet");

    // Commit only a.py.
    let files = vec!["a.py".to_string()];
    repo.commit("first", &files, false, &files).unwrap();
    let st = repo.status().unwrap();
    assert_eq!(st.changes, vec![FileChange { path: "b.py".into(), old_path: None, status: FileStatus::Untracked }]);

    // Modify and stage b separately; commit a only must not include b.
    let both = vec!["a.py".to_string(), "b.py".to_string()];
    repo.commit("second", &both, false, &["b.py".to_string()]).unwrap();
    fs::write(d.join("a.py"), "x = 10\n").unwrap();
    fs::write(d.join("b.py"), "y = 20\n").unwrap();
    sh(&d, &["add", "b.py"]);
    repo.commit("only a", &files, false, &[]).unwrap();
    let st = repo.status().unwrap();
    assert_eq!(st.changes.len(), 1);
    assert_eq!(st.changes[0].path, "b.py");

    let log = repo.log(10).unwrap();
    assert_eq!(log.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), vec!["only a", "second", "first"]);
    assert!(log[0].refs.iter().any(|r| r.contains("main")));
    let files = repo.commit_files(&log[0].hash).unwrap();
    assert_eq!(files, vec![FileChange { path: "a.py".into(), old_path: None, status: FileStatus::Modified }]);
    assert_eq!(repo.file_at("HEAD~1", "a.py").unwrap().as_deref(), Some("x = 1\n"));
    assert_eq!(repo.file_at("HEAD", "missing.py").unwrap(), None);

    // Branches.
    repo.create_branch("feature").unwrap();
    let br = repo.branches().unwrap();
    assert!(br.iter().any(|b| b.name == "feature" && b.current));
    let main = br.iter().find(|b| b.name == "main").unwrap().clone();
    repo.checkout(&main).unwrap();
    assert_eq!(repo.status().unwrap().branch.as_deref(), Some("main"));

    // Rollback b.py.
    let st = repo.status().unwrap();
    repo.rollback(&st.changes).unwrap();
    assert!(repo.status().unwrap().changes.is_empty());
    assert_eq!(fs::read_to_string(d.join("b.py")).unwrap(), "y = 2\n");

    // Relative paths, case-insensitive on Windows.
    assert_eq!(repo.rel(&d.join("sub").join("c.py")).as_deref(), Some("sub/c.py"));
}

#[test]
fn status_parsing_renames_and_branch_ab() {
    let raw = "# branch.oid abc\0# branch.head dev\0# branch.upstream origin/dev\0# branch.ab +2 -1\0\
               1 .M N... 100644 100644 100644 h1 h2 src/x.py\0\
               2 R. N... 100644 100644 100644 h1 h2 R100 new name.py\0old.py\0\
               ? notes.txt\0";
    let st = parse_status(raw);
    assert_eq!(st.branch.as_deref(), Some("dev"));
    assert_eq!((st.ahead, st.behind), (2, 1));
    assert_eq!(st.changes.len(), 3);
    let r = st.changes.iter().find(|c| c.status == FileStatus::Renamed).unwrap();
    assert_eq!(r.path, "new name.py");
    assert_eq!(r.old_path.as_deref(), Some("old.py"));
}
