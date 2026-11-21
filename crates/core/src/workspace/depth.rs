//! Refusing YAML nested very deep before it is parsed (#132).
//!
//! The YAML scanner takes time quadratic in the depth of nesting: a file of
//! 100 000 `[` took 35 s to be refused. No inventory file needs more than a
//! handful of levels (a host's `custom` or `serial` mapping is two, a saved
//! job's answers three), so a file nested deeper than [`MAX_NESTING`] is a
//! problem at once, found by one pass over its text.
//!
//! The pass knows enough YAML to tell nesting from text that merely holds
//! brackets: it skips quoted scalars, comments and block scalars (`|`,
//! `>`), takes `[` and `{` for nesting only where a value starts (or inside
//! a flow collection), and counts block nesting by indentation and by the
//! `- ` and `? ` indicators that open a level on the same line. It can only
//! count too few levels, never too many, for a file YAML reads.

/// The deepest nesting a workspace file may have.
pub(crate) const MAX_NESTING: usize = 32;

/// Where the pass is between lines.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Within {
    Nothing,
    /// A double-quoted scalar.
    Double,
    /// A single-quoted scalar.
    Single,
    /// A block scalar whose parent line is indented this much: lines
    /// indented more (and blank ones) are its text.
    Block(usize),
}

/// Whether `text` nests deeper than `limit` levels.
pub(crate) fn nested_deeper_than(text: &str, limit: usize) -> bool {
    let mut within = Within::Nothing;
    // The indentation of each open block level.
    let mut block: Vec<usize> = Vec::new();
    let mut flow = 0usize;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let chars: Vec<char> = line.chars().collect();
        let indent = chars.iter().take_while(|c| **c == ' ').count();
        let mut at = 0;
        match within {
            Within::Block(parent) => {
                if indent >= chars.len() || indent > parent {
                    continue;
                }
                within = Within::Nothing;
            }
            Within::Double => match skip_double(&chars, 0) {
                Some(end) => {
                    within = Within::Nothing;
                    at = end;
                }
                None => continue,
            },
            Within::Single => match skip_single(&chars, 0) {
                Some(end) => {
                    within = Within::Nothing;
                    at = end;
                }
                None => continue,
            },
            Within::Nothing => {}
        }
        // A new line outside any collection: its indentation says which
        // block level it is at.
        let mut node_start = at == 0;
        if at == 0 && flow == 0 {
            let rest: String = chars[indent..].iter().collect();
            if rest.is_empty() || rest.starts_with('#') {
                continue;
            }
            if rest.starts_with("---") || rest.starts_with("...") {
                block.clear();
                continue;
            }
            while block.last().is_some_and(|&open| open >= indent) {
                block.pop();
            }
            block.push(indent);
            at = indent;
        }
        let mut on_line = 0usize;
        let mut previous = if at == 0 { ' ' } else { chars[at - 1] };
        while at < chars.len() {
            let c = chars[at];
            let next = chars.get(at + 1).copied().unwrap_or(' ');
            let blank_next = next == ' ' || next == '\t';
            if block.len() + on_line + flow > limit {
                return true;
            }
            if c == ' ' || c == '\t' {
                previous = c;
                at += 1;
                continue;
            }
            if c == '#' && (previous == ' ' || previous == '\t' || at == 0) {
                break;
            }
            if node_start {
                match c {
                    '-' | '?' if flow == 0 && blank_next => {
                        on_line += 1;
                        at += 1;
                        previous = c;
                        continue;
                    }
                    '[' | '{' => {
                        flow += 1;
                        at += 1;
                        previous = c;
                        continue;
                    }
                    '"' => {
                        match skip_double(&chars, at + 1) {
                            Some(end) => at = end,
                            None => {
                                within = Within::Double;
                                break;
                            }
                        }
                        node_start = false;
                        previous = '"';
                        continue;
                    }
                    '\'' => {
                        match skip_single(&chars, at + 1) {
                            Some(end) => at = end,
                            None => {
                                within = Within::Single;
                                break;
                            }
                        }
                        node_start = false;
                        previous = '\'';
                        continue;
                    }
                    '&' | '!' => {
                        // An anchor or a tag: the node comes after it.
                        while at < chars.len() && chars[at] != ' ' {
                            at += 1;
                        }
                        continue;
                    }
                    '|' | '>' if flow == 0 => {
                        within = Within::Block(indent);
                        break;
                    }
                    _ => node_start = false,
                }
            }
            // Inside a plain scalar, or after a node.
            match c {
                ':' if blank_next || (flow > 0 && matches!(next, ',' | ']' | '}')) => {
                    node_start = true;
                }
                ',' if flow > 0 => node_start = true,
                '[' | '{' if flow > 0 => {
                    flow += 1;
                    node_start = true;
                }
                ']' | '}' if flow > 0 => flow -= 1,
                _ => {}
            }
            previous = c;
            at += 1;
        }
        if block.len() + on_line + flow > limit {
            return true;
        }
    }
    false
}

/// The position after the double-quoted scalar whose text starts at
/// `from`, or `None` if it goes on past the line.
fn skip_double(chars: &[char], from: usize) -> Option<usize> {
    let mut at = from;
    while at < chars.len() {
        match chars[at] {
            '\\' => at += 2,
            '"' => return Some(at + 1),
            _ => at += 1,
        }
    }
    None
}

/// The position after the single-quoted scalar whose text starts at
/// `from` (`''` is a quote inside it), or `None` if it goes on past the
/// line.
fn skip_single(chars: &[char], from: usize) -> Option<usize> {
    let mut at = from;
    while at < chars.len() {
        if chars[at] == '\'' {
            if chars.get(at + 1) == Some(&'\'') {
                at += 2;
                continue;
            }
            return Some(at + 1);
        }
        at += 1;
    }
    None
}
