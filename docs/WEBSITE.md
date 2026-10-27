# The website

<https://balls0fsteel.github.io/PaTorrent/> is PaTorrent's project site: what it is, the
downloads of the newest release, questions and answers, and pointers to the documentation, until
a user guide comes with a later version (PLAN.md, V2.0 goal 4). It also serves `update.txt`, the
file PaTorrent's update check reads.

The pages are hand-written HTML and CSS in `site/`: no framework, no generator, no external
fonts, scripts, analytics or trackers, and no cookies. `site/build.py` (Python 3 standard library
only) copies them to `_site/` and fills in the newest release from the GitHub releases API;
`.github/workflows/pages.yml` runs it and deploys `_site/` to GitHub Pages.

## Files

| Path | What |
|---|---|
| `site/index.html` | Home: what PaTorrent is, the download button, screenshots, features, platforms |
| `site/download.html` | Every file of the release on offer, how to start each, SmartScreen and Gatekeeper, the daemon, checksums, updating |
| `site/faq.html` | The license in plain words, settings and portable mode, languages, the Web UI and remote mode, privacy and the update check, private trackers |
| `site/docs.html` | Links to the documents in the repository, a command-line summary |
| `site/404.html` | Served by Pages for any missing path; its `<base>` makes its links start at the site's root |
| `site/css/style.css` | All styles: colour tokens for light and dark (`prefers-color-scheme`), layout down to 360 px |
| `site/js/site.js` | The only script (about 20 lines): shows the visitor's own system's download line and marks its section; the pages work without it |
| `site/img/icons.svg` | A sprite of 15 of the app's own UI symbols (the bold family, `assets/ui-icon-explorations/svg-size/bold/24`); the page sets their colour |
| `site/img/add-new-torrent.png` | A copy of `docs/screenshots/add-new-torrent.png` |
| `site/img/screenshot-windows.png` | The main window on Windows, shown on the home page (added separately; the build warns while it is missing) |
| `site/build.py` | The build (below) |
| `site/test_build.py` | Its tests; `site/testdata/releases.json` is a made-up releases answer they use (a stable release, newer pre-releases, a draft, a tag that is no version, an unknown file) |

The app icon is not copied into `site/`: the build takes 16, 32, 64, 128 and 256 px from
`assets/patina-icons/exports/app/<family>/`, the family `assets/patina-icons/app-icon.txt`
names, so the site follows the app when the brand changes. Screenshots should stay small (a PNG
of the window at 100 % scaling, run through an optimiser); the build gives every local PNG
`<img>` without a size its `width` and `height`, so the page does not jump while it loads.

## Previewing it

From the repository root, with Python 3 (on Windows, the venv's `python.exe`):

```bash
python3 site/build.py                     # the real releases, from the API (60 requests an hour without a token)
python3 -m http.server 8000 --directory _site
```

A token in `GITHUB_TOKEN` or `GH_TOKEN` is used when set. Then open <http://127.0.0.1:8000/>.
Without the network, or to see a stable release, newer
pre-releases and an unknown file at once, build from the saved answer instead:

```bash
python3 site/build.py --releases-json site/testdata/releases.json
gh api repos/Balls0fSteel/PaTorrent/releases > releases.json   # a current answer to save
```

`404.html` links from `/PaTorrent/`, as on Pages; add `--base-path /` to preview it at the root.
The build prints which release it chose, what `update.txt` says and any broken local link,
image or `#anchor`; `--strict` makes those fail the build. The tests:

```bash
python3 -m unittest discover -s site -v
```

`--out DIR` builds elsewhere. The build deletes the output folder first, so it refuses a folder
that is not an earlier build (one without `.nojekyll`), the repository and `site/`.

## What the build fills in

The pages carry `<!-- build:NAME -->...<!-- /build:NAME -->` blocks; the text between the
markers is what the raw file shows, and the build replaces it. An unknown name fails the build.

| Block | Where | Becomes |
|---|---|---|
| `hero-download` | home | the download button, the release's badge, date and notes, a direct link per system |
| `release` | download | the version on offer, its badge and links; a newer pre-release, if any |
| `files-windows`, `files-linux`, `files-macos` | download | that platform's files (name, kind, architecture, size) and how to start each kind, with the real file names |
| `files-other` | download | release files the build does not recognise, if any |
| `checksums` | download | the `SHA256SUMS` link and the SHA-256 GitHub recorded for each file |
| `latest-version` | every footer | the version on offer |
| `release-notes` | docs | a link to the release's notes |
| `if-prerelease` | home, download | its own contents, kept only while the release on offer is a pre-release (or there is none) |
| `base` | 404 | `<base href="/PaTorrent/">` (`--base-path`) |

**Which release.** The published (non-draft) releases whose tag is a version (below), ordered by
semantic-versioning precedence: the highest stable release, or if there is none, the highest
pre-release, labelled "Release candidate" (an `rc` suffix) or "Pre-release". With a stable
release on offer, a higher pre-release gets a one-line pointer. Whether a release is a
pre-release is GitHub's flag, which the Release workflow sets from a `-` suffix.

**Which files.** By file name, from the release's actual asset list (`docs/RELEASING.md`), so a
release without a file simply has no link to it:

| Pattern (`<v>` the version) | Section, kind |
|---|---|
| `patorrent-<v>-windows-x64-setup.exe` | Windows, installer |
| `patorrent-<v>-windows-x64.zip` | Windows, portable zip |
| `PaTorrent-<v>-{x86_64,aarch64}.AppImage` | Linux, AppImage |
| `PaTorrent-<v>-{x86_64,aarch64}.flatpak` | Linux, Flatpak bundle |
| `patorrent-<v>-linux-{x86_64,aarch64}.tar.gz` | Linux, plain binaries |
| `patorrent-nox-<v>-{x86_64,aarch64}.AppImage` | Linux, headless AppImage |
| `PaTorrent-<v>-{arm64,x86_64}.dmg` | macOS, disk image |
| `patorrent-nox-<v>-macos-{arm64,x86_64}.tar.gz` | macOS, headless |
| `SHA256SUMS` | checksums |

A new kind of file needs a `Kind` in `KINDS` in `site/build.py` (pattern, section, title,
description, and its start steps in `how_to_start`) and a line in `Classification` in the tests.
If the releases cannot be read, every block links to the releases page on GitHub instead.

## Deploying

`.github/workflows/pages.yml` builds and deploys on:

- a push to `main` that changes `site/`, the workflow or `assets/patina-icons/app-icon.txt`;
- release events (`published`, `edited`, `deleted`, `unpublished`, `released`, `prereleased`),
  which the owner's actions on a release send. Events caused by a workflow's `GITHUB_TOKEN`
  start no workflow, so a release published by a workflow would wait for the next run below.
  A release event's run checks out `main`, not the release's tag;
- every Monday at 05:37 UTC, as a safety net;
- by hand: Actions > Pages > Run workflow, or `gh workflow run pages.yml`.

The build job (`contents: read`) runs the tests, then `site/build.py --require-releases` with
the workflow's `GITHUB_TOKEN`, and uploads `_site/` with `actions/upload-pages-artifact`
(`include-hidden-files` for `.nojekyll`). If the API cannot be read, the run fails and the
deployed site stays as it was, rather than being replaced by one whose `update.txt` names no
version. The deploy job (`pages: write`, `id-token: write`) runs `actions/configure-pages` and
`actions/deploy-pages`. Runs share the `pages` concurrency group: one deploys at a time, and a
newer run waiting replaces an older one waiting. GitHub Pages caches files for a few minutes, so
a new release reaches `update.txt` a little after its run.

## update.txt

The update check of PaTorrent (from 1.0.1 on) reads
<https://balls0fsteel.github.io/PaTorrent/update.txt> when it starts and every 12 hours, sending
only the User-Agent `PaTorrent/<version>`; Preferences > General > Privacy turns it off. The
app's parser follows this contract; change both together.

- UTF-8, LF line endings. Lines are `key=value`; a line starting with `#` is a comment. Unknown
  keys are ignored.
- `stable=<version>`: the highest published (non-draft) release that is not a pre-release.
- `prerelease=<version>`: the highest published pre-release, even when it is lower than `stable`.
- A key is left out when there is no such release. The file always exists: at worst it holds
  only the comments.
- A version is the release's tag without its leading `v`, and matches
  `^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$`; releases whose tags do not are skipped.
- "Highest" is semantic-versioning precedence: `1.0.0-rc.1` < `1.0.0-rc.2` < `1.0.0-rc.10` <
  `1.0.0` < `1.0.1`. Pre-release identifiers made of digits compare as numbers, others in ASCII
  order, digits sort before letters, and when every identifier of the shorter list equals the
  other's, the longer list is higher (`1.0.0-rc` < `1.0.0-rc.1`).

For example:

```
# PaTorrent update information: the newest releases at https://github.com/Balls0fSteel/PaTorrent/releases
# Read by PaTorrent's update check; written by .github/workflows/pages.yml.
stable=1.0.0
prerelease=1.0.0-rc.2
```

## One-time setup (the owner)

As of 2026-10-27 the repository's Pages site is set to "Deploy from a branch" (`main`, `/`),
which publishes the repository root through Jekyll, and the `github-pages` environment lets only
`main` deploy. Before the workflow can deploy:

1. **Settings > Pages > Build and deployment > Source: GitHub Actions.** The same with the CLI
   (an administrator's token):

   ```bash
   gh api -X PUT repos/Balls0fSteel/PaTorrent/pages -f build_type=workflow
   ```

   From then on, pushes to `main` no longer publish the repository root; the site is what this
   workflow deploys.
2. **Settings > Environments > github-pages > Deployment branches and tags: add a tag rule
   `v*`** beside the `main` branch rule. A release event's run is on the release's tag, and
   without this rule the deploy job is refused ("Tag ... is not allowed to deploy to
   github-pages due to environment protection rules"). With the CLI:

   ```bash
   gh api -X POST repos/Balls0fSteel/PaTorrent/environments/github-pages/deployment-branch-policies -f name='v*' -f type=tag
   ```

3. Once the workflow is on `main`, run it once by hand (Actions > Pages > Run workflow, or
   `gh workflow run pages.yml`) and check the site and
   <https://balls0fsteel.github.io/PaTorrent/update.txt>.
