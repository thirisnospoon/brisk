use super::*;

fn buf(t: &str) -> Buffer {
    Buffer::new(t)
}

#[test]
fn typing_and_undo_grouping() {
    let mut b = buf("");
    for c in "hello".chars() {
        b.type_char(c);
    }
    assert_eq!(b.text(), "hello");
    b.undo();
    assert_eq!(b.text(), "", "consecutive typing is one undo step");
    b.redo();
    assert_eq!(b.text(), "hello");
}

#[test]
fn auto_pairs() {
    let mut b = buf("");
    b.type_char('(');
    assert_eq!(b.text(), "()");
    assert_eq!(b.primary().head, 1);
    b.type_char(')');
    assert_eq!(b.text(), "()");
    assert_eq!(b.primary().head, 2);

    let mut b = buf("x");
    b.set_caret(1, false);
    b.type_char('\'');
    assert_eq!(b.text(), "x'", "no pair after identifier char");
}

#[test]
fn backspace_removes_empty_pair_and_indent() {
    let mut b = buf("()");
    b.set_caret(1, false);
    b.backspace();
    assert_eq!(b.text(), "");

    let mut b = buf("        x");
    b.set_caret(8, false);
    b.backspace();
    assert_eq!(b.text(), "    x");
}

#[test]
fn smart_newline_python() {
    let mut b = buf("def f():");
    b.set_caret(8, false);
    b.newline();
    assert_eq!(b.text(), "def f():\n    ");
    assert_eq!(b.primary().head, 13);

    let mut b = buf("f()");
    b.set_caret(2, false);
    b.newline();
    assert_eq!(b.text(), "f(\n    \n)");
    assert_eq!(b.pos_to_line_col(b.primary().head), (1, 4));
}

#[test]
fn duplicate_and_delete_line() {
    let mut b = buf("a\nbc\nd");
    b.set_caret(3, false); // line 1, col 1
    b.duplicate();
    assert_eq!(b.text(), "a\nbc\nbc\nd");
    assert_eq!(b.pos_to_line_col(b.primary().head), (2, 1));
    b.delete_lines();
    assert_eq!(b.text(), "a\nbc\nd");
    b.set_caret(b.len_chars(), false);
    b.delete_lines();
    assert_eq!(b.text(), "a\nbc");
}

#[test]
fn move_lines() {
    let mut b = buf("one\ntwo\nthree");
    b.set_caret(5, false); // "two", col 1
    b.move_lines(true);
    assert_eq!(b.text(), "two\none\nthree");
    assert_eq!(b.pos_to_line_col(b.primary().head), (0, 1));
    b.move_lines(false);
    b.move_lines(false);
    assert_eq!(b.text(), "one\nthree\ntwo");
    assert_eq!(b.pos_to_line_col(b.primary().head), (2, 1));
}

#[test]
fn comment_toggle() {
    let mut b = buf("    x = 1\n    y = 2");
    b.select_all();
    b.toggle_line_comment();
    assert_eq!(b.text(), "#     x = 1\n#     y = 2");
    b.toggle_line_comment();
    assert_eq!(b.text(), "    x = 1\n    y = 2");
}

#[test]
fn comment_toggle_blank_lines_and_partial_selection() {
    // Blank lines inside the selection stay as they are, both ways.
    let mut b = buf("def f():\n\n    return 1\n");
    b.select_all();
    b.toggle_line_comment();
    assert_eq!(b.text(), "# def f():\n\n#     return 1\n");
    b.toggle_line_comment();
    assert_eq!(b.text(), "def f():\n\n    return 1\n");
    // Selection ending at column 0 of the next line leaves that line alone.
    let mut b = buf("a = 1\nb = 2\nc = 3");
    b.selections = vec![Selection { anchor: 0, head: 12, goal_col: None }];
    b.toggle_line_comment();
    assert_eq!(b.text(), "# a = 1\n# b = 2\nc = 3");
    // Only blank lines: they get the marker (PyCharm does the same).
    let mut b = buf("x\n\n\ny");
    b.selections = vec![Selection { anchor: 2, head: 4, goal_col: None }];
    b.toggle_line_comment();
    assert_eq!(b.text(), "x\n# \n# \ny");
    // Mixed: some commented, some not -> all get commented; "#x" without a space uncomments.
    let mut b = buf("#x\ny");
    b.select_all();
    b.toggle_line_comment();
    assert_eq!(b.text(), "# #x\n# y");
    let mut b = buf("#x\n#y");
    b.select_all();
    b.toggle_line_comment();
    assert_eq!(b.text(), "x\ny");
}

#[test]
fn multi_caret_insert() {
    let mut b = buf("a\nb\nc");
    b.selections = vec![Selection::caret(1), Selection::caret(3), Selection::caret(5)];
    b.insert("!");
    assert_eq!(b.text(), "a!\nb!\nc!");
    assert_eq!(b.selections.iter().map(|s| s.head).collect::<Vec<_>>(), vec![2, 5, 8]);
    b.undo();
    assert_eq!(b.text(), "a\nb\nc");
}

#[test]
fn alt_j_occurrences() {
    let mut b = buf("foo bar foo baz foo");
    b.set_caret(1, false);
    b.add_next_occurrence();
    assert_eq!(b.selected_text(), "foo");
    b.add_next_occurrence();
    b.add_next_occurrence();
    assert_eq!(b.selections.len(), 3);
    b.insert("x");
    assert_eq!(b.text(), "x bar x baz x");
}

#[test]
fn word_movement() {
    let b = buf("self.value = 42");
    assert_eq!(b.word_boundary(0, true), 4);
    assert_eq!(b.word_boundary(4, true), 5);
    assert_eq!(b.word_boundary(5, true), 11);
    assert_eq!(b.word_boundary(15, false), 13);
}

#[test]
fn smart_home_and_goal_column() {
    let mut b = buf("    abc\nx\n    defgh");
    b.set_caret(7, false);
    b.move_carets(Movement::LineStart, false);
    assert_eq!(b.primary().head, 4);
    b.move_carets(Movement::LineStart, false);
    assert_eq!(b.primary().head, 0);

    b.set_caret(7, false); // col 7
    b.move_carets(Movement::Down, false);
    assert_eq!(b.pos_to_line_col(b.primary().head), (1, 1));
    b.move_carets(Movement::Down, false);
    assert_eq!(b.pos_to_line_col(b.primary().head), (2, 7), "goal column preserved");
}

#[test]
fn crlf_roundtrip() {
    let b = buf("a\r\nb\r\n");
    assert_eq!(b.text(), "a\nb\n");
    assert_eq!(b.text_for_save(), "a\r\nb\r\n");
}

#[test]
fn indent_unindent_selection() {
    let mut b = buf("a\nb");
    b.select_all();
    b.indent();
    assert_eq!(b.text(), "    a\n    b");
    b.unindent();
    assert_eq!(b.text(), "a\nb");
}

#[test]
fn join_and_case() {
    let mut b = buf("x = (\n    1)");
    b.join_lines();
    assert_eq!(b.text(), "x = ( 1)");
    let mut b = buf("abc");
    b.select_all();
    b.toggle_case();
    assert_eq!(b.text(), "ABC");
}

#[test]
fn replace_ranges_is_one_undo_step() {
    let mut b = Buffer::new("import os\nx == None\n");
    b.set_caret(19, false); // end of the second line
    // Delete the first line and rewrite the comparison in one quick fix.
    b.replace_ranges(vec![(0..10, String::new()), (10..19, "x is None".into())]);
    assert_eq!(b.text(), "x is None\n");
    assert_eq!(b.primary().head, 9);
    b.undo();
    assert_eq!(b.text(), "import os\nx == None\n");
}
