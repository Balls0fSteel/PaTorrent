//! What the GUIs' closing window (D47) says, toolkit-independent: the step the engine is on,
//! one row per move still to finish, and what Quit Now gives up. The Win32 GUI (`closing.rs`)
//! and the GTK GUI (`closing.rs`) draw it and drive [`patorrent_core::Exit`] themselves.

use std::time::Duration;

use patorrent_core::{Exit, ExitPhase, ExitStatus};

use crate::{fmt, tr};

/// How long a shutdown may take before the window shows.
pub const SHOW_AFTER: Duration = Duration::from_secs(1);

/// How often the window looks again.
pub const REFRESH: Duration = Duration::from_millis(200);

/// The question names this many moves; more are counted in one line.
const ASK_MOVES: usize = 2;

/// The window can go: the engine has ended, or Quit Now has got it past saving (the rest is
/// left to end with the process, [`Exit::abandon`]).
pub fn done(exit: &Exit, s: &ExitStatus) -> bool {
    exit.is_finished() || (s.hurried && s.phase >= ExitPhase::Closing)
}

/// The step's sentence.
pub fn phase_text(s: &ExitStatus) -> String {
    match s.phase {
        ExitPhase::Stopping => tr!(ExitStopping).to_owned(),
        ExitPhase::Moving => tr!(ExitMoving).to_owned(),
        ExitPhase::Saving => tr!(ExitSaving).to_owned(),
        ExitPhase::Trackers => tr!(ExitTrackers).to_owned(),
        ExitPhase::Closing => tr!(ExitWriting, size = fmt::size(s.unwritten)),
    }
}

/// "Closing for m:ss".
pub fn elapsed_text(secs: u64) -> String {
    tr!(ExitElapsed, time = format!("{}:{:02}", secs / 60, secs % 60))
}

/// The list's column titles.
pub fn columns() -> [&'static str; 3] {
    [tr!(ColName), tr!(ColDone), tr!(ExitColMoveTo)]
}

/// One row per move still to finish: name, progress (or waiting), destination.
pub fn rows(s: &ExitStatus) -> Vec<[String; 3]> {
    s.moves
        .iter()
        .map(|m| {
            let done = if m.started {
                tr!(ExitMoveProgress, done = fmt::size(m.done), total = fmt::size(m.total))
            } else {
                tr!(ExitMoveWaiting).to_owned()
            };
            [m.name.clone(), done, m.to.clone()]
        })
        .collect()
}

/// All the moves' progress together, 0.0 to 1.0; `None` when there is nothing to measure (a
/// bar running back and forth then).
pub fn fraction(s: &ExitStatus) -> Option<f64> {
    let (done, total) = s.moves.iter().fold((0u64, 0u64), |(d, t), m| (d + m.done, t + m.total));
    (s.phase == ExitPhase::Moving && total > 0).then(|| done as f64 / total as f64)
}

/// Whether Quit Now is offered: not while saving, which always finishes, nor once asked.
pub fn quit_offered(s: &ExitStatus) -> bool {
    !s.hurried && s.phase != ExitPhase::Saving
}

/// What Quit Now gives up now, one "• " line each; empty when nothing. Past two moves the rest
/// are counted, so the question stays readable with dozens.
pub fn consequences(s: &ExitStatus) -> Vec<String> {
    let named = if s.moves.len() > ASK_MOVES + 1 { ASK_MOVES } else { s.moves.len() };
    let mut out: Vec<String> = s.moves[..named]
        .iter()
        .map(|m| tr!(ExitAbortMove, name = m.name, folder = m.from, dest = m.to))
        .collect();
    if named < s.moves.len() {
        out.push(tr!(ExitAbortMoreMoves, count = s.moves.len() - named));
    }
    if s.phase <= ExitPhase::Trackers {
        out.push(tr!(ExitAbortTrackers).to_owned());
    }
    if s.unwritten > 0 {
        out.push(tr!(ExitAbortWrites, size = fmt::size(s.unwritten)));
    }
    out
}

/// The Quit Now question for `lines` ([`consequences`]); `None` when there is nothing to ask
/// about.
pub fn question(lines: &[String]) -> Option<String> {
    (!lines.is_empty()).then(|| tr!(ExitAskQuit, list = lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use patorrent_core::ExitMove;

    use super::*;

    fn moving(n: usize) -> ExitStatus {
        let moves = (0..n)
            .map(|i| ExitMove {
                name: format!("Debian {i}"),
                from: r"C:\Downloads".into(),
                to: r"\\nas\done".into(),
                done: 3 << 30,
                total: 4 << 30,
                started: i == 0,
            })
            .collect();
        ExitStatus { phase: ExitPhase::Moving, moves, unwritten: 1 << 20, hurried: false }
    }

    #[test]
    fn the_window_lists_every_move() {
        let s = moving(20);
        assert_eq!(phase_text(&s), tr!(ExitMoving));
        let rows = rows(&s);
        assert_eq!(rows.len(), 20);
        assert_eq!(
            rows[0],
            ["Debian 0".to_owned(), "3.00 GB of 4.00 GB".to_owned(), r"\\nas\done".to_owned()]
        );
        assert_eq!(rows[19][1], tr!(ExitMoveWaiting), "not started yet");
        assert_eq!(fraction(&s), Some(0.75));
        let closing = ExitStatus { phase: ExitPhase::Closing, moves: Vec::new(), ..moving(0) };
        assert!(phase_text(&closing).contains("1.00 MB"), "{}", phase_text(&closing));
        assert_eq!(fraction(&closing), None);
        assert_eq!(elapsed_text(65), "Closing for 1:05");
    }

    #[test]
    fn quit_now_lists_what_it_gives_up() {
        let all = consequences(&moving(1));
        assert_eq!(all.len(), 3, "{all:?}");
        assert!(
            all[0].contains("Debian 0") && all[0].contains(r"C:\Downloads") && all[0].contains(r"\\nas\done")
        );
        assert_eq!(all[1], tr!(ExitAbortTrackers));
        assert!(all[2].contains("1.00 MB"));
        // three moves are named; past that two are and the rest counted
        assert_eq!(consequences(&moving(3)).len(), 5);
        let many = consequences(&moving(20));
        assert_eq!(many.len(), 5, "{many:?}");
        assert!(many[1].contains("Debian 1"));
        assert_eq!(many[2], tr!(ExitAbortMoreMoves, count = 18));
        // writing the last data: only that; nothing left: no question at all
        let closing = ExitStatus { phase: ExitPhase::Closing, moves: Vec::new(), ..moving(0) };
        assert_eq!(consequences(&closing).len(), 1);
        let nothing = ExitStatus { phase: ExitPhase::Closing, ..Default::default() };
        assert!(consequences(&nothing).is_empty() && question(&[]).is_none());
        // not while saving, nor twice
        assert!(quit_offered(&moving(1)));
        assert!(!quit_offered(&ExitStatus { phase: ExitPhase::Saving, ..Default::default() }));
        assert!(!quit_offered(&ExitStatus { hurried: true, ..moving(1) }));
    }
}
