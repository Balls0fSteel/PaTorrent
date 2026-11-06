//! "Start Rookparcel on system startup": the per-profile LaunchAgent,
//! `~/Library/LaunchAgents/<id>[.<profile>].plist` with `RunAtLoad`, written and read as the GTK
//! bundle does (`frontend::integration::mac`: the same labels and the bundle's executable,
//! `Rookparcel.app/Contents/MacOS/Rookparcel`, so an agent the GTK bundle wrote keeps working when
//! the native app replaces it). "Start minimized" adds `--minimized`. A box is checked only when
//! the agent starts this very program in this profile. macOS 13 and later list the agent in
//! System Settings > General > Login Items and announce it when it is written.

use std::path::PathBuf;

use objc2_foundation::NSBundle;
use rookparcel_frontend::integration::MINIMIZED;
use rookparcel_frontend::integration::mac::{agent_label, agent_path, launch_agent, launch_agent_arguments};
use rookparcel_frontend::profile;

/// The home folder the agent goes below (`HOME`, as launchd and Finder see it).
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_absolute())
}

fn path() -> Option<PathBuf> {
    Some(agent_path(&home()?, profile::current().name.as_deref()))
}

/// What the LaunchAgent starts: the bundle's executable (this executable outside a bundle), in
/// a named profile with `--profile <name>` (D42).
pub fn command() -> Vec<String> {
    let bundle = NSBundle::mainBundle();
    let from_bundle =
        bundle.bundleIdentifier().and_then(|_| bundle.executablePath()).map(|p| PathBuf::from(p.to_string()));
    let Some(program) = from_bundle.or_else(|| std::env::current_exe().ok()) else { return Vec::new() };
    let mut c = vec![program.to_string_lossy().into_owned()];
    c.extend(profile::current().args());
    c
}

/// The agent's arguments after the program and profile, when it starts this program.
fn agent_args(command: &[String]) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(path()?).ok()?;
    let args = launch_agent_arguments(&text)?;
    (!command.is_empty() && args.starts_with(command)).then(|| args[command.len()..].to_vec())
}

/// (autostart, minimized) as the agent has them now.
pub fn read() -> (bool, bool) {
    match agent_args(&command()) {
        Some(rest) => (true, rest.iter().any(|a| a == MINIMIZED)),
        None => (false, false),
    }
}

/// Writes the agent (with `--minimized` when `minimized`) or removes it (`Err`: the file's
/// error).
pub fn write(autostart: bool, minimized: bool) -> Result<(), String> {
    let Some(path) = path() else { return Err("HOME".into()) };
    let fail = |e: std::io::Error| format!("{}: {e}", path.display());
    if !autostart {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(fail(e)),
            _ => Ok(()),
        };
    }
    let mut program = command();
    if program.is_empty() {
        return Err("current_exe".into());
    }
    if minimized {
        program.push(MINIMIZED.to_owned());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let label = agent_label(profile::current().name.as_deref());
    std::fs::write(&path, launch_agent(&label, &program)).map_err(fail)
}
