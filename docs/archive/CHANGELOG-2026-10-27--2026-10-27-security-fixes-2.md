# Changelog archive: 2026-10-27, security fixes SEC-03 and SEC-23 to SEC-26

Moved verbatim from the root changelog on 2026-10-28.

## 2026-10-27_204307_1793130187: Security fix SEC-03: Web UI security settings need the current password; proxies get no bypass

- **Current password:** the Web UI's login and security settings (`SECURITY_FIELDS` plus the password) change only with `webui.current_password`, a new write-only field in the Preferences dialog's Authentication group, or `web_ui_current_password` in the shim. A wrong one ends the requesting session. This also closes SEC-24's open part. New strings `PrefCurrentPassword` and `PrefNeedCurrentPassword`, machine-translated into the 34 languages.
- **Reverse proxies:** forwarded requests (`X-Forwarded-For`, `Forwarded`, `X-Real-IP`) never get the local-auth bypass; behind a loopback proxy the last `X-Forwarded-For` address is the client for bans.
- **Ban race:** a login attempt counts as a failure before the PBKDF2 hash and is undone on success.
- **Docs:** docs/WEBUI.md's Security section describes the Origin rule as coded.
- **Tests:** three new Web UI tests, each part proven to fail when reverted. Existing tests that change the password or the domain list now send the current password. The harness takes a PBKDF2 iteration count.

## 2026-10-27_202906_1793129346: Security fix SEC-26: fetches stay off the local network unless the user named it

- **Confirmed:** add-by-URL and RSS fetches connected wherever a URL or redirect pointed, so a feed operator could make the client request loopback or LAN URLs.
- **Fix:** `FetchOptions::reach` (core `net/fetch.rs`). `Reach::User` (the default, for URLs the user gave) allows any address, but after a public address the redirects stay public. `Reach::Public`, for feed items unless their feed is itself local (`Reach::for_links_from`), allows public addresses only. `AddRequest` carries `feed_url`, and the worker's `Fetcher::download` now takes the request.
- **Tests:** `public_addresses`, `content_urls_reach_public_addresses_only`, `a_public_server_cannot_redirect_into_the_network` and `feed_items_stay_off_the_local_network`, proven to fail without the address check or the redirect lock.

## 2026-10-27_201937_1793128777: Security fix SEC-25: forwarded torrents come only from equal or higher integrity

- **Confirmed:** the main window let any process on the desktop send `WM_COPYDATA` (`MSGFLT_ALLOW`), so a sandboxed process could add magnets, URLs and paths, silently when the Add dialog is off. A null `lpData` reached `slice::from_raw_parts`.
- **Fix:** `copydata_args` checks the pointer, alignment and size; the window sets `MSGFLT_DISALLOW`. A second instance that Windows refuses (access denied: the first runs as administrator) shows `ForwardRefused` instead of losing the torrent. The new string is in all 35 `.lng` files; the 34 translations are machine-made.
- **Tests:** `copydata_args_checks_its_data` and `lower_integrity_processes_cannot_forward`, which starts the test binary at low integrity with `CreateProcessAsUserW` and checks a control window gets the message while the guarded one does not. Each was proven to fail without its part of the fix.

## 2026-10-27_201104_1793128264: Security fix SEC-24: a new Web UI password ends the other logins

- **Confirmed:** a password change kept every SID session, open WebSocket and `/pa/remote` connection, so a stolen session survived the change meant to lock it out.
- **Fix:** `prefs::commit` now takes the requester's session key (`apply_preferences`, `commands::serve` pass it). A new password drops every other session, whose WebSockets close at the next status tick, and disconnects every remote client (`Shared::end_remotes`; the remote client's `overflow` flag became `ended`).
- **Tests:** `a_new_password_ends_the_other_logins` and `a_new_password_ends_remote_sessions`, each proven to fail without its part. Still open: asking for the old password.

## 2026-10-27_200447_1793127887: Security fix SEC-23: the service install checks an existing state folder

- **Confirmed:** `service install` took an existing `%ProgramData%\PaTorrent\nox` as it was. Any user can make that folder first and keep owning it, so they could plant `webui.json`, read the log, or make `Downloads` a junction.
- **Fix:** the install refuses when an existing parent, the folder or anything in it has an untrusted owner (anyone but SYSTEM, Administrators, the service accounts, TrustedInstaller or the installer), or holds a junction or link. Otherwise the folder's ACL is reset to the protected one before its contents are checked. Needs the `Win32_System_Threading` feature of windows-sys (`OpenProcessToken`).
- **Tests:** `a_state_dir_from_another_user_is_refused` and `an_existing_state_dir_gets_the_protected_acl`, proven to fail with the old behaviour; the refusal test also fails without the contents walk. Still open: a warning for an exe folder that non-admins can write.
- **Seen once:** core's `keys_left_by_processes_that_are_gone_are_swept` failed in one full run and passed alone three times and in the next full run; it uses a fixed key name in the user's key store.
