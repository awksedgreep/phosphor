#!/usr/bin/env bash
# The CRM demo builds EVERYTHING through the UI, so its seed is an
# empty database — the whole point of the story.
set -euo pipefail
DB="${1:-/tmp/phosphor-crm.db}"
rm -f "$DB" "$DB"-journal "$DB"-wal "$DB"-shm
sqlite3 "$DB" "PRAGMA journal_mode = DELETE; SELECT 'empty db ready';"
echo "seeded empty $DB"
