//! Line diffs: which lines two texts share, and which one of them lacks.
//!
//! [`diff_lines`] compares two texts line by line (Myers' algorithm, after
//! the common start and end are set aside) and lists every line of both in
//! order, each [`DiffLine`] marked as in both, only in the old text or only
//! in the new one, with its 1-based line numbers. Line breaks are LF or
//! CRLF; a final line break does not make an empty last line.

use serde::Serialize;

/// How many differing lines the exact comparison handles in the part of
/// the texts that differs; beyond it, that part is shown as removed and
/// then added (still correct, only not the shortest).
pub const MAX_DIFF_EDITS: usize = 2000;

/// One line of a diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DiffLine {
    /// In both texts: its line number in each.
    Same {
        old: usize,
        new: usize,
        text: String,
    },
    /// Only in the old text.
    Removed { old: usize, text: String },
    /// Only in the new text.
    Added { new: usize, text: String },
}

impl DiffLine {
    /// The line's text.
    pub fn text(&self) -> &str {
        match self {
            Self::Same { text, .. } | Self::Removed { text, .. } | Self::Added { text, .. } => text,
        }
    }

    /// Whether the line is in one text only.
    pub fn differs(&self) -> bool {
        !matches!(self, Self::Same { .. })
    }
}

/// `text`'s lines, without their breaks (LF or CRLF).
pub fn lines(text: &str) -> Vec<&str> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

/// Every line of `old` and `new`, in order, marked as shared or not.
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffLine> {
    let a = lines(old);
    let b = lines(new);
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let middle = edits(&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);

    let mut diff = Vec::with_capacity(a.len().max(b.len()));
    let same = |i: usize, j: usize| DiffLine::Same {
        old: i + 1,
        new: j + 1,
        text: a[i].to_string(),
    };
    for i in 0..prefix {
        diff.push(same(i, i));
    }
    for edit in middle {
        diff.push(match edit {
            Edit::Same(i, j) => same(prefix + i, prefix + j),
            Edit::Removed(i) => DiffLine::Removed {
                old: prefix + i + 1,
                text: a[prefix + i].to_string(),
            },
            Edit::Added(j) => DiffLine::Added {
                new: prefix + j + 1,
                text: b[prefix + j].to_string(),
            },
        });
    }
    for n in 0..suffix {
        diff.push(same(a.len() - suffix + n, b.len() - suffix + n));
    }
    diff
}

#[derive(Debug, Clone, Copy)]
enum Edit {
    Same(usize, usize),
    Removed(usize),
    Added(usize),
}

/// A shortest list of edits turning `a` into `b` (Myers), or everything
/// removed then added if they differ in more than [`MAX_DIFF_EDITS`] lines.
fn edits(a: &[&str], b: &[&str]) -> Vec<Edit> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let fallback = || {
        (0..a.len())
            .map(Edit::Removed)
            .chain((0..b.len()).map(Edit::Added))
            .collect()
    };
    if n == 0 || m == 0 {
        return fallback();
    }
    let max = (n + m).min(MAX_DIFF_EDITS as isize);
    let offset = max + 1;
    let mut v = vec![0isize; 2 * offset as usize + 1];
    // trace[d] = v[-d..=d] before step d, to walk back from the end.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    for d in 0..=max {
        trace.push(v[(offset - d) as usize..=(offset + d) as usize].to_vec());
        for k in (-d..=d).step_by(2) {
            let at = |k: isize| (offset + k) as usize;
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                return walk_back(&trace, d, n, m);
            }
        }
    }
    fallback()
}

/// The edits of a path that reached the end at step `last`.
fn walk_back(trace: &[Vec<isize>], last: isize, n: isize, m: isize) -> Vec<Edit> {
    let mut edits = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..=last).rev() {
        let v = &trace[d as usize];
        let at = |k: isize| v[(k + d) as usize];
        let k = x - y;
        let previous = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let px = at(previous);
        let py = px - previous;
        while x > px && y > py {
            x -= 1;
            y -= 1;
            edits.push(Edit::Same(x as usize, y as usize));
        }
        if previous == k + 1 {
            edits.push(Edit::Added(py as usize));
        } else {
            edits.push(Edit::Removed(px as usize));
        }
        x = px;
        y = py;
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        edits.push(Edit::Same(x as usize, y as usize));
    }
    edits.reverse();
    edits
}
