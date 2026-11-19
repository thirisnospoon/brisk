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

#[test]
fn blame_and_file_log() {
    let (d, repo) = temp_repo("blame");
    fs::write(d.join("a.py"), "x = 1\ny = 2\n").unwrap();
    let files = vec!["a.py".to_string()];
    repo.commit("first", &files, false, &files).unwrap();
    fs::write(d.join("a.py"), "x = 1\ny = 3\n").unwrap();
    repo.commit("second", &files, false, &[]).unwrap();
    let b = repo.blame("a.py", None).unwrap();
    assert_eq!(b.iter().map(|l| l.summary.as_str()).collect::<Vec<_>>(), vec!["first", "second"]);
    assert_eq!(b[0].author, "Test");
    assert_eq!(b[0].email, "test@example.com");
    // Unsaved editor text: the new line is not committed.
    let b = repo.blame("a.py", Some("x = 1\nnew\ny = 3\n")).unwrap();
    assert_eq!(b.len(), 3);
    assert!(b[1].is_uncommitted() && !b[0].is_uncommitted());
    let log = repo.file_log("a.py", 10).unwrap();
    assert_eq!(log.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), vec!["second", "first"]);
    // History preview: the patch of one commit; history for line 2 only.
    let patch = repo.file_patch(&log[0].hash, "a.py").unwrap();
    assert!(patch.contains("-y = 2") && patch.contains("+y = 3"), "{patch}");
    let lines = repo.line_log("a.py", 2, 2, 10).unwrap();
    assert_eq!(lines.iter().map(|(c, _)| c.subject.as_str()).collect::<Vec<_>>(), vec!["second", "first"]);
    assert!(lines[0].1.contains("+y = 3"), "{}", lines[0].1);
    assert_eq!(repo.line_log("a.py", 1, 1, 10).unwrap().len(), 1, "line 1 never changed after the first commit");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn stash_roundtrip() {
    let (d, repo) = temp_repo("stash");
    fs::write(d.join("a.py"), "x = 1\n").unwrap();
    let files = vec!["a.py".to_string()];
    repo.commit("first", &files, false, &files).unwrap();
    fs::write(d.join("a.py"), "x = 2\n").unwrap();
    fs::write(d.join("new.py"), "n = 1\n").unwrap();
    repo.stash_push("my work", true).unwrap();
    assert_eq!(fs::read_to_string(d.join("a.py")).unwrap(), "x = 1\n");
    assert!(!d.join("new.py").exists());
    let list = repo.stash_list().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "stash@{0}");
    assert!(list[0].message.contains("my work"), "{}", list[0].message);
    let changed = repo.stash_files("stash@{0}").unwrap();
    let mut paths: Vec<&str> = changed.iter().map(|c| c.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, vec!["a.py", "new.py"]);
    let a = changed.iter().find(|c| c.path == "a.py").unwrap();
    assert_eq!(repo.stash_file_versions("stash@{0}", a), ("x = 1\n".to_string(), "x = 2\n".to_string()));
    repo.stash_apply("stash@{0}", false).unwrap();
    assert_eq!(fs::read_to_string(d.join("a.py")).unwrap(), "x = 2\n");
    assert_eq!(repo.stash_list().unwrap().len(), 1, "apply keeps the stash");
    repo.stash_drop("stash@{0}").unwrap();
    assert!(repo.stash_list().unwrap().is_empty());
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn log_filters() {
    let (d, repo) = temp_repo("logq");
    fs::write(d.join("a.py"), "x = 1\n").unwrap();
    let files = vec!["a.py".to_string()];
    repo.commit("Add parser", &files, false, &files).unwrap();
    repo.git(&["switch", "-q", "-c", "feature"]).unwrap();
    fs::write(d.join("a.py"), "x = 2\n").unwrap();
    repo.git(&["-c", "user.name=Bob", "-c", "user.email=bob@example.com", "commit", "-q", "-am", "Fix PARSER bug"]).unwrap();
    repo.git(&["switch", "-q", "-"]).unwrap();
    let subjects = |q: &LogQuery| repo.log_query(100, q).unwrap().iter().map(|c| c.subject.clone()).collect::<Vec<_>>();
    assert_eq!(subjects(&LogQuery::default()), vec!["Fix PARSER bug", "Add parser"]);
    let text = LogQuery { text: "parser bug".into(), ..Default::default() };
    assert_eq!(subjects(&text), vec!["Fix PARSER bug"], "case-insensitive, fixed string");
    let bob = LogQuery { author: Some("Bob".into()), ..Default::default() };
    assert_eq!(subjects(&bob), vec!["Fix PARSER bug"]);
    let branch = LogQuery { branch: Some(repo.git(&["branch", "--show-current"]).unwrap().trim().to_string()), ..Default::default() };
    assert_eq!(subjects(&branch), vec!["Add parser"], "the feature commit is not on the first branch");
    let first = repo.log(10).unwrap().pop().unwrap();
    let by_hash = LogQuery { text: first.hash[..7].to_string(), ..Default::default() };
    assert_eq!(subjects(&by_hash), vec!["Add parser"]);
    let authors = repo.authors().unwrap();
    assert!(authors.contains(&"Bob".to_string()) && authors.contains(&"Test".to_string()), "{authors:?}");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn push_pull_fetch() {
    let (d, repo) = temp_repo("remote");
    let origin = d.with_file_name(format!("{}-origin.git", d.file_name().unwrap().to_string_lossy()));
    let _ = fs::remove_dir_all(&origin);
    let other = d.with_file_name(format!("{}-other", d.file_name().unwrap().to_string_lossy()));
    let _ = fs::remove_dir_all(&other);
    sh(&d, &["init", "-q", "--bare", origin.to_str().unwrap()]);
    fs::write(d.join("a.py"), "x = 1\n").unwrap();
    let files = vec!["a.py".to_string()];
    repo.commit("first", &files, false, &files).unwrap();
    assert!(repo.outgoing().is_err(), "no remote yet");
    sh(&d, &["remote", "add", "origin", origin.to_str().unwrap()]);
    let o = repo.outgoing().unwrap();
    assert!(o.new_branch && o.commits.len() == 1, "{o:?}");
    assert!(repo.push_outgoing(&o).unwrap().starts_with("Pushed 1 commit to new branch origin/"));
    fs::write(d.join("a.py"), "x = 2\n").unwrap();
    repo.commit("second", &files, false, &[]).unwrap();
    let o = repo.outgoing().unwrap();
    assert!(!o.new_branch, "{o:?}");
    assert_eq!(o.commits.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), vec!["second"]);
    assert_eq!(repo.push_outgoing(&o).unwrap(), format!("Pushed 1 commit to {}", o.target));
    assert!(repo.outgoing().unwrap().commits.is_empty());
    // Someone else pushes: Fetch reports it, Update Project brings it in.
    let branch = o.target.split_once('/').unwrap().1.to_string();
    sh(&d, &["clone", "-q", "-b", &branch, origin.to_str().unwrap(), other.to_str().unwrap()]);
    sh(&other, &["config", "user.name", "Other"]);
    sh(&other, &["config", "user.email", "other@example.com"]);
    fs::write(other.join("b.py"), "y = 1\n").unwrap();
    sh(&other, &["add", "b.py"]);
    sh(&other, &["commit", "-q", "-m", "from other"]);
    sh(&other, &["push", "-q"]);
    assert_eq!(repo.fetch_summary().unwrap(), format!("Fetched: 1 commit incoming from {}", o.target));
    assert_eq!(repo.update_project().unwrap(), "Updated: 1 commit, 1 file changed");
    assert_eq!(repo.update_project().unwrap(), "All files are up-to-date");
    let _ = fs::remove_dir_all(&d);
    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&other);
}

#[test]
fn branch_merge_and_delete() {
    let (d, repo) = temp_repo("branchops");
    fs::write(d.join("a.py"), "x = 1\n").unwrap();
    let files = vec!["a.py".to_string()];
    repo.commit("first", &files, false, &files).unwrap();
    let main = repo.git(&["branch", "--show-current"]).unwrap().trim().to_string();
    assert!(repo.delete_branch(&main, true).is_err(), "never the current branch");
    sh(&d, &["switch", "-q", "-c", "topic"]);
    fs::write(d.join("b.py"), "y = 1\n").unwrap();
    sh(&d, &["add", "b.py"]);
    sh(&d, &["commit", "-q", "-m", "topic work"]);
    sh(&d, &["switch", "-q", &main]);
    assert!(!repo.is_merged("topic"));
    let e = repo.delete_branch("topic", false).unwrap_err();
    assert!(e.0.contains("not fully merged"), "{e:?}");
    assert_eq!(repo.merge_into_current("topic").unwrap(), "Fast-forwarded to topic");
    assert!(d.join("b.py").exists() && repo.is_merged("topic"));
    repo.delete_branch("topic", false).unwrap();
    sh(&d, &["switch", "-q", "-c", "spike"]);
    sh(&d, &["commit", "-q", "--allow-empty", "-m", "spike"]);
    sh(&d, &["switch", "-q", &main]);
    repo.delete_branch("spike", true).unwrap();
    assert!(repo.branches().unwrap().iter().all(|b| b.name != "spike" && b.name != "topic"));
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn partial_commit_by_hunks() {
    let (d, repo) = temp_repo("partial");
    fs::write(d.join("p.py"), "a\nb\nc\nd\ne\n").unwrap();
    fs::write(d.join("q.py"), "q\n").unwrap();
    let both = vec!["p.py".to_string(), "q.py".to_string()];
    repo.commit("first", &both, false, &both).unwrap();
    fs::write(d.join("p.py"), "a\nB\nc\nd\nE\nf\n").unwrap();
    fs::write(d.join("q.py"), "q2\n").unwrap();
    fs::write(d.join("staged.py"), "s\n").unwrap();
    sh(&d, &["add", "staged.py"]);
    // Exclude the second hunk of p.py; q.py goes whole; staged.py stays out.
    let excluded = std::collections::HashSet::from([hunk_key(&(4..5), ["E", "f"])]);
    repo.commit_partial("second", &["q.py".to_string()], &[("p.py".to_string(), excluded)], false).unwrap();
    assert_eq!(repo.file_at("HEAD", "p.py").unwrap().as_deref(), Some("a\nB\nc\nd\ne\n"));
    assert_eq!(repo.file_at("HEAD", "q.py").unwrap().as_deref(), Some("q2\n"));
    assert_eq!(repo.file_at("HEAD", "staged.py").unwrap(), None);
    assert_eq!(fs::read_to_string(d.join("p.py")).unwrap(), "a\nB\nc\nd\nE\nf\n");
    let st = repo.status().unwrap();
    let mut got: Vec<(String, FileStatus)> = st.changes.iter().map(|c| (c.path.clone(), c.status)).collect();
    got.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(got, vec![("p.py".into(), FileStatus::Modified), ("staged.py".into(), FileStatus::Added)]);
    assert!(!d.join(".git").join("brisk-partial-index").exists());
}

#[test]
fn log_actions() {
    let (d, repo) = temp_repo("logops");
    fs::write(d.join("f.py"), "a\n").unwrap();
    let f = vec!["f.py".to_string()];
    repo.commit("a", &f, false, &f).unwrap();
    sh(&d, &["checkout", "-q", "-b", "side"]);
    fs::write(d.join("g.py"), "g\n").unwrap();
    let g = vec!["g.py".to_string()];
    repo.commit("add g", &g, false, &g).unwrap();
    let side = repo.git(&["rev-parse", "HEAD"]).unwrap().trim().to_string();
    sh(&d, &["checkout", "-q", "main"]);
    // Cherry-Pick and Revert.
    assert!(repo.cherry_pick(&side).unwrap().starts_with("Cherry-picked"));
    assert!(d.join("g.py").exists());
    let head = repo.git(&["rev-parse", "HEAD"]).unwrap().trim().to_string();
    repo.revert_commit(&head).unwrap();
    assert!(!d.join("g.py").exists());
    // Undo Commit: only the last one; its changes come back as local changes.
    assert!(repo.undo_commit(&head).is_err());
    let head2 = repo.git(&["rev-parse", "HEAD"]).unwrap().trim().to_string();
    repo.undo_commit(&head2).unwrap();
    assert_eq!(repo.git(&["rev-parse", "HEAD"]).unwrap().trim(), head);
    sh(&d, &["reset", "-q", "--hard"]);
    // Reset Current Branch to Here (hard) and Checkout Revision.
    let first = repo.git(&["rev-list", "--max-parents=0", "HEAD"]).unwrap().trim().to_string();
    repo.reset_to(&first, ResetMode::Hard).unwrap();
    assert!(!d.join("g.py").exists());
    repo.checkout_revision(&side).unwrap();
    assert!(repo.git(&["branch", "--show-current"]).unwrap().trim().is_empty());
    sh(&d, &["checkout", "-q", "main"]);
    // A conflicting cherry-pick stops; the Commit window's commit finishes it.
    fs::write(d.join("g.py"), "main\n").unwrap();
    repo.commit("g on main", &g, false, &g).unwrap();
    let err = repo.cherry_pick(&side).unwrap_err().to_string();
    assert!(err.contains("conflicts in 1 file"), "{err}");
    assert_eq!(repo.pending_operation(), Some("cherry-pick"));
    fs::write(d.join("g.py"), "resolved\n").unwrap();
    repo.commit("add g (picked)", &g, false, &[]).unwrap();
    assert_eq!(repo.pending_operation(), None);
    assert_eq!(repo.file_at("HEAD", "g.py").unwrap().as_deref(), Some("resolved\n"));
}
