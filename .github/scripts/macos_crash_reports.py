"""Prints the crash reports macOS wrote during a CI job (~/Library/Logs/DiagnosticReports).

A test binary that crashes (Core Foundation traps on misuse, as `CFRelease(NULL)` does) prints
no test output; its report names the exception, the trap message ("asi"), the crashing thread
(a test thread is named after its test) and that thread's frames.
"""

import glob
import json
import os

files = sorted(glob.glob(os.path.expanduser('~/Library/Logs/DiagnosticReports/*.ips')))
print(len(files), 'crash reports')
for f in files:
    text = open(f, errors='replace').read()
    _, _, body = text.partition('\n')
    try:
        d = json.loads(body)
    except ValueError:
        print(text[:20000])
        continue
    print('==', os.path.basename(f))
    print('exception:', d.get('exception'))
    print('termination:', d.get('termination'))
    print('asi:', d.get('asi'))
    images = d.get('usedImages', [])
    for t in d.get('threads', []):
        if not t.get('triggered'):
            continue
        print('thread:', t.get('name'), t.get('queue'))
        for fr in t.get('frames', []):
            i = fr.get('imageIndex')
            image = images[i].get('name', '?') if i is not None and i < len(images) else '?'
            symbol = fr.get('symbol', hex(fr.get('imageOffset', 0)))
            print('  ', image, symbol, '+', fr.get('symbolLocation', ''))
