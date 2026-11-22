//! Whether a session window's page answers (#120): the shell's questions
//! and the page's answers, at the times given.

use std::time::{Duration, Instant};

use enodix_core::layout::{Answered, PageAnswers, WindowId, PAGE_ANSWER_WAIT, PAGE_ASK_EVERY};

const ONE: WindowId = WindowId(1);
const TWO: WindowId = WindowId(2);

/// Asks `windows` every `PAGE_ASK_EVERY` from `start` for `span`, as the
/// shell does, and returns the windows found not answering at each ask.
fn ask_for(
    answers: &mut PageAnswers,
    windows: &[WindowId],
    start: Instant,
    span: Duration,
    answering: &[WindowId],
) -> Vec<(Duration, Vec<WindowId>)> {
    let mut said = Vec::new();
    let mut at = Duration::ZERO;
    while at <= span {
        let now = start + at;
        for &window in windows {
            answers.ask(window, now);
            if answering.contains(&window) {
                answers.answered(window);
            }
        }
        let found = answers.not_answering(now);
        if !found.is_empty() {
            said.push((at, found));
        }
        at += PAGE_ASK_EVERY;
    }
    said
}

#[test]
fn a_page_that_answers_is_never_taken_for_silent() {
    let mut answers = PageAnswers::new();
    let said = ask_for(
        &mut answers,
        &[ONE],
        Instant::now(),
        Duration::from_secs(600),
        &[ONE],
    );
    assert!(said.is_empty(), "{said:?}");
}

#[test]
fn a_page_busy_for_less_than_the_wait_is_not_taken_for_silent() {
    let mut answers = PageAnswers::new();
    let start = Instant::now();
    // Busy: questions at 0, 2, 4, 6 and 8 s go unanswered, then all are
    // answered at 9.9 s.
    for second in [0, 2, 4, 6, 8] {
        answers.ask(ONE, start + Duration::from_secs(second));
        assert!(answers
            .not_answering(start + Duration::from_secs(second))
            .is_empty());
    }
    assert_eq!(answers.answered(ONE), Answered::AsBefore);
    let later = start + Duration::from_millis(9900) + PAGE_ANSWER_WAIT;
    assert!(answers.not_answering(later).is_empty());
}

#[test]
fn a_page_silent_for_the_wait_is_said_once_and_answers_again() {
    let mut answers = PageAnswers::new();
    let start = Instant::now();
    let said = ask_for(
        &mut answers,
        &[ONE, TWO],
        start,
        Duration::from_secs(30),
        &[TWO],
    );
    // Said once, when its first unanswered question is the wait old.
    assert_eq!(said, vec![(PAGE_ANSWER_WAIT, vec![ONE])]);
    assert!(answers.is_silent(ONE));
    assert!(!answers.is_silent(TWO));
    // It answers again: said once, and the watch starts afresh.
    assert_eq!(answers.answered(ONE), Answered::Again);
    assert_eq!(answers.answered(ONE), Answered::AsBefore);
    assert!(!answers.is_silent(ONE));
    let again = start + Duration::from_secs(40);
    let said = ask_for(&mut answers, &[ONE], again, Duration::from_secs(30), &[]);
    assert_eq!(said, vec![(PAGE_ANSWER_WAIT, vec![ONE])]);
}

#[test]
fn a_window_gone_is_forgotten() {
    let mut answers = PageAnswers::new();
    let start = Instant::now();
    answers.ask(ONE, start);
    answers.ask(TWO, start);
    answers.keep_only(&[TWO]);
    let found = answers.not_answering(start + PAGE_ANSWER_WAIT);
    assert_eq!(found, vec![TWO]);
    assert!(!answers.is_silent(ONE));
}
