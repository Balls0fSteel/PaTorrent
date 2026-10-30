#!/usr/bin/env python3
"""Builds the PaTorrent website into a folder GitHub Pages can serve (docs/WEBSITE.md).

    python3 site/build.py                          # releases from the GitHub API, into _site/
    python3 site/build.py --releases-json FILE     # releases from a saved API answer (offline)

What it does:

1. Copies site/ (without this script and its tests) to the output folder, and the app icon's
   PNGs from the family `assets/patina-icons/app-icon.txt` names to img/icon-<size>.png.
2. Reads the releases (`GET /repos/<repo>/releases`, every page), skips drafts and tags that are
   not versions, and fills the pages' `<!-- build:NAME -->...<!-- /build:NAME -->` blocks: the
   download buttons, the file lists per platform (built from each release's real asset list,
   classified by file name, so a missing file simply has no link) and the checksums. The release
   shown is the highest stable one, else the highest pre-release; without any release, or when
   the API cannot be read, the blocks link to the GitHub releases page instead.
3. Writes update.txt, the manifest PaTorrent's update check reads (the contract is in
   docs/WEBSITE.md): `stable=` the highest published non-pre-release, `prerelease=` the highest
   published pre-release, each left out when there is none.
4. Reads the entries of news.html (`<article class="news-entry" id=... data-date=...>`, newest
   first), writes news.atom, the Atom feed of them, and fills the home page's latest-news line.
5. Writes .nojekyll, gives every local PNG <img> without a size its width and height, and checks
   that every local link, image and #anchor in the pages resolves inside the output.

Python 3.9+ standard library only; it runs on the Pages workflow's Ubuntu runner and on Windows.
"""

from __future__ import annotations

import argparse
import html
import json
import os
import re
import shutil
import struct
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from datetime import datetime
from html.parser import HTMLParser
from pathlib import Path
from typing import Optional

SITE = Path(__file__).resolve().parent
ROOT = SITE.parent
DEFAULT_REPO = "Balls0fSteel/PaTorrent"
API = "https://api.github.com"

# What the update check accepts as a version (docs/WEBSITE.md): the tag without its leading v.
VERSION_RE = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?")

# Files of site/ that are not part of the site.
NOT_PUBLISHED = ("build.py", "test_*.py", "testdata", "__pycache__", "*.pyc")

# The app icon sizes the pages use (img/icon-<size>.png).
ICON_SIZES = (16, 32, 64, 128, 256)

MONTHS = ("January", "February", "March", "April", "May", "June", "July", "August",
          "September", "October", "November", "December")


class BuildError(Exception):
    pass


# ---------------------------------------------------------------------------------------------
# Versions


def version_of(tag: str) -> Optional[str]:
    """The version a release tag names (`v1.0.0-rc.1` -> `1.0.0-rc.1`), or None when it is not
    one the update manifest may carry."""
    if not isinstance(tag, str):
        return None
    v = tag[1:] if tag.startswith("v") else tag
    return v if VERSION_RE.fullmatch(v) else None


def semver_key(version: str) -> tuple:
    """A sort key in semantic-versioning precedence: 1.0.0-rc.1 < 1.0.0-rc.2 < 1.0.0-rc.10 <
    1.0.0 < 1.0.1. Pre-release identifiers compare numerically when all digits, else in ASCII
    order, numbers before words; a longer list wins when the shorter one is its prefix."""
    core, _, pre = version.partition("-")
    major, minor, patch = (int(x) for x in core.split("."))
    if not pre:
        return (major, minor, patch, 1, ())
    ids = tuple((0, int(p), "") if p.isdigit() else (1, 0, p) for p in pre.split("."))
    return (major, minor, patch, 0, ids)


# ---------------------------------------------------------------------------------------------
# Releases and their files


@dataclass
class Asset:
    name: str
    url: str
    size: int
    digest: str = ""  # "sha256:<hex>" when GitHub recorded one
    kind: str = ""
    arch: str = ""


@dataclass
class Release:
    tag: str
    version: str
    prerelease: bool
    url: str
    published: str
    assets: list = field(default_factory=list)

    @property
    def is_rc(self) -> bool:
        pre = self.version.partition("-")[2]
        return pre.lower().startswith("rc")

    @property
    def label(self) -> str:
        if not self.prerelease:
            return "Stable release"
        return "Release candidate" if self.is_rc else "Pre-release"


@dataclass
class Kind:
    key: str
    platform: str
    pattern: str  # VER stands for the version
    title: str
    what: str  # HTML

    def match(self, name: str):
        rx = "^" + self.pattern.replace("VER", r"(?P<v>[0-9][0-9A-Za-z.+-]*?)") + "$"
        return re.match(rx, name)


# Every release file, by file name (docs/RELEASING.md). Order = order on the page.
KINDS = [
    Kind("windows-setup", "windows", r"patorrent-VER-windows-(?P<arch>x64)-setup\.exe",
         "Installer", "Installs PaTorrent; a newer installer upgrades it in place."),
    Kind("windows-zip", "windows", r"patorrent-VER-windows-(?P<arch>x64)\.zip",
         "Portable zip", "<code>patorrent.exe</code> (the GUI) and <code>patorrent-nox.exe</code> "
         "(the headless daemon); statically linked, nothing to install."),
    Kind("appimage", "linux", r"PaTorrent-VER-(?P<arch>x86_64|aarch64)\.AppImage",
         "AppImage", "The GTK 4 GUI with its libraries bundled. Needs glibc 2.39 or newer "
         "(Ubuntu 24.04, Fedora 40, Debian 13 or later)."),
    Kind("flatpak", "linux", r"PaTorrent-VER-(?P<arch>x86_64|aarch64)\.flatpak",
         "Flatpak bundle", "The GTK 4 GUI on the GNOME 51 runtime, which comes from Flathub."),
    Kind("linux-tar", "linux", r"patorrent-VER-linux-(?P<arch>x86_64|aarch64)\.tar\.gz",
         "Plain binaries", "<code>patorrent-gtk</code>, <code>patorrent-nox</code>, the desktop "
         "entry and the systemd units. They use the system's GTK 4 (4.12 or newer) and "
         "OpenSSL 3, as on Ubuntu 24.04."),
    Kind("nox-appimage", "linux", r"patorrent-nox-VER-(?P<arch>x86_64|aarch64)\.AppImage",
         "Headless AppImage", "<code>patorrent-nox</code> with OpenSSL bundled, for servers."),
    Kind("dmg", "macos", r"PaTorrent-VER-(?P<arch>arm64|x86_64)\.dmg",
         "Disk image", "<code>PaTorrent.app</code>: the GTK 4 GUI with its libraries bundled "
         "(macOS 15 or newer)."),
    Kind("macos-nox", "macos", r"patorrent-nox-VER-macos-(?P<arch>arm64|x86_64)\.tar\.gz",
         "Headless", "<code>patorrent-nox</code> (Apple silicon: macOS 11 or newer; Intel: "
         "macOS 10.14 or newer)."),
    Kind("checksums", "checksums", r"SHA256SUMS",
         "Checksums", "The SHA-256 of every file in the release."),
]
KIND_BY_KEY = {k.key: k for k in KINDS}

PLATFORMS = {
    "windows": "Windows",
    "linux": "Linux",
    "macos": "macOS",
    "checksums": "Checksums",
    "other": "Other files",
}

# x86 first on Windows and Linux, Apple silicon first on macOS (the GUI is arm64 only).
ARCH_RANK = {"x64": 0, "arm64": 0, "x86_64": 1, "aarch64": 2, "": 3}


def arch_label(platform: str, arch: str) -> str:
    if platform == "macos":
        return {"arm64": "Apple silicon", "x86_64": "Intel"}.get(arch, arch)
    return {"x64": "x64", "x86_64": "x86_64", "aarch64": "aarch64 (ARM)"}.get(arch, arch)


def classify(name: str):
    """(kind key, arch) for a release file name, or None for a file we do not know."""
    for kind in KINDS:
        m = kind.match(name)
        if m:
            return kind.key, m.groupdict().get("arch") or ""
    return None


def parse_releases(data) -> list:
    """The published releases whose tag is a version, from the API's JSON (a list)."""
    if not isinstance(data, list):
        raise BuildError("the releases JSON is not a list (an API error message?)")
    out = []
    for r in data:
        if not isinstance(r, dict) or r.get("draft", False):
            continue
        version = version_of(r.get("tag_name", ""))
        if version is None:
            continue
        assets = []
        for a in r.get("assets") or []:
            if not isinstance(a, dict):
                continue
            name, url = a.get("name"), a.get("browser_download_url")
            if not isinstance(name, str) or not isinstance(url, str):
                continue
            if not url.startswith("https://") or a.get("state", "uploaded") != "uploaded":
                continue
            size = a.get("size") if isinstance(a.get("size"), int) else 0
            digest = a.get("digest") if isinstance(a.get("digest"), str) else ""
            asset = Asset(name=name, url=url, size=size, digest=digest)
            c = classify(name)
            if c:
                asset.kind, asset.arch = c
            assets.append(asset)
        html_url = r.get("html_url")
        if not isinstance(html_url, str) or not html_url.startswith("https://"):
            html_url = ""
        out.append(Release(
            tag=r["tag_name"], version=version, prerelease=bool(r.get("prerelease", False)),
            url=html_url, published=r.get("published_at") or "", assets=assets))
    return out


def highest(releases: list) -> Optional[Release]:
    return max(releases, key=lambda r: semver_key(r.version), default=None)


def pick(releases: list):
    """(the highest stable release, the highest pre-release); either may be None."""
    stable = highest([r for r in releases if not r.prerelease])
    pre = highest([r for r in releases if r.prerelease])
    return stable, pre


def update_manifest(stable: Optional[Release], pre: Optional[Release], repo: str) -> str:
    lines = [
        f"# PaTorrent update information: the newest releases at https://github.com/{repo}/releases",
        "# Read by PaTorrent's update check; written by .github/workflows/pages.yml.",
    ]
    if stable:
        lines.append(f"stable={stable.version}")
    if pre:
        lines.append(f"prerelease={pre.version}")
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------------------------
# Reading the releases


def _next_link(link_header: Optional[str]) -> Optional[str]:
    for part in (link_header or "").split(","):
        m = re.match(r'\s*<([^>]+)>\s*;\s*rel="next"', part)
        if m:
            return m.group(1)
    return None


def fetch_releases(repo: str, token: Optional[str], attempts: int = 3) -> list:
    """Every release of `repo` from the GitHub API (all pages). Retries network errors and 5xx
    answers; raises on anything else."""
    url = f"{API}/repos/{repo}/releases?per_page=100"
    headers = {
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "PaTorrent-site-build",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    out = []
    for _page in range(20):
        for attempt in range(1, attempts + 1):
            try:
                req = urllib.request.Request(url, headers=headers)
                with urllib.request.urlopen(req, timeout=30) as resp:
                    data = json.loads(resp.read().decode("utf-8"))
                    link = resp.headers.get("Link")
                break
            except urllib.error.HTTPError as e:
                if e.code < 500 or attempt == attempts:
                    raise
            except OSError:  # URLError, timeouts, resets
                if attempt == attempts:
                    raise
            time.sleep(2 * attempt)
        if not isinstance(data, list):
            raise BuildError("the releases API did not answer with a list")
        out.extend(data)
        url = _next_link(link)
        if not url:
            break
    return out


# ---------------------------------------------------------------------------------------------
# HTML


def esc(s: str) -> str:
    return html.escape(str(s), quote=True)


def human_size(n: int) -> str:
    if n < 1024:
        return f"{n} B"
    for unit in ("kB", "MB", "GB"):
        n /= 1024
        if n < 1024 or unit == "GB":
            return f"{n:.1f} {unit}" if n < 100 else f"{n:.0f} {unit}"
    return f"{n:.1f} GB"


def nice_date(iso: str) -> tuple:
    """('2026-10-27', '27 October 2026') from the API's timestamp; ('', '') if unreadable."""
    try:
        d = datetime.strptime(iso[:10], "%Y-%m-%d")
    except (TypeError, ValueError):
        return "", ""
    return d.strftime("%Y-%m-%d"), f"{d.day} {MONTHS[d.month - 1]} {d.year}"


def stem(name: str) -> str:
    for ext in (".tar.gz", ".zip"):
        if name.endswith(ext):
            return name[: -len(ext)]
    return name


def commands(*lines: str) -> str:
    return '<pre class="cmd"><code>' + "\n".join(esc(x) for x in lines) + "</code></pre>"


def how_to_start(kind: str, assets: list) -> str:
    """The first-start steps for one kind of file, written with its real file name."""
    a = assets[0]
    n = a.name
    alt = ""  # the other architectures' files
    if len(assets) > 1:
        names = " and ".join(f"<code>{esc(x.name)}</code>" for x in assets[1:])
        alt = f'<p class="alt">The same with {names}.</p>'
    if kind == "windows-setup":
        return (f"<p>Run <code>{esc(n)}</code>. Running the installer of a newer version later "
                "upgrades the installed copy in place.</p>")
    if kind == "windows-zip":
        return (f"<p>Unzip <code>{esc(n)}</code> anywhere and start <code>patorrent.exe</code>.</p>")
    if kind == "appimage":
        return ("<p>Make it executable and run it:</p>"
                + commands(f"chmod +x {n}", f"./{n}") + alt
                + "<p>Without FUSE 2 on the system, start it with "
                "<code>--appimage-extract-and-run</code>.</p>")
    if kind == "flatpak":
        return ("<p>The bundle holds the application; the runtime comes from Flathub:</p>"
                + commands("flatpak remote-add --if-not-exists --user flathub "
                           "https://dl.flathub.org/repo/flathub.flatpakrepo",
                           f"flatpak install --user {n}",
                           "flatpak run io.github.balls0fsteel.PaTorrent") + alt)
    if kind == "linux-tar":
        return ("<p>Unpack it and start the GUI:</p>"
                + commands(f"tar -xzf {n}", f"./{stem(n)}/patorrent-gtk") + alt
                + "<p>The folder also has <code>patorrent-nox</code>, the desktop entry and the "
                "systemd units (<code>systemd/system</code>, <code>systemd/user</code>).</p>")
    if kind == "nox-appimage":
        return ("<p>A terminal program; this starts it with the Web UI on port 8080:</p>"
                + commands(f"chmod +x {n}", f"./{n} run --webui 8080 --webui-pass PASS") + alt)
    if kind == "dmg":
        return (f"<p>Open <code>{esc(n)}</code> and drag <code>PaTorrent.app</code> to "
                "Applications. The first time, right-click (or Control-click) it, choose Open, "
                'then Open again: see <a href="#gatekeeper">Gatekeeper</a>.</p>')
    if kind == "macos-nox":
        return ("<p>Unpack it and start the daemon with the Web UI on port 8080:</p>"
                + commands(f"tar -xzf {n}", f"./{stem(n)}/patorrent-nox run --webui 8080 --webui-pass PASS")
                + alt + "<p>If macOS refuses to start it, remove the quarantine flag: "
                f"<code>xattr -d com.apple.quarantine {esc(stem(n))}/patorrent-nox</code>.</p>")
    return ""


def files_table(platform: str, assets: list) -> str:
    rows, described = [], set()
    for a in assets:
        kind = KIND_BY_KEY.get(a.kind)
        title = kind.title if kind else "File"
        arch = arch_label(platform, a.arch) if a.arch else ""
        label = esc(title) + (f" · {esc(arch)}" if arch else "")
        what = ""
        if kind and kind.key not in described:  # once per kind, on its first row
            described.add(kind.key)
            what = f" — {kind.what}"
        rows.append(
            "<tr>"
            f'<td class="name"><a href="{esc(a.url)}">{esc(a.name)}</a>'
            f'<span class="what"><span class="kind">{label}</span>{what}</span></td>'
            f'<td class="size">{esc(human_size(a.size)) if a.size else ""}</td>'
            "</tr>")
    return (f'<table class="listview files"><caption class="vh">{esc(PLATFORMS[platform])} files</caption>'
            '<thead><tr><th scope="col">Name</th><th scope="col" class="size">Size</th></tr></thead>'
            "<tbody>" + "".join(rows) + "</tbody></table>")


def group(release: Optional[Release]) -> dict:
    """The release's files per platform, sorted as on the page."""
    order = {k.key: i for i, k in enumerate(KINDS)}
    groups = {}
    if not release:
        return groups
    for a in release.assets:
        platform = KIND_BY_KEY[a.kind].platform if a.kind else "other"
        groups.setdefault(platform, []).append(a)
    for assets in groups.values():
        assets.sort(key=lambda a: (order.get(a.kind, 99), ARCH_RANK.get(a.arch, 3), a.name))
    return groups


def platform_block(platform: str, release: Optional[Release], releases_url: str) -> str:
    if not release:
        return ('<p class="note">The file list could not be filled in when this page was built. '
                f'Every file is on the <a href="{esc(releases_url)}">releases page on GitHub</a>.</p>')
    assets = group(release).get(platform, [])
    if not assets:
        return (f"<p class=\"note\">PaTorrent {esc(release.version)} has no {esc(PLATFORMS[platform])} "
                f'files. Other versions are on the <a href="{esc(releases_url)}">releases page</a>.</p>')
    out = [files_table(platform, assets)]
    if platform in ("windows", "linux", "macos"):
        by_kind = {}
        for a in assets:
            if a.kind:
                by_kind.setdefault(a.kind, []).append(a)
        steps = []
        for kind in KINDS:
            if kind.key in by_kind:
                body = how_to_start(kind.key, by_kind[kind.key])
                if body:
                    steps.append(f'<div class="step"><h4>{esc(kind.title)}</h4>{body}</div>')
        if steps:
            out.append('<h3>Starting it</h3><div class="steps">' + "".join(steps) + "</div>")
    return "".join(out)


def checksums_block(release: Optional[Release], releases_url: str) -> str:
    if not release:
        return (f'<p>Each release on the <a href="{esc(releases_url)}">releases page</a> has a '
                "<code>SHA256SUMS</code> file.</p>")
    sums = next((a for a in release.assets if a.kind == "checksums"), None)
    out = []
    if sums:
        out.append(f'<p><a class="btn btn-primary btn-small" href="{esc(sums.url)}">Download SHA256SUMS</a> '
                   f"for PaTorrent {esc(release.version)}.</p>")
    else:
        out.append(f"<p>PaTorrent {esc(release.version)} has no <code>SHA256SUMS</code> file.</p>")
    lines = [f"{a.digest.split(':', 1)[1]}  {a.name}" for a in release.assets
             if a.digest.startswith("sha256:") and a.kind != "checksums"]
    if lines:
        out.append("<details><summary>SHA-256 of each file, as GitHub recorded it on upload</summary>"
                   '<pre class="sums"><code>' + esc("\n".join(lines)) + "</code></pre>"
                   "<p>These should match the lines in <code>SHA256SUMS</code>.</p></details>")
    return "".join(out)


def direct_links(release: Release) -> str:
    """One hidden line per system with its direct download; site.js shows the visitor's."""
    groups = group(release)

    def link(a: Asset) -> str:
        size = f" ({esc(human_size(a.size))})".replace(" ", "\u00a0") if a.size else ""
        return f'<a href="{esc(a.url)}">{esc(a.name)}</a>{size}'

    def first(kind: str):
        return next((a for a in release.assets if a.kind == kind), None)

    out = []
    win = [(label, a) for label, a in (("the installer", first("windows-setup")),
                                       ("the portable zip", first("windows-zip"))) if a]
    if win:
        out.append('<p class="direct" data-os="windows" hidden>For Windows: '
                   + ", or ".join(f"{label} {link(a)}" for label, a in win) + "</p>")
    dmg = next((a for a in groups.get("macos", []) if a.kind == "dmg" and a.arch == "arm64"), None)
    if dmg:
        out.append('<p class="direct" data-os="macos" hidden>For a Mac with Apple silicon: '
                   + link(dmg) + '. Intel Macs: <a href="download.html#macos">the headless '
                   "daemon</a>.</p>")
    if groups.get("linux"):
        out.append('<p class="direct" data-os="linux" hidden>For Linux: '
                   '<a href="download.html#linux">AppImage, Flatpak or plain binaries</a>.</p>')
    return "".join(out)


def hero_block(featured: Optional[Release], releases_url: str) -> str:
    if not featured:
        return ('<div class="cta"><p class="cta-buttons">'
                f'<a class="btn btn-primary" href="{esc(releases_url)}">Download from GitHub</a> '
                '<a class="btn btn-ghost" href="download.html#which">Which file do I need?</a></p></div>')
    iso, nice = nice_date(featured.published)
    badge = "badge-pre" if featured.prerelease else "badge-stable"
    meta = f'<span class="badge {badge}">{esc(featured.label)}</span>'
    if nice:
        meta += f' Published <time datetime="{iso}">{nice}</time>'
    if featured.url:
        meta += f' · <a href="{esc(featured.url)}">Release notes</a>'
    return ('<div class="cta"><p class="cta-buttons">'
            f'<a class="btn btn-primary" href="download.html">Download PaTorrent {esc(featured.version)}</a> '
            '<a class="btn btn-ghost" href="download.html#which">Which file do I need?</a></p>'
            f'<p class="cta-meta">{meta}</p>{direct_links(featured)}</div>')


def release_block(featured: Optional[Release], newer_pre: Optional[Release], releases_url: str) -> str:
    if not featured:
        return ('<p class="release-line">The newest release could not be read when this page was '
                f'built. Every release is on the <a href="{esc(releases_url)}">releases page on '
                "GitHub</a>.</p>")
    iso, nice = nice_date(featured.published)
    badge = "badge-pre" if featured.prerelease else "badge-stable"
    out = [f'<p class="release-line"><strong>PaTorrent {esc(featured.version)}</strong> '
           f'<span class="badge {badge}">{esc(featured.label)}</span>']
    if nice:
        out.append(f' published <time datetime="{iso}">{nice}</time>')
    if featured.url:
        out.append(f' · <a href="{esc(featured.url)}">Release notes</a>')
    out.append(f' · <a href="{esc(releases_url)}">All releases</a></p>')
    if newer_pre:
        out.append(f'<p class="note">A newer pre-release, <a href="{esc(newer_pre.url or releases_url)}">'
                   f"{esc(newer_pre.version)}</a>, is out for testing.</p>")
    return "".join(out)


def other_block(featured: Optional[Release]) -> str:
    assets = group(featured).get("other", []) if featured else []
    if not assets:
        return ""
    return ('<section class="platform" id="other" aria-labelledby="other-h">'
            '<h2 id="other-h">Other files</h2>' + files_table("other", assets) + "</section>")


def blocks_for(releases: Optional[list], repo: str) -> dict:
    """What each `<!-- build:NAME -->` block becomes, from the API's JSON. None = the releases
    could not be read."""
    releases_url = f"https://github.com/{repo}/releases"
    stable, pre = pick(parse_releases(releases)) if releases is not None else (None, None)
    featured = stable or pre
    newer_pre = pre if stable and pre and semver_key(pre.version) > semver_key(stable.version) else None
    show_pre_notice = featured is None or featured.prerelease
    version = featured.version if featured else ""
    return {
        "hero-download": hero_block(featured, releases_url),
        "release": release_block(featured, newer_pre, releases_url),
        "files-windows": platform_block("windows", featured, releases_url),
        "files-linux": platform_block("linux", featured, releases_url),
        "files-macos": platform_block("macos", featured, releases_url),
        "files-other": other_block(featured),
        "checksums": checksums_block(featured, releases_url),
        "latest-version": (f'<a href="download.html">{esc(version)}</a>' if version
                           else f'<a href="{esc(releases_url)}">Releases</a>'),
        "release-notes": (f'<a href="{esc(featured.url)}">Release notes for {esc(version)}</a>'
                          if featured and featured.url
                          else f'<a href="{esc(releases_url)}">Release notes</a>'),
        # kept only while the release on offer is a pre-release (or there is none yet)
        "if-prerelease": lambda inner: inner if show_pre_notice else "",
    }


BLOCK_RE = re.compile(r"<!-- build:([a-z0-9-]+) -->(.*?)<!-- /build:\1 -->", re.S)


def fill(text: str, blocks: dict, where: str = "") -> str:
    def repl(m):
        name, inner = m.group(1), m.group(2)
        if name not in blocks:
            raise BuildError(f"{where}: unknown block <!-- build:{name} -->")
        value = blocks[name]
        return value(inner) if callable(value) else value

    out = BLOCK_RE.sub(repl, text)
    if "<!-- build:" in out or "<!-- /build:" in out:
        raise BuildError(f"{where}: a build block is not closed")
    return out


# ---------------------------------------------------------------------------------------------
# News and its Atom feed


NEWS_PAGE = "news.html"
NEWS_FEED = "news.atom"
# The year the feed began: part of every entry's tag: URI, so ids never change (RFC 4151).
NEWS_SINCE = "2026"
NEWS_AUTHOR = ("DukeCorp1996", "https://dukecorp1996.x35.in/")

ARTICLE_RE = re.compile(r"<article\b([^>]*)>(.*?)</article>", re.S)
ATTR_RE = re.compile(r'([a-z-]+)="([^"]*)"')
NEWS_ID_RE = re.compile(r"[A-Za-z][A-Za-z0-9._-]*")


@dataclass
class NewsEntry:
    id: str
    date: str  # YYYY-MM-DD
    title: str  # plain text
    lede: str  # HTML: the entry's first paragraph
    body: str  # HTML: everything after the heading


class _Text(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.parts = []

    def handle_data(self, data):
        self.parts.append(data)


def plain_text(fragment: str) -> str:
    p = _Text()
    p.feed(fragment)
    p.close()
    return re.sub(r"\s+", " ", "".join(p.parts)).strip()


def site_url_for(repo: str) -> str:
    """The Pages address of `repo`: https://<owner>.github.io/<name>/."""
    owner, _, name = repo.partition("/")
    return f"https://{owner.lower()}.github.io/{name}/"


def parse_news(text: str, where: str = NEWS_PAGE) -> list:
    """The entries of news.html, newest first. Each is
    `<article class="news-entry" id="v1.2.3" data-date="YYYY-MM-DD">` holding an <h2> with a
    `<span class="news-title">`, then a `<p class="news-lede">` and the rest. Fails the build on
    a malformed entry, a repeated id or entries out of order."""
    entries, seen = [], set()
    for m in ARTICLE_RE.finditer(text):
        attrs = dict(ATTR_RE.findall(m.group(1)))
        if "news-entry" not in attrs.get("class", "").split():
            continue
        inner = m.group(2)
        eid, date = attrs.get("id", ""), attrs.get("data-date", "")
        if not NEWS_ID_RE.fullmatch(eid):
            raise BuildError(f"{where}: a news entry has no usable id ({eid!r})")
        if eid in seen:
            raise BuildError(f"{where}: two news entries have the id {eid}")
        seen.add(eid)
        try:
            if datetime.strptime(date, "%Y-%m-%d").strftime("%Y-%m-%d") != date:
                raise ValueError
        except ValueError:
            raise BuildError(f"{where}: news entry {eid}: data-date {date!r} is not YYYY-MM-DD") from None
        title = re.search(r'<span class="news-title">(.*?)</span>', inner, re.S)
        lede = re.search(r'<p class="news-lede">(.*?)</p>', inner, re.S)
        head_end = inner.find("</h2>")
        if not title or not lede or head_end < 0:
            raise BuildError(f"{where}: news entry {eid} needs an <h2> with a news-title and a news-lede paragraph")
        entries.append(NewsEntry(id=eid, date=date, title=plain_text(title.group(1)),
                                 lede=lede.group(1).strip(), body=inner[head_end + 5:].strip()))
    if not entries:
        raise BuildError(f"{where}: no news entries")
    for newer, older in zip(entries, entries[1:]):
        if newer.date < older.date:
            raise BuildError(f"{where}: news entry {older.id} ({older.date}) is newer than "
                             f"{newer.id} ({newer.date}) above it; keep the newest first")
    return entries


def absolute_links(fragment: str, page_url: str) -> str:
    """The fragment with its relative href and src made absolute against `page_url`."""
    def repl(m):
        ref = html.unescape(m.group(2))
        if urllib.parse.urlsplit(ref).scheme or ref.startswith("//"):
            return m.group(0)
        return f'{m.group(1)}="{esc(urllib.parse.urljoin(page_url, ref))}"'
    return re.sub(r'\b(href|src)="([^"]*)"', repl, fragment)


def atom_feed(entries: list, site_url: str) -> str:
    """The Atom feed (RFC 4287) of the news entries."""
    page_url = site_url + NEWS_PAGE
    url = urllib.parse.urlsplit(site_url)
    path = url.path.strip("/")
    tag_base = f"tag:{url.hostname},{NEWS_SINCE}:{path + '/' if path else ''}news"

    def x(s: str) -> str:
        return html.escape(s, quote=True)

    lines = [
        '<?xml version="1.0" encoding="utf-8"?>',
        '<feed xmlns="http://www.w3.org/2005/Atom" xml:lang="en">',
        "  <title>PaTorrent news</title>",
        "  <subtitle>What changed in each PaTorrent release, newest first.</subtitle>",
        f"  <id>{x(tag_base)}</id>",
        f'  <link rel="self" type="application/atom+xml" href="{x(site_url + NEWS_FEED)}"/>',
        f'  <link rel="alternate" type="text/html" href="{x(page_url)}"/>',
        f"  <updated>{entries[0].date}T00:00:00Z</updated>",
        f"  <author><name>{x(NEWS_AUTHOR[0])}</name><uri>{x(NEWS_AUTHOR[1])}</uri></author>",
        f"  <icon>{x(site_url + 'img/icon-64.png')}</icon>",
    ]
    for e in entries:
        lines += [
            "  <entry>",
            f"    <title>{x(e.title)}</title>",
            f"    <id>{x(tag_base)}/{x(e.id)}</id>",
            f'    <link rel="alternate" type="text/html" href="{x(page_url)}#{x(e.id)}"/>',
            f"    <published>{e.date}T00:00:00Z</published>",
            f"    <updated>{e.date}T00:00:00Z</updated>",
            f"    <summary>{x(plain_text(e.lede))}</summary>",
            f'    <content type="html">{x(absolute_links(e.body, page_url))}</content>',
            "  </entry>",
        ]
    lines.append("</feed>")
    text = "\n".join(lines) + "\n"
    ET.fromstring(text.encode("utf-8"))  # well-formed, or the build stops here
    return text


def latest_news_block(entries: list) -> str:
    """The home page's line pointing at the newest news entry."""
    e = entries[0]
    iso, nice = nice_date(e.date)
    return ('<p class="latest-news"><svg class="ico ico-sm ico-purple" aria-hidden="true" '
            'focusable="false"><use href="img/icons.svg#i-rss"/></svg>'
            f'<span><strong>Latest news, <time datetime="{iso}">{nice}</time>:</strong> '
            f'<a href="{NEWS_PAGE}#{esc(e.id)}">{esc(e.title)}</a>. {e.lede} '
            f'<a href="{NEWS_PAGE}">All news</a></span></p>')


# ---------------------------------------------------------------------------------------------
# Images and links


def png_size(path: Path) -> Optional[tuple]:
    with open(path, "rb") as f:
        head = f.read(24)
    if len(head) < 24 or head[:8] != b"\x89PNG\r\n\x1a\n" or head[12:16] != b"IHDR":
        return None
    return struct.unpack(">II", head[16:24])


IMG_RE = re.compile(r"<img\b[^>]*>", re.I)


def add_image_sizes(text: str, page: Path, out: Path, warnings: list) -> str:
    """Gives each local PNG <img> without a width its file's width and height, so the page
    does not jump while it loads."""
    def repl(m):
        tag = m.group(0)
        if re.search(r"\swidth\s*=", tag):
            return tag
        src = re.search(r'\ssrc\s*=\s*"([^"]+)"', tag)
        if not src or "://" in src.group(1) or not src.group(1).lower().endswith(".png"):
            return tag
        target = (page.parent / urllib.parse.unquote(src.group(1))).resolve()
        if not target.is_file():
            return tag  # the link check reports it
        size = png_size(target)
        if not size:
            warnings.append(f"{page.relative_to(out)}: {src.group(1)} is not a readable PNG")
            return tag
        return tag[:4] + f' width="{size[0]}" height="{size[1]}"' + tag[4:]

    return IMG_RE.sub(repl, text)


class _Refs(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.ids, self.refs, self.base = set(), [], None

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if a.get("id"):
            self.ids.add(a["id"])
        if tag == "a" and a.get("name"):
            self.ids.add(a["name"])
        if tag == "base" and a.get("href"):
            self.base = a["href"]
        for key in ("href", "src"):
            if a.get(key) and tag in ("a", "link", "img", "script", "source", "use"):
                self.refs.append((tag, a[key]))
        if a.get("srcset"):
            for part in a["srcset"].split(","):
                if part.strip():
                    self.refs.append((tag, part.split()[0]))

    handle_startendtag = handle_starttag


def check_links(out: Path, base_path: str) -> list:
    """Problems with the local links, images and #anchors of every page in `out`."""
    pages, problems = {}, []
    for page in sorted(out.rglob("*.html")):
        p = _Refs()
        p.feed(page.read_text(encoding="utf-8"))
        pages[page.resolve()] = p
    for page, p in pages.items():
        rel = page.relative_to(out.resolve())
        for tag, ref in p.refs:
            url = urllib.parse.urlsplit(ref)
            if url.scheme or ref.startswith("//"):
                continue  # external
            if p.base:
                # 404.html is served for any missing path, so its <base> names the site's root
                # and its relative links start there
                if url.path.startswith("/") and not url.path.startswith(base_path):
                    problems.append(f"{rel}: <{tag}> {ref}: outside the site's path {base_path}")
                    continue
                inner = url.path[len(base_path):] if url.path.startswith("/") else url.path
                path = out / urllib.parse.unquote(inner)
            elif url.path.startswith("/"):
                problems.append(f"{rel}: <{tag}> {ref}: root-relative; the site lives under "
                                f"{base_path}, so use a relative link")
                continue
            else:
                path = page.parent / urllib.parse.unquote(url.path) if url.path else page
            path = path.resolve()
            if path.is_dir():
                path = path / "index.html"
            if not path.is_file():
                problems.append(f"{rel}: <{tag}> {ref}: no such file")
                continue
            frag = url.fragment
            if frag and path.suffix in (".html", ".svg"):
                if path.suffix == ".html":
                    ids = pages.get(path, _Refs()).ids
                else:
                    ids = set(re.findall(r'\bid="([^"]+)"', path.read_text(encoding="utf-8")))
                if frag not in ids:
                    problems.append(f"{rel}: <{tag}> {ref}: no #{frag} there")
    return problems


# ---------------------------------------------------------------------------------------------
# The build


def icon_family() -> Path:
    note = ROOT / "assets" / "patina-icons" / "app-icon.txt"
    family = next((line.strip() for line in note.read_text(encoding="utf-8").splitlines()
                   if line.strip() and not line.strip().startswith("#")), "")
    folder = ROOT / "assets" / "patina-icons" / "exports" / "app" / family
    if not family or not folder.is_dir():
        raise BuildError(f"{note} names no icon family in {folder.parent}")
    return folder


def prepare_out(out: Path) -> None:
    out = out.resolve()
    for forbidden in (ROOT, SITE, *SITE.parents):
        if out == forbidden:
            raise BuildError(f"refusing to build into {out}")
    if SITE in out.parents:
        raise BuildError(f"refusing to build inside {SITE}")
    if out.exists():
        if any(out.iterdir()) and not (out / ".nojekyll").exists():
            raise BuildError(f"{out} exists and is not an earlier build (no .nojekyll); not deleting it")
        shutil.rmtree(out)


def build(out: Path, releases: Optional[list], repo: str, base_path: str) -> list:
    """Writes the site to `out`; returns the warnings."""
    warnings = []
    prepare_out(out)
    shutil.copytree(SITE, out, ignore=shutil.ignore_patterns(*NOT_PUBLISHED))

    family = icon_family()
    (out / "img").mkdir(exist_ok=True)
    for size in ICON_SIZES:
        shutil.copyfile(family / f"{size}.png", out / "img" / f"icon-{size}.png")

    news_page = out / NEWS_PAGE
    if not news_page.is_file():
        raise BuildError(f"site/{NEWS_PAGE} is missing")
    news = parse_news(news_page.read_text(encoding="utf-8"))

    blocks = blocks_for(releases, repo)
    blocks["base"] = f'<base href="{esc(base_path)}">'
    blocks["latest-news"] = latest_news_block(news)
    for page in sorted(out.rglob("*.html")):
        text = page.read_text(encoding="utf-8")
        text = fill(text, blocks, str(page.relative_to(out)))
        text = add_image_sizes(text, page, out, warnings)
        page.write_text(text, encoding="utf-8", newline="\n")

    stable, pre = pick(parse_releases(releases)) if releases is not None else (None, None)
    with open(out / "update.txt", "w", encoding="utf-8", newline="\n") as f:
        f.write(update_manifest(stable, pre, repo))
    with open(out / NEWS_FEED, "w", encoding="utf-8", newline="\n") as f:
        f.write(atom_feed(news, site_url_for(repo)))
    (out / ".nojekyll").write_bytes(b"")

    warnings.extend(check_links(out, base_path))
    return warnings


def summary(releases: Optional[list]) -> str:
    if releases is None:
        return "releases: not read (fallback links to the releases page)"
    parsed = parse_releases(releases)
    stable, pre = pick(parsed)
    featured = stable or pre
    lines = [f"releases: {len(releases)} in the answer, {len(parsed)} published with a version tag",
             f"stable: {stable.version if stable else '-'}",
             f"prerelease: {pre.version if pre else '-'}",
             f"shown: {featured.version + ' (' + featured.label.lower() + ')' if featured else '-'}"]
    if featured:
        kinds = sorted({a.kind or 'other:' + a.name for a in featured.assets})
        lines.append(f"files: {len(featured.assets)} ({', '.join(kinds)})")
    return "\n".join(lines)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", type=Path, default=ROOT / "_site",
                    help="output folder, deleted and rewritten (default: _site in the repository)")
    ap.add_argument("--releases-json", type=Path, metavar="FILE",
                    help="read the releases from this saved API answer instead of the network "
                         "(e.g. gh api repos/OWNER/NAME/releases > FILE)")
    ap.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY") or DEFAULT_REPO,
                    help="OWNER/NAME (default: $GITHUB_REPOSITORY, else %(default)s)")
    ap.add_argument("--base-path", default=None,
                    help="the site's path on its host, used by 404.html (default: /NAME/)")
    ap.add_argument("--require-releases", action="store_true",
                    help="fail instead of building the fallback when the releases cannot be read")
    ap.add_argument("--strict", action="store_true", help="fail on warnings (broken links)")
    args = ap.parse_args(argv)
    base_path = args.base_path or f"/{args.repo.split('/')[-1]}/"
    if not base_path.endswith("/"):
        base_path += "/"

    releases = None
    try:
        if args.releases_json:
            releases = json.loads(args.releases_json.read_text(encoding="utf-8"))
        else:
            token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
            releases = fetch_releases(args.repo, token)
        parse_releases(releases)  # rejects an answer that is not a release list
    except (OSError, ValueError, BuildError, urllib.error.URLError) as e:
        if args.require_releases:
            print(f"error: the releases could not be read: {e}", file=sys.stderr)
            return 1
        print(f"warning: the releases could not be read ({e}); building the fallback",
              file=sys.stderr)
        releases = None

    try:
        warnings = build(args.out, releases, args.repo, base_path)
    except BuildError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1

    report = summary(releases)
    news = parse_news((args.out / NEWS_PAGE).read_text(encoding="utf-8"))
    report += f"\nnews: {len(news)} entries, newest {news[0].id} ({news[0].date}), {NEWS_FEED} written"
    print(report)
    print("update.txt:\n  " + (args.out / "update.txt").read_text(encoding="utf-8").rstrip("\n")
          .replace("\n", "\n  "))
    for w in warnings:
        print(f"warning: {w}", file=sys.stderr)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as f:
            f.write("### Website\n\n```\n" + report + "\n```\n")
            for w in warnings:
                f.write(f"- warning: {w}\n")
    print(f"built {args.out}")
    return 1 if args.strict and warnings else 0


if __name__ == "__main__":
    sys.exit(main())
