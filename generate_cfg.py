#!/usr/bin/env python3
# Copyright (c) 2026 Faris Alfarhan
# SPDX-License-Identifier: GPL-3.0-only

"""Read applets.toml and generate RUSTFLAGS with --cfg for enabled applets."""

import re
import sys

def parse_applets_toml(path="applets.toml"):
    """Parse the [applets] section and return dict of {name: bool}."""
    with open(path) as f:
        content = f.read()

    # Find the [applets] section
    match = re.search(r'\[applets\]\n(.*)', content, re.DOTALL)
    if not match:
        return {}

    section = match.group(1)
    applets = {}

    for line in section.strip().split('\n'):
        line = line.strip()
        if not line or line.startswith('#'):
            continue
        # Match: name = true  or  # name = true
        m = re.match(r'#?\s*(\w+)\s*=\s*(true|false)', line)
        if m:
            name, value = m.groups()
            applets[name] = value == 'true'

    return applets

def main():
    applets = parse_applets_toml()
    flags = []
    for name, enabled in applets.items():
        if enabled:
            flags.append(f'--cfg applet_{name}')

    # Also add the base cfg flags
    flags.append('-C link-arg=-Tlinker.ld')
    flags.append('-C relocation-model=static')

    print(' '.join(flags))

if __name__ == '__main__':
    main()
