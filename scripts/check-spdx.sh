#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Fails if a tracked source file lacks the SPDX header required by CLAUDE.md (licence rule).
# JSON cannot carry comments and is exempt; firmware/compat.json states it in "$comment".
set -eu
cd "$(dirname "$0")/.."
missing=$(git ls-files --cached --others --exclude-standard \
  '*.rs' '*.ts' '*.tsx' '*.mjs' '*.js' '*.css' '*.html' '*.sh' '*.py' '*.toml' '*.yml' '*.yaml' '*.x' \
  | while read -r f; do head -n 3 "$f" | grep -q 'SPDX-License-Identifier: GPL-3.0-or-later' || echo "$f"; done)
if [ -n "$missing" ]; then
  echo "Missing SPDX-License-Identifier: GPL-3.0-or-later in:"
  echo "$missing"
  exit 1
fi
echo "SPDX headers OK"
