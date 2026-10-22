#!/usr/bin/env python3
"""Builds PaTorrent's macOS bundle and checks Mach-O files for libraries outside it (PLAN.md D23,
Road to 1.0 item 8). Run by the `macos` job of .github/workflows/release.yml.

  macos_bundle.py app --binary target/release/patorrent-gtk --prefix "$(brew --prefix)"
                      --icon-dir DIR --app-id ID --version V --out PaTorrent.app [--extra FILE]...
      PaTorrent.app: Contents/MacOS/PaTorrent (macos_launcher.c: sets GTK's and GLib's paths,
      then execs patorrent-gtk), Contents/MacOS/patorrent-gtk, Contents/Frameworks/*.dylib
      (every non-system library, found by walking `otool -L` from the binary and the modules,
      rewritten to @executable_path/../Frameworks), Contents/Resources/lib (GdkPixbuf loaders
      with a loaders.cache naming them by @executable_path, GIO modules),
      Contents/Resources/share (compiled GSettings schemas, the Adwaita and hicolor icon
      themes), Contents/Resources/PaTorrent.icns, Contents/Info.plist. Every Mach-O inside is
      ad-hoc signed; the job then signs the whole app.

  macos_bundle.py deps --lib-dir DIR --ref REF BINARY...
      The same library walk for plain binaries (patorrent-nox in a tarball, say): copies what
      they need into DIR and rewrites the references to REF/<name> (`@executable_path/lib`).

  macos_bundle.py check [--exec-dir DIR] PATH...
      Fails when a Mach-O under PATH needs a library that is neither the system's (/usr/lib,
      /System) nor inside PATH, or keeps an absolute LC_RPATH. Prints each file's minimum
      macOS version.

Every outside program (otool, install_name_tool, codesign, clang, iconutil, the GLib and
GdkPixbuf tools) runs through `Tools`. `--dry-run FIXTURE.json` replaces it with `FakeTools`,
which answers otool from the fixture and prints the commands that would change files instead
of running them, so the logic can be tried without a Mac.

Python 3.9 or later (the runner's /usr/bin/python3).
"""

import argparse
import glob
import json
import os
import plistlib
import re
import shutil
import stat
import subprocess
import sys

SYSTEM = ('/usr/lib/', '/System/')
FRAMEWORKS_REF = '@executable_path/../Frameworks'
PIXBUF = 'lib/gdk-pixbuf-2.0/2.10.0'
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

    def query_loaders(self, program, loaders):
        return self._out([program] + loaders)

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
    {"files": {path: {"id", "deps", "rpaths", "minos"}}, "query_loaders": text}, where "{root}"
    in any string stands for the fixture's directory. install_name_tool edits the fixture's
    entries, so `check` sees what the real run would leave; the other programs are printed."""

    dry = True

    def __init__(self, fixture):
        root = os.path.dirname(os.path.abspath(fixture)).replace('\\', '/')
        with open(fixture) as f:
            data = json.loads(f.read().replace('{root}', root))
        # otool -L lists a library's own id first; parse_otool drops it, and so does this
        self.db = {key(p): dict(v, deps=[d for d in v.get('deps', []) if d != v.get('id')])
                   for p, v in data['files'].items()}
        self.loaders_text = data.get('query_loaders', '')

    def inspect(self, path):
        e = self.db.get(key(path))
        if e is None:
            raise BundleError('dry run: no fixture entry for ' + path)
        return {'id': e.get('id'), 'deps': list(e.get('deps', [])),
                'rpaths': list(e.get('rpaths', [])), 'minos': e.get('minos')}

    def query_loaders(self, program, loaders):
        log('(dry run) {} {}'.format(program, ' '.join(loaders)))
        return self.loaders_text

    def run(self, argv):
        log('(dry run)', ' '.join(argv))
        if argv[0] == 'install_name_tool':
            e = self.db[key(argv[-1])]
            i = 1
            while i < len(argv) - 1:
                if argv[i] == '-change':
                    e['deps'] = [argv[i + 2] if d == argv[i + 1] else d for d in e['deps']]
                    i += 3
                elif argv[i] == '-id':
                    e['id'] = argv[i + 1]
                    i += 2
                elif argv[i] == '-delete_rpath':
                    e['rpaths'].remove(argv[i + 1])
                    i += 2
                else:
                    raise BundleError('dry run: unknown install_name_tool option ' + argv[i])
        elif argv[0] == 'clang':
            out = argv[argv.index('-o') + 1]
            with open(out, 'wb') as f:
                f.write(MACHO_MAGIC[0])
            self.db[key(out)] = {'deps': ['/usr/lib/libSystem.B.dylib'], 'rpaths': [], 'minos': '11.0'}

    def copy(self, src, dest):
        Tools.copy(self, src, dest)
        if key(src) in self.db:
            self.db[key(dest)] = json.loads(json.dumps(self.db[key(src)]))


def expand(path, loader_dir, exec_dir):
    for token, base in (('@loader_path/', loader_dir), ('@executable_path/', exec_dir)):
        if path.startswith(token):
            return os.path.join(base, path[len(token):])
    return path


def resolve(dep, src, rpaths, exec_dir, fallback_dirs):
    """The file `src` loads for `dep`, as dyld would find it, or None."""
    loader_dir = os.path.dirname(src)
    if dep.startswith('@rpath/'):
        rest = dep[len('@rpath/'):]
        candidates = [os.path.join(expand(r, loader_dir, exec_dir), rest) for r in rpaths]
    else:
        candidates = [expand(dep, loader_dir, exec_dir)]
    candidates += [os.path.join(d, os.path.basename(dep)) for d in fallback_dirs]
    for i, c in enumerate(candidates):
        if os.path.isfile(c):
            if i >= len(candidates) - len(fallback_dirs):
                log('  note: {} (from {}) found only in {}'.format(dep, src, os.path.dirname(c)))
            return os.path.realpath(c)
    return None


def bundle_libraries(tools, roots, lib_dir, ref, exec_dir, fallback_dirs=()):
    """Copies every non-system library that `roots` ([(source, file in the bundle)]) need,
    transitively, into `lib_dir`, and points every reference at `ref`/<name>. Returns the names
    copied. Absolute and @loader_path LC_RPATHs are removed (nothing uses @rpath afterwards)."""
    by_real = {}   # real source path -> name in lib_dir
    by_name = {}   # name in lib_dir -> real source path
    queue = list(roots)
    root_rpaths = []
    for src, _ in roots[:1]:
        root_rpaths = tools.inspect(src)['rpaths']
    done = set()
    edits = []
    missing = []
    while queue:
        src, dest = queue.pop(0)
        if key(dest) in done:
            continue
        done.add(key(dest))
        info = tools.inspect(src)
        change = []
        for dep in info['deps']:
            if dep.startswith(SYSTEM):
                continue
            real = resolve(dep, src, info['rpaths'] + root_rpaths, exec_dir, fallback_dirs)
            if real is None:
                missing.append('{} needs {}, which was not found'.format(src, dep))
                continue
            name = by_real.get(key(real))
            if name is None:
                name = os.path.basename(dep)
                if name in by_name:
                    raise BundleError('two different libraries are named {}: {} and {}'.format(
                        name, by_name[name], real))
                by_real[key(real)] = name
                by_name[name] = real
                target = os.path.join(lib_dir, name)
                log('  {} <- {}'.format(name, real))
                tools.copy(real, target)
                queue.append((real, target))
            change.append((dep, '{}/{}'.format(ref, name)))
        in_lib = os.path.dirname(key(dest)) == key(lib_dir)
        new_id = '{}/{}'.format(ref, os.path.basename(dest)) if in_lib else None
        drop = [r for r in info['rpaths'] if not r.startswith('@executable_path')]
        edits.append((dest, new_id, change, drop))
    if missing:
        raise BundleError('\n'.join(missing))
    for dest, new_id, change, drop in edits:
        argv = ['install_name_tool']
        for r in drop:
            argv += ['-delete_rpath', r]
        if new_id:
            argv += ['-id', new_id]
        for old, new in change:
            if old != new:
                argv += ['-change', old, new]
        if len(argv) > 1:
            tools.run(argv + [dest])
    return sorted(by_name)


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


def sign_inner(tools, top):
    """Ad-hoc signs every Mach-O under `top` (install_name_tool broke their signatures, and
    arm64 macOS kills code with a broken one)."""
    for d, _, names in os.walk(top):
        for n in sorted(names):
            f = os.path.join(d, n)
            if is_macho(f):
                tools.run(['codesign', '--force', '--sign', '-', f])


def pixbuf_loaders(tools, prefix, res):
    """Copies Homebrew's GdkPixbuf loaders and writes a loaders.cache naming them by
    @executable_path (dyld resolves that in dlopen, which GModule calls with the name as is,
    so the cache needs no rewriting at run time). Returns [(source, copy)]."""
    src_dir = os.path.join(prefix, PIXBUF, 'loaders')
    dest_dir = os.path.join(res, PIXBUF, 'loaders')
    os.makedirs(dest_dir, exist_ok=True)
    loaders = sorted(glob.glob(os.path.join(src_dir, '*.so')) + glob.glob(os.path.join(src_dir, '*.dylib')))
    log('GdkPixbuf loaders in {}: {}'.format(src_dir, ', '.join(os.path.basename(p) for p in loaders) or 'none'))
    if not loaders:
        log('::warning::no GdkPixbuf loaders found; GTK will only draw the formats it decodes itself')
    query = os.path.join(prefix, 'bin', 'gdk-pixbuf-query-loaders')
    text = tools.query_loaders(query, loaders) if loaders else ''
    known = {key(p): p for p in loaders}
    out = []
    roots = []
    for line in text.splitlines():
        m = re.match(r'^"([^"]+)"\s*$', line)
        if m:
            path = m.group(1)
            if key(path) not in known:
                raise BundleError('loaders.cache names {}, which is not one of the loaders queried'.format(path))
            dest = os.path.join(dest_dir, os.path.basename(path))
            tools.copy(path, dest)
            roots.append((os.path.realpath(path), dest))
            line = '"@executable_path/../Resources/{}/loaders/{}"'.format(PIXBUF, os.path.basename(path))
        elif line.startswith('#') and ('/' in line or '\\' in line):
            continue  # comments naming the build machine's paths
        out.append(line)
    if loaders and not roots:
        raise BundleError('gdk-pixbuf-query-loaders named none of the loaders:\n' + text)
    cache = os.path.join(res, PIXBUF, 'loaders.cache')
    with open(cache, 'w') as f:
        f.write('\n'.join(out) + '\n')
    log('loaders.cache:\n' + '\n'.join(out))
    return roots


def gio_modules(tools, prefix, res):
    src_dir = os.path.join(prefix, 'lib', 'gio', 'modules')
    dest_dir = os.path.join(res, 'lib', 'gio', 'modules')
    os.makedirs(dest_dir, exist_ok=True)
    roots = []
    for p in sorted(glob.glob(os.path.join(src_dir, '*.so')) + glob.glob(os.path.join(src_dir, '*.dylib'))):
        dest = os.path.join(dest_dir, os.path.basename(p))
        tools.copy(p, dest)
        roots.append((os.path.realpath(p), dest))
    log('GIO modules: {}'.format(', '.join(os.path.basename(d) for _, d in roots) or 'none'))
    return roots


def schemas(tools, prefix, res, staging):
    src_dir = os.path.join(prefix, 'share', 'glib-2.0', 'schemas')
    files = sorted(glob.glob(os.path.join(src_dir, '*.gschema.xml')) + glob.glob(os.path.join(src_dir, '*.enums.xml')))
    if not files:
        raise BundleError('no GSettings schemas in ' + src_dir)
    names = [os.path.basename(f) for f in files]
    log('GSettings schemas: ' + ', '.join(names))
    if not any(n.startswith('org.gtk.gtk4.') for n in names):
        log('::warning::none of GTK 4\'s schemas (org.gtk.gtk4.*) found: the file chooser will abort')
    os.makedirs(staging, exist_ok=True)
    for f in files:
        shutil.copy2(f, staging)
    dest = os.path.join(res, 'share', 'glib-2.0', 'schemas')
    os.makedirs(dest, exist_ok=True)
    tools.run([os.path.join(prefix, 'bin', 'glib-compile-schemas'), '--targetdir=' + dest, staging])
    if not tools.dry and not os.path.isfile(os.path.join(dest, 'gschemas.compiled')):
        raise BundleError('glib-compile-schemas made no gschemas.compiled')


def icon_themes(tools, prefix, res):
    """Adwaita (the toolbar's document-open, media-playback-start, ... come from it) and
    hicolor's index.theme (the fallback theme every lookup ends in)."""
    icons = os.path.join(res, 'share', 'icons')
    adwaita = os.path.join(prefix, 'share', 'icons', 'Adwaita')
    if not os.path.isdir(adwaita):
        raise BundleError('{} is missing (brew install adwaita-icon-theme)'.format(adwaita))
    # links copied as files: a bundle must not point outside itself
    shutil.copytree(adwaita, os.path.join(icons, 'Adwaita'), symlinks=False, ignore_dangling_symlinks=True)
    # Homebrew's share/icons/Adwaita may be a merge of several formulae's links (gtk4 and
    # libadwaita add icons there) without the theme's own index.theme linked; take it from
    # the adwaita-icon-theme keg then. Without it gtk4-update-icon-cache refuses the theme
    # and GTK still finds the icons, only without the cache.
    index = os.path.join(icons, 'Adwaita', 'index.theme')
    if not os.path.isfile(index):
        kegs = sorted(glob.glob(os.path.join(prefix, 'Cellar', 'adwaita-icon-theme', '*', 'share', 'icons',
                                             'Adwaita', 'index.theme')))
        if kegs:
            shutil.copy2(kegs[-1], index)
            log('Adwaita index.theme taken from {}'.format(kegs[-1]))
        else:
            log('::warning::{} has no index.theme and no adwaita-icon-theme keg has one; entries: {}'.format(
                adwaita, ', '.join(sorted(os.listdir(adwaita))[:20])))
    hicolor = os.path.join(prefix, 'share', 'icons', 'hicolor', 'index.theme')
    os.makedirs(os.path.join(icons, 'hicolor'), exist_ok=True)
    if os.path.isfile(hicolor):
        shutil.copy2(hicolor, os.path.join(icons, 'hicolor'))
    else:
        log('::warning::{} is missing'.format(hicolor))
    cache_tool = os.path.join(prefix, 'bin', 'gtk4-update-icon-cache')
    if os.path.isfile(cache_tool) or tools.dry:
        for theme in ('Adwaita', 'hicolor'):
            if os.path.isfile(os.path.join(icons, theme, 'index.theme')) or tools.dry:
                tools.run([cache_tool, '--quiet', '--force', os.path.join(icons, theme)])
            else:
                log('::warning::no icon cache for {}: it has no index.theme'.format(theme))


ICONSET = [  # (iconutil's name, the PNG's size)
    ('icon_16x16.png', 16), ('icon_16x16@2x.png', 32), ('icon_32x32.png', 32), ('icon_32x32@2x.png', 64),
    ('icon_128x128.png', 128), ('icon_128x128@2x.png', 256), ('icon_256x256.png', 256),
    ('icon_256x256@2x.png', 512), ('icon_512x512.png', 512),
]


def icns(tools, icon_dir, res, staging):
    iconset = os.path.join(staging, 'PaTorrent.iconset')
    os.makedirs(iconset, exist_ok=True)
    for name, size in ICONSET:
        src = os.path.join(icon_dir, '{}.png'.format(size))
        if not os.path.isfile(src):
            raise BundleError('icon {} is missing'.format(src))
        shutil.copy2(src, os.path.join(iconset, name))
    tools.run(['iconutil', '--convert', 'icns', '--output', os.path.join(res, 'PaTorrent.icns'), iconset])


def info_plist(app_id, version, minos):
    """`version` is the workflow's (1.0.0, or 1.0.0-dev.abc1234 for a run without a tag):
    Apple's version keys take numbers only."""
    numeric = re.match(r'\d+(\.\d+){0,2}', version)
    short = numeric.group(0) if numeric else '0.0.0'
    return {
        'CFBundleDevelopmentRegion': 'en',
        'CFBundleDisplayName': 'PaTorrent',
        'CFBundleExecutable': 'PaTorrent',
        'CFBundleIconFile': 'PaTorrent',
        'CFBundleIdentifier': app_id,
        'CFBundleInfoDictionaryVersion': '6.0',
        'CFBundleName': 'PaTorrent',
        'CFBundlePackageType': 'APPL',
        'CFBundleShortVersionString': short,
        'CFBundleVersion': short,
        'PaTorrentVersion': version,
        'LSMinimumSystemVersion': minos,
        'LSApplicationCategoryType': 'public.app-category.utilities',
        'NSHighResolutionCapable': True,
        # macOS 15 asks before an app may use the local network (LSD multicast, UPnP, NAT-PMP);
        # the prompt shows this text and the app's name instead of the host's
        'NSLocalNetworkUsageDescription': 'PaTorrent finds peers on your local network (Local Service Discovery) and asks your router to forward its port.',
        'NSSupportsAutomaticGraphicsSwitching': True,
        'CFBundleDocumentTypes': [{
            'CFBundleTypeName': 'BitTorrent file',
            'CFBundleTypeRole': 'Viewer',
            'LSHandlerRank': 'Default',
            'CFBundleTypeExtensions': ['torrent'],
            'CFBundleTypeMIMETypes': ['application/x-bittorrent'],
            'CFBundleTypeIconFile': 'PaTorrent',
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
            'CFBundleURLSchemes': ['magnet'],
        }],
    }


def make_app(tools, a):
    app = os.path.abspath(a.out)
    contents = os.path.join(app, 'Contents')
    macos = os.path.join(contents, 'MacOS')
    frameworks = os.path.join(contents, 'Frameworks')
    res = os.path.join(contents, 'Resources')
    staging = os.path.abspath(a.out) + '.staging'
    for d in (app, staging):
        if os.path.exists(d):
            shutil.rmtree(d)
    for d in (macos, frameworks, res):
        os.makedirs(d)
    prefix = os.path.abspath(a.prefix)
    here = os.path.dirname(os.path.abspath(__file__))

    log('== launcher')
    tools.run(['clang', '-O2', '-Wall', '-Wextra', '-Werror', '-mmacosx-version-min=11.0',
               '-o', os.path.join(macos, 'PaTorrent'), os.path.join(here, 'macos_launcher.c')])
    gui = os.path.join(macos, 'patorrent-gtk')
    tools.copy(a.binary, gui)

    log('== modules')
    roots = [(os.path.realpath(a.binary), gui)]
    roots += pixbuf_loaders(tools, prefix, res)
    roots += gio_modules(tools, prefix, res)
    # GTK's own modules (lib/gtk-4.0: print backends, media) stay out: the GUI does neither, and
    # Homebrew's cups print backend would bring CUPS-related libraries for nothing
    os.makedirs(os.path.join(res, 'lib', 'gtk-4.0'), exist_ok=True)

    log('== libraries (otool -L, transitively)')
    names = bundle_libraries(tools, roots, frameworks, FRAMEWORKS_REF, os.path.dirname(os.path.realpath(a.binary)),
                             fallback_dirs=[os.path.join(prefix, 'lib')])
    log('{} libraries in Contents/Frameworks'.format(len(names)))

    log('== data')
    schemas(tools, prefix, res, os.path.join(staging, 'schemas'))
    icon_themes(tools, prefix, res)
    icns(tools, a.icon_dir, res, staging)
    for f in a.extra:
        shutil.copy2(f, res)

    log('== check')
    problems, minos = check(tools, app, macos)
    cache = os.path.join(res, PIXBUF, 'loaders.cache')
    with open(cache) as f:
        for m in re.finditer(r'^"(@executable_path/[^"]+)"', f.read(), re.M):
            if not os.path.isfile(expand(m.group(1), '', macos)):
                problems.append('loaders.cache: {} is not in the bundle'.format(m.group(1)))
    report(problems, a.out)
    minos = max(minos or '11.0', '11.0', key=version_key)
    log('minimum macOS (the highest LC_BUILD_VERSION minos inside): ' + minos)

    with open(os.path.join(contents, 'Info.plist'), 'wb') as f:
        plistlib.dump(info_plist(a.app_id, a.version, minos), f)
    with open(os.path.join(contents, 'PkgInfo'), 'w') as f:
        f.write('APPL????')

    log('== ad-hoc signing every Mach-O inside')
    sign_inner(tools, contents)
    shutil.rmtree(staging)
    if a.minos_file:
        with open(a.minos_file, 'w') as f:
            f.write(minos + '\n')


def main():
    p = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    p.add_argument('--dry-run', metavar='FIXTURE', help='answer otool from FIXTURE (JSON), print the changes')
    sub = p.add_subparsers(dest='cmd', required=True)
    s = sub.add_parser('app')
    s.add_argument('--binary', required=True)
    s.add_argument('--prefix', required=True, help="Homebrew's prefix (brew --prefix)")
    s.add_argument('--icon-dir', required=True, help='folder with 16.png ... 512.png')
    s.add_argument('--app-id', required=True)
    s.add_argument('--version', required=True)
    s.add_argument('--out', required=True)
    s.add_argument('--extra', action='append', default=[], help='a file for Contents/Resources')
    s.add_argument('--minos-file', help='write the minimum macOS version here')
    s = sub.add_parser('deps')
    s.add_argument('--lib-dir', required=True)
    s.add_argument('--ref', required=True, help='the reference prefix, e.g. @executable_path/lib')
    s.add_argument('binaries', nargs='+')
    s = sub.add_parser('check')
    s.add_argument('--exec-dir', help='what @executable_path is (default: each file\'s folder)')
    s.add_argument('paths', nargs='+')
    a = p.parse_args()
    tools = FakeTools(a.dry_run) if a.dry_run else Tools()
    try:
        if a.cmd == 'app':
            make_app(tools, a)
        elif a.cmd == 'deps':
            for b in a.binaries:
                b = os.path.abspath(b)
                names = bundle_libraries(tools, [(os.path.realpath(b), b)], os.path.abspath(a.lib_dir), a.ref,
                                         os.path.dirname(b))
                log('{}: {}'.format(b, ', '.join(names) or 'only system libraries, nothing bundled'))
                problems = check(tools, b, None)[0]
                if os.path.isdir(a.lib_dir):
                    problems += check(tools, os.path.abspath(a.lib_dir), os.path.dirname(b))[0]
                report(problems, b)
                if names:
                    sign_inner(tools, a.lib_dir)
                    tools.run(['codesign', '--force', '--sign', '-', b])
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
