"""Tests for site/build.py: version selection, the update manifest, file classification and a
whole build from site/testdata/releases.json (a made-up releases answer).

    python3 -m unittest discover -s site -v
"""

import importlib.util
import json
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

HERE = Path(__file__).resolve().parent
# loaded by path: "site" and "build" are also names of installed modules
_spec = importlib.util.spec_from_file_location("patorrent_site_build", HERE / "build.py")
b = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = b  # dataclasses look their module up while the class is made
_spec.loader.exec_module(b)

SAMPLE = json.loads((HERE / "testdata" / "releases.json").read_text(encoding="utf-8"))
REPO = "Balls0fSteel/PaTorrent"


def rel(tag, prerelease=False, draft=False, assets=(), published="2026-11-01T00:00:00Z"):
    return {
        "tag_name": tag, "prerelease": prerelease, "draft": draft, "published_at": published,
        "html_url": f"https://github.com/{REPO}/releases/tag/{tag}",
        "assets": [{"name": n, "size": 1234567, "state": "uploaded",
                    "browser_download_url": f"https://github.com/{REPO}/releases/download/{tag}/{n}"}
                   for n in assets],
    }


class Versions(unittest.TestCase):
    def test_tags(self):
        good = {"v1.0.0": "1.0.0", "v1.0.0-rc.1": "1.0.0-rc.1", "1.2.3": "1.2.3",
                "v10.20.30-beta-2.x": "10.20.30-beta-2.x", "v0.0.1-0": "0.0.1-0"}
        for tag, version in good.items():
            self.assertEqual(b.version_of(tag), version, tag)
        for tag in ["v1.0", "v1", "1.0.0.0", "v1.0.0+build.5", "v1.0.0-", "v1.0.0-rc_1",
                    "nightly", "", "V1.0.0", "vv1.0.0", "v1.0.0\n", " v1.0.0", "v1.0.0 ",
                    "v١.0.0", "v1.0.0-rc.é", None, 7]:
            self.assertIsNone(b.version_of(tag), repr(tag))

    def test_precedence(self):
        ordered = ["0.9.9", "1.0.0-0", "1.0.0-1", "1.0.0-alpha", "1.0.0-alpha.1",
                   "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2", "1.0.0-beta.11",
                   "1.0.0-rc", "1.0.0-rc.1", "1.0.0-rc.2", "1.0.0-rc.9", "1.0.0-rc.10", "1.0.0",
                   "1.0.1-rc.1", "1.0.1", "1.9.0", "1.10.0", "2.0.0"]
        shuffled = list(reversed(ordered))
        self.assertEqual(sorted(shuffled, key=b.semver_key), ordered)
        self.assertLess(b.semver_key("1.0.0-rc.9"), b.semver_key("1.0.0-rc.10"))
        self.assertGreater(b.semver_key("1.0.0"), b.semver_key("1.0.0-rc.99"))


class Selection(unittest.TestCase):
    def pick(self, data):
        return b.pick(b.parse_releases(data))

    def test_sample(self):
        stable, pre = self.pick(SAMPLE)
        self.assertEqual(stable.version, "1.0.0")  # the draft v1.2.0 is not published
        self.assertEqual(pre.version, "1.1.0-rc.10")  # rc.10 above rc.2; the nightly tag skipped

    def test_only_prereleases(self):
        stable, pre = self.pick([rel("v1.0.0-rc.1", True), rel("v1.0.0-rc.2", True)])
        self.assertIsNone(stable)
        self.assertEqual(pre.version, "1.0.0-rc.2")

    def test_flag_not_suffix_decides(self):
        # what GitHub calls a pre-release is one, whatever its version looks like
        stable, pre = self.pick([rel("v1.0.0-rc.3", False), rel("v1.0.0", True)])
        self.assertEqual(stable.version, "1.0.0-rc.3")
        self.assertEqual(pre.version, "1.0.0")

    def test_nothing_usable(self):
        stable, pre = self.pick([rel("v2.0.0", draft=True), rel("latest"), rel("v1.0")])
        self.assertIsNone(stable)
        self.assertIsNone(pre)

    def test_not_a_list(self):
        with self.assertRaises(b.BuildError):
            b.parse_releases({"message": "Not Found"})


class Manifest(unittest.TestCase):
    HEAD = ("# PaTorrent update information: the newest releases at "
            "https://github.com/Balls0fSteel/PaTorrent/releases\n"
            "# Read by PaTorrent's update check; written by .github/workflows/pages.yml.\n")

    def manifest(self, data):
        stable, pre = b.pick(b.parse_releases(data))
        return b.update_manifest(stable, pre, REPO)

    def test_both(self):
        self.assertEqual(self.manifest(SAMPLE), self.HEAD + "stable=1.0.0\nprerelease=1.1.0-rc.10\n")

    def test_only_prerelease(self):
        self.assertEqual(self.manifest([rel("v1.0.0-rc.1", True)]), self.HEAD + "prerelease=1.0.0-rc.1\n")

    def test_older_prerelease_still_named(self):
        text = self.manifest([rel("v1.0.0"), rel("v1.0.0-rc.2", True)])
        self.assertEqual(text, self.HEAD + "stable=1.0.0\nprerelease=1.0.0-rc.2\n")

    def test_comments_only(self):
        self.assertEqual(self.manifest([]), self.HEAD)
        self.assertEqual(b.update_manifest(None, None, REPO), self.HEAD)

    def test_format(self):
        text = self.manifest(SAMPLE)
        self.assertNotIn("\r", text)
        self.assertTrue(text.endswith("\n"))
        for line in text.splitlines():
            self.assertTrue(line.startswith("#") or b.version_of(line.split("=", 1)[1]), line)


class Classification(unittest.TestCase):
    def test_release_files(self):
        v = "1.0.0-rc.1"
        expected = {
            f"patorrent-{v}-windows-x64-setup.exe": ("windows-setup", "x64"),
            f"patorrent-{v}-windows-x64.zip": ("windows-zip", "x64"),
            f"patorrent-{v}-linux-x86_64.tar.gz": ("linux-tar", "x86_64"),
            f"patorrent-{v}-linux-aarch64.tar.gz": ("linux-tar", "aarch64"),
            f"PaTorrent-{v}-x86_64.AppImage": ("appimage", "x86_64"),
            f"PaTorrent-{v}-aarch64.AppImage": ("appimage", "aarch64"),
            f"patorrent-nox-{v}-x86_64.AppImage": ("nox-appimage", "x86_64"),
            f"patorrent-nox-{v}-aarch64.AppImage": ("nox-appimage", "aarch64"),
            f"PaTorrent-{v}-x86_64.flatpak": ("flatpak", "x86_64"),
            f"PaTorrent-{v}-aarch64.flatpak": ("flatpak", "aarch64"),
            f"PaTorrent-{v}-arm64.dmg": ("dmg", "arm64"),
            f"patorrent-nox-{v}-macos-arm64.tar.gz": ("macos-nox", "arm64"),
            f"patorrent-nox-{v}-macos-x86_64.tar.gz": ("macos-nox", "x86_64"),
            "SHA256SUMS": ("checksums", ""),
        }
        for name, kind in expected.items():
            self.assertEqual(b.classify(name), kind, name)
        for name in ["SHA256SUMS.asc", "patorrent-1.0.0-windows-x64.zip.sig", "notes.txt",
                     "patorrent-1.0.0-windows-x86.zip", "PaTorrent-1.0.0-x86_64.appimage",
                     "patorrent-1.0.0-x86_64.AppImage", "patorrent-nightly-windows-x64.zip",
                     "PaTorrent-1.0.0-riscv64.flatpak", "patorrent-nox-1.0.0-macos-arm64.zip"]:
            self.assertIsNone(b.classify(name), name)

    def test_live_release_shape(self):
        # the 13 files of 1.0.0-rc.1, the first published release
        data = [rel("v1.0.0-rc.1", True, assets=[
            "PaTorrent-1.0.0-rc.1-aarch64.AppImage", "PaTorrent-1.0.0-rc.1-aarch64.flatpak",
            "PaTorrent-1.0.0-rc.1-arm64.dmg", "patorrent-1.0.0-rc.1-linux-aarch64.tar.gz",
            "patorrent-1.0.0-rc.1-linux-x86_64.tar.gz", "patorrent-1.0.0-rc.1-windows-x64.zip",
            "PaTorrent-1.0.0-rc.1-x86_64.AppImage", "PaTorrent-1.0.0-rc.1-x86_64.flatpak",
            "patorrent-nox-1.0.0-rc.1-aarch64.AppImage", "patorrent-nox-1.0.0-rc.1-macos-arm64.tar.gz",
            "patorrent-nox-1.0.0-rc.1-macos-x86_64.tar.gz", "patorrent-nox-1.0.0-rc.1-x86_64.AppImage",
            "SHA256SUMS"])]
        (r,) = b.parse_releases(data)
        self.assertTrue(all(a.kind for a in r.assets))
        groups = b.group(r)
        self.assertEqual([a.kind for a in groups["windows"]], ["windows-zip"])
        self.assertEqual([(a.kind, a.arch) for a in groups["linux"]], [
            ("appimage", "x86_64"), ("appimage", "aarch64"), ("flatpak", "x86_64"),
            ("flatpak", "aarch64"), ("linux-tar", "x86_64"), ("linux-tar", "aarch64"),
            ("nox-appimage", "x86_64"), ("nox-appimage", "aarch64")])
        self.assertEqual([(a.kind, a.arch) for a in groups["macos"]], [
            ("dmg", "arm64"), ("macos-nox", "arm64"), ("macos-nox", "x86_64")])
        self.assertNotIn("other", groups)

    def test_skipped_assets(self):
        data = [rel("v1.0.0", assets=["patorrent-1.0.0-windows-x64.zip", "SHA256SUMS"])]
        data[0]["assets"][0]["state"] = "starter"  # an upload that never finished
        data[0]["assets"][1]["browser_download_url"] = "http://example.com/SHA256SUMS"
        (r,) = b.parse_releases(data)
        self.assertEqual(r.assets, [])


class Pages(unittest.TestCase):
    def test_fallback(self):
        blocks = b.blocks_for(None, REPO)
        for name in ("hero-download", "release", "files-windows", "files-linux", "files-macos",
                     "checksums"):
            self.assertIn(f"https://github.com/{REPO}/releases", blocks[name], name)
        self.assertEqual(blocks["if-prerelease"]("x"), "x")
        self.assertEqual(blocks["files-other"], "")

    def test_prerelease_only(self):
        blocks = b.blocks_for([rel("v1.0.0-rc.1", True, assets=["patorrent-1.0.0-rc.1-windows-x64.zip"])], REPO)
        self.assertIn("Download PaTorrent 1.0.0-rc.1", blocks["hero-download"])
        self.assertIn("Release candidate", blocks["hero-download"])
        self.assertIn('data-os="windows"', blocks["hero-download"])
        self.assertNotIn('data-os="macos"', blocks["hero-download"])
        self.assertNotIn("setup.exe", blocks["files-windows"])
        self.assertIn("has no Linux files", blocks["files-linux"])
        self.assertEqual(blocks["if-prerelease"]("notice"), "notice")

    def test_stable_with_newer_prerelease(self):
        blocks = b.blocks_for(SAMPLE, REPO)
        self.assertIn("Download PaTorrent 1.0.0<", blocks["hero-download"])
        self.assertIn("Stable release", blocks["release"])
        self.assertIn("1.1.0-rc.10", blocks["release"])  # the newer pre-release is pointed at
        self.assertEqual(blocks["if-prerelease"]("notice"), "")
        self.assertIn("patorrent-1.0.0-windows-x64-setup.exe", blocks["files-windows"])
        self.assertIn("notes.txt", blocks["files-other"])
        self.assertIn("./patorrent-1.0.0-linux-x86_64/patorrent-gtk", blocks["files-linux"])
        self.assertNotIn("1.2.0", "".join(v for v in blocks.values() if isinstance(v, str)))

    def test_escaping(self):
        data = [rel("v1.0.0", assets=['patorrent-1.0.0-windows-x64.zip', '<b>"x"</b>'])]
        blocks = b.blocks_for(data, REPO)
        self.assertNotIn("<b>", blocks["files-other"])
        self.assertIn("&lt;b&gt;&quot;x&quot;&lt;/b&gt;", blocks["files-other"])

    def test_fill(self):
        text = "a<!-- build:x -->old<!-- /build:x -->b<!-- build:keep -->in<!-- /build:keep -->c"
        self.assertEqual(b.fill(text, {"x": "NEW", "keep": lambda s: s.upper()}), "aNEWbINc")
        with self.assertRaises(b.BuildError):
            b.fill("<!-- build:nope -->x<!-- /build:nope -->", {})
        with self.assertRaises(b.BuildError):
            b.fill("<!-- build:x -->never closed", {"x": ""})

    def test_sizes(self):
        self.assertEqual(b.human_size(1263), "1.2 kB")
        self.assertEqual(b.human_size(4503783), "4.3 MB")
        self.assertEqual(b.human_size(127_000_000), "121 MB")
        self.assertEqual(b.human_size(512), "512 B")


def article(eid, date, title=None, lede="Short.", body="<ul><li>x</li></ul>"):
    title = title if title is not None else f"PaTorrent {eid[1:]}"
    return (f'<article class="news-entry" id="{eid}" data-date="{date}">'
            f'<h2><a href="#{eid}"><time>{date}</time> — <span class="news-title">{title}</span></a></h2>'
            f'<p class="news-lede">{lede}</p>{body}</article>')


class News(unittest.TestCase):
    SITE_URL = "https://balls0fsteel.github.io/PaTorrent/"

    def test_real_page(self):
        entries = b.parse_news((HERE / "news.html").read_text(encoding="utf-8"))
        ids = [e.id for e in entries]
        self.assertEqual(len(ids), len(set(ids)))
        for e in entries:
            self.assertTrue(e.id.startswith("v") and b.version_of(e.id), e.id)
            self.assertEqual(e.title, f"PaTorrent {e.id[1:]}")
            self.assertTrue(e.lede)
        # newest first, by date and, within a day, by version
        self.assertEqual([e.date for e in entries], sorted((e.date for e in entries), reverse=True))
        versions = [e.id[1:] for e in entries]
        self.assertEqual(versions, sorted(versions, key=b.semver_key, reverse=True))

    def test_parse(self):
        text = "<main>" + article("v1.1.0", "2026-10-28", lede="A <code>x</code> &amp; y.") \
               + '<article class="other">ignored</article>' + article("v1.0.1", "2026-10-27") + "</main>"
        e1, e2 = b.parse_news(text)
        self.assertEqual((e1.id, e1.date, e1.title), ("v1.1.0", "2026-10-28", "PaTorrent 1.1.0"))
        self.assertEqual(e1.lede, "A <code>x</code> &amp; y.")
        self.assertTrue(e1.body.startswith('<p class="news-lede">'))
        self.assertEqual(e2.id, "v1.0.1")

    def test_rejects(self):
        bad = [
            "<p>no entries</p>",
            article("v1.0.0", "2026-10-26") + article("v1.0.1", "2026-10-27"),  # oldest first
            article("v1.0.0", "2026-10-26") + article("v1.0.0", "2026-10-26"),  # same id twice
            article("v1.0.0", "26 Oct 2026"),
            article("v1.0.0", "2026-02-30"),
            article("", "2026-10-26"),
            article("v1.0.0", "2026-10-26").replace('class="news-lede"', 'class="x"'),
            article("v1.0.0", "2026-10-26").replace("news-title", "x"),
        ]
        for text in bad:
            with self.assertRaises(b.BuildError, msg=text):
                b.parse_news(text)

    def test_feed(self):
        entries = b.parse_news(
            article("v1.1.0", "2026-10-28", title="PaTorrent 1.1.0 &amp; &lt;more&gt;",
                    body='<p><a href="faq.html#search">FAQ</a> <a href="#v1.0.1">older</a> '
                         '<a href="https://example.com/x">out</a> <img src="img/a.png" alt=""></p>')
            + article("v1.0.1", "2026-10-27"))
        text = b.atom_feed(entries, self.SITE_URL)
        root = ET.fromstring(text.encode("utf-8"))
        ns = {"a": "http://www.w3.org/2005/Atom"}
        for tag in ("title", "id", "updated", "author/a:name"):
            self.assertIsNotNone(root.find(f"a:{tag}", ns), tag)
        self.assertEqual(root.find("a:id", ns).text, "tag:balls0fsteel.github.io,2026:PaTorrent/news")
        self.assertEqual(root.find("a:updated", ns).text, "2026-10-28T00:00:00Z")
        self.assertEqual(root.find("a:link[@rel='self']", ns).get("href"), self.SITE_URL + "news.atom")
        self.assertEqual(root.find("a:link[@rel='alternate']", ns).get("href"), self.SITE_URL + "news.html")
        items = root.findall("a:entry", ns)
        self.assertEqual(len(items), 2)
        first = items[0]
        self.assertEqual(first.find("a:title", ns).text, "PaTorrent 1.1.0 & <more>")
        self.assertEqual(first.find("a:id", ns).text, "tag:balls0fsteel.github.io,2026:PaTorrent/news/v1.1.0")
        self.assertEqual(first.find("a:link", ns).get("href"), self.SITE_URL + "news.html#v1.1.0")
        for tag in ("id", "title", "updated", "published", "summary", "content"):
            self.assertIsNotNone(first.find(f"a:{tag}", ns), tag)
        content = first.find("a:content", ns)
        self.assertEqual(content.get("type"), "html")
        self.assertIn(f'href="{self.SITE_URL}faq.html#search"', content.text)
        self.assertIn(f'href="{self.SITE_URL}news.html#v1.0.1"', content.text)
        self.assertIn('href="https://example.com/x"', content.text)
        self.assertIn(f'src="{self.SITE_URL}img/a.png"', content.text)
        self.assertEqual(first.find("a:summary", ns).text, "Short.")
        # ids do not depend on anything but the site and the entry
        again = b.atom_feed(entries[1:], self.SITE_URL)
        self.assertIn("<id>tag:balls0fsteel.github.io,2026:PaTorrent/news/v1.0.1</id>", again)

    def test_site_url(self):
        self.assertEqual(b.site_url_for("Balls0fSteel/PaTorrent"), self.SITE_URL)
        root_feed = b.atom_feed(b.parse_news(article("v1.0.0", "2026-10-26")), "https://example.org/")
        self.assertIn("<id>tag:example.org,2026:news</id>", root_feed)

    def test_latest_news(self):
        entries = b.parse_news(article("v1.1.17", "2026-10-30", lede="Search <code>x</code>.")
                               + article("v1.1.7", "2026-10-30"))
        line = b.latest_news_block(entries)
        self.assertIn('href="news.html#v1.1.17"', line)
        self.assertIn("PaTorrent 1.1.17", line)
        self.assertIn('<time datetime="2026-10-30">30 October 2026</time>', line)
        self.assertIn("Search <code>x</code>.", line)
        self.assertNotIn("1.1.7<", line)


class WholeBuild(unittest.TestCase):
    def test_build_from_sample(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "_site"
            warnings = b.build(out, SAMPLE, REPO, "/PaTorrent/")
            self.assertEqual((out / "update.txt").read_bytes(), (
                Manifest.HEAD + "stable=1.0.0\nprerelease=1.1.0-rc.10\n").encode("utf-8"))
            self.assertTrue((out / ".nojekyll").is_file())
            for page in ("index.html", "download.html", "faq.html", "docs.html", "404.html",
                         "news.html", "benchmarks.html"):
                text = (out / page).read_text(encoding="utf-8")
                self.assertNotIn("<!-- build:", text, page)
                nav = text.split("</nav>")[0]
                self.assertIn('href="news.html"', nav, page)  # every page links the news

            # the feed: well-formed Atom, an entry per news entry, newest first
            news = b.parse_news((out / "news.html").read_text(encoding="utf-8"))
            feed = ET.fromstring((out / "news.atom").read_bytes())
            ns = {"a": "http://www.w3.org/2005/Atom"}
            self.assertEqual(feed.tag, "{http://www.w3.org/2005/Atom}feed")
            ids = [e.find("a:id", ns).text for e in feed.findall("a:entry", ns)]
            self.assertEqual(ids, [f"tag:balls0fsteel.github.io,2026:PaTorrent/news/{e.id}" for e in news])
            index = (out / "index.html").read_text(encoding="utf-8")
            self.assertIn(f'<a href="news.html#{news[0].id}">{news[0].title}</a>', index)
            self.assertIn('type="application/atom+xml"', (out / "news.html").read_text(encoding="utf-8"))
            for size in b.ICON_SIZES:
                self.assertEqual(b.png_size(out / "img" / f"icon-{size}.png"), (size, size))
            self.assertFalse((out / "build.py").exists())
            self.assertFalse((out / "testdata").exists())
            self.assertFalse(any(out.glob("test_*.py")))
            self.assertIn('<base href="/PaTorrent/">', (out / "404.html").read_text(encoding="utf-8"))
            # the main screenshot is added to site/img separately; nothing else may be missing
            self.assertEqual([w for w in warnings if "screenshot-windows.png" not in w], [])

            # a second build replaces the first
            b.build(out, None, REPO, "/PaTorrent/")
            self.assertEqual((out / "update.txt").read_text(encoding="utf-8"), Manifest.HEAD)

    def test_refuses_foreign_folder(self):
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / "precious.txt").write_text("keep me", encoding="utf-8")
            with self.assertRaises(b.BuildError):
                b.prepare_out(Path(tmp))
            self.assertTrue((Path(tmp) / "precious.txt").exists())
        with self.assertRaises(b.BuildError):
            b.prepare_out(HERE)
        with self.assertRaises(b.BuildError):
            b.prepare_out(HERE.parent)

    def test_link_check(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            (out / "a.html").write_text(
                '<a href="b.html#there">ok</a><a href="b.html#gone">bad anchor</a>'
                '<img src="missing.png" alt=""><a href="/rooted.html">rooted</a>'
                '<a href="https://example.com/x">external</a><a href="#self">self</a>'
                '<p id="self"></p>', encoding="utf-8")
            (out / "b.html").write_text('<h2 id="there">x</h2>', encoding="utf-8")
            problems = b.check_links(out, "/PaTorrent/")
            self.assertEqual(len(problems), 3, problems)
            self.assertTrue(any("#gone" in p for p in problems))
            self.assertTrue(any("missing.png" in p for p in problems))
            self.assertTrue(any("rooted" in p for p in problems))


if __name__ == "__main__":
    unittest.main()
