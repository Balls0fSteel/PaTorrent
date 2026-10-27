# Planning and changelog history

Last updated: 2026-10-27_095101_1793091061

Start with [PLAN.md](../../PLAN.md) and [CHANGELOG.md](../../CHANGELOG.md). Active decision rationale is in [DECISIONS.md](../DECISIONS.md); detailed unfinished work is in [BACKLOG.md](../BACKLOG.md).

These are historical snapshots, not current instructions or task lists. Load them only when a task needs older evidence. Links and paths inside the snapshots keep their original repository-root context; they have not been rewritten.

| Archive | Coverage | Contents |
|---|---|---|
| [Changelog, 2026-10-15 through 2026-10-23](CHANGELOG-2026-10-15--2026-10-23.md) | `2026-10-15_100350_1792051430` through `2026-10-23_155639_1792763799` | All 148 entries from the original changelog, preserved verbatim. Its two newest entries were also kept in the root changelog until 2026-10-26_105105_1793008265, when they were removed there to keep it under budget. |
| [Changelog, 2026-10-23 through 2026-10-26](CHANGELOG-2026-10-23--2026-10-26.md) | `2026-10-23_161421_1792764861` through `2026-10-26_101510_1793006110` | Six entries moved verbatim from the root changelog: the plan/changelog compaction and the UI icon exploration through the revision 3 review. |
| [Changelog, 2026-10-26 (icon revisions 4-6)](CHANGELOG-2026-10-26--2026-10-26.md) | `2026-10-26_101801_1793006281` through `2026-10-26_145432_1793022872` | Seven entries moved verbatim from the root changelog on 2026-10-27: the network/tracker icon decision, the revision 4-6 redraws and their independent reviews. |
| [Changelog, 2026-10-26 (icon integration, 1.0.0-rc.1)](CHANGELOG-2026-10-26--2026-10-26-rc1.md) | `2026-10-26_150634_1793023594` through `2026-10-26_215725_1793051845` | Three entries moved verbatim from the root changelog on 2026-10-27: the bold artwork integration, the 1.0.0-rc.1 version and release path, and the RSS date-parser fuzz fix. |
| [Changelog, 2026-10-27 (rc.1 published, installer, website)](CHANGELOG-2026-10-27--2026-10-27-installer.md) | `2026-10-27_065010_1793083810` through `2026-10-27_084916_1793087356` | Three entries moved verbatim from the root changelog on 2026-10-27: rc.1's publication, the version display and update check (D32), and the Windows installer and website (D31, D33). |
| [Plan snapshot, 2026-10-23](PLAN-2026-10-23.md) | Last updated `2026-10-23_155639_1792763799` | The complete original plan, including phase checklists, decisions D1-D26, resolved questions and historical TODO notes. Superseded claims remain as historical evidence. |

To locate an old event, search before opening the file:

```powershell
rg -n '^## ' docs/archive/CHANGELOG-2026-10-15--2026-10-23.md
rg -n -C 4 '2026-10-20_131729|super.seed' docs/archive/CHANGELOG-2026-10-15--2026-10-23.md
```

Keep archives as plain Markdown so Git diffs, repository browsing and targeted searches work without extraction. Add new dated files with unique names and update this index. Keep root documents around 16 KiB / 200 lines or less, moving complete older changelog entries and summarizing completed plan work before they exceed that budget. Never move unresolved work solely into a historical snapshot.
