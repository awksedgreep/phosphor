#!/usr/bin/env bash
# Regenerate every demo GIF from scratch — fully scripted, no human.
#   tools/demo/demo.sh
# Needs: agg (cargo install --git https://github.com/asciinema/agg)
# and any monospace font (AGG_FONT to override).
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FONT="${AGG_FONT:-CaskaydiaMono Nerd Font Mono}"

cargo build --release --manifest-path "$ROOT/Cargo.toml"
"$ROOT/tools/demo/seed.sh" /tmp/phosphor-demo.db
python3 "$ROOT/tools/demo/scenarios.py"
cd "$ROOT/docs/demo"
for c in browse builders health appmode split; do
  [ -f "$c.cast" ] || continue   # health needs the timeless extension
  # split is recorded at 140x40: a smaller font keeps it readable.
  size=16
  [ "$c" = split ] && size=13
  agg -q --font-size "$size" --font-family "$FONT" "$c.cast" "$c.gif"
done
# The long-form CRM story lives in its own pipeline (tools/demo/crm.py):
# it records, verifies, merges, and renders crm.gif itself.
python3 "$ROOT/tools/demo/crm.py" --merge
agg -q --font-size 13 --font-family "$FONT" "$ROOT/docs/demo/crm.cast" "$ROOT/docs/demo/crm.gif"
echo "GIFs regenerated in docs/demo/"
