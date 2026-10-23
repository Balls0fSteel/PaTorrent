# Planning and changelog history

Last updated: 2026-10-23_161421_1792764861

Start with [PLAN.md](../../PLAN.md) and [CHANGELOG.md](../../CHANGELOG.md). Active decision rationale is in [DECISIONS.md](../DECISIONS.md); detailed unfinished work is in [BACKLOG.md](../BACKLOG.md).

These are historical snapshots, not current instructions or task lists. Load them only when a task needs older evidence. Links and paths inside the snapshots keep their original repository-root context; they have not been rewritten.

| Archive | Coverage | Contents |
|---|---|---|
| [Changelog, 2026-10-15 through 2026-10-23](CHANGELOG-2026-10-15--2026-10-23.md) | `2026-10-15_100350_1792051430` through `2026-10-23_155639_1792763799` | All 148 entries from the original changelog, preserved verbatim. The two newest entries also remain in the root changelog for the current handoff. |
| [Plan snapshot, 2026-10-23](PLAN-2026-10-23.md) | Last updated `2026-10-23_155639_1792763799` | The complete original plan, including phase checklists, decisions D1-D26, resolved questions and historical TODO notes. Superseded claims remain as historical evidence. |

To locate an old event, search before opening the file:

```powershell
rg -n '^## ' docs/archive/CHANGELOG-2026-10-15--2026-10-23.md
rg -n -C 4 '2026-10-20_131729|super.seed' docs/archive/CHANGELOG-2026-10-15--2026-10-23.md
```

Keep archives as plain Markdown so Git diffs, repository browsing and targeted searches work without extraction. Add new dated files with unique names and update this index. Keep root documents around 16 KiB / 200 lines or less, moving complete older changelog entries and summarizing completed plan work before they exceed that budget. Never move unresolved work solely into a historical snapshot.
