#!/usr/bin/env python3
"""Builds Rookparcel's macOS app and checks Mach-O files for libraries outside it (D52, D61,
docs/MACOS_GUI.md). Run by the `macos` job of .github/workflows/release.yml and by
macos-gate8.yml.

  macos_bundle.py native --binary target/aarch64-apple-darwin/release/rookparcel-mac
                         [--binary target/x86_64-apple-darwin/release/rookparcel-mac]
                         --icon-dir DIR --app-id ID --version V --out Rookparcel.app [--extra FILE]...
      The native AppKit GUI (crates/cocoa): Contents/MacOS/Rookparcel is the binary itself (two
      are joined with `lipo` into one universal binary), Contents/Info.plist (NSPrincipalClass
      NSApplication, macOS 11 at least, CFBundleLocalizations for the shipped languages, the
      document and URL types), Contents/Resources/Rookparcel.icns (with the 1024 px entry when
      DIR has a 1024.png) and the --extra files; the binary may link only the system's
      libraries. The whole app is ad-hoc signed.

  macos_bundle.py check [--exec-dir DIR] PATH...
      Fails when a Mach-O under PATH needs a library that is neither the system's (/usr/lib,
      /System) nor inside PATH, or keeps an absolute LC_RPATH. Prints each file's minimum
      macOS version.

Every outside program (otool, lipo, codesign, iconutil) runs through `Tools`. `--dry-run
FIXTURE.json` replaces it with `FakeTools`, which answers otool from the fixture and prints the
commands that would change files instead of running them, so the logic can be tried without a
Mac.

Python 3.9 or later (the runner's /usr/bin/python3).
"""

import argparse
import json
import os
import plistlib
import re
import shutil
import stat
import subprocess
import sys

SYSTEM = ('/usr/lib/', '/System/')
MACHO_MAGIC = (b'\xcf\xfa\xed\xfe', b'\xce\xfa\xed\xfe', b'\xfe\xed\xfa\xcf', b'\xfe\xed\xfa\xce',
               b'\xca\xfe\xba\xbe')


class BundleError(Exception):
    pass


def log(*parts):
    print(*parts, flush=True)


def is_macho(path):
    if os.path.islink(path) or not os.path.isfile(path):
        return False
    with open(path, 'rb') as f:
        return f.read(4) in MACHO_MAGIC


def version_key(v):
    return tuple(int(p) for p in re.findall(r'\d+', v or '0'))


def parse_otool(text_L, text_D, text_l):
    """otool -L, -D and -l output -> {'id', 'deps', 'rpaths', 'minos'}."""
    ids = [ln.strip() for ln in text_D.splitlines()[1:] if ln.strip() and not ln.rstrip().endswith(':')]
    own = ids[0] if ids else None
    deps = []
    for ln in text_L.splitlines():
        if not ln.startswith('\t'):
            continue
        name = ln.strip().split(' (compatibility version')[0].split(' (current version')[0].strip()
        if name and name != own and name not in deps:
            deps.append(name)
    rpaths = []
    for m in re.finditer(r'cmd LC_RPATH\s*\n\s*cmdsize \d+\s*\n\s*path (.*?) \(offset \d+\)', text_l):
        if m.group(1) not in rpaths:
            rpaths.append(m.group(1))
    versions = re.findall(r'^\s*minos (\S+)', text_l, re.M)
    versions += re.findall(r'cmd LC_VERSION_MIN_MACOSX\s*\n\s*cmdsize \d+\s*\n\s*version (\S+)', text_l)
    minos = max(versions, key=version_key) if versions else None
    return {'id': own, 'deps': deps, 'rpaths': rpaths, 'minos': minos}


class Tools:
    """The real programs. `inspect` only reads; `run` changes files."""

    dry = False

    def _out(self, argv):
        r = subprocess.run(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True)
        if r.returncode != 0:
            raise BundleError('{} failed ({}):\n{}{}'.format(' '.join(argv), r.returncode, r.stdout, r.stderr))
        return r.stdout

    def inspect(self, path):
        return parse_otool(self._out(['otool', '-L', path]), self._out(['otool', '-D', path]),
                           self._out(['otool', '-l', path]))

    def run(self, argv):
        log('+', ' '.join(argv))
        r = subprocess.run(argv)
        if r.returncode != 0:
            raise BundleError('{} failed ({})'.format(' '.join(argv), r.returncode))

    def copy(self, src, dest):
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        shutil.copy2(src, dest)
        os.chmod(dest, os.stat(dest).st_mode | stat.S_IWUSR)


def key(path):
    return os.path.normcase(os.path.realpath(path))


class FakeTools(Tools):
    """--dry-run: otool answers come from a fixture
    {"files": {path: {"id", "deps", "rpaths", "minos"}}}, where "{root}" in any string stands
    for the fixture's directory. `lipo -create` makes a fixture entry for its output (the first
    input's); the other programs are printed."""

    dry = True

    def __init__(self, fixture):
        root = os.path.dirname(os.path.abspath(fixture)).replace('\\', '/')
        with open(fixture) as f:
            data = json.loads(f.read().replace('{root}', root))
        # otool -L lists a library's own id first; parse_otool drops it, and so does this
        self.db = {key(p): dict(v, deps=[d for d in v.get('deps', []) if d != v.get('id')])
                   for p, v in data['files'].items()}

    def inspect(self, path):
        e = self.db.get(key(path))
        if e is None:
            raise BundleError('dry run: no fixture entry for ' + path)
        return {'id': e.get('id'), 'deps': list(e.get('deps', [])),
                'rpaths': list(e.get('rpaths', [])), 'minos': e.get('minos')}

    def run(self, argv):
        log('(dry run)', ' '.join(argv))
        if argv[0] == 'lipo':
            out = argv[argv.index('-output') + 1]
            with open(out, 'wb') as f:
                f.write(MACHO_MAGIC[4])
            first = self.db.get(key(argv[argv.index('-create') + 1]), {})
            self.db[key(out)] = json.loads(json.dumps(first))

    def copy(self, src, dest):
        Tools.copy(self, src, dest)
        if key(src) in self.db:
            self.db[key(dest)] = json.loads(json.dumps(self.db[key(src)]))


def expand(path, loader_dir, exec_dir):
    for token, base in (('@loader_path/', loader_dir), ('@executable_path/', exec_dir)):
        if path.startswith(token):
            return os.path.join(base, path[len(token):])
    return path


def check(tools, top, exec_dir):
    """Problems with every Mach-O under `top` (a file or a folder), and the highest minimum
    macOS version among them."""
    files = []
    if os.path.isdir(top):
        for d, _, names in os.walk(top):
            files += [os.path.join(d, n) for n in sorted(names)]
    else:
        files = [top]
    problems = []
    minos = None
    for f in files:
        if not is_macho(f):
            continue
        info = tools.inspect(f)
        log('  {}: macOS {}, {} libraries'.format(os.path.relpath(f, os.path.dirname(top) or '.'),
                                                  info['minos'], len(info['deps'])))
        if version_key(info['minos']) > version_key(minos):
            minos = info['minos']
        for dep in info['deps']:
            if dep.startswith(SYSTEM):
                continue
            if dep.startswith('@executable_path/') or dep.startswith('@loader_path/'):
                p = expand(dep, os.path.dirname(f), exec_dir or os.path.dirname(f))
                if not os.path.isfile(p):
                    problems.append('{}: {} is not in the bundle ({})'.format(f, dep, os.path.normpath(p)))
            else:
                problems.append('{}: {} is outside the bundle'.format(f, dep))
        for r in info['rpaths']:
            if not r.startswith('@'):
                problems.append('{}: LC_RPATH {} is outside the bundle'.format(f, r))
    return problems, minos


def report(problems, what):
    if problems:
        for p in problems:
            log('::error::' + p)
        raise BundleError('{}: {} problem(s), listed above'.format(what, len(problems)))


ICONSET = [  # (iconutil's name, the PNG's size)
    ('icon_16x16.png', 16), ('icon_16x16@2x.png', 32), ('icon_32x32.png', 32), ('icon_32x32@2x.png', 64),
    ('icon_128x128.png', 128), ('icon_128x128@2x.png', 256), ('icon_256x256.png', 256),
    ('icon_256x256@2x.png', 512), ('icon_512x512.png', 512),
]


def icns(tools, icon_dir, res, staging):
    """With the 1024 px entry (icon_512x512@2x) when `icon_dir` has a 1024.png; without one the
    icns ends at 512 px, and this says so."""
    iconset = os.path.join(staging, 'Rookparcel.iconset')
    os.makedirs(iconset, exist_ok=True)
    entries = list(ICONSET)
    if os.path.isfile(os.path.join(icon_dir, '1024.png')):
        entries.append(('icon_512x512@2x.png', 1024))
    else:
        log('note: {} has no 1024.png: the icns has no 1024 px entry (icon_512x512@2x)'.format(icon_dir))
    for name, size in entries:
        src = os.path.join(icon_dir, '{}.png'.format(size))
        if not os.path.isfile(src):
            raise BundleError('icon {} is missing'.format(src))
        shutil.copy2(src, os.path.join(iconset, name))
    tools.run(['iconutil', '--convert', 'icns', '--output', os.path.join(res, 'Rookparcel.icns'), iconset])


def info_plist(app_id, version, minos):
    """`version` is the workflow's (1.0.0, or 1.0.0-dev.abc1234 for a run without a tag):
    Apple's version keys take numbers only."""
    numeric = re.match(r'\d+(\.\d+){0,2}', version)
    short = numeric.group(0) if numeric else '0.0.0'
    return {
        'CFBundleDevelopmentRegion': 'en',
        'CFBundleDisplayName': 'Rookparcel',
        'CFBundleExecutable': 'Rookparcel',
        'CFBundleIconFile': 'Rookparcel',
        'CFBundleIdentifier': app_id,
        'CFBundleInfoDictionaryVersion': '6.0',
        'CFBundleName': 'Rookparcel',
        'CFBundlePackageType': 'APPL',
        'CFBundleShortVersionString': short,
        'CFBundleVersion': short,
        'RookparcelVersion': version,
        'LSMinimumSystemVersion': minos,
        'LSApplicationCategoryType': 'public.app-category.utilities',
        'NSHighResolutionCapable': True,
        # macOS 15 asks before an app may use the local network (LSD multicast, UPnP, NAT-PMP);
        # the prompt shows this text and the app's name instead of the host's
        'NSLocalNetworkUsageDescription': 'Rookparcel finds peers on your local network (Local Service Discovery) and asks your router to forward its port.',
        # Auto Shutdown's Sleep and Shut Down are Apple events to loginwindow (Apple's QA1134):
        # macOS may ask once before an app sends Apple events, with this text
        'NSAppleEventsUsageDescription': 'Rookparcel asks macOS to sleep or shut down when Auto Shutdown is set to.',
        'NSSupportsAutomaticGraphicsSwitching': True,
        'CFBundleDocumentTypes': [{
            'CFBundleTypeName': 'BitTorrent file',
            'CFBundleTypeRole': 'Viewer',
            'LSHandlerRank': 'Default',
            'CFBundleTypeExtensions': ['torrent'],
            'CFBundleTypeMIMETypes': ['application/x-bittorrent'],
            'CFBundleTypeIconFile': 'Rookparcel',
            'LSItemContentTypes': ['org.bittorrent.torrent'],
        }],
        'UTImportedTypeDeclarations': [{
            'UTTypeIdentifier': 'org.bittorrent.torrent',
            'UTTypeDescription': 'BitTorrent file',
            'UTTypeConformsTo': ['public.data'],
            'UTTypeTagSpecification': {
                'public.filename-extension': ['torrent'],
                'public.mime-type': ['application/x-bittorrent'],
            },
        }],
        'CFBundleURLTypes': [{
            'CFBundleURLName': 'Magnet link',
            'CFBundleTypeRole': 'Viewer',
            'CFBundleURLSchemes': ['magnet'],
        }],
    }


def languages(lang_dir=None):
    """The shipped languages as Apple names them (CFBundleLocalizations): English and every
    crates/frontend/lang/<code>.lng (de, pt-BR, zh-Hans, ...)."""
    lang_dir = lang_dir or os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', 'crates',
                                        'frontend', 'lang')
    codes = sorted(f[:-4] for f in os.listdir(lang_dir) if f.endswith('.lng'))
    if 'en' not in codes:
        raise BundleError('{} has no en.lng'.format(lang_dir))
    return ['en'] + [c for c in codes if c != 'en']


def native_plist(app_id, version, minos, localizations):
    """The app's Info.plist: info_plist's keys (document and URL types, the UTI, the usage
    texts) with AppKit's principal class and the shipped languages."""
    p = info_plist(app_id, version, minos)
    p['NSPrincipalClass'] = 'NSApplication'
    p['CFBundleLocalizations'] = localizations
    p['CFBundleAllowMixedLocalizations'] = True
    return p


def make_native(tools, a):
    app = os.path.abspath(a.out)
    contents = os.path.join(app, 'Contents')
    macos = os.path.join(contents, 'MacOS')
    res = os.path.join(contents, 'Resources')
    staging = app + '.staging'
    for d in (app, staging):
        if os.path.exists(d):
            shutil.rmtree(d)
    for d in (macos, res, staging):
        os.makedirs(d)
    exe = os.path.join(macos, 'Rookparcel')

    log('== binary')
    if len(a.binary) > 2:
        raise BundleError('at most two binaries (arm64 and x86_64)')
    if len(a.binary) == 2:
        tools.run(['lipo', '-create', os.path.abspath(a.binary[0]), os.path.abspath(a.binary[1]), '-output', exe])
    else:
        tools.copy(a.binary[0], exe)
    if not tools.dry:
        tools.run(['lipo', '-info', exe])

    log('== data')
    icns(tools, a.icon_dir, res, staging)
    for f in a.extra:
        shutil.copy2(f, res)

    log('== check')
    problems, minos = check(tools, app, macos)
    report(problems, a.out)
    minos = max(minos or '11.0', '11.0', key=version_key)
    log('minimum macOS (the highest LC_BUILD_VERSION minos inside): ' + minos)
    with open(os.path.join(contents, 'Info.plist'), 'wb') as f:
        plistlib.dump(native_plist(a.app_id, a.version, minos, languages(a.lang_dir)), f)
    with open(os.path.join(contents, 'PkgInfo'), 'w') as f:
        f.write('APPL????')

    log('== ad-hoc signing')
    tools.run(['codesign', '--force', '--sign', '-', app])
    shutil.rmtree(staging)
    if a.minos_file:
        with open(a.minos_file, 'w') as f:
            f.write(minos + '\n')


def main():
    p = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    p.add_argument('--dry-run', metavar='FIXTURE', help='answer otool from FIXTURE (JSON), print the changes')
    sub = p.add_subparsers(dest='cmd', required=True)
    s = sub.add_parser('native')
    s.add_argument('--binary', required=True, action='append', help='rookparcel-mac; twice for a universal app')
    s.add_argument('--icon-dir', required=True, help='folder with 16.png ... 512.png (and 1024.png)')
    s.add_argument('--app-id', required=True)
    s.add_argument('--version', required=True)
    s.add_argument('--out', required=True)
    s.add_argument('--extra', action='append', default=[], help='a file for Contents/Resources')
    s.add_argument('--lang-dir', help='the .lng files (default: crates/frontend/lang)')
    s.add_argument('--minos-file', help='write the minimum macOS version here')
    s = sub.add_parser('check')
    s.add_argument('--exec-dir', help='what @executable_path is (default: each file\'s folder)')
    s.add_argument('paths', nargs='+')
    a = p.parse_args()
    tools = FakeTools(a.dry_run) if a.dry_run else Tools()
    try:
        if a.cmd == 'native':
            make_native(tools, a)
        else:
            for path in a.paths:
                problems, minos = check(tools, path, a.exec_dir)
                report(problems, path)
                log('{}: only system libraries or its own; minimum macOS {}'.format(path, minos))
    except BundleError as e:
        log('::error::' + str(e).splitlines()[0])
        log(str(e))
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
