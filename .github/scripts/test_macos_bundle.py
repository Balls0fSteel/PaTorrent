#!/usr/bin/env python3
"""Tests of macos_bundle.py that run anywhere (Windows included): the `native` and `check`
commands with --dry-run (FakeTools answers otool from a fixture and prints what would change
files), and the Info.plist.

  python3 -I .github/scripts/test_macos_bundle.py
"""

import importlib.util
import io
import json
import os
import plistlib
import shutil
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
SPEC = importlib.util.spec_from_file_location('macos_bundle', os.path.join(HERE, 'macos_bundle.py'))
mb = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(mb)


def icon_dir():
    with open(os.path.join(ROOT, 'assets', 'patina-icons', 'app-icon.txt')) as f:
        family = f.read().strip()
    return os.path.join(ROOT, 'assets', 'patina-icons', 'exports', 'app', family)


class Native(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.bins = []
        files = {}
        for arch in ('arm64', 'x86_64'):
            b = os.path.join(self.tmp, 'rookparcel-mac-' + arch)
            with open(b, 'wb') as f:
                f.write(mb.MACHO_MAGIC[0] + b'\0' * 60)
            self.bins.append(b)
            files['{root}/' + os.path.basename(b)] = {
                'deps': ['/usr/lib/libSystem.B.dylib', '/System/Library/Frameworks/AppKit.framework/Versions/C/AppKit'],
                'rpaths': [], 'minos': '11.0'}
        self.fixture = os.path.join(self.tmp, 'fixture.json')
        with open(self.fixture, 'w') as f:
            json.dump({'files': files}, f)

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def run_cmd(self, argv):
        out = io.StringIO()
        old = sys.argv
        sys.argv = ['macos_bundle.py', '--dry-run', self.fixture] + argv
        try:
            with redirect_stdout(out):
                code = mb.main()
        finally:
            sys.argv = old
        return code, out.getvalue()

    def native(self, binaries, icons=None):
        app = os.path.join(self.tmp, 'Rookparcel.app')
        argv = ['native']
        for b in binaries:
            argv += ['--binary', b]
        argv += ['--icon-dir', icons or icon_dir(), '--app-id', 'io.github.balls0fsteel.Rookparcel',
                 '--version', '1.2.6-dev.abc1234', '--out', app,
                 '--extra', os.path.join(ROOT, 'LICENSE.md'), '--extra', os.path.join(ROOT, 'THIRD-PARTY-NOTICES.md')]
        code, log = self.run_cmd(argv)
        return code, log, app

    def test_one_binary(self):
        code, log, app = self.native(self.bins[:1])
        self.assertEqual(code, 0, log)
        self.assertTrue(os.path.isfile(os.path.join(app, 'Contents', 'MacOS', 'Rookparcel')))
        for f in ('LICENSE.md', 'THIRD-PARTY-NOTICES.md'):
            self.assertTrue(os.path.isfile(os.path.join(app, 'Contents', 'Resources', f)), f)
        self.assertNotIn('lipo -create', log)
        self.assertIn('iconutil --convert icns', log)
        self.assertIn('codesign --force --sign - ' + app, log)
        # the exported icons stop at 512 px
        self.assertIn('no 1024 px entry', log)
        self.assertFalse(os.path.exists(app + '.staging'))
        with open(os.path.join(app, 'Contents', 'Info.plist'), 'rb') as f:
            p = plistlib.load(f)
        self.assertEqual(p['NSPrincipalClass'], 'NSApplication')
        self.assertEqual(p['CFBundleExecutable'], 'Rookparcel')
        self.assertEqual(p['LSMinimumSystemVersion'], '11.0')
        self.assertEqual(p['CFBundleShortVersionString'], '1.2.6')
        self.assertEqual(p['RookparcelVersion'], '1.2.6-dev.abc1234')
        self.assertEqual(len(p['CFBundleLocalizations']), 35)
        self.assertEqual(p['CFBundleLocalizations'][0], 'en')
        for code in ('de', 'he', 'pt-BR', 'zh-Hans', 'zh-Hant'):
            self.assertIn(code, p['CFBundleLocalizations'])
        self.assertEqual(p['CFBundleURLTypes'][0]['CFBundleURLSchemes'], ['magnet'])
        self.assertEqual(p['CFBundleDocumentTypes'][0]['CFBundleTypeExtensions'], ['torrent'])
        self.assertEqual(p['UTImportedTypeDeclarations'][0]['UTTypeIdentifier'], 'org.bittorrent.torrent')
        self.assertIn('NSLocalNetworkUsageDescription', p)
        # Auto Shutdown's Sleep and Shut Down are Apple events to loginwindow
        self.assertTrue(p['NSAppleEventsUsageDescription'])

    def test_two_binaries_are_one_universal(self):
        code, log, app = self.native(self.bins)
        self.assertEqual(code, 0, log)
        exe = os.path.join(app, 'Contents', 'MacOS', 'Rookparcel')
        self.assertIn('lipo -create {} {} -output {}'.format(self.bins[0], self.bins[1], exe), log)
        with open(exe, 'rb') as f:
            self.assertEqual(f.read(4), mb.MACHO_MAGIC[4])

    def test_1024_entry_when_there_is_a_source(self):
        icons = os.path.join(self.tmp, 'icons')
        shutil.copytree(icon_dir(), icons)
        shutil.copy2(os.path.join(icons, '512.png'), os.path.join(icons, '1024.png'))
        code, log, app = self.native(self.bins[:1], icons)
        self.assertEqual(code, 0, log)
        self.assertNotIn('no 1024 px entry', log)

    def test_a_library_outside_the_system_fails(self):
        with open(self.fixture) as f:
            data = json.load(f)
        data['files']['{root}/rookparcel-mac-arm64']['deps'].append('/opt/homebrew/lib/libgtk-4.1.dylib')
        with open(self.fixture, 'w') as f:
            json.dump(data, f)
        code, log, _ = self.native(self.bins[:1])
        self.assertEqual(code, 1)
        self.assertIn('outside the bundle', log)

    def test_check_passes_a_system_only_binary(self):
        code, log = self.run_cmd(['check', self.bins[0]])
        self.assertEqual(code, 0, log)
        self.assertIn('minimum macOS 11.0', log)

    def test_the_gtk_commands_are_gone(self):
        # the GTK bundle (`app`) and its library walk (`deps`) left with D61
        self.assertFalse(os.path.exists(os.path.join(HERE, 'macos_launcher.c')))
        for cmd in ('app', 'deps'):
            old = sys.argv
            sys.argv = ['macos_bundle.py', cmd, '--help']
            try:
                with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                    with self.assertRaises(SystemExit) as e:
                        mb.main()
            finally:
                sys.argv = old
            self.assertEqual(e.exception.code, 2, cmd)


if __name__ == '__main__':
    unittest.main()
