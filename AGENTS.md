# Repository instructions

If work can be parallelized and doing so could improve speed or quality, delegate it to subagents rather than doing everything yourself.

## Versioning

- Every change set that alters the programs bumps the patch version (D37): `[workspace.package]
  version` in `Cargo.toml` and `assemblyIdentity version` in
  `crates/win32/res/patorrent.exe.manifest`, in the same commit. Documentation-only changes
  need no bump. `PEER_ID_PREFIX` (`crates/core/src/version.rs`) is a separate counter, bumped
  only for an engine change that alters wire behaviour.

## Planning and history

- Read `PLAN.md` and `CHANGELOG.md` before substantial changes; keep updates concise and timestamped as `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).
- `PLAN.md` holds current status, next steps, release gates and open choices. Put detailed unresolved follow-ups in the relevant section of `docs/BACKLOG.md` and decision rationale in `docs/DECISIONS.md`. Preserve decision IDs and release-gate numbers when editing.
- `CHANGELOG.md` holds recent completed changes, issues, fixes and decisions. Record outcomes and meaningful validation; avoid repeating the plan or full command logs.
- Keep each root file around 16 KiB / 200 lines or less. Before it grows beyond that, move complete older changelog entries to a uniquely named dated Markdown archive under `docs/archive/` and update its index. Summarize completed plan work; preserve unfinished work in the active plan or backlog.
- `docs/archive/README.md` indexes historical snapshots. Do not load archived files during routine startup or implementation. Search for a specific date, timestamp or topic there only when history is needed, then read the relevant entry. Snapshot links and paths retain their original repository-root context.
- Do not rewrite dated snapshots. Keep historical records intact and correct superseded claims in the active documents instead.
