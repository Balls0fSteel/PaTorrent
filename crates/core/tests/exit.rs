//! D47: a session closing in the background reports its steps and ends; "Quit now" is harmless
//! when nothing is in flight.

use std::time::{Duration, Instant};

use patorrent_core::{ExitPhase, Session, SessionParams, Settings};

fn session() -> Session {
    let settings = Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        ..Settings::default()
    };
    Session::start(SessionParams { settings, state_dir: None, ..Default::default() }).unwrap()
}

#[test]
fn a_background_shutdown_reaches_closing_and_ends() {
    for hurry in [false, true] {
        let exit = session().begin_shutdown();
        if hurry {
            exit.hurry();
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while !exit.is_finished() {
            assert!(Instant::now() < deadline, "the engine thread ends");
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = exit.status();
        assert_eq!(status.phase, ExitPhase::Closing);
        assert!(status.moves.is_empty());
        assert_eq!(status.unwritten, 0);
        assert_eq!(status.hurried, hurry);
        exit.wait();
    }
}
