//! Instant apply (D61): Preferences without OK, Cancel or Apply, as the macOS GUI has them. Each
//! control commits on its own (a check box when clicked, a text field when editing ends), and
//! what it holds is applied at once when it can be stored. A value that cannot be stored stays
//! in its control with a message under it and is not applied (the last good value stays); it
//! stays *pending* and is tried again at every later commit, so fixing another field (the proxy
//! host a proxy type needs) lets it through. A field whose value is fine but would fail
//! [`check`] together with the others (a proxy type without a host) is held the same way,
//! without failing the fields committed beside it.
//!
//! [`Live`] keeps what each control was last given ([`Live::write`]) and the pending fields;
//! [`Live::commit`] says what to apply and which messages to show; [`Live::revert`] gives the
//! pending fields' applied texts back when the window closes.

use std::collections::HashMap;

use super::{Field, Page, check, fields};
use crate::options::Options;

/// What one commit asks of the window.
#[derive(Debug, Default)]
pub struct Commit {
    /// The options to apply (`None`: nothing new to apply).
    pub options: Option<Options>,
    /// The messages to show under fields now (the others show none): a value that is not one
    /// (`Field::apply`'s message) under its field, and what keeps a held field back
    /// ([`check`]'s message) under the field [`check`] names.
    pub errors: Vec<(&'static str, String)>,
}

/// The fields' applied texts and the fields that wait.
#[derive(Clone, Debug, Default)]
pub struct Live {
    shown: HashMap<&'static str, String>,
    /// Edited and not applied: a bad value or a held one, in page order.
    pending: Vec<&'static str>,
}

impl Live {
    /// `o` as every control shows it, remembered as applied. Pending fields are left out: they
    /// keep what the user typed.
    pub fn write(&mut self, pages: &[Page], o: &Options) -> Vec<(&'static str, String)> {
        self.shown = fields(pages).map(|f| (f.key, f.shown(o))).collect();
        fields(pages).filter(|f| !self.pending.contains(&f.key)).map(|f| (f.key, f.shown(o))).collect()
    }

    /// The applied text of `key` (as last written).
    pub fn shown(&self, key: &str) -> Option<&str> {
        self.shown.get(key).map(String::as_str)
    }

    /// The fields edited and not applied.
    pub fn pending(&self) -> &[&'static str] {
        &self.pending
    }

    /// The control `key` committed: applies it and every pending field to `applied` with their
    /// controls' texts (`now`), in page order. A field whose text is its applied one is no
    /// longer pending.
    pub fn commit(
        &mut self,
        pages: &[Page],
        applied: &Options,
        key: &'static str,
        now: impl Fn(&'static str) -> String,
    ) -> Commit {
        // a stored set that [`check`] refuses already (an old ini) must not block every change
        let base = check(applied).err();
        let fine = |o: &Options| match check(o) {
            Ok(()) => true,
            Err(e) => Some(e) == base,
        };
        let mut new = applied.clone();
        let mut errors = Vec::new();
        let mut bad = Vec::new();
        let mut held: Vec<(&Field, String)> = Vec::new();
        let wanted = |k: &str| k == key || self.pending.contains(&k);
        for f in fields(pages).filter(|f| wanted(f.key)) {
            let value = now(f.key);
            if self.shown.get(f.key) == Some(&value) {
                continue;
            }
            let mut candidate = new.clone();
            match f.apply(&mut candidate, &value) {
                Err(message) => {
                    errors.push((f.key, message));
                    bad.push(f.key);
                }
                Ok(()) if fine(&candidate) => new = candidate,
                Ok(()) => held.push((f, value)),
            }
        }
        // a held field goes through once a later one made it fine (the type after its host)
        loop {
            let before = held.len();
            held.retain(|(f, value)| {
                let mut candidate = new.clone();
                if f.apply(&mut candidate, value).is_ok() && fine(&candidate) {
                    new = candidate;
                    false
                } else {
                    true
                }
            });
            if held.len() == before {
                break;
            }
        }
        if !held.is_empty() {
            let mut all = new.clone();
            for (f, value) in &held {
                let _ = f.apply(&mut all, value);
            }
            if let Err(e) = check(&all) {
                errors.push(e);
            }
        }
        let waiting: Vec<&'static str> = held.iter().map(|(f, _)| f.key).chain(bad).collect();
        self.pending = fields(pages).map(|f| f.key).filter(|k| waiting.contains(k)).collect();
        Commit { options: (new != *applied).then_some(new), errors }
    }

    /// Gives up the pending fields (the window closes): their applied texts, to put back.
    pub fn revert(&mut self) -> Vec<(&'static str, String)> {
        let pending = std::mem::take(&mut self.pending);
        pending.into_iter().map(|k| (k, self.shown.get(k).cloned().unwrap_or_default())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pref_pages::{Features, PROXY_TYPE, pages};
    use crate::prefs::Prefs;

    fn all() -> Vec<Page> {
        pages(Features { tray: true, desktop: true, packaged: false, updates: true, alternate_rows: true })
    }

    /// The controls: what was written, with `edits` over it.
    fn controls(live: &Live, edits: &[(&str, &str)]) -> impl Fn(&'static str) -> String + use<> {
        let mut values: HashMap<&'static str, String> =
            live.shown.iter().map(|(k, v)| (*k, v.clone())).collect();
        for (k, v) in edits {
            let key = *values.keys().find(|x| **x == *k).expect("a field");
            values.insert(key, (*v).to_owned());
        }
        move |k| values.get(k).cloned().unwrap_or_default()
    }

    #[test]
    fn a_change_applies_and_a_bad_number_waits() {
        let pages = all();
        let applied = Options::load(&Prefs::default(), "/dl");
        let mut live = Live::default();
        let written = live.write(&pages, &applied);
        assert!(written.iter().any(|(k, _)| *k == "port"));
        let port = live.shown("port").unwrap().to_owned();

        // nothing changed: nothing to apply
        let c = live.commit(&pages, &applied, "enable_dht", controls(&live, &[]));
        assert!(c.options.is_none() && c.errors.is_empty());

        // a port out of range: shown under the field, not applied
        let c = live.commit(&pages, &applied, "port", controls(&live, &[("port", "99999")]));
        assert!(c.options.is_none());
        assert_eq!(c.errors.len(), 1);
        assert_eq!(c.errors[0].0, "port");
        assert_eq!(live.pending(), ["port"]);

        // another box still applies; the port's message stays
        let c = live.commit(
            &pages,
            &applied,
            "enable_dht",
            controls(&live, &[("port", "99999"), ("enable_dht", "false")]),
        );
        let o = c.options.expect("DHT off applies");
        assert!(!o.settings.enable_dht);
        assert_eq!(
            crate::settings::listen_port(&o.settings),
            crate::settings::listen_port(&applied.settings)
        );
        assert_eq!(c.errors.iter().map(|e| e.0).collect::<Vec<_>>(), ["port"]);
        // written back, the port keeps the typed text
        let written = live.write(&pages, &o);
        assert!(!written.iter().any(|(k, _)| *k == "port"));

        // the window closes: the applied port goes back
        assert_eq!(live.revert(), [("port", port)]);
        assert!(live.pending().is_empty());
    }

    #[test]
    fn a_held_field_waits_for_the_one_it_needs() {
        let pages = all();
        let applied = Options::load(&Prefs::default(), "/dl");
        let mut live = Live::default();
        live.write(&pages, &applied);
        // SOCKS5 without a host: held, the message under the host
        let c = live.commit(&pages, &applied, PROXY_TYPE, controls(&live, &[(PROXY_TYPE, "2")]));
        assert!(c.options.is_none());
        assert_eq!(c.errors.iter().map(|e| e.0).collect::<Vec<_>>(), ["proxy_hostname"]);
        assert_eq!(live.pending(), [PROXY_TYPE]);
        // the host comes: it applies alone, the type now waits for a port
        let edits = [(PROXY_TYPE, "2"), ("proxy_hostname", "proxy.lan"), ("proxy_port", "1080")];
        let c = live.commit(&pages, &applied, "proxy_hostname", controls(&live, &edits[..2]));
        let applied = c.options.expect("the host applies");
        assert_eq!(applied.settings.proxy_hostname, "proxy.lan");
        assert_eq!(c.errors.iter().map(|e| e.0).collect::<Vec<_>>(), ["proxy_port"]);
        live.write(&pages, &applied);
        // the port comes: all three apply
        let c = live.commit(&pages, &applied, "proxy_port", controls(&live, &edits));
        let o = c.options.expect("type and port apply");
        assert_eq!(o.settings.proxy_port, 1080);
        assert_eq!(o.settings.proxy_type, rookparcel_core::ProxyType::Socks5);
        assert_eq!(o.settings.proxy_hostname, "proxy.lan");
        assert!(c.errors.is_empty() && live.pending().is_empty());
    }

    #[test]
    fn a_stored_problem_does_not_block_other_changes() {
        let pages = all();
        let mut applied = Options::load(&Prefs::default(), "/dl");
        applied.settings.proxy_type = rookparcel_core::ProxyType::Socks5;
        applied.settings.proxy_hostname.clear();
        assert!(check(&applied).is_err());
        let mut live = Live::default();
        live.write(&pages, &applied);
        let c = live.commit(&pages, &applied, "enable_lsd", controls(&live, &[("enable_lsd", "false")]));
        assert!(!c.options.expect("applies").settings.enable_lsd);
        // back to its applied text: no longer pending, no message
        let c = live.commit(&pages, &applied, "port", controls(&live, &[("port", "0")]));
        assert_eq!(live.pending(), ["port"]);
        assert!(c.options.is_none());
        let c = live.commit(&pages, &applied, "port", controls(&live, &[]));
        assert!(c.options.is_none() && c.errors.is_empty() && live.pending().is_empty());
    }
}
