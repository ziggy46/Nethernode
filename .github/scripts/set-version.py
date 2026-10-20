#!/usr/bin/env python3
"""Sets the version of every NetherNode package before an edge build.

    set-version.py 0.2.10-edge.41

Cargo.toml's workspace version, the workspace packages in Cargo.lock (so
`--locked` builds still work), and web/package.json all get it.
"""

import json
import re
import sys
from pathlib import Path

version = sys.argv[1]
if not re.fullmatch(r"\d+\.\d+\.\d+(-edge\.\d+)?", version):
    sys.exit(f"not a NetherNode version: {version}")

manifest = Path("Cargo.toml")
text = manifest.read_text()
match = re.search(r'(?m)^version = "([^"]+)"$', text)
if not match:
    sys.exit("Cargo.toml has no workspace version")
base = match.group(1)
manifest.write_text(text[: match.start()] + f'version = "{version}"' + text[match.end() :])

# Workspace packages are the ones without a `source` (not from a registry).
lock = Path("Cargo.lock")
packages = lock.read_text().split("\n[[package]]\n")
changed = 0
for i, package in enumerate(packages):
    if "\nsource = " not in package and f'\nversion = "{base}"' in package:
        packages[i] = package.replace(f'\nversion = "{base}"', f'\nversion = "{version}"', 1)
        changed += 1
lock.write_text("\n[[package]]\n".join(packages))
if changed == 0:
    sys.exit(f"no workspace packages at {base} in Cargo.lock")

web = Path("web/package.json")
package = json.loads(web.read_text())
package["version"] = version
web.write_text(json.dumps(package, indent=2) + "\n")

print(f"{base} -> {version} ({changed} packages)")
