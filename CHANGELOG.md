# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

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

## 2026-10-27_185951_1793123991: Security fix SEC-22: web seed chunk headers are limited

- **Confirmed:** a web seed's chunk header (size line, extensions, last-chunk trailers) had no length limit and was rescanned from its start on every read, so a server sending `1;` and then endless bytes cost unbounded memory and quadratic CPU.
- **Fix:** `parse_chunk_header` fails past `MAX_CHUNK_HEADER` (4 KiB), complete or not, and `WebPeer::feed` parses an incomplete header again only when a newline arrives (`chunk_scanned`). The other HTTP clients (`net/fetch`, `proto/http`) were already bounded.
- **Tests:** `chunk_headers_are_limited` and `a_dripping_chunk_header_is_cut_off` (a parse counter in test builds), each proven to fail without its half of the fix.

## 2026-10-27_183523_1793122523: Security fix SEC-21: a stalled metadata download starts over

- **Confirmed:** the first data message fixed the metadata size, so one peer answering with a false `total_size` (up to 30 MiB) made every honest block a mismatch and the magnet stayed in "downloading metadata" until restart.
- **Fix:** `MetadataFetch::stalled`, checked on each peer tick, drops a fetch whose blocks stopped coming for 60 s and pauses the peers that sent them (20-70 s, outstanding requests forgotten). The bad-data pause now also holds for peers that announced a `metadata_size`, where libtorrent's does not; this also changes the pause after a failed hash check.
- **Test:** `a_false_size_cannot_stall_the_fetch`, proven to fail without the watchdog and without the pause change. No engine-level test: it would need a 60 s wait.

## 2026-10-27_160928_1793117368: Security fix SEC-20: seed mode checks each piece before it uploads it

- **Confirmed:** `SEED_MODE` only marked every piece as had; nothing hashed them later, so a torrent added that way (Web UI `skip_checking`, the add dialogs) uploaded whatever the files held.
- **Fix:** a seed-mode torrent keeps the pieces it has verified. A request for an unverified piece waits while the disk hashes that piece once (new `DiskIo::async_verify`, the check's v1/v2 `PieceHasher`). A pass releases the waiting uploads; a failure ends seed mode with a full recheck (libtorrent `leave_seed_mode`). Once every piece passed, the torrent is an ordinary seed.
- **Restart:** while seed mode lasts, resume data saves no `pieces`, so the next start checks the files rather than trusting them for good.
- `Torrent` grew by 8 bytes (the boxed seed-mode state); the D21 size bound in `a_stopped_torrent_stays_compact` moved from 672 to 680.
- Validation: three end-to-end tests in `tests/super_seed.rs` (real data is uploaded after its check; other data of the right size uploads nothing and ends with 0 pieces; a restart checks instead of seeding). The last two fail with their part of the fix taken out. Core, frontend, nox and Win32 suites pass; workspace clippy and rustfmt clean.

## 2026-10-27_152737_1793114857: Security fix SEC-10: send buffers and the MSE handshake

- **Rejects:** a REJECT (peer wire, super seeding, ut_metadata) is queued only while the peer's send buffer is below `send_buffer_watermark`; past it the peer is not reading, and each further request used to add 17 bytes for good.
- **Backstop:** a peer whose send buffer passes `max(4 MiB, 4 × watermark)` is disconnected when it next sends ("peer does not read: send buffer full"); uploads and metadata stop near their own limits, so only a peer that does not read gets there.
- **MSE:** the peer's Diffie-Hellman key must be in `2..=p-2` (`PeError::InvalidDhKey`), and the sync point must lie within the 512-byte pad even when it all arrives in one read.
- Validation: a raw peer sending 1,000,000 invalid requests without reading gets about 30,000 rejects (all of them before); with the reject check disabled the test fails because the 4 MiB backstop drops the peer, so both layers are exercised. Unit tests for degenerate keys and pad offsets 0/512/513/5000. Core, frontend, nox and Win32 suites pass; workspace clippy and rustfmt clean.

## 2026-10-27_152058_1793114458: Security fix SEC-04: Web UI slots before login

- **Lingering close:** both (after an error answer and after a WebSocket close) now stop after 2 s in all, not 2 s per read; one client trickling a byte every 1.9 s used to hold a slot for up to 1 MiB of reads, and 64 of them locked everyone out.
- **Slots:** the acceptor admits 64 connections in all and 8 per address (an IPv6 /64 counting as one, like bans). Loopback has no per-address limit, so a reverse proxy on the same machine keeps working.
- **Bans at accept:** a banned address gets its 403 before anything is read and holds its slot only for the linger. Answering straight from the acceptor was tried first: Windows reset the connection over the unread request and the client lost the 403, and lingering in the acceptor would stall every accept. The banned text is now one constant (`api::BANNED`).
- Validation: unit tests for the slot accounting and the trickling client (2 s instead of about 60 s), the accept-time ban in the Web UI ban test; core, frontend, nox and Win32 suites pass; workspace clippy and rustfmt clean. Still open: an overall TLS handshake deadline, the JSON duplicate-key cost (after login).

## 2026-10-27_150953_1793113793: Security fix SEC-16: imports are checked, feeds cannot nest forever

- **Decision D36** (owner; supersedes D35's "trusting their pieces"): imported records lose their claimed and partial pieces, so the engine checks the files, and every `mapped_files` path is cleaned element by element into the save folder. The confirm question now lists the save folders (up to 12, then a count). A crafted backup can no longer get a private file of the right length seeded or map a download into Startup.
- **RSS:** start tags past the parser's depth limit are only counted, and an end tag closes one of them first; 200,000 `<a>` then 200,000 `</b>` used to mean about 4e10 name comparisons.
- **Behaviour change:** every import runs a full check (time once per import); files µTorrent had moved outside the save folder are downloaded again inside it. `nox run --import` help and the V1.1 plan line updated.
- Validation: unit tests for hostile `targets`/`mapped_files`, dropped pieces and the folder list; end-to-end tests on a real session (all pieces claimed over other data: none taken; a partial claim over complete data: all five found by the check); the RSS depth test. Three existing tests that asserted trusted pieces now assert the check. Core, frontend, nox and Win32 suites pass; workspace clippy and rustfmt clean. Moved three older entries to `docs/archive/CHANGELOG-2026-10-27--2026-10-27-v1.1-benchmarks.md`.

## 2026-10-27_144050_1793112050: Security fix SEC-14: remote login over HTTPS

- **Default:** Connect to Remote has Use HTTPS ticked unless the user turned it off before.
- **Plain HTTP:** `remote::connect` refuses it to any host but `localhost` or a loopback address unless `Target::allow_plaintext` is set (`ConnectError::Plaintext`, nothing sent). The Win32 and GTK dialogs set it only after the new `RemotePlaintext` question; nox's own default is still HTTP, so a LAN setup keeps working after one Yes.
- **Deadline:** reading the server's answer head now stops at the connect deadline; a server dripping one byte at a time held it for up to 16 KB of read timeouts.
- **Strings:** `RemotePlaintext` in en and the other 34 tables; the translations are machine-made and need native review (as with V1.1's strings).
- Validation: core tests for the refusal (nothing reaches a listener) and the dripping server (gives up at 1 s), and a frontend test for the default and the question; core, frontend, nox and Win32 suites and the language-table checks pass; workspace clippy and rustfmt clean; GTK clippy clean for Linux with a stand-in pkg-config. Not verified: either dialog by hand.
