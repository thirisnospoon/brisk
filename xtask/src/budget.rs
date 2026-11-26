//! Size, startup and memory budget of the release build: one shot, under a minute, for every
//! merge. Prints a table and exits with 1 when a limit is exceeded (2 when something cannot be
//! measured).
//!
//! MEMBUDGET is a smoke, so it runs on a build with the smoke feature (`--smoke-exe`); the
//! release exe has no smokes. `--build` builds both, the smoke one with the smoke profile.
//!
//! `--size-only` checks exe_mb and zip_mb and launches nothing: start time and memory on shared
//! machines vary too much to fail a build on.
//!
//! Reported, not checked: start_cold_ms (the first launch), the startup profile (named moments
//! since process start), private bytes, and the peak working sets of ty and ruff during
//! MEMBUDGET. Every launch gets a fresh APPDATA in the temp folder, so the machine's settings and
//! history do not count; MEMBUDGET runs on a temporary copy of the project (it writes files
//! there).

use crate::mb;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HELP: &str = "usage: cargo xtask budget [--build] [--package] [--runs 3] [--exe target/release/brisk.exe]
       [--zip dist/x.zip] [--project examples/demo_project] [--smoke-exe target/smoke/brisk.exe]
       [--no-membudget] [--size-only] [--out results.json]

  --build          build the release exe (and the smoke exe, unless --no-membudget / --size-only)
  --package        package the release exe first (cargo xtask package-portable --no-build)
  --runs N         launches for the start time (median); the last also measures idle memory
  --exe, --smoke-exe, --zip, --project   what to measure (defaults: the repository's)
  --no-membudget   skip the BRISK_SMOKE_MEMBUDGET run
  --size-only      check the exe and zip sizes only; launches nothing
  --out FILE       also write the results as JSON";

/// MB are working sets.
const LIMITS: [(&str, f64); 5] = [
    ("exe_mb", 26.0),        // release brisk.exe
    ("zip_mb", 39.0),        // the portable zip (--zip, else the newest dist/*.zip; --package rebuilds it)
    ("start_ms", 400.0),     // process start -> first window frame drawn, median of --runs launches
    ("idle_mb", 110.0),      // brisk.exe 5 s after the first frame, project open, nothing else
    ("membudget_mb", 200.0), // BRISK_SMOKE_MEMBUDGET at its end: 200k-line run, 50 tabs, Find in Files
];

struct Opts {
    build: bool,
    package: bool,
    runs: usize,
    exe: Option<PathBuf>,
    smoke_exe: Option<PathBuf>,
    zip: Option<PathBuf>,
    project: Option<PathBuf>,
    no_membudget: bool,
    size_only: bool,
    out: Option<PathBuf>,
}

fn parse_args(args: &[String]) -> Result<Opts, String> {
    let mut o =
        Opts { build: false, package: false, runs: 3, exe: None, smoke_exe: None, zip: None, project: None, no_membudget: false, size_only: false, out: None };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--build" => o.build = true,
            "--package" => o.package = true,
            "--no-membudget" => o.no_membudget = true,
            "--size-only" => o.size_only = true,
            "--runs" => o.runs = value()?.parse().map_err(|_| "--runs needs a number".to_string())?,
            "--exe" => o.exe = Some(value()?.into()),
            "--smoke-exe" => o.smoke_exe = Some(value()?.into()),
            "--zip" => o.zip = Some(value()?.into()),
            "--project" => o.project = Some(value()?.into()),
            "--out" => o.out = Some(value()?.into()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(o)
}

/// A measured value: a number, a text (a count or a duration as the smoke printed it), or none.
#[derive(Clone, Debug, PartialEq)]
enum Val {
    Num(f64),
    Text(String),
    None,
}

impl Val {
    fn json(&self) -> serde_json::Value {
        match self {
            Val::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => serde_json::Value::from(*n as i64),
            Val::Num(n) => serde_json::Value::from(*n),
            Val::Text(s) => serde_json::Value::from(s.as_str()),
            Val::None => serde_json::Value::Null,
        }
    }

    fn show(&self) -> String {
        match self {
            Val::Num(n) => n.to_string(),
            Val::Text(s) => s.clone(),
            Val::None => String::new(),
        }
    }
}

/// The results in the order they were measured.
#[derive(Default)]
struct Results(Vec<(&'static str, Val)>);

impl Results {
    fn set(&mut self, key: &'static str, v: Val) {
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(e) => e.1 = v,
            None => self.0.push((key, v)),
        }
    }
}

/// Working sets of a process's descendants, by name: ty, ruff and the rest (MB).
#[derive(Clone, Copy, Debug, Default)]
struct Kids {
    ty: f64,
    ruff: f64,
    other: f64,
}

fn child_memory(pid: u32) -> Kids {
    let all = brisk_platform::process::processes();
    let mut k = Kids::default();
    for p in brisk_platform::process::descendants(&all, pid) {
        let name = Path::new(&p.name).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        let m = mb(p.working_set as f64);
        match name.as_str() {
            "ty" => k.ty += m,
            "ruff" => k.ruff += m,
            _ => k.other += m,
        }
    }
    k
}

fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// A folder of its own in the temp folder for one launch.
fn temp_dir(prefix: &str) -> PathBuf {
    let n = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos()) ^ std::process::id().rotate_left(16);
    std::env::temp_dir().join(format!("{prefix}{n:08x}"))
}

fn launch(dir: &Path, project: &Path, envs: &[(&str, String)], exe: &Path) -> Command {
    let appdata = dir.join("appdata");
    let _ = std::fs::create_dir_all(&appdata);
    let mut c = Command::new(exe);
    c.arg(project).current_dir(dir).env("APPDATA", &appdata);
    for (k, v) in envs {
        c.env(k, v);
    }
    c
}

/// One launch's start time, startup marks and (for the full launch) idle memory.
#[derive(Default)]
struct Run {
    start_ms: Option<i64>,
    marks: Vec<(String, i64)>,
    idle_ws_mb: Option<f64>,
    idle_private_mb: Option<f64>,
    kids: Option<Kids>,
}

fn spawn(mut c: Command) -> Result<(Child, u64), String> {
    let before = unix_ms(SystemTime::now());
    let child = c.spawn().map_err(|e| format!("cannot start {:?}: {e}", c.get_program()))?;
    // The system's creation time, like PowerShell's Process.StartTime.
    let start = brisk_platform::process::start_unix_ms(&child).unwrap_or(before);
    Ok((child, start))
}

fn read_report(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// One launch with the in-app probe (BRISK_BUDGET).
fn start_with_probe(exe: &Path, project: &Path, full: bool, notes: &mut Vec<String>) -> Result<Run, String> {
    let tmp = temp_dir("brisk-budget-");
    let report = tmp.join("report.json");
    let mut envs = vec![("BRISK_BUDGET", report.display().to_string())];
    if !full {
        envs.push(("BRISK_BUDGET_STARTUP_ONLY", "1".into()));
    }
    let (mut child, start) = spawn(launch(&tmp, project, &envs, exe))?;
    let mut run = Run::default();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut exited = false;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        if run.kids.is_some() {
            continue;
        }
        if read_report(&report).is_some_and(|d| d["stage"] == "idle") {
            run.kids = Some(child_memory(child.id()));
        }
    }
    if !exited {
        notes.push("a launch timed out".into());
        let _ = child.kill();
    }
    let _ = child.wait();
    if let Some(d) = read_report(&report) {
        let at = |k: &str| d[k].as_i64().unwrap_or(0);
        if at("first_frame_unix_ms") > 0 {
            let start = start as i64;
            run.start_ms = Some(at("first_frame_unix_ms") - start);
            if let Some(m) = d["marks"].as_object() {
                run.marks.extend(m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_i64()? - start))));
            }
            run.marks.push(("first_render".into(), at("first_render_unix_ms") - start));
            run.marks.push(("first_frame".into(), at("first_frame_unix_ms") - start));
        }
        run.idle_ws_mb = d["idle_ws_mb"].as_f64();
        run.idle_private_mb = d["idle_private_mb"].as_f64();
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(run)
}

/// The same from outside, for an exe built without the probe: start_ms is process start ->
/// main window created (the first frame follows within a few ms), idle is the working set 5 s
/// later. The launch is then killed (its children die with its job).
fn start_from_outside(exe: &Path, project: &Path, full: bool) -> Result<Run, String> {
    let tmp = temp_dir("brisk-budget-");
    let (mut child, start) = spawn(launch(&tmp, project, &[], exe))?;
    let mut run = Run::default();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut window = false;
    while Instant::now() < deadline && !matches!(child.try_wait(), Ok(Some(_))) {
        if brisk_platform::process::has_window(child.id()) {
            window = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if window {
        let ms = unix_ms(SystemTime::now()) as i64 - start as i64;
        run.start_ms = Some(ms);
        run.marks.push(("window_created".into(), ms));
        if full {
            std::thread::sleep(Duration::from_secs(5));
            if let Some((ws, private)) = brisk_platform::process::memory_of(child.id()) {
                run.idle_ws_mb = Some(mb(ws as f64));
                run.idle_private_mb = Some(mb(private as f64));
            }
            run.kids = Some(child_memory(child.id()));
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(run)
}

/// Does the exe have the probe? (Its env var name is in the binary.)
fn has_probe(exe: &Path) -> bool {
    let needle = b"BRISK_BUDGET_STARTUP_ONLY";
    std::fs::read(exe).is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle))
}

/// The middle value; the lower one of the two middles for an even count.
fn median(xs: &[i64]) -> Option<i64> {
    let mut s = xs.to_vec();
    s.sort();
    s.get(s.len().checked_sub(1)? / 2).copied()
}

/// What `smoke-membudget: working set …` reports: working set and private MB, live children
/// and the total time.
fn parse_membudget(line: &str) -> Option<(i64, i64, String, String)> {
    let rest = line.split_once("working set ")?.1;
    let (ws, rest) = rest.split_once(" MB, private ")?;
    let (private, rest) = rest.split_once(" MB")?;
    let children = rest.split_once("live children ")?.1.split_whitespace().next()?.trim_end_matches(',');
    let total = rest.split_once("total ")?.1;
    let time: String = total.chars().take_while(|c| c.is_alphanumeric() || *c == '.').collect();
    let digits = time.chars().take_while(|c| c.is_ascii_digit() || *c == '.').count();
    if digits == 0 || digits == time.len() {
        return None;
    }
    Some((ws.trim().parse().ok()?, private.trim().parse().ok()?, children.to_string(), time))
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        if e.path().is_dir() {
            copy_dir(&e.path(), &to.join(e.file_name()))?;
        } else {
            std::fs::copy(e.path(), to.join(e.file_name()))?;
        }
    }
    Ok(())
}

/// BRISK_SMOKE_MEMBUDGET on a copy of the project (its own verdict is switched off with a
/// huge budget: the limit is checked here); ty and ruff are sampled while it runs.
fn membudget(smoke_exe: &Path, project: &Path, r: &mut Results, notes: &mut Vec<String>) -> Result<(), String> {
    let tmp = temp_dir("brisk-budget-mb-");
    let proj = tmp.join("project");
    copy_dir(project, &proj).map_err(|e| format!("cannot copy {}: {e}", project.display()))?;
    let mut c = launch(&tmp, &proj, &[("BRISK_SMOKE_MEMBUDGET", "100000".into())], smoke_exe);
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let (mut child, _) = spawn(c)?;
    let reader = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut p) = pipe {
                let mut bytes = Vec::new();
                let _ = p.read_to_end(&mut bytes);
                s = String::from_utf8_lossy(&bytes).into_owned();
            }
            s
        })
    };
    let err = reader(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let out = reader(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let (mut ty, mut ruff) = (0.0f64, 0.0f64);
    let deadline = Instant::now() + Duration::from_secs(240);
    let mut exited = false;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            exited = true;
            break;
        }
        let k = child_memory(child.id());
        ty = ty.max(k.ty);
        ruff = ruff.max(k.ruff);
        std::thread::sleep(Duration::from_millis(500));
    }
    if !exited {
        notes.push("MEMBUDGET timed out".into());
        let _ = child.kill();
    }
    let _ = child.wait();
    let text = err.join().unwrap_or_default() + &out.join().unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    match lines.iter().rev().find(|l| l.contains("smoke-membudget: working set")).and_then(|l| parse_membudget(l)) {
        Some((ws, private, children, time)) => {
            r.set("membudget_mb", Val::Num(ws as f64));
            r.set("membudget_private_mb", Val::Num(private as f64));
            r.set("membudget_children", Val::Text(children));
            r.set("membudget_time", Val::Text(time));
        }
        None => {
            r.set("membudget_mb", Val::None);
            let tail = lines[lines.len().saturating_sub(8)..].join("\n");
            notes.push(format!("MEMBUDGET printed no result; its log tail:\n{tail}"));
        }
    }
    for l in &lines {
        if let Some(rest) = l.split_once("smoke-membudget: ").map(|(_, r)| r) {
            let tabs = rest.split_once(' ').is_some_and(|(n, w)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && w.starts_with("tabs"));
            if rest.starts_with("run finished") || tabs {
                notes.push(l.trim().to_string());
            }
        }
    }
    r.set("ty_peak_mb", Val::Num(ty));
    r.set("ruff_peak_mb", Val::Num(ruff));
    if ty == 0.0 {
        notes.push("ty did not run during MEMBUDGET (not next to the interpreter or on PATH?)".into());
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

/// A table with a header and dashes, numbers right-aligned.
fn table(head: &[&str], rows: &[Vec<String>]) -> String {
    let numeric = |c: usize| rows.iter().all(|r| r[c].is_empty() || r[c].parse::<f64>().is_ok());
    let widths: Vec<usize> = (0..head.len()).map(|c| rows.iter().map(|r| r[c].chars().count()).chain([head[c].len()]).max().unwrap_or(0)).collect();
    let line = |cells: Vec<String>| {
        let parts: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(c, s)| if numeric(c) && c > 0 { format!("{s:>w$}", w = widths[c]) } else { format!("{s:<w$}", w = widths[c]) })
            .collect();
        parts.join(" ").trim_end().to_string()
    };
    let mut out = vec![String::new(), line(head.iter().map(|h| h.to_string()).collect()), line(head.iter().map(|h| "-".repeat(h.len())).collect())];
    out.extend(rows.iter().map(|r| line(r.clone())));
    out.push(String::new());
    out.join("\n")
}

fn absolute(p: PathBuf) -> PathBuf {
    std::path::absolute(&p).unwrap_or(p)
}

pub fn run(args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let mut o = match parse_args(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("budget: {e}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };
    let root = crate::root();
    let exe = absolute(o.exe.take().unwrap_or_else(|| root.join("target").join("release").join("brisk.exe")));
    let smoke_exe = absolute(o.smoke_exe.take().unwrap_or_else(|| root.join("target").join("smoke").join("brisk.exe")));
    let project = absolute(o.project.take().unwrap_or_else(|| root.join("examples").join("demo_project")));

    if o.build {
        if crate::cargo(&["build", "--release", "-p", "brisk-app"]).is_err() {
            println!("cargo build failed");
            return ExitCode::from(2);
        }
        if !o.no_membudget && !o.size_only && crate::cargo(&["build", "--profile", "smoke", "-p", "brisk-app", "--features", "smoke"]).is_err() {
            println!("cargo build --features smoke failed");
            return ExitCode::from(2);
        }
    }
    if !exe.is_file() {
        println!("No {} (pass --build, or --exe)", exe.display());
        return ExitCode::from(2);
    }
    if o.package {
        match crate::package::package(&root, true) {
            Ok(z) => println!("Packaged {}", z.display()),
            Err(e) => {
                println!("package-portable: {e}");
                return ExitCode::from(2);
            }
        }
    }

    let mut r = Results::default();
    let mut notes: Vec<String> = Vec::new();
    let mut limits: Vec<(&str, f64)> = LIMITS.to_vec();
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    r.set("exe_mb", Val::Num(mb(std::fs::metadata(&exe).map_or(0, |m| m.len()) as f64)));
    let zip = o.zip.take().map(absolute).or_else(|| {
        let zips =
            std::fs::read_dir(root.join("dist")).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")));
        zips.max_by_key(|p| modified(p))
    });
    match zip.filter(|z| z.is_file()) {
        Some(z) => {
            r.set("zip_mb", Val::Num(mb(std::fs::metadata(&z).map_or(0, |m| m.len()) as f64)));
            if modified(&z) < modified(&exe) {
                notes.push(format!("zip {} is older than brisk.exe (--package rebuilds it)", z.file_name().unwrap_or_default().to_string_lossy()));
            }
        }
        None => {
            r.set("zip_mb", Val::None);
            notes.push("no zip in dist/ (--package builds one)".into());
        }
    }

    // 1. Startup: --runs launches; the last one also stays 5 s for the idle working set.
    if o.size_only {
        o.runs = 0;
        o.no_membudget = true;
        limits.retain(|(k, _)| !matches!(*k, "start_ms" | "idle_mb" | "membudget_mb"));
    }
    let probe = has_probe(&exe);
    if !probe {
        notes.push(
            "this exe has no BRISK_BUDGET probe: start_ms is process start -> main window created, idle measured from outside, no startup profile".into(),
        );
    }
    let mut starts = Vec::new();
    let mut marks: Vec<(String, Vec<i64>)> = Vec::new();
    let mut last: Option<Run> = None;
    for i in 1..=o.runs {
        let full = i == o.runs;
        let run = if probe { start_with_probe(&exe, &project, full, &mut notes) } else { start_from_outside(&exe, &project, full) };
        let run = match run {
            Ok(run) => run,
            Err(e) => {
                println!("{e}");
                return ExitCode::from(2);
            }
        };
        let Some(ms) = run.start_ms else {
            println!("Launch {i} did not report a first frame");
            return ExitCode::from(2);
        };
        println!("launch {i}: {} {ms} ms", if probe { "first frame" } else { "window created" });
        starts.push(ms);
        for (k, v) in &run.marks {
            match marks.iter_mut().find(|(n, _)| n == k) {
                Some((_, vs)) => vs.push(*v),
                None => marks.push((k.clone(), vec![*v])),
            }
        }
        if i == 1 {
            r.set("start_cold_ms", Val::Num(ms as f64));
        }
        last = Some(run);
    }
    if !o.size_only {
        r.set("start_ms", median(&starts).map_or(Val::None, |m| Val::Num(m as f64)));
        let last = last.unwrap_or_default();
        r.set("idle_mb", last.idle_ws_mb.map_or(Val::None, Val::Num));
        r.set("idle_private_mb", last.idle_private_mb.map_or(Val::None, Val::Num));
        if let Some(k) = last.kids {
            r.set("idle_children_mb", Val::Num(((k.ty + k.ruff + k.other) * 10.0).round_ties_even() / 10.0));
        }
    }

    // 2. MEMBUDGET.
    if !o.no_membudget {
        if !smoke_exe.is_file() {
            println!("No {} for MEMBUDGET (pass --build, --smoke-exe or --no-membudget)", smoke_exe.display());
            return ExitCode::from(2);
        }
        if let Err(e) = membudget(&smoke_exe, &project, &mut r, &mut notes) {
            println!("{e}");
            return ExitCode::from(2);
        }
    }

    let mut over: Vec<&str> = Vec::new();
    let rows: Vec<Vec<String>> =
        r.0.iter()
            .map(|(key, val)| {
                let lim = limits.iter().find(|(k, _)| k == key).map(|(_, l)| *l);
                let status = match (lim, val) {
                    (None, _) => "",
                    (Some(_), Val::None) => "n/a",
                    (Some(l), Val::Num(v)) if *v > l => {
                        over.push(key);
                        "OVER"
                    }
                    (Some(_), _) => "ok",
                };
                vec![key.to_string(), val.show(), lim.map(|l| l.to_string()).unwrap_or_default(), status.to_string()]
            })
            .collect();
    println!("{}", table(&["Metric", "Value", "Budget", "Status"], &rows));

    // Where the start goes: each moment's median since process start and the step from the previous.
    let mut meds: Vec<(String, i64)> = marks.iter().filter_map(|(k, vs)| Some((k.clone(), median(vs)?))).collect();
    meds.sort_by_key(|(_, m)| *m);
    let mut prev = 0;
    let prof: Vec<Vec<String>> = meds
        .iter()
        .map(|(k, m)| {
            let row = vec![k.clone(), m.to_string(), (m - prev).to_string()];
            prev = *m;
            row
        })
        .collect();
    if !prof.is_empty() {
        println!("{}", table(&["Startup", "ms since start", "step ms"], &prof));
    }
    for n in &notes {
        println!("note: {n}");
    }
    if let Some(out) = &o.out {
        let mut m = serde_json::Map::new();
        for (k, v) in &r.0 {
            m.insert((*k).to_string(), v.json());
        }
        m.insert("over".into(), serde_json::Value::from(over.iter().map(|s| s.to_string()).collect::<Vec<_>>()));
        m.insert("startup".into(), meds.iter().map(|(k, v)| (k.clone(), serde_json::Value::from(*v))).collect::<serde_json::Map<_, _>>().into());
        m.insert("limits".into(), limits.iter().map(|(k, v)| ((*k).to_string(), Val::Num(*v).json())).collect::<serde_json::Map<_, _>>().into());
        let text = serde_json::to_string_pretty(&serde_json::Value::Object(m)).unwrap_or_default();
        if let Err(e) = std::fs::write(out, text + "\n") {
            println!("cannot write {}: {e}", out.display());
            return ExitCode::from(2);
        }
    }
    if !over.is_empty() {
        println!("OVER BUDGET: {}", over.join(", "));
        return ExitCode::from(1);
    }
    println!("Within budget");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membudget_line() {
        let l = "smoke-membudget: working set 147 MB, private 175 MB, budget 250 MB, live children 2, total 8.6s -> OK";
        assert_eq!(parse_membudget(l), Some((147, 175, "2".into(), "8.6s".into())));
        assert_eq!(parse_membudget("smoke-membudget: 50 tabs opened"), None);
    }

    #[test]
    fn medians() {
        assert_eq!(median(&[300, 100, 200]), Some(200));
        assert_eq!(median(&[4, 1, 3, 2]), Some(2));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn arguments() {
        let a: Vec<String> = ["--size-only", "--runs", "5", "--exe", "x.exe"].iter().map(|s| s.to_string()).collect();
        let o = parse_args(&a).unwrap();
        assert!(o.size_only && o.runs == 5 && o.exe == Some(PathBuf::from("x.exe")));
        assert!(parse_args(&["--runs".to_string()]).is_err());
        assert!(parse_args(&["--nope".to_string()]).is_err());
    }

    #[test]
    fn table_aligns_numbers_right() {
        let t = table(&["Metric", "Value"], &[vec!["exe_mb".into(), "21.6".into()], vec!["zip_mb".into(), "37".into()]]);
        assert!(t.contains("exe_mb  21.6\nzip_mb    37"), "{t}");
    }
}
