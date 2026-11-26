//! Exception breakpoints: the class list and the setExceptionBreakpoints arguments.

use serde_json::{json, Value};

/// A Python exception breakpoint for a class: `ValueError`, `mypkg.MyError`.
#[derive(Clone, Debug, PartialEq)]
pub struct ExcClass {
    pub name: String,
    /// Stop where it is raised, even if handled.
    pub raised: bool,
    /// Stop when nothing handles it.
    pub uncaught: bool,
    pub enabled: bool,
}

/// A new row: stops where raised and when uncaught.
impl Default for ExcClass {
    fn default() -> Self {
        Self { name: String::new(), raised: true, uncaught: true, enabled: true }
    }
}

/// setExceptionBreakpoints arguments. With class breakpoints everything goes as
/// exceptionOptions (debugpy then ignores `filters`); "Python Exceptions" alone means all.
pub fn exception_args(muted: bool, raised: bool, uncaught: bool, classes: &[ExcClass]) -> Value {
    if muted {
        return json!({"filters": []});
    }
    let mut filters = Vec::new();
    if raised {
        filters.push("raised");
    }
    if uncaught {
        filters.push("uncaught");
    }
    let classes: Vec<&ExcClass> = classes.iter().filter(|c| c.enabled && !c.name.trim().is_empty() && (c.raised || c.uncaught)).collect();
    if classes.is_empty() {
        return json!({"filters": filters});
    }
    let all = json!({"names": ["Python Exceptions"]});
    let mut opts = Vec::new();
    // Raised covers uncaught: the raise happens first.
    if raised {
        opts.push(json!({"path": [all], "breakMode": "always"}));
    } else if uncaught {
        opts.push(json!({"path": [all], "breakMode": "unhandled"}));
    }
    for c in classes {
        let mode = if c.raised { "always" } else { "unhandled" };
        opts.push(json!({"path": [all, {"names": [c.name.trim()]}], "breakMode": mode}));
    }
    json!({"filters": filters, "exceptionOptions": opts})
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exception_breakpoint_args() {
        let c = |n: &str, raised: bool| ExcClass { name: n.into(), raised, uncaught: true, enabled: true };
        assert_eq!(exception_args(false, false, true, &[]), json!({"filters": ["uncaught"]}));
        assert_eq!(exception_args(true, true, true, &[c("ValueError", true)]), json!({"filters": []}));
        let v = exception_args(false, false, true, &[c("ValueError", true), c("KeyError", false), c("", true)]);
        let opts = v["exceptionOptions"].as_array().unwrap();
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[0], json!({"path": [{"names": ["Python Exceptions"]}], "breakMode": "unhandled"}));
        assert_eq!(opts[1], json!({"path": [{"names": ["Python Exceptions"]}, {"names": ["ValueError"]}], "breakMode": "always"}));
        assert_eq!(opts[2]["breakMode"], "unhandled");
        // Nothing for all exceptions: only the classes.
        let v = exception_args(false, false, false, &[ExcClass { enabled: false, ..c("A", true) }, c("B", true)]);
        assert_eq!(v["exceptionOptions"].as_array().unwrap().len(), 1);
    }
}
