#!/usr/bin/env python3
# Synthetic monitoring history for measuring the Reliability history at scale. Writes a scratch
# database the app can open with --db (never point it at the real one).
# Usage: tools/gen-monitor.py out.db DAYS INTERVAL_MIN RESOLVERS   e.g. out.db 30 1 24 (3.1M samples)
import random, sqlite3, sys, time
db, days, interval, nres = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
c = sqlite3.connect(db)
c.executescript("""
PRAGMA journal_mode=WAL; PRAGMA synchronous=OFF;
CREATE TABLE IF NOT EXISTS runs(id TEXT PRIMARY KEY, started_at INTEGER NOT NULL, finished_at INTEGER, status TEXT NOT NULL, summary TEXT NOT NULL, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS monitor(ts INTEGER NOT NULL, resolver_id TEXT NOT NULL, test TEXT NOT NULL, us INTEGER, ok INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS monitor_ts ON monitor(ts);
CREATE TABLE IF NOT EXISTS cache(key TEXT PRIMARY KEY, fetched_at INTEGER NOT NULL, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS custom(id TEXT PRIMARY KEY, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, body TEXT NOT NULL);
""")
ids = ["cloudflare","google","quad9","opendns","adguard","controld","nextdns","dns4eu","he","ultradns","level3","mullvad",
       "cloudflare-security","cloudflare-family","quad9-ecs","quad9-unsecured","opendns-family","adguard-unfiltered",
       "adguard-family","cleanbrowsing-security","cleanbrowsing-family","controld-malware","controld-ads","system-0"][:nres]
now = int(time.time() * 1000)
start = now - days * 86_400_000
rows = []
step = interval * 60_000
t = start - start % step
down = {}
while t < now:
    for r in ids:
        if down.get(r, 0) > 0:
            down[r] -= 1; up = False
        else:
            if random.random() < 0.0005: down[r] = random.randint(1, 8)
            up = True
        for k, base in (("cached", 12), ("uncached", 30), ("tld", 22)):
            ok = up and random.random() > 0.002
            rows.append((t, r, k, int((base + random.expovariate(1 / 6)) * 1000) if ok else None, 1 if ok else 0))
    if len(rows) > 200_000:
        c.executemany("INSERT INTO monitor VALUES (?,?,?,?,?)", rows); rows = []
    t += step
c.executemany("INSERT INTO monitor VALUES (?,?,?,?,?)", rows)
c.execute("INSERT OR REPLACE INTO settings VALUES ('monitor', ?)",
          ('{"enabled":false,"interval_min":%d,"resolvers":[%s],"retention_days":365}' % (interval, ",".join('"%s"' % i for i in ids)),))
c.commit()
print(c.execute("select count(*) from monitor").fetchone()[0], "rows")
