//! PyCharm's "Store as project file": a configuration is also kept in `.run/<name>.run.xml`
//! (PyCharm's own format), so it goes to VCS and a teammate's PyCharm or Brisk picks it up.
//! Saving writes the files of such configurations and deletes the ones that are no longer
//! shared; loading takes every `.run/*.run.xml` Brisk understands as the source of truth
//! (a `git pull` that changes or removes one shows up in the run configurations).

use crate::config::{configurations_in, RunConfig, RunFile, RunKind, OnStart};
use std::path::{Path, PathBuf};

pub const DIR: &str = ".run";

/// `.run/<name>.run.xml`, characters Windows does not allow in file names replaced by `_`.
pub fn file_name(name: &str) -> String {
    let safe: String = name.chars().map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c }).collect();
    format!("{}.run.xml", safe.trim())
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// PyCharm keeps test options as Python string literals.
fn py_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A project-relative path as PyCharm writes it.
fn project_path(p: &str) -> String {
    let p = p.trim();
    if p.is_empty() || p.starts_with("$PROJECT_DIR$") || Path::new(p).is_absolute() {
        p.to_string()
    } else {
        format!("$PROJECT_DIR$/{p}")
    }
}

/// PyCharm's configuration type (and factory) of a kind.
pub fn idea_type(kind: RunKind) -> (&'static str, &'static str) {
    match kind {
        RunKind::Python => ("PythonConfigurationType", "Python"),
        RunKind::Pytest => ("tests", "py.test"),
        RunKind::Unittest => ("tests", "Unittests"),
        RunKind::Doctest => ("tests", "Doctests"),
        RunKind::Compound => ("CompoundRunConfigurationType", ""),
        RunKind::FastApi => ("Python.FastAPI", "FastAPI"),
        RunKind::Flask => ("Python.FlaskServer", "Flask server"),
        RunKind::Django => ("Python.DjangoServer", "Django server"),
        RunKind::DjangoTests => ("DjangoTestsConfigurationType", "Django tests"),
    }
}

/// The `.run.xml` text of `c`. `kind_of` names the kinds of other configurations (compound
/// members and Before launch steps).
pub fn to_xml(root: &Path, c: &RunConfig, kind_of: &dyn Fn(&str) -> Option<RunKind>) -> String {
    let (ty, factory) = idea_type(c.kind);
    let mut attrs = format!("name=\"{}\" type=\"{ty}\"", esc(&c.name));
    if !factory.is_empty() {
        attrs.push_str(&format!(" factoryName=\"{}\"", esc(factory)));
    }
    match c.on_start {
        OnStart::Nothing => attrs.push_str(" activateToolWindowBeforeRun=\"false\""),
        OnStart::Focus => attrs.push_str(" focusToolWindowBeforeRun=\"true\""),
        _ => {}
    }
    if c.allow_multiple {
        attrs.push_str(" allowRunningInParallel=\"true\"");
    }
    let mut body: Vec<String> = Vec::new();
    let opt = |name: &str, value: &str| format!("<option name=\"{name}\" value=\"{}\" />", esc(value));
    if c.kind == RunKind::Compound {
        for m in crate::compound_names(&c.target) {
            let t = kind_of(&m).map(|k| idea_type(k).0).unwrap_or("PythonConfigurationType");
            body.push(format!("<toRun name=\"{}\" type=\"{t}\" />", esc(&m)));
        }
        body.push("<method v=\"2\" />".into());
        return wrap(&attrs, &body);
    }
    let module = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    body.push(format!("<module name=\"{}\" />", esc(&module)));
    body.push(opt("ENV_FILES", &c.env_files.split(';').map(str::trim).filter(|p| !p.is_empty()).map(project_path).collect::<Vec<_>>().join(";")));
    body.push(opt("INTERPRETER_OPTIONS", &c.interpreter_options));
    body.push(opt("PARENT_ENVS", "true"));
    let mut envs = c.env.clone();
    if matches!(c.kind, RunKind::Python | RunKind::FastApi | RunKind::Flask | RunKind::Django) {
        envs.entry("PYTHONUNBUFFERED".into()).or_insert_with(|| "1".into());
    }
    if matches!(c.kind, RunKind::Django | RunKind::DjangoTests) {
        // PyCharm takes manage.py from the project's Django settings; name it explicitly.
        let module = c.target.trim_end_matches(".py").replace(['/', '\\'], ".");
        if !module.is_empty() && module != "manage" {
            envs.insert("PYCHARM_DJANGO_MANAGE_MODULE".into(), module);
        }
    }
    if !envs.is_empty() {
        body.push("<envs>".into());
        for (k, v) in &envs {
            body.push(format!("  <env name=\"{}\" value=\"{}\" />", esc(k), esc(v)));
        }
        body.push("</envs>".into());
    }
    body.push(opt("SDK_HOME", c.interpreter.as_deref().unwrap_or("")));
    body.push(opt("WORKING_DIRECTORY", &project_path(&c.working_directory)));
    body.push(opt("IS_MODULE_SDK", if c.interpreter.is_some() { "false" } else { "true" }));
    body.push(opt("ADD_CONTENT_ROOTS", "true"));
    body.push(opt("ADD_SOURCE_ROOTS", "true"));
    match c.kind {
        RunKind::Python => {
            body.push(opt("SCRIPT_NAME", &if c.module { String::new() } else { project_path(&c.target) }));
            if c.module {
                body.push(opt("MODULE_NAME", &c.target));
            }
            body.push(opt("PARAMETERS", &c.parameters));
            body.push(opt("SHOW_COMMAND_LINE", if c.python_console { "true" } else { "false" }));
            body.push(opt("EMULATE_TERMINAL", if c.emulate_terminal { "true" } else { "false" }));
            body.push(opt("MODULE_MODE", if c.module { "true" } else { "false" }));
            body.push(opt("REDIRECT_INPUT", "false"));
            body.push(opt("INPUT_FILE", ""));
        }
        RunKind::Pytest | RunKind::Unittest | RunKind::Doctest => {
            // One path (quoted when it has spaces): PATH; several or none: CUSTOM, the
            // targets going with the arguments.
            let t = c.target.trim();
            let single = t.strip_prefix('"').and_then(|s| s.strip_suffix('"')).filter(|s| !s.contains('"')).map(str::to_string).or_else(|| (!t.is_empty() && !t.contains(' ')).then(|| t.to_string()));
            let mut args = c.parameters.trim().to_string();
            if single.is_none() && !t.is_empty() {
                args = format!("{t} {args}").trim().to_string();
            }
            if c.kind == RunKind::Pytest {
                body.push(opt("_new_keywords", &py_str(&c.keywords)));
                body.push(opt("_new_parameters", &py_str("")));
            }
            body.push(opt("_new_additionalArguments", &py_str(&args)));
            body.push(opt("_new_target", &py_str(&single.as_deref().map(project_path).unwrap_or_default())));
            body.push(opt("_new_targetType", &py_str(if single.is_some() { "PATH" } else { "CUSTOM" })));
        }
        RunKind::FastApi => {
            let module = c.target.split(':').next().unwrap_or("");
            body.push(opt("file", &project_path(&format!("{}.py", module.replace('.', "/")))));
            body.push(opt("additionalOptions", &c.parameters));
        }
        RunKind::Flask => {
            let (m, app) = c.target.split_once(':').unwrap_or((c.target.as_str(), ""));
            body.push(opt("target", m));
            body.push(opt("targetType", "PYTHON"));
            body.push(opt("application", app));
            body.push(opt("additionalOptions", &c.parameters));
        }
        RunKind::Django => {
            body.push(opt("additionalOptions", &c.parameters));
        }
        RunKind::DjangoTests => {
            body.push(opt("TARGET", &c.parameters));
            body.push(opt("USE_OPTIONS", "false"));
        }
        RunKind::Compound => {}
    }
    let steps: Vec<String> = crate::compound_names(&c.before_run)
        .into_iter()
        .map(|n| {
            let t = kind_of(&n).map(|k| idea_type(k).0).unwrap_or("PythonConfigurationType");
            format!("  <option name=\"RunConfigurationTask\" enabled=\"true\" run_configuration_name=\"{}\" run_configuration_type=\"{t}\" />", esc(&n))
        })
        .collect();
    if steps.is_empty() {
        body.push("<method v=\"2\" />".into());
    } else {
        body.push("<method v=\"2\">".into());
        body.extend(steps);
        body.push("</method>".into());
    }
    wrap(&attrs, &body)
}

fn wrap(attrs: &str, body: &[String]) -> String {
    let mut s = format!("<component name=\"ProjectRunConfigurationManager\">\n  <configuration default=\"false\" {attrs}>\n");
    for l in body {
        s.push_str("    ");
        s.push_str(l);
        s.push('\n');
    }
    s.push_str("  </configuration>\n</component>");
    s
}

/// The configurations in `.run/*.run.xml` that Brisk understands, with their files.
pub fn read_all(root: &Path) -> Vec<(PathBuf, RunConfig)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(root.join(DIR)).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with(".xml")).collect();
    files.sort();
    let mut out = Vec::new();
    for p in files {
        let Ok(xml) = std::fs::read_to_string(&p) else { continue };
        for mut c in configurations_in(root, &xml, "ProjectRunConfigurationManager").0 {
            if let Some(rest) = c.working_directory.strip_prefix("$PROJECT_DIR$/") {
                c.working_directory = rest.to_string();
            }
            c.project_file = true;
            c.temporary = false;
            out.push((p.clone(), c));
        }
    }
    out
}

/// Take the project files into `f`: a file's configuration replaces the one of the same
/// name (or is added), and a shared configuration whose file is gone is removed.
pub fn merge(root: &Path, f: &mut RunFile) {
    if !root.join(DIR).is_dir() && !f.configurations.iter().any(|c| c.project_file) {
        return;
    }
    let shared = read_all(root);
    f.configurations.retain(|c| !c.project_file || shared.iter().any(|(_, s)| s.name == c.name));
    for (_, c) in shared {
        match f.configurations.iter_mut().find(|o| o.name == c.name) {
            Some(o) => {
                // Brisk-only settings without a place in PyCharm's file stay as they were.
                let before_command = std::mem::take(&mut o.before_command);
                let on_start = if o.on_start == OnStart::Services && c.on_start == OnStart::Show { OnStart::Services } else { c.on_start };
                *o = RunConfig { before_command, on_start, ..c };
            }
            None => f.configurations.push(c),
        }
    }
}

/// Write the files of the shared configurations of `f` (only when their text changed) and
/// delete the `.run` files of configurations that are no longer shared, renamed or removed.
/// Files Brisk does not understand (other run configuration types) are left alone.
pub fn sync(root: &Path, f: &RunFile) -> std::io::Result<()> {
    let dir = root.join(DIR);
    let kind_of = |n: &str| f.get(n).map(|c| c.kind);
    let mut keep: Vec<PathBuf> = Vec::new();
    for c in f.configurations.iter().filter(|c| c.project_file) {
        let path = dir.join(file_name(&c.name));
        let text = to_xml(root, c, &kind_of) + "\n";
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            std::fs::create_dir_all(&dir)?;
            // Another file may hold this configuration (named differently by PyCharm).
            if let Some((other, _)) = read_all(root).into_iter().find(|(p, o)| o.name == c.name && *p != path) {
                if read_all_in(&other) == 1 {
                    let _ = std::fs::remove_file(other);
                }
            }
            std::fs::write(&path, text)?;
        }
        keep.push(path);
    }
    for (p, c) in read_all(root) {
        let shared = f.get(&c.name).is_some_and(|o| o.project_file);
        if !shared && !keep.contains(&p) && read_all_in(&p) == 1 {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}

/// How many configurations Brisk understands in one file (a file holding several, or ones
/// of other types, is never deleted).
fn read_all_in(p: &Path) -> usize {
    let Ok(xml) = std::fs::read_to_string(p) else { return 0 };
    let Ok(doc) = roxmltree::Document::parse(&xml) else { return 0 };
    let total = doc.descendants().filter(|n| n.has_tag_name("configuration")).count();
    let known = configurations_in(p.parent().and_then(Path::parent).unwrap_or(Path::new(".")), &xml, "ProjectRunConfigurationManager").0.len();
    if total == known { known } else { usize::MAX }
}
