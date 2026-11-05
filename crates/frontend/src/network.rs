//! "Use only this network" (D54): the engine host's adapters as the Preferences drop-down lists
//! them, the rule's text from the controls and back, and what the status bar and the General
//! tab say about it ("Waiting for the network", "Your address as trackers see it").
//!
//! The adapters are the engine's (the remote nox's in remote mode): a frontend passes every
//! alert to [`on_alert`], which keeps the last [`Alert::NetworkAdapters`] and session
//! statistics, and asks for the adapters with `SessionHandle::post_network_adapters` when it
//! starts and when Preferences opens.

use std::sync::Mutex;

use rookparcel_core::{Alert, NetAdapter, NetworkRule, NetworkStatus, SessionStats};

use crate::tr;

/// What the engine said last.
struct Known {
    adapters: Vec<NetAdapter>,
    status: NetworkStatus,
    external: [Option<std::net::IpAddr>; 2],
}

static KNOWN: Mutex<Known> =
    Mutex::new(Known { adapters: Vec::new(), status: NetworkStatus::Any, external: [None, None] });

fn known<T>(f: impl FnOnce(&mut Known) -> T) -> T {
    f(&mut KNOWN.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Follows the engine's alerts (the adapters and the session statistics).
pub fn on_alert(a: &Alert) {
    match a {
        Alert::NetworkAdapters(v) => set_adapters(v.clone()),
        Alert::SessionStats(s) => on_stats(s),
        _ => {}
    }
}

/// Sets the adapters the drop-down lists.
pub fn set_adapters(v: Vec<NetAdapter>) {
    known(|k| k.adapters = v);
}

/// The adapters last reported.
pub fn adapters() -> Vec<NetAdapter> {
    known(|k| k.adapters.clone())
}

fn on_stats(s: &SessionStats) {
    known(|k| {
        k.status = s.network;
        k.external = [s.external_v4, s.external_v6];
    });
}

/// The rule drop-down's value for "Address range": the text box beside it holds the range.
pub const RANGE: &str = "range";

/// The drop-down's value for the rule `rule` (`network_rule`): empty (off), `vpn`, [`RANGE`]
/// or `adapter:<id>`.
pub fn choice_of(rule: &str) -> String {
    match NetworkRule::parse(rule) {
        Ok(NetworkRule::Range(..)) => RANGE.to_owned(),
        Ok(r) => r.to_string(),
        Err(_) if rule.trim_start().starts_with("range:") => RANGE.to_owned(),
        Err(_) => rule.to_owned(),
    }
}

/// The rule after the drop-down was set to `choice`; "Address range" keeps a range already set.
pub fn rule_for_choice(rule: &str, choice: &str) -> String {
    if choice == RANGE {
        if rule.starts_with("range:") { rule.to_owned() } else { "range:".to_owned() }
    } else {
        choice.to_owned()
    }
}

/// The range text box's text for `rule` (empty unless it is a range).
pub fn range_of(rule: &str) -> String {
    rule.strip_prefix("range:").unwrap_or_default().to_owned()
}

/// The rule after the range text box was set to `text`: only a range rule takes it.
pub fn rule_for_range(rule: &str, text: &str) -> String {
    if rule.starts_with("range:") { format!("range:{}", text.trim()) } else { rule.to_owned() }
}

/// Why `rule` cannot be stored, for the user: a range that is not one.
pub fn check_rule(rule: &str) -> Result<(), String> {
    match NetworkRule::parse(rule) {
        Ok(_) => Ok(()),
        Err(_) if rule.starts_with("range:") => Err(tr!(PrefBadRange, range = range_of(rule))),
        Err(e) => Err(e),
    }
}

/// The drop-down's `(value, text)` choices: off, any VPN adapter, an address range, then each
/// adapter by name (loopback left out). An adapter the rule names that is not there now is
/// listed too, as not connected, so the setting shows as it is.
pub fn rule_choices(rule: &str) -> Vec<(String, String)> {
    let mut out = vec![
        (String::new(), tr!(NetRuleOff).to_owned()),
        ("vpn".to_owned(), tr!(NetRuleVpn).to_owned()),
        (RANGE.to_owned(), tr!(NetRuleRange).to_owned()),
    ];
    let adapters = adapters();
    for a in &adapters {
        if a.addresses.iter().all(|ip| ip.is_loopback()) && !a.addresses.is_empty() {
            continue;
        }
        out.push((format!("adapter:{}", a.id), a.name.clone()));
    }
    let current = choice_of(rule);
    if !out.iter().any(|(v, _)| *v == current) {
        let name = current.strip_prefix("adapter:").unwrap_or(&current).to_owned();
        out.push((current, tr!(NetAdapterMissing, name)));
    }
    out
}

/// "Waiting for the network" while the rule finds no adapter, for the status bar's network part
/// (before the rest of its text).
pub fn status_text(s: &SessionStats) -> Option<String> {
    matches!(s.network, NetworkStatus::Waiting | NetworkStatus::Missing).then(|| tr!(NetWaiting).to_owned())
}

/// The status bar's network part: the waiting notice before `rest` while the network is
/// missing.
pub fn with_status(s: &SessionStats, rest: String) -> String {
    match status_text(s) {
        Some(w) => format!("{w} | {rest}"),
        None => rest,
    }
}

/// The addresses trackers see, or "not known yet".
pub fn external_text(s: &SessionStats) -> String {
    let v: Vec<String> = [s.external_v4, s.external_v6].iter().flatten().map(|a| a.to_string()).collect();
    if v.is_empty() { tr!(NetExternalUnknown).to_owned() } else { v.join(", ") }
}

/// The status bar's network tooltip: "Your address as trackers see it: ...".
pub fn tooltip(s: &SessionStats) -> String {
    tr!(NetExternal, address = external_text(s))
}

/// The General tab's last line: the label and the addresses, from the last statistics.
pub fn general_row() -> (&'static str, String) {
    let s = known(|k| SessionStats {
        network: k.status,
        external_v4: k.external[0],
        external_v6: k.external[1],
        ..SessionStats::default()
    });
    (tr!(GenExternal), external_text(&s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_controls_compose_the_rule() {
        assert_eq!(choice_of(""), "");
        assert_eq!(choice_of("vpn"), "vpn");
        assert_eq!(choice_of("adapter:wg0"), "adapter:wg0");
        assert_eq!(choice_of("range:10.2.0.0/16"), RANGE);
        assert_eq!(choice_of("range:"), RANGE, "a range being typed");
        // the drop-down, then the text box (the order a dialog applies them in)
        let r = rule_for_choice("vpn", RANGE);
        assert_eq!(r, "range:");
        let r = rule_for_range(&r, " 10.2.0.0/16 ");
        assert_eq!(r, "range:10.2.0.0/16");
        assert_eq!(rule_for_choice(&r, RANGE), r, "a range is kept");
        assert_eq!(range_of(&r), "10.2.0.0/16");
        assert_eq!(rule_for_range("vpn", "10.0.0.0/8"), "vpn", "the box only counts for a range");
        assert_eq!(rule_for_choice(&r, ""), "");
        assert!(check_rule("range:10.2.0.0/16").is_ok());
        assert!(check_rule("range:").is_err());
        assert!(check_rule("range:10.2.0.0/40").is_err());
        assert!(check_rule("").is_ok());
    }

    #[test]
    fn choices_list_the_adapters_and_a_missing_one() {
        set_adapters(vec![
            NetAdapter {
                id: "lo".into(),
                name: "lo".into(),
                vpn: false,
                addresses: vec!["127.0.0.1".parse().unwrap()],
            },
            NetAdapter { id: "{G}".into(), name: "Mullvad".into(), vpn: true, addresses: vec![] },
        ]);
        let c = rule_choices("adapter:{G}");
        let values: Vec<&str> = c.iter().map(|(v, _)| v.as_str()).collect();
        assert_eq!(values, ["", "vpn", RANGE, "adapter:{G}"]);
        assert_eq!(c[3].1, "Mullvad");
        let c = rule_choices("adapter:tun9");
        assert_eq!(c.last().unwrap().0, "adapter:tun9");
        assert!(c.last().unwrap().1.contains("tun9"));
    }

    #[test]
    fn the_status_bar_and_tooltip() {
        let mut s = SessionStats::default();
        assert_eq!(status_text(&s), None);
        assert_eq!(with_status(&s, "Network OK".into()), "Network OK");
        s.network = NetworkStatus::Waiting;
        assert_eq!(with_status(&s, "Network OK".into()), "Waiting for the network | Network OK");
        assert_eq!(tooltip(&s), "Your address as trackers see it: not known yet");
        s.external_v4 = Some("203.0.113.7".parse().unwrap());
        s.external_v6 = Some("2001:db8::7".parse().unwrap());
        assert_eq!(external_text(&s), "203.0.113.7, 2001:db8::7");
    }
}
