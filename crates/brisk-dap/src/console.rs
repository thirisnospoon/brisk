//! Completion in the Debug console.

/// Where the name being typed at the end of the console input begins (byte index).
pub fn completion_start(input: &str) -> usize {
    input.char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || *c == '_').last().map(|(i, _)| i).unwrap_or(input.len())
}

/// The longest prefix every label shares (case-sensitive).
pub fn common_prefix<'a>(labels: impl IntoIterator<Item = &'a str>) -> String {
    let mut it = labels.into_iter();
    let Some(first) = it.next() else { return String::new() };
    let mut p: &str = first;
    for l in it {
        let n = p.chars().zip(l.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a.len_utf8()).sum();
        p = &p[..n];
    }
    p.to_string()
}

/// Public names first (dunder and _private last), keeping debugpy's order otherwise.
pub fn order_completions(items: &mut [(String, String)]) {
    items.sort_by_key(|(l, _)| (l.starts_with("__"), l.starts_with('_')));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_completion() {
        assert_eq!(completion_start("items.ap"), 6);
        assert_eq!(completion_start("len(ite"), 4);
        assert_eq!(completion_start("x + "), 4);
        assert_eq!(common_prefix(["append", "apply"]), "app");
        assert_eq!(common_prefix(["a"]), "a");
        let mut v = vec![("__len__".to_string(), String::new()), ("_x".into(), String::new()), ("append".into(), String::new())];
        order_completions(&mut v);
        assert_eq!(v.iter().map(|i| i.0.as_str()).collect::<Vec<_>>(), ["append", "_x", "__len__"]);
    }
}
