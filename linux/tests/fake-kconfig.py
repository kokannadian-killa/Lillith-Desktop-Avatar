#!/usr/bin/env python3
# Minimal fake kreadconfig6/kwriteconfig6 for linux/tests (KConfig escaping of \n, \\).
import sys, os, re
mode = os.path.basename(sys.argv[0])
a = sys.argv[1:]
def opt(n):
    i = a.index(n); v = a[i+1]; del a[i:i+2]; return v
f = opt('--file'); g = opt('--group'); k = opt('--key')
if '--' in a: a.remove('--')
path = f if f.startswith('/') else os.path.join(os.environ.get('XDG_CONFIG_HOME', os.path.expanduser('~/.config')), f)
lines = open(path).read().split('\n') if os.path.exists(path) else []
esc = lambda v: v.replace('\\', '\\\\').replace('\n', '\\n')
def unesc(v):
    return re.sub(r'\\(.)', lambda m: {'n': '\n', 't': '\t', 's': ' '}.get(m.group(1), m.group(1)), v)
cur = None; found = None
for i, l in enumerate(lines):
    if l.startswith('['): cur = l[1:-1]; continue
    if cur == g and l.split('=', 1)[0] == k: found = i
if mode.startswith('kread'):
    if found is not None: print(unesc(lines[found].split('=', 1)[1]))
    sys.exit(0)
val = a[0]
if found is not None:
    lines[found] = f'{k}={esc(val)}'
else:
    try: gi = lines.index(f'[{g}]'); lines.insert(gi+1, f'{k}={esc(val)}')
    except ValueError: lines += ['', f'[{g}]', f'{k}={esc(val)}']
open(path, 'w').write('\n'.join(lines))
