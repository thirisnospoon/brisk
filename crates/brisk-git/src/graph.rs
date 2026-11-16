//! Commit graph layout for the Log view: assigns each commit a lane and computes the line
//! segments to draw in its row. O(commits × lanes), no allocation per lane beyond a Vec.

/// A line within one row. Lanes are columns; `color` is stable along a lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub from: usize,
    pub to: usize,
    pub color: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphRow {
    /// Lane of the commit dot.
    pub node: usize,
    pub node_color: usize,
    /// Top half: row top → row middle.
    pub top: Vec<Segment>,
    /// Bottom half: row middle → row bottom.
    pub bottom: Vec<Segment>,
    /// Number of lanes in use around this row (for width).
    pub width: usize,
}

/// `commits` are (hash, parents) in display order (children before parents).
pub fn layout<'a>(commits: impl IntoIterator<Item = (&'a str, &'a [String])>) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut colors: Vec<usize> = Vec::new();
    let mut next_color = 0usize;
    let mut rows = Vec::new();

    for (hash, parents) in commits {
        // Lanes expecting this commit.
        let expecting: Vec<usize> = lanes.iter().enumerate().filter(|(_, h)| h.as_deref() == Some(hash)).map(|(i, _)| i).collect();
        let node = match expecting.first() {
            Some(&i) => i,
            None => {
                // A branch tip: new lane.
                let i = lanes.iter().position(Option::is_none).unwrap_or(lanes.len());
                if i == lanes.len() {
                    lanes.push(None);
                    colors.push(0);
                }
                colors[i] = next_color;
                next_color += 1;
                i
            }
        };
        let node_color = colors[node];

        let mut top = Vec::new();
        for (i, h) in lanes.iter().enumerate() {
            if let Some(h) = h {
                if h == hash {
                    top.push(Segment { from: i, to: node, color: colors[i] });
                } else {
                    top.push(Segment { from: i, to: i, color: colors[i] });
                }
            }
        }
        // Lanes that merged into this commit end here.
        for &i in &expecting {
            lanes[i] = None;
        }

        // Parents: first parent continues the node's lane.
        let mut parent_lanes = Vec::new();
        for (pi, p) in parents.iter().enumerate() {
            // The first parent always continues the node's own lane (keeps main straight, even if
            // another lane already waits for that parent — they converge at the parent).
            if pi > 0 || lanes[node].is_some() {
                if let Some(existing) = lanes.iter().position(|h| h.as_deref() == Some(p.as_str())) {
                    parent_lanes.push(existing);
                    continue;
                }
            }
            let lane = if pi == 0 && lanes[node].is_none() {
                node
            } else {
                let i = lanes.iter().position(Option::is_none).unwrap_or(lanes.len());
                if i == lanes.len() {
                    lanes.push(None);
                    colors.push(0);
                }
                if i != node {
                    colors[i] = next_color;
                    next_color += 1;
                }
                i
            };
            lanes[lane] = Some(p.clone());
            parent_lanes.push(lane);
        }

        let mut bottom = Vec::new();
        for (i, h) in lanes.iter().enumerate() {
            if h.is_none() {
                continue;
            }
            if parent_lanes.contains(&i) {
                bottom.push(Segment { from: node, to: i, color: colors[i] });
            }
            // Pass-through for lanes that weren't touched by this commit (a parent lane may also
            // be an existing one: then it has both a pass-through and the merge-in edge).
            let was_expecting_other = top.iter().any(|s| s.from == i && s.to == i);
            if was_expecting_other {
                bottom.push(Segment { from: i, to: i, color: colors[i] });
            }
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
            colors.pop();
        }
        let width = top.iter().chain(&bottom).map(|s| s.from.max(s.to) + 1).max().unwrap_or(1).max(node + 1);
        rows.push(GraphRow { node, node_color, top, bottom, width });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn linear_history_is_one_lane() {
        let c = [("c", s(&["b"])), ("b", s(&["a"])), ("a", s(&[]))];
        let rows = layout(c.iter().map(|(h, p)| (*h, p.as_slice())));
        assert!(rows.iter().all(|r| r.node == 0 && r.width == 1));
        assert!(rows[2].bottom.is_empty());
    }

    #[test]
    fn branch_and_merge() {
        // m merges f into main: m(p1=b, p2=f), f(p=a), b(p=a), a
        let c = [("m", s(&["b", "f"])), ("f", s(&["a"])), ("b", s(&["a"])), ("a", s(&[]))];
        let rows = layout(c.iter().map(|(h, p)| (*h, p.as_slice())));
        assert_eq!(rows[0].node, 0);
        assert_eq!(rows[0].bottom.len(), 2, "merge commit fans out to two parents");
        assert_eq!(rows[1].node, 1, "feature commit on second lane");
        assert_eq!(rows[2].node, 0);
        // a is expected by both lanes → both converge into lane 0.
        assert_eq!(rows[3].node, 0);
        assert!(rows[3].top.iter().any(|s| s.from == 1 && s.to == 0));
        assert_eq!(rows[3].width, 2);
    }
}
