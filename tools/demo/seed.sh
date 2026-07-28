#!/usr/bin/env bash
# Build a fresh, deterministic demo database for the GIFs.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DB="${1:-/tmp/phosphor-demo.db}"
DBHEALTH_EXT="${DBHEALTH_EXT:-$ROOT/../timeless-libsql/target/release/libdbhealth_ext}"

rm -f "$DB" "$DB"-journal "$DB"-wal "$DB"-shm
sqlite3 "$DB" <<'SQL'
CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT NOT NULL, city TEXT, balance REAL);
INSERT INTO customers(name,city,balance) VALUES
 ('Ada','London',120.50),('Grace','Arlington',80.00),('Edsger','Austin',200.00),
 ('Barbara','London',310.00),('Donald','Austin',45.50),('Niklaus','Zurich',520.00),
 ('John','Cambridge',89.90),('Margaret','Arlington',150.00);
CREATE TABLE orders(id INTEGER PRIMARY KEY, customer TEXT NOT NULL, product TEXT, qty INTEGER, amount REAL, region TEXT);
INSERT INTO orders(customer,product,qty,amount,region) VALUES
 ('Ada','compiler',1,99.00,'east'),('Grace','linker',2,45.00,'east'),
 ('Barbara','abstraction',3,120.00,'east'),('Niklaus','pascal',1,80.00,'west'),
 ('Edsger','semaphore',5,25.00,'west'),('Donald','tex',1,7.99,'west'),
 ('Ada','engine',1,250.00,'east'),('Margaret','apollo',1,400.00,'east');

CREATE TABLE _phosphor_apps (id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL, description TEXT);
CREATE TABLE _phosphor_items (id INTEGER PRIMARY KEY, app_id INTEGER NOT NULL, label TEXT NOT NULL,
  action_kind TEXT NOT NULL, action_ref TEXT, hotkey TEXT, seq INTEGER DEFAULT 0);
INSERT INTO _phosphor_apps(name) VALUES ('crm');
INSERT INTO _phosphor_items(app_id,label,action_kind,action_ref,seq) VALUES
 (1,'Customers','browse','customers',0),
 (1,'Orders','browse','orders',1),
 (1,'Balances report','report','customers',2);

CREATE TABLE _phosphor_forms (id INTEGER PRIMARY KEY, table_ref TEXT UNIQUE NOT NULL,
  layout_json TEXT NOT NULL, version INTEGER DEFAULT 1);
INSERT INTO _phosphor_forms(table_ref, layout_json) VALUES ('customers',
'{"v":2,"size":{"w":56,"h":14},"texts":[{"x":18,"y":0,"text":"CUSTOMER CARD"}],"boxes":[{"x":1,"y":1,"w":52,"h":11}],"fields":[
 {"column":"name","label":"Name","include":true,"required":true,"x":4,"y":3,"width":28},
 {"column":"city","label":"City","include":true,"required":false,"x":4,"y":5,"width":28},
 {"column":"balance","label":"Balance","include":true,"required":false,"x":4,"y":7,"width":12},
 {"column":"id","label":"ID","include":false,"required":false}]}');
SQL

# dbhealth with a fast cadence so the live console moves on camera.
sqlite3 "$DB" ".load $DBHEALTH_EXT" \
  "CREATE VIRTUAL TABLE dbhealth USING dbhealth(every=2);
   INSERT INTO dbhealth(dbhealth) VALUES ('sample');"
echo "seeded $DB"
