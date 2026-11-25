//! The user's changes to the default keymap (Settings → Keymap, or a PyCharm keymap
//! imported once). An action listed here gets exactly these shortcuts instead of its
//! defaults; an empty list removes them. Double Shift is not affected.

use crate::{Action, Binding, Keymap, Keystroke, Trigger};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserKeymap {
    /// Action id → shortcuts, each "ctrl-alt-l" or a two-stroke "ctrl-k ctrl-c".
    #[serde(default)]
    pub bindings: BTreeMap<String, Vec<String>>,
}

/// "ctrl-k ctrl-c" for a chord.
pub fn chord_id(ks: &[Keystroke]) -> String {
    ks.iter().map(Keystroke::id).collect::<Vec<_>>().join(" ")
}

pub fn parse_chord(s: &str) -> Option<Vec<Keystroke>> {
    let v: Vec<Keystroke> = s.split_whitespace().map(Keystroke::parse).collect::<Option<_>>()?;
    (!v.is_empty() && v.iter().all(|k| !k.is_modifier_only())).then_some(v)
}

/// Two bindings can fire in the same place: one is global or both share a context.
fn overlaps(a: Option<&str>, b: Option<&str>) -> bool {
    a.is_none() || b.is_none() || a == b
}

/// Two shortcuts get in each other's way: the same, or one is the first stroke of the other
/// (with Ctrl+K and Ctrl+K, Ctrl+C both bound, only the chord fires).
fn clash(a: &[Keystroke], b: &[Keystroke]) -> bool {
    let n = a.len().min(b.len());
    a[..n] == b[..n]
}

impl Keymap {
    /// The keyboard shortcuts of an action (not Double Shift).
    pub fn chords_for(&self, action: Action) -> Vec<Vec<Keystroke>> {
        self.bindings
            .iter()
            .filter(|b| b.action == action)
            .filter_map(|b| match &b.trigger {
                Trigger::Keys(k) => Some(k.clone()),
                Trigger::DoubleTap(_) => None,
            })
            .collect()
    }

    /// Where an action's shortcuts apply: the context of its default bindings.
    pub fn context_of(&self, action: Action) -> Option<&'static str> {
        self.bindings.iter().find(|b| b.action == action).and_then(|b| b.context)
    }

    /// Other actions whose shortcuts clash with the chord where `action` works: the same
    /// keys, or one is the first stroke of the other.
    pub fn conflicts(&self, action: Action, chord: &[Keystroke]) -> Vec<Action> {
        let ctx = self.context_of(action);
        let mut v: Vec<Action> = self
            .bindings
            .iter()
            .filter(|b| b.action != action && overlaps(b.context, ctx))
            .filter(|b| matches!(&b.trigger, Trigger::Keys(k) if clash(k, chord)))
            .map(|b| b.action)
            .collect();
        v.dedup();
        v
    }
}

impl UserKeymap {
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// `base` with these changes applied. Unknown action ids and bad shortcuts are skipped.
    pub fn apply(&self, base: &Keymap) -> Keymap {
        let mut km = base.clone();
        for (id, chords) in &self.bindings {
            let Some(action) = Action::from_id(id) else { continue };
            let context = base.context_of(action);
            km.bindings.retain(|b| b.action != action || matches!(b.trigger, Trigger::DoubleTap(_)));
            for c in chords {
                if let Some(ks) = parse_chord(c) {
                    km.bindings.push(Binding { trigger: Trigger::Keys(ks), action, context });
                }
            }
        }
        km
    }

    /// Gives `action` these shortcuts. Back to the defaults, the entry disappears.
    pub fn set(&mut self, base: &Keymap, action: Action, chords: Vec<Vec<Keystroke>>) {
        let id = action.info().id.to_string();
        if chords == base.chords_for(action) {
            self.bindings.remove(&id);
        } else {
            self.bindings.insert(id, chords.iter().map(|c| chord_id(c)).collect());
        }
    }

    /// Adds a shortcut to `action` and takes it away from the actions it conflicts with
    /// (PyCharm's "Remove" answer). Returns those actions.
    pub fn assign(&mut self, base: &Keymap, action: Action, chord: Vec<Keystroke>) -> Vec<Action> {
        let current = self.apply(base);
        let taken = current.conflicts(action, &chord);
        for other in &taken {
            let rest: Vec<_> = current.chords_for(*other).into_iter().filter(|c| !clash(c, &chord)).collect();
            self.set(base, *other, rest);
        }
        let mut mine = current.chords_for(action);
        if !mine.contains(&chord) {
            mine.push(chord);
        }
        self.set(base, action, mine);
        taken
    }

    pub fn remove(&mut self, base: &Keymap, action: Action, chord: &[Keystroke]) {
        let rest = self.apply(base).chords_for(action).into_iter().filter(|c| c.as_slice() != chord).collect();
        self.set(base, action, rest);
    }

    pub fn reset(&mut self, action: Action) {
        self.bindings.remove(action.info().id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{pycharm_keymap, KeyResolver, Resolution};

    fn ks(s: &str) -> Vec<Keystroke> {
        parse_chord(s).unwrap()
    }

    #[test]
    fn override_conflict_and_reset() {
        let base = pycharm_keymap();
        let mut u = UserKeymap::default();
        // Ctrl+D is Duplicate Line in the editor; give it to Delete Line instead.
        let taken = u.assign(&base, Action::DeleteLine, ks("ctrl-d"));
        assert_eq!(taken, [Action::DuplicateLine]);
        let km = u.apply(&base);
        let mut r = KeyResolver::new(km.clone());
        assert_eq!(r.key_down(ks("ctrl-d").remove(0), &["Editor"]), Resolution::Action(Action::DeleteLine));
        assert_eq!(r.key_down(ks("ctrl-y").remove(0), &["Editor"]), Resolution::Action(Action::DeleteLine));
        assert!(km.chords_for(Action::DuplicateLine).is_empty());
        // Double Shift survives an override of Search Everywhere.
        u.set(&base, Action::SearchEverywhere, vec![ks("ctrl-alt-shift-s")]);
        assert!(u.apply(&base).triggers_for(Action::SearchEverywhere).iter().any(|t| matches!(t, Trigger::DoubleTap(_))));
        u.remove(&base, Action::DeleteLine, &ks("ctrl-y"));
        assert_eq!(u.apply(&base).chords_for(Action::DeleteLine), [ks("ctrl-d")]);
        u.reset(Action::DeleteLine);
        u.reset(Action::DuplicateLine);
        u.reset(Action::SearchEverywhere);
        assert!(u.is_empty());
        // Setting the defaults back leaves nothing stored.
        u.set(&base, Action::Rename, base.chords_for(Action::Rename));
        assert!(u.is_empty());
    }

    #[test]
    fn prefix_conflicts() {
        // Ctrl+K is Commit by default.
        let base = pycharm_keymap();
        let mut u = UserKeymap::default();
        // Either way round: the chord after the single stroke, or the single stroke after the chord.
        assert_eq!(base.conflicts(Action::CommentLine, &ks("ctrl-k ctrl-c")), [Action::Commit]);
        u.set(&base, Action::CommentLine, vec![ks("ctrl-k ctrl-c")]);
        assert!(u.apply(&base).conflicts(Action::Commit, &ks("ctrl-k")).contains(&Action::CommentLine));
        // Remove: the single stroke goes, the chord fires.
        let taken = u.assign(&base, Action::CommentLine, ks("ctrl-k ctrl-c"));
        assert_eq!(taken, [Action::Commit]);
        let km = u.apply(&base);
        assert!(km.chords_for(Action::Commit).is_empty());
        let mut r = KeyResolver::new(km);
        assert_eq!(r.key_down(ks("ctrl-k").remove(0), &["Editor"]), Resolution::Pending);
        assert_eq!(r.key_down(ks("ctrl-c").remove(0), &["Editor"]), Resolution::Action(Action::CommentLine));
    }

    #[test]
    fn json_round_trip() {
        let mut u = UserKeymap::default();
        u.bindings.insert("CommentLine".into(), vec!["ctrl-k ctrl-c".into()]);
        let text = serde_json::to_string(&u).unwrap();
        assert_eq!(text, r#"{"bindings":{"CommentLine":["ctrl-k ctrl-c"]}}"#);
        assert_eq!(serde_json::from_str::<UserKeymap>(&text).unwrap(), u);
    }
}
