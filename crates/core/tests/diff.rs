//! Line diffs, as the jobs view and config history show them.

use enodix_core::diff::{diff_lines, lines, DiffLine, MAX_DIFF_EDITS};

fn same(old: usize, new: usize, text: &str) -> DiffLine {
    DiffLine::Same {
        old,
        new,
        text: text.into(),
    }
}

fn removed(old: usize, text: &str) -> DiffLine {
    DiffLine::Removed {
        old,
        text: text.into(),
    }
}

fn added(new: usize, text: &str) -> DiffLine {
    DiffLine::Added {
        new,
        text: text.into(),
    }
}

/// The two texts a diff was made of, from its lines.
fn sides(diff: &[DiffLine]) -> (Vec<&str>, Vec<&str>) {
    let mut old = Vec::new();
    let mut new = Vec::new();
    for line in diff {
        match line {
            DiffLine::Same { text, .. } => {
                old.push(text.as_str());
                new.push(text.as_str());
            }
            DiffLine::Removed { text, .. } => old.push(text),
            DiffLine::Added { text, .. } => new.push(text),
        }
    }
    (old, new)
}

#[test]
fn identical_texts_are_all_the_same() {
    let text = "a\nb\nc\n";
    assert_eq!(
        diff_lines(text, text),
        [same(1, 1, "a"), same(2, 2, "b"), same(3, 3, "c")]
    );
    assert!(diff_lines("", "").is_empty());
}

#[test]
fn a_changed_line_is_removed_then_added_with_its_line_numbers() {
    let old = "interface e1\n description uplink\n mtu 1500\n";
    let new = "interface e1\n description core\n mtu 1500\n shutdown\n";
    assert_eq!(
        diff_lines(old, new),
        [
            same(1, 1, "interface e1"),
            removed(2, " description uplink"),
            added(2, " description core"),
            same(3, 3, " mtu 1500"),
            added(4, " shutdown"),
        ]
    );
}

#[test]
fn lines_only_in_one_text_at_either_end() {
    assert_eq!(
        diff_lines("b\nc", "a\nb\nc\nd"),
        [
            added(1, "a"),
            same(1, 2, "b"),
            same(2, 3, "c"),
            added(4, "d")
        ]
    );
    assert_eq!(diff_lines("x\n", ""), [removed(1, "x")]);
    assert_eq!(diff_lines("", "y"), [added(1, "y")]);
}

#[test]
fn crlf_and_lf_texts_compare_by_their_lines() {
    assert_eq!(lines("a\r\nb\r\n"), ["a", "b"]);
    assert_eq!(lines("a\n\nb"), ["a", "", "b"]);
    assert!(lines("\n").is_empty() || lines("\n") == [""]);
    assert!(diff_lines("a\r\nb\r\n", "a\nb")
        .iter()
        .all(|l| !l.differs()));
}

#[test]
fn the_diff_is_a_shortest_one() {
    // Moving one line: one removal and one addition, the rest shared.
    let old = "1\n2\n3\n4\n5\n6\n";
    let new = "2\n3\n4\n5\n6\n1\n";
    let diff = diff_lines(old, new);
    assert_eq!(diff.iter().filter(|l| l.differs()).count(), 2);
    assert_eq!(sides(&diff), (lines(old), lines(new)));
}

#[test]
fn every_line_of_both_texts_is_shown_in_order() {
    // Pseudo-random texts over a small alphabet, so that lines repeat.
    let mut seed = 7u32;
    let mut next = || {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        (seed >> 16) % 5
    };
    for _ in 0..200 {
        let old: Vec<String> = (0..next() * 3).map(|_| next().to_string()).collect();
        let new: Vec<String> = (0..next() * 3).map(|_| next().to_string()).collect();
        let (old, new) = (old.join("\n"), new.join("\n"));
        let diff = diff_lines(&old, &new);
        assert_eq!(sides(&diff), (lines(&old), lines(&new)), "{old:?} {new:?}");
        // Line numbers count up on each side.
        let (mut o, mut n) = (0, 0);
        for line in &diff {
            match *line {
                DiffLine::Same { old, new, .. } => {
                    assert_eq!((old, new), (o + 1, n + 1));
                    o = old;
                    n = new;
                }
                DiffLine::Removed { old, .. } => {
                    assert_eq!(old, o + 1);
                    o = old;
                }
                DiffLine::Added { new, .. } => {
                    assert_eq!(new, n + 1);
                    n = new;
                }
            }
        }
    }
}

#[test]
fn texts_differing_too_much_are_still_shown_whole() {
    let old: Vec<String> = (0..MAX_DIFF_EDITS + 10)
        .map(|i| format!("old {i}"))
        .collect();
    let new: Vec<String> = (0..MAX_DIFF_EDITS + 10)
        .map(|i| format!("new {i}"))
        .collect();
    let (old, new) = (
        format!("same\n{}\nend", old.join("\n")),
        format!("same\n{}\nend", new.join("\n")),
    );
    let diff = diff_lines(&old, &new);
    assert_eq!(diff.first(), Some(&same(1, 1, "same")));
    assert_eq!(diff.last().map(DiffLine::text), Some("end"));
    assert_eq!(sides(&diff), (lines(&old), lines(&new)));
}
