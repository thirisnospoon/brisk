//! The symbols around the caret (class › method) for the breadcrumbs, cached: the outline is
//! recomputed when the text changes, the chain when the text or the caret line changes, so a
//! frame that changes neither costs nothing.

use std::rc::Rc;

use brisk_syntax::OutlineItem;

use super::Editor;

/// Cache of `Editor::caret_scope`.
#[derive(Default)]
pub(super) struct ScopeCache {
    /// (buffer version, outline).
    outline: Option<(u64, Rc<[OutlineItem]>)>,
    /// (buffer version, caret line, indices of the enclosing items, outermost first).
    chain: Option<(u64, usize, Rc<[usize]>)>,
}

impl Editor {
    /// The file's outline (classes, functions, methods in document order), cached per version.
    pub fn outline_cached(&mut self) -> Rc<[OutlineItem]> {
        let v = self.buffer.version();
        if let Some((cv, o)) = &self.scope_cache.outline {
            if *cv == v {
                return o.clone();
            }
        }
        let o: Rc<[OutlineItem]> = self.outline().into();
        self.scope_cache.outline = Some((v, o.clone()));
        o
    }

    /// The outline and the items enclosing the caret line, outermost first.
    pub fn caret_scope(&mut self) -> (Rc<[OutlineItem]>, Rc<[usize]>) {
        let outline = self.outline_cached();
        let v = self.buffer.version();
        let line = self.caret_line_col().0;
        if let Some((cv, cl, c)) = &self.scope_cache.chain {
            if *cv == v && *cl == line {
                return (outline, c.clone());
            }
        }
        let chain: Rc<[usize]> = enclosing(&outline, line).into();
        self.scope_cache.chain = Some((v, line, chain.clone()));
        (outline, chain)
    }
}

/// Indices of the items whose definition spans `line`, outermost first (the outline is in
/// document order, so nested items follow their parent).
pub fn enclosing(outline: &[OutlineItem], line: usize) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for (i, o) in outline.iter().enumerate() {
        if o.line <= line && line <= o.end_line {
            while out.last().is_some_and(|&p| outline[p].depth >= o.depth) {
                out.pop();
            }
            out.push(i);
        }
    }
    out
}

/// The items on the same level as `item` under the same parent (`None`: the top level), in
/// document order: what a breadcrumb's popup lists.
pub fn siblings(outline: &[OutlineItem], parent: Option<usize>, depth: usize) -> Vec<usize> {
    let (from, to) = match parent {
        Some(p) => (outline[p].line, outline[p].end_line),
        None => (0, usize::MAX),
    };
    outline.iter().enumerate().filter(|(_, o)| o.depth == depth && o.line >= from && o.line <= to).map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use brisk_syntax::OutlineKind;

    fn item(name: &str, depth: usize, line: usize, end_line: usize) -> OutlineItem {
        OutlineItem { name: name.into(), kind: if depth == 0 { OutlineKind::Class } else { OutlineKind::Method }, depth, byte: 0, line, end_line }
    }

    #[test]
    fn chain_and_siblings() {
        // class A (0-9): f (1-3), g (4-9): inner (5-6); def top (11-12)
        let o = vec![item("A", 0, 0, 9), item("f", 1, 1, 3), item("g", 1, 4, 9), item("inner", 2, 5, 6), item("top", 0, 11, 12)];
        assert_eq!(enclosing(&o, 5), vec![0, 2, 3]);
        assert_eq!(enclosing(&o, 2), vec![0, 1]);
        assert_eq!(enclosing(&o, 8), vec![0, 2]);
        assert_eq!(enclosing(&o, 10), Vec::<usize>::new());
        assert_eq!(enclosing(&o, 12), vec![4]);
        assert_eq!(siblings(&o, None, 0), vec![0, 4]);
        assert_eq!(siblings(&o, Some(0), 1), vec![1, 2]);
        assert_eq!(siblings(&o, Some(2), 2), vec![3]);
    }
}
