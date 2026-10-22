# PatinaTorrent state icons

Thirty original SVG assets on a 16 × 16 grid, designed for a compact classic desktop torrent list. Every file has a transparent background, an accessible title and description, explicit colors, and no fonts, filters, linked assets, or embedded bitmaps. The source of truth for names and state mapping is [manifest.json](manifest.json).

## Coverage

- **18 torrent status variants:** downloading, seeding and metadata download (each with a separate force-start version); checking files and resume data; both queued-checking variants; queued download and seed; paused complete and incomplete; stopped complete and incomplete; moving; error.
- **One core detail:** `wanted-complete.svg` distinguishes `TorrentState::Finished` (wanted pieces done while some files are skipped) from `Seeding`. The existing frontend displays active finished torrents as **Seeding**, so this asset is supplemental. The frontend's **Finished** status means a stopped complete torrent and maps to `stopped-complete.svg`.
- **Four tracker states:** not contacted, working, updating and error.
- **Two network states:** incoming connectivity observed, or no incoming connection observed yet.
- **Four optional future states:** stalled download, idle seed, no peers and allocating space. These have no current dedicated frontend labels. Their presence does not add detection logic or change application behavior.
- **One reusable overlay:** force-start lightning badge, already included in the three force-start SVGs.

The two checking engine states currently share **Checking …** text but get distinct magnifier and saved-data symbols. The same distinction is preserved when queued. Pause is always two bars; stop is a square; complete states add or become a check. Arrow direction distinguishes download and seed even without color. Queue lines distinguish waiting from transfer. The force badge is secondary to the main state.

## Selection order

Follow `crates/frontend/src/status.rs`:

1. Error wins over every other condition.
2. Moving storage wins next.
3. Checking files or resume data chooses a checking icon; `PAUSED && AUTO_MANAGED` chooses its queued version.
4. For other paused torrents, `AUTO_MANAGED` selects queued download or queued seed. Otherwise `user_paused` selects a paused variant. A user stop selects stopped incomplete or stopped complete (**Finished**).
5. Active metadata download gets its document icon. Other active torrents use `is_finished` to select seeding or downloading. Absence of `AUTO_MANAGED` selects a force-start variant.

`manifest.json` contains both the six base core-state mappings and an individually documented source condition for every file. The PNG exports are used by both GUIs (the torrent list, the Trackers tab and the status bar); see [../INTEGRATION.md](../INTEGRATION.md).

## Palette and rendering

| Meaning | Color |
|---|---|
| Downloading / Patina teal | `#147D72` |
| Seeding blue | `#3978BE` |
| Checking, queue, attention | `#AD700A` |
| Paused, stopped, inactive | `#6D7882` |
| Error | `#C8443D` |

Render at 16 px for list rows and 32 or 48 px for larger uses. Filled arrows, pause bars and stop shapes use integral pixel boundaries; curved outlines use 1.5 px strokes. Preserve the view box when exporting. The white interior of the error symbol is intentional. These colors target the project's classic light interface.

**Validation:** all 30 SVGs parse as XML, use `viewBox="0 0 16 16"`, and render successfully at 16, 24, 32 and 48 px with nonempty alpha and transparent corners. A contact sheet at actual 16 px and larger sizes was visually inspected on light and dark backgrounds. No application build is needed for this asset-only addition.

These are original vector designs under the repository's license; no artwork was taken from µTorrent or another client.
