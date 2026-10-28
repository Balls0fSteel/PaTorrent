# Changelog archive: 2026-10-27 (interop harness, security close-out, SEC-07)

Three entries moved verbatim from the root changelog on 2026-10-28.

## 2026-10-27_224006_1793137206: Interop harness waits for qBittorrent's log

- `tools/swarm` (`lt-interop`): when the client takes no TCP, it waits for qBittorrent's log to report the torrent added (or finished, for a seed) plus 2 s, instead of a fixed 3 s; a uTP connect sent too early went unanswered for the whole connect timeout and hung the run. `LT_INTEROP_TRACE` also lists closed connections. Built and clippy-clean; not yet run against qBittorrent here.

## 2026-10-27_212434_1793132674: Security fixes: every SECURITY.md finding fixed or accepted

- **Fixed:** SEC-08 (Windows device names and bidi isolates in file names), SEC-09 (v2 Merkle trees capped at 2 GiB per torrent), SEC-11 (DHT replies matched by port; no loopback/LAN/port-0 endpoints from internet nodes), SEC-15 (HTTP proxies get host names by default, no local DHT name lookups, strict CONNECT status line), SEC-17 (owner-only settings files and folders on Unix, `UMask=0077`, `O_NOFOLLOW` pid file), SEC-18 (sanitized magnet names, Open Folder through the shell API, `--shell "%1"` associations).
- **Remainders closed:** SEC-04 (20 s TLS handshake deadline, linear JSON key check), SEC-05 (auto-load, IP filter outside the lock, save roots through links, generic fetch errors), SEC-07 (soft-fail revocation on SChannel and Secure Transport), SEC-12 (`no_connect_privileged_ports`, no loopback peers from trackers elsewhere; remote protocol 9), SEC-23 (warning for a user-writable exe folder), SEC-26 (no error oracle).
- **SEC-03 test:** `parallel_guesses_stop_at_the_ban_limit` failed on every run: a login refused by the ban also said "Fails.". A client banned while its request waited now gets 403, as at accept.
- **Accepted, recorded in SECURITY.md:** any folder for a Web UI login without `--webui-save-root`, no OpenSSL revocation, no Keychain/libsecret, LAN peers and privileged ports as in libtorrent, giant v2 torrents refused.
- **Validation:** fmt, clippy and the workspace tests pass on Windows; each new test was proven to fail with its fix reverted. On the Xubuntu VM the core, frontend and nox tests pass, the SEC-17 Unix tests included (2026-10-27_214505_1793133905). A portable client given a split `--shell` link opened locally and kept only the sanitized magnet. Secure Transport is compile-checked only; Open Folder was not clicked.
- **Linux import fix:** two SEC-16 import tests had failed on Linux since that fix: µTorrent's Windows paths were not split at `\`, so the save folder was lost. `split_windows_path` splits them on every platform.
- **Owner decision:** the remainders named in SEC-05, 07, 09, 12 and 17 are accepted as design choices. The audit stays local (`SECURITY-AUDIT.md`, untracked); the 38 unpushed commits were rewritten so the tracked SECURITY.md is the policy only, then pushed.
- **CI on the push (run 40581362457):** CI (macOS, Ubuntu, Windows, MSRV, fmt) and Fuzz green; the import fix also cleared the earlier macOS/Ubuntu failure. FreeBSD failed `the_control_url_stays_on_the_device`: it has no 127.0.0.2, so an accepted device was disabled by its failed connection. The test now checks that the external IP was asked for.
- **Hand checks:** macOS (temporary branch, run 40587141481) refuses `revoked-rsa-dv.ssl.com` online and from the cache; `revoked.badssl.com` gets no status there (soft-fail). The owner's Open Folder check on a portable build reached no share but showed the save folder's parent for a torrent without files yet; that fallback now opens the save folder itself.

## 2026-10-27_205407_1793130847: Security fix SEC-07: TLS sessions stay failed, clients need TLS 1.2, feeds need close_notify

- **Fix:** SChannel and OpenSSL clear `handshaking` only after the certificate check. `TlsStream` remembers a session error and returns it on every later call. Secure Transport requires its `verified` flag at the end of a client handshake. Clients require TLS 1.2 or later on all three back ends. `fetch` refuses an EOF-framed body when the TLS server closed without close_notify.
- **Tests:** four regressions (mock session, stand-in stream, a real SChannel client pinned to the wrong certificate), each proven to fail when reverted. The live HTTPS tests (`LT_NETWORK_TESTS=1`) pass. The OpenSSL and Secure Transport changes are compile-checked only. Still open: revocation.
