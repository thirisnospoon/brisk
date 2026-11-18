//! Run configurations: `.brisk/run.json` in the project. On first open, Python
//! configurations are imported from PyCharm (`.idea/runConfigurations/*.xml` and the
//! RunManager component of `.idea/workspace.xml`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const CONFIG_FILE: &str = ".brisk/run.json";
/// PyCharm keeps the last five temporary ("Current File") configurations.
pub const MAX_TEMPORARY: usize = 5;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    pub name: String,
    /// Python script/module or pytest (PyCharm's "Python" and "Python tests > pytest").
    #[serde(default, rename = "type", skip_serializing_if = "RunKind::is_python")]
    pub kind: RunKind,
    /// Run `target` as a module (`python -m target`), PyCharm's "module name" mode.
    #[serde(default, skip_serializing_if = "is_false")]
    pub module: bool,
    /// Script path (relative to the project, `$PROJECT_DIR$/…` or absolute) or module name.
    /// pytest: a file, folder or node (`tests/test_x.py::TestA::test_b`), several separated
    /// by spaces; empty runs everything pytest finds in the working directory.
    pub target: String,
    /// pytest `-k` expression.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub keywords: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub parameters: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub interpreter_options: String,
    /// Empty: the script's directory (module mode: the project root).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub working_directory: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// PyCharm's "Paths to .env files": `;`-separated, relative to the project or absolute;
    /// `env` overrides their variables.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub env_files: String,
    /// Interpreter for this configuration only; `None` uses the project interpreter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpreter: Option<String>,
    /// Before launch: configurations run first, one after another (`;`-separated names);
    /// each must exit with 0 or the launch stops.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub before_run: String,
    /// Before launch: a shell command run after `before_run`, in the working directory and
    /// with the configuration's environment; a non-zero exit stops the launch.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub before_command: String,
    /// Created by Ctrl+Shift+F10 from a file; replaced after [`MAX_TEMPORARY`].
    #[serde(default, skip_serializing_if = "is_false")]
    pub temporary: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunKind {
    #[default]
    Python,
    Pytest,
    /// PyCharm's Compound: `target` lists other configurations (`;`-separated names), all
    /// started together, each in its own Run tab.
    Compound,
    /// PyCharm's FastAPI template: `python -m uvicorn <target>`; `target` is `module:app`.
    FastApi,
    /// PyCharm's Flask server: `python -m flask --app <target> run`.
    Flask,
    /// PyCharm's Django Server: `python <target> runserver <parameters>`; `target` is `manage.py`.
    Django,
    /// PyCharm's Django tests: `python <target> test <parameters>` (labels and options).
    #[serde(rename = "djangotests")]
    DjangoTests,
}

impl RunKind {
    fn is_python(&self) -> bool {
        *self == RunKind::Python
    }
}

/// Quote a pytest target that contains spaces, so it survives [`crate::split_args`].
fn quote_target(t: String) -> String {
    if t.contains(' ') {
        format!("\"{t}\"")
    } else {
        t
    }
}

impl RunConfig {
    /// The temporary configuration PyCharm creates for "Run 'file'": named after the file.
    pub fn for_file(root: &Path, file: &Path) -> Self {
        let name = file.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "script".into());
        RunConfig { name, target: crate::relativize(root, file), temporary: true, ..Default::default() }
    }

    /// The temporary pytest configuration for a folder, a test file, or a class/test in it
    /// (`scope` is the node path, e.g. `["TestA", "test_b"]`), named like PyCharm does:
    /// "pytest in tests", "pytest in test_x.py", "pytest for test_x.TestA.test_b".
    pub fn for_tests(root: &Path, path: &Path, scope: &[String]) -> Self {
        let rel = crate::relativize(root, path);
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| rel.clone());
        let stem = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let (name, target) = if scope.is_empty() {
            (format!("pytest in {file_name}"), rel)
        } else {
            (format!("pytest for {stem}.{}", scope.join(".")), format!("{rel}::{}", scope.join("::")))
        };
        RunConfig { name, kind: RunKind::Pytest, target: quote_target(target), temporary: true, ..Default::default() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFile {
    /// Project interpreter path; `None` picks the project's virtualenv automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpreter: Option<String>,
    /// Name of the selected configuration; `None` is "Current File".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    #[serde(default)]
    pub configurations: Vec<RunConfig>,
}

impl RunFile {
    pub fn get(&self, name: &str) -> Option<&RunConfig> {
        self.configurations.iter().find(|c| c.name == name)
    }

    /// The configurations a compound starts, nested compounds expanded (each once, cycles
    /// skipped). An error names a configuration that does not exist.
    pub fn compound_members(&self, cfg: &RunConfig) -> Result<Vec<RunConfig>, String> {
        fn walk(f: &RunFile, cfg: &RunConfig, seen: &mut Vec<String>, out: &mut Vec<RunConfig>) -> Result<(), String> {
            for name in compound_names(&cfg.target) {
                if seen.contains(&name) {
                    continue;
                }
                seen.push(name.clone());
                let c = f.get(&name).ok_or_else(|| format!("'{}' includes '{name}', which does not exist", cfg.name))?;
                if c.kind == RunKind::Compound {
                    walk(f, c, seen, out)?;
                } else {
                    out.push(c.clone());
                }
            }
            Ok(())
        }
        let mut out = Vec::new();
        walk(self, cfg, &mut vec![cfg.name.clone()], &mut out)?;
        Ok(out)
    }

    /// What runs before `cfg` starts, in order. An error names a configuration that does
    /// not exist, is `cfg` itself or is a Compound (it never finishes as one process).
    pub fn before_steps(&self, cfg: &RunConfig) -> Result<Vec<BeforeStep>, String> {
        let mut out = Vec::new();
        for name in compound_names(&cfg.before_run) {
            let c = self.get(&name).ok_or_else(|| format!("'{}' runs '{name}' before launch, which does not exist", cfg.name))?;
            if c.name == cfg.name {
                return Err(format!("'{}' runs itself before launch", cfg.name));
            }
            if c.kind == RunKind::Compound {
                return Err(format!("'{}' runs the Compound '{name}' before launch: pick its configurations instead", cfg.name));
            }
            out.push(BeforeStep::Run(c.clone()));
        }
        let line = cfg.before_command.trim();
        if !line.is_empty() {
            out.push(BeforeStep::Command(line.to_string()));
        }
        Ok(out)
    }

    pub fn selected_config(&self) -> Option<&RunConfig> {
        self.selected.as_deref().and_then(|n| self.get(n))
    }

    /// Add or refresh a temporary configuration and select it. A permanent configuration
    /// with the same target is reused instead (PyCharm does the same).
    pub fn use_for_file(&mut self, cfg: RunConfig) -> String {
        if let Some(c) = self.configurations.iter().find(|c| c.kind == cfg.kind && !c.module && c.target == cfg.target) {
            let n = c.name.clone();
            self.selected = Some(n.clone());
            return n;
        }
        let mut name = cfg.name.clone();
        let mut i = 1;
        while self.get(&name).is_some() {
            i += 1;
            name = format!("{} ({i})", cfg.name);
        }
        self.configurations.push(RunConfig { name: name.clone(), ..cfg });
        let temps: Vec<usize> = self.configurations.iter().enumerate().filter(|(_, c)| c.temporary).map(|(i, _)| i).collect();
        if temps.len() > MAX_TEMPORARY {
            self.configurations.remove(temps[0]);
        }
        self.selected = Some(name.clone());
        name
    }
}

/// One Before launch step.
#[derive(Clone, Debug, PartialEq)]
pub enum BeforeStep {
    /// Another configuration, in its own Run tab.
    Run(RunConfig),
    /// A shell command line.
    Command(String),
}

/// The member names of a compound's `target` (and of `before_run`).
pub fn compound_names(target: &str) -> Vec<String> {
    target.split(';').map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect()
}

/// Load `.brisk/run.json`, or import from `.idea` when it does not exist yet.
/// Returns the file and the number of imported configurations.
pub fn load(root: &Path) -> (RunFile, usize) {
    if let Ok(s) = std::fs::read_to_string(root.join(CONFIG_FILE)) {
        if let Ok(f) = serde_json::from_str::<RunFile>(&s) {
            return (f, 0);
        }
    }
    let f = import_idea(root);
    let n = f.configurations.len();
    (f, n)
}

pub fn save(root: &Path, f: &RunFile) -> std::io::Result<()> {
    let path = root.join(CONFIG_FILE);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let json = serde_json::to_string_pretty(f).map_err(std::io::Error::other)?;
    std::fs::write(path, json + "\n")
}

/// PyCharm stores pytest options as Python string literals: `"tests/test_x.py"`.
fn unquote(v: &str) -> String {
    let v = v.trim();
    let inner = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')));
    match inner {
        Some(i) => i.replace("\\\"", "\"").replace("\\\\", "\\"),
        None => v.to_string(),
    }
}

/// `tests.test_x.TestA.test_b` (PyCharm's "Module name" target) as a pytest node:
/// the longest dotted prefix that is a file in the project, the rest as `::` parts.
fn python_target(root: &Path, dotted: &str) -> String {
    let parts: Vec<&str> = dotted.split('.').collect();
    for k in (1..=parts.len()).rev() {
        let rel = parts[..k].join("/");
        if root.join(format!("{rel}.py")).is_file() {
            let mut t = format!("{rel}.py");
            for p in &parts[k..] {
                t.push_str("::");
                t.push_str(p);
            }
            return t;
        }
        if k == parts.len() && root.join(&rel).is_dir() {
            return rel;
        }
    }
    dotted.to_string()
}

fn parse_configuration(root: &Path, node: roxmltree::Node) -> Option<RunConfig> {
    if node.attribute("default") == Some("true") {
        return None;
    }
    let kind = match (node.attribute("type"), node.attribute("factoryName")) {
        (Some("CompoundRunConfigurationType"), _) => {
            let members: Vec<&str> = node.children().filter(|n| n.has_tag_name("toRun")).filter_map(|n| n.attribute("name")).collect();
            return Some(RunConfig { name: node.attribute("name")?.to_string(), kind: RunKind::Compound, target: members.join(";"), ..Default::default() });
        }
        (Some("PythonConfigurationType"), _) => RunKind::Python,
        (Some("tests"), Some("py.test")) => RunKind::Pytest,
        (Some("Python.FastAPI"), _) => RunKind::FastApi,
        (Some("Python.FlaskServer"), _) => RunKind::Flask,
        (Some("Python.DjangoServer"), _) => RunKind::Django,
        (Some("DjangoTestsConfigurationType"), _) => RunKind::DjangoTests,
        _ => return None,
    };
    // FastAPI / Flask / Django options, read after the loop.
    let mut web: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    let mut c = RunConfig { name: node.attribute("name")?.to_string(), kind, temporary: node.attribute("temporary") == Some("true"), ..Default::default() };
    let (mut test_target, mut target_type) = (String::new(), String::from("PATH"));
    let mut extra = Vec::new();
    let mut script = String::new();
    let mut module_name = String::new();
    let mut sdk = String::new();
    let mut module_sdk = true;
    for ch in node.children().filter(|n| n.is_element()) {
        match ch.tag_name().name() {
            "option" => {
                let v = ch.attribute("value").unwrap_or("").to_string();
                match ch.attribute("name").unwrap_or("") {
                    "SCRIPT_NAME" => script = v,
                    "MODULE_NAME" => module_name = v,
                    "MODULE_MODE" => c.module = v == "true",
                    "PARAMETERS" => c.parameters = v,
                    "INTERPRETER_OPTIONS" => c.interpreter_options = v,
                    "WORKING_DIRECTORY" => c.working_directory = v,
                    "ENV_FILES" => c.env_files = v.replace("$PROJECT_DIR$/", ""),
                    "SDK_HOME" => sdk = v,
                    "IS_MODULE_SDK" => module_sdk = v != "false",
                    "_new_target" => test_target = unquote(&v),
                    "_new_targetType" => target_type = unquote(&v),
                    "_new_keywords" => c.keywords = unquote(&v),
                    n @ ("file" | "additionalOptions" | "target" | "targetType" | "application" | "flaskDebug" | "host" | "port" | "runNoReload" | "TARGET" | "USE_OPTIONS" | "OPTIONS") => {
                        web.insert(n, v);
                    }
                    "_new_parameters" | "_new_additionalArguments" => {
                        let v = unquote(&v);
                        if !v.is_empty() {
                            extra.push(v);
                        }
                    }
                    _ => {}
                }
            }
            // Before launch: <method v="2"><option name="RunConfigurationTask" run_configuration_name="X"/></method>
            "method" => {
                let names: Vec<&str> = ch
                    .children()
                    .filter(|n| n.has_tag_name("option") && n.attribute("name") == Some("RunConfigurationTask") && n.attribute("enabled") != Some("false"))
                    .filter_map(|n| n.attribute("run_configuration_name"))
                    .collect();
                c.before_run = names.join(";");
            }
            "envs" => {
                for e in ch.children().filter(|n| n.has_tag_name("env")) {
                    if let (Some(k), Some(v)) = (e.attribute("name"), e.attribute("value")) {
                        // PyCharm adds this one to every config; we always set it anyway.
                        if k != "PYTHONUNBUFFERED" {
                            c.env.insert(k.to_string(), v.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if !module_sdk && !sdk.is_empty() {
        c.interpreter = Some(sdk);
    }
    if matches!(kind, RunKind::FastApi | RunKind::Flask | RunKind::Django | RunKind::DjangoTests) {
        c.working_directory = c.working_directory.replace("$PROJECT_DIR$/", "");
        if c.working_directory == "$PROJECT_DIR$" {
            c.working_directory.clear();
        }
        return server_configuration(root, c, web);
    }
    if kind == RunKind::Pytest {
        c.parameters = extra.join(" ");
        let t = test_target.replace("$PROJECT_DIR$/", "").replace("$PROJECT_DIR$", "");
        c.target = quote_target(match target_type.as_str() {
            "PYTHON" => python_target(root, &t),
            "CUSTOM" => String::new(),
            _ => t,
        });
        return Some(c);
    }
    c.target = if c.module { module_name } else { script };
    if c.target.is_empty() {
        return None;
    }
    c.target = c.target.replace("$PROJECT_DIR$/", "");
    Some(c)
}

/// PyCharm's FastAPI (`file`), Flask Server (`target` path or module + `application`) and
/// Django Server / Django tests configurations as Brisk's `module:app` / `manage.py` ones.
fn server_configuration(root: &Path, mut c: RunConfig, web: std::collections::HashMap<&str, String>) -> Option<RunConfig> {
    let opt = |k: &str| web.get(k).map(|v| v.trim().to_string()).unwrap_or_default();
    let on = |k: &str| web.get(k).is_some_and(|v| v == "true");
    // uvicorn and flask import the module from the working directory.
    let base = if c.working_directory.is_empty() { root.to_path_buf() } else { crate::resolve(root, &c.working_directory) };
    let module = |path: &str| {
        let file = crate::resolve(root, path);
        crate::web::module_of(&base, &file).or_else(|| crate::web::module_of(root, &file)).map(|m| (file, m))
    };
    let mut params = Vec::new();
    match c.kind {
        RunKind::FastApi => {
            let (file, m) = module(&opt("file"))?;
            let var = std::fs::read_to_string(&file).ok().and_then(|s| crate::web::app_in_source(&s)).map(|(_, v)| v).unwrap_or_else(|| "app".into());
            c.target = format!("{m}:{var}");
            params.push(opt("additionalOptions"));
        }
        RunKind::Flask => {
            let target = opt("target");
            let m = if opt("targetType") == "PYTHON" { target } else { module(&target)?.1 };
            let app = opt("application");
            c.target = if app.is_empty() { m } else { format!("{m}:{app}") };
            if on("flaskDebug") {
                c.env.entry("FLASK_DEBUG".into()).or_insert_with(|| "1".into());
            }
            params.push(opt("additionalOptions"));
        }
        _ => {
            // PyCharm takes manage.py from the project's Django settings; the tests
            // configuration may name it as a module.
            let named = c.env.remove("PYCHARM_DJANGO_MANAGE_MODULE").map(|m| root.join(format!("{}.py", m.replace('.', "/")))).filter(|p| p.is_file());
            let manage = named.or_else(|| crate::django::find_manage(root).map(|(m, _)| m));
            c.target = manage.map(|m| crate::relativize(root, &m)).unwrap_or_else(|| "manage.py".into());
            if c.kind == RunKind::Django {
                let (host, port) = (opt("host"), opt("port"));
                params.push(match (host.is_empty(), port.is_empty()) {
                    (_, true) if host.is_empty() => String::new(),
                    (true, false) => port,
                    (false, true) => format!("{host}:8000"),
                    (false, false) => format!("{host}:{port}"),
                    _ => String::new(),
                });
                if on("runNoReload") {
                    params.push("--noreload".into());
                }
                params.push(opt("additionalOptions"));
            } else {
                params.push(opt("TARGET"));
                if on("USE_OPTIONS") {
                    params.push(opt("OPTIONS"));
                }
            }
        }
    }
    c.parameters = params.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ");
    Some(c)
}

pub(crate) fn configurations_in(root: &Path, xml: &str, component: &str) -> (Vec<RunConfig>, Option<String>) {
    let Ok(doc) = roxmltree::Document::parse(xml) else { return (Vec::new(), None) };
    let mut out = Vec::new();
    let mut selected = None;
    for comp in doc.descendants().filter(|n| n.has_tag_name("component") && n.attribute("name") == Some(component)) {
        // selected="Python.main" or "Python tests.pytest in tests"
        // selected="Python.main", "Python tests.pytest in tests", "Django server.site"...
        if let Some(s) = comp.attribute("selected") {
            let known = ["Python tests.", "Python.", "Django server.", "Django tests.", "FastAPI.", "Flask server.", "Compound."];
            selected = known.iter().find_map(|p| s.strip_prefix(p)).or_else(|| s.split_once('.').map(|(_, n)| n)).map(str::to_string);
        }
        out.extend(comp.children().filter(|n| n.has_tag_name("configuration")).filter_map(|n| parse_configuration(root, n)));
    }
    (out, selected)
}

/// Python run configurations from PyCharm: shared ones in `.idea/runConfigurations/` and
/// `.run/*.run.xml` ("Store as project file"), then workspace ones (including temporary)
/// from `.idea/workspace.xml`.
pub fn import_idea(root: &Path) -> RunFile {
    let mut f = RunFile::default();
    for dir in [root.join(".idea/runConfigurations"), root.join(".run")] {
        let mut files: Vec<_> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "xml")).collect();
        files.sort();
        for p in files {
            if let Ok(xml) = std::fs::read_to_string(&p) {
                for c in configurations_in(root, &xml, "ProjectRunConfigurationManager").0 {
                    if f.get(&c.name).is_none() {
                        f.configurations.push(c);
                    }
                }
            }
        }
    }
    if let Ok(xml) = std::fs::read_to_string(root.join(".idea/workspace.xml")) {
        let (cfgs, selected) = configurations_in(root, &xml, "RunManager");
        for c in cfgs {
            if f.get(&c.name).is_none() {
                f.configurations.push(c);
            }
        }
        f.selected = selected.filter(|s| f.get(s).is_some());
    }
    f
}
