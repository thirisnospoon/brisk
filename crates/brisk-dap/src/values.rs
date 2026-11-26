//! Values as debugpy shows them: full values through hex, return values, tracebacks.

/// An expression whose value is the hex of `expr`'s str() (a string) or repr() (anything else).
pub fn full_value_expression(expr: &str) -> String {
    format!("(lambda _v: (_v if isinstance(_v, str) else repr(_v)).encode('utf-8', 'replace').hex())(({expr}))")
}

/// debugpy's repr of a hex string ('6869') back to its text.
pub fn decode_hex_repr(r: &str) -> Option<String> {
    let h = r.trim().trim_matches(|c| c == '\'' || c == '"');
    if h.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..h.len()).step_by(2).map(|i| u8::from_str_radix(h.get(i..i + 2)?, 16).ok()).collect();
    Some(String::from_utf8_lossy(&bytes?).into_owned())
}

/// debugpy (showReturnValue) names the value a call just returned "(return) f".
pub fn return_label(name: &str) -> Option<String> {
    name.strip_prefix("(return) ").map(|f| format!("↩ {f}()"))
}

/// debugpy's exceptionInfo traceback lists the innermost frame first and ends with the
/// exception line: Python's order (innermost last), without that line.
pub fn python_order(stack: &str) -> String {
    let mut lines: Vec<&str> = stack.trim_end().lines().collect();
    while lines.last().is_some_and(|l| !l.starts_with(' ')) {
        lines.pop();
    }
    if !lines.first().is_some_and(|l| l.trim_start().starts_with("File \"")) {
        return lines.join("\n");
    }
    let mut frames: Vec<Vec<&str>> = Vec::new();
    for l in lines {
        if l.trim_start().starts_with("File \"") || frames.is_empty() {
            frames.push(vec![l]);
        } else if let Some(f) = frames.last_mut() {
            f.push(l);
        }
    }
    frames.reverse();
    frames.concat().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_value_labels() {
        assert_eq!(return_label("(return) add").as_deref(), Some("↩ add()"));
        assert_eq!(return_label("total"), None);
    }

    #[test]
    fn traceback_in_python_order() {
        let s = "  File \"a.py\", line 2, in f\n    return 1 / x\n  File \"a.py\", line 5, in <module>\n    f(0)\nZeroDivisionError: division by zero\n";
        assert_eq!(python_order(s), "  File \"a.py\", line 5, in <module>\n    f(0)\n  File \"a.py\", line 2, in f\n    return 1 / x");
    }
}
