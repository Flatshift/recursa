#!/usr/bin/env python3
# Builds the two name lists the Extended test bundles. Standard library only.
#   tools/build-names.py common --check [--work DIR]     keep the names in data/names-common.txt that resolve and hold a long TTL
#   tools/build-names.py pool --classes                  verify the Wikidata class IDs (label and count of official websites)
#   tools/build-names.py pool [--work DIR]               build data/names-pool.txt from Wikidata official websites
# Flags: --work DIR scratch for downloads and the resumable progress file, --rate N lookups per second (default 20),
# --limit N small trial run (writes to the work folder, never over the bundled files), --doh URL the pool's liveness resolver.
import argparse, ipaddress, json, os, random, re, sys, time, threading, urllib.error, urllib.parse, urllib.request
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
COMMON = os.path.join(ROOT, "data", "names-common.txt")
POOL = os.path.join(ROOT, "data", "names-pool.txt")
UA = "Recursa-DNS-Benchmark-build/1 (+https://github.com/flatshift/recursa)"
PSL_URL = "https://publicsuffix.org/list/public_suffix_list.dat"
SPARQL = "https://query.wikidata.org/sparql"
WDAPI = "https://www.wikidata.org/w/api.php"
CHECK_ENDPOINTS = ["https://dns.google/resolve", "https://cloudflare-dns.com/dns-query"]
LIVENESS = "https://cloudflare-dns.com/dns-query"
COMMON_KEEP, MIN_TTL = 500, 900
PAGE, PAGE_PAUSE = 20000, 2
BLOCKED_TLDS = {"local", "localhost", "internal", "onion", "test", "invalid", "example", "lan", "home", "corp", "arpa"}
# Country TLDs left out because filtering resolvers and networks often block or drop names under them, which reads as loss.
EXCLUDED_TLDS = {"ru", "su", "xn--p1ai", "cn", "xn--fiqs8s", "xn--fiqz9s", "by", "ir", "kp", "sy", "cu"}

# Wikidata classes whose members' official websites make the pool, as (QID, expected English label). Direct instance-of only.
# Never: political parties and politicians, religious bodies, health providers, adult, gambling, trade unions, people.
CLASSES = [
    ("Q515", "city"),
    ("Q3957", "town"),
    ("Q15284", "municipality"),
    ("Q1549591", "big city"),
    ("Q484170", "commune of France"),
    ("Q262166", "municipality in Germany"),
    ("Q747074", "comune of Italy"),
    ("Q2074737", "municipality of Spain"),
    ("Q127448", "municipality of Sweden"),
    ("Q755707", "municipality of Norway"),
    ("Q2039348", "municipality of the Netherlands"),
    ("Q3184121", "municipality of Brazil"),
    ("Q70208", "municipality of Switzerland"),
    ("Q667509", "municipality of Austria"),
    ("Q494721", "city of Japan"),
    ("Q3918", "university"),
    ("Q189004", "college"),
    ("Q3914", "school"),
    ("Q31855", "research institute"),
    ("Q33506", "museum"),
    ("Q7075", "library"),
    ("Q166118", "archives"),
    ("Q11032", "newspaper"),
    ("Q41298", "magazine"),
    ("Q2085381", "publishing house"),
    ("Q14350", "radio station"),
    ("Q1616075", "television station"),
    ("Q24354", "theatre building"),
    ("Q43501", "zoo"),
    ("Q167346", "botanical garden"),
    ("Q1248784", "airport"),
    ("Q783794", "company"),
    ("Q4830453", "business"),
    ("Q6881511", "enterprise"),
    ("Q327333", "government agency"),
    ("Q163740", "nonprofit organization"),
]

# Social networks, platforms and hosting: a name there says nothing about a place or an organisation.
PLATFORMS = {
    "facebook.com", "fb.com", "fb.me", "twitter.com", "x.com", "instagram.com", "youtube.com", "youtu.be", "linkedin.com",
    "tiktok.com", "wikipedia.org", "wikimedia.org", "wikidata.org", "blogspot.com", "blogger.com", "wordpress.com",
    "wordpress.org", "github.com", "github.io", "gitlab.com", "google.com", "googleusercontent.com", "t.me", "telegram.org",
    "vk.com", "weibo.com", "pinterest.com", "tumblr.com", "medium.com", "flickr.com", "linktr.ee", "wixsite.com", "wix.com",
    "weebly.com", "squarespace.com", "webnode.com", "jimdo.com", "jimdofree.com", "sites.google.com", "archive.org",
    "web.archive.org", "line.me", "whatsapp.com", "snapchat.com", "reddit.com", "twitch.tv", "vimeo.com", "issuu.com",
    "yelp.com", "tripadvisor.com", "amazon.com", "ebay.com", "microsoft.com", "apple.com", "netlify.app", "vercel.app",
    "herokuapp.com", "azurewebsites.net", "ning.com", "yahoo.com", "live.com", "mail.ru", "ok.ru", "livejournal.com",
}

# Hold back this many seconds after a 429 when the answer carries no Retry-After.
DEFAULT_RETRY = 30


def log(*a):
    print(*a, file=sys.stderr, flush=True)


class Rate:
    # One shared pace for every thread: at most `per_s` calls a second.
    def __init__(self, per_s):
        self.step, self.next, self.lock = 1.0 / per_s, time.monotonic(), threading.Lock()

    def wait(self):
        with self.lock:
            now = time.monotonic()
            self.next = max(self.next, now)
            sleep = self.next - now
            self.next += self.step
        if sleep > 0:
            time.sleep(sleep)


def fetch(url, accept=None, retries=5):
    # GET with the build User-Agent. Honours 429 and Retry-After; None after the retries.
    req = urllib.request.Request(url, headers={"User-Agent": UA, **({"Accept": accept} if accept else {})})
    for attempt in range(retries):
        try:
            with urllib.request.urlopen(req, timeout=90) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code in (429, 503):
                ra = e.headers.get("Retry-After", "")
                time.sleep(min(int(ra) if ra.isdigit() else DEFAULT_RETRY, 300))
            elif e.code in (500, 502, 504):
                time.sleep(5 * (attempt + 1))
            else:
                log("HTTP", e.code, url[:120])
                return None
        except (urllib.error.URLError, TimeoutError, ConnectionError, OSError) as e:
            time.sleep(5 * (attempt + 1))
    return None


def doh(endpoint, name, rate):
    # (rcode, [(data, ttl), ...]) for the A records of `name`, or None when the endpoint did not answer.
    rate.wait()
    body = fetch(endpoint + "?" + urllib.parse.urlencode({"name": name, "type": "A"}), "application/dns-json")
    if body is None:
        return None
    try:
        j = json.loads(body)
    except ValueError:
        return None
    return j.get("Status", -1), [(a["data"], a.get("TTL", 0)) for a in j.get("Answer", []) if a.get("type") == 1]


def public_a(answers):
    out = []
    for data, ttl in answers:
        try:
            if ipaddress.ip_address(data).is_global:
                out.append(ttl)
        except ValueError:
            pass
    return out


# ---- Public Suffix List ----

class Psl:
    def __init__(self, text):
        self.rules, self.private = set(), set()
        section = None
        for line in text.splitlines():
            if "===BEGIN ICANN DOMAINS===" in line:
                section = "icann"
            elif "===BEGIN PRIVATE DOMAINS===" in line:
                section = "private"
            line = line.split()[0] if line.split() else ""
            if not line or line.startswith("//") or section is None:
                continue
            self.rules.add(line)
            if section == "private":
                self.private.add(line)

    def registrable(self, host):
        # The registrable domain of `host` under an ICANN suffix; None if it is a suffix itself, unlisted or under a private suffix.
        labels = host.split(".")
        best = None  # (length in labels, rule, is_exception)
        for i in range(len(labels)):
            suffix = ".".join(labels[i:])
            n = len(labels) - i
            if "!" + suffix in self.rules:
                best = (n - 1, "!" + suffix, True)
                break
            if suffix in self.rules and (best is None or n > best[0]):
                best = (n, suffix, False)
            wild = ".".join(["*"] + labels[i + 1:])
            if i + 1 < len(labels) and wild in self.rules and (best is None or n > best[0]):
                best = (n, wild, False)
        if best is None or best[1] in self.private:
            return None
        return ".".join(labels[-(best[0] + 1):]) if len(labels) > best[0] else None


def load_psl(work):
    path = os.path.join(work, "public_suffix_list.dat")
    if not os.path.exists(path):
        body = fetch(PSL_URL)
        if body is None:
            sys.exit("could not download the Public Suffix List")
        with open(path, "wb") as f:
            f.write(body)
    with open(path, encoding="utf-8") as f:
        return Psl(f.read())


def label_ok(host):
    labels = host.split(".")
    return (
        len(labels) >= 2 and len(host) <= 253 and labels[-1] not in BLOCKED_TLDS | EXCLUDED_TLDS and not labels[-1].isdigit()
        and all(l and len(l) <= 63 and not l.startswith("-") and not l.endswith("-") and re.fullmatch(r"[a-z0-9-]+", l) for l in labels)
    )


def read_names(path):
    with open(path, encoding="utf-8") as f:
        return [l.strip() for l in f if l.strip() and not l.startswith("#")]


# ---- common --check ----

def cmd_common(a, work, rate):
    psl = load_psl(work)
    # The header is the comment block before the first blank line; each later comment starts a category.
    text = open(COMMON, encoding="utf-8").read().split("\n\n", 1)
    header = text[0].splitlines()
    cats = []
    for line in text[1].splitlines():
        if line.startswith("#"):
            cats.append((line, []))
        elif line.strip() and cats:
            cats[-1][1].append(line.strip())
    names = [n for _, ns in cats for n in ns]
    seen, todo = set(), []
    for n in names:
        if n not in seen:
            seen.add(n)
            todo.append(n)
    if a.limit:
        todo = todo[: a.limit]

    def check(name):
        if not label_ok(name) or psl.registrable(name) != name:
            return name, False, "not a registrable name"
        ttl = 0
        for ep in CHECK_ENDPOINTS:
            r = doh(ep, name, rate)
            if r is None or r[0] != 0:
                return name, False, "no NOERROR from " + ep
            ttls = public_a(r[1])
            if not ttls:
                return name, False, "no public A at " + ep
            ttl = max(ttl, max(ttls))
        return name, ttl >= MIN_TTL, "largest TTL %d" % ttl

    with ThreadPoolExecutor(8) as ex:
        results = list(ex.map(check, todo))
    kept = {n for n, ok, _ in results if ok}
    for n, ok, why in results:
        if not ok:
            log("drop", n, why)
    # Cap at 500, spread evenly over the categories: take from each in turn.
    by_cat = [[n for n in ns if n in kept] for _, ns in cats]
    chosen, i = set(), 0
    total = sum(len(c) for c in by_cat)
    pos = [0] * len(by_cat)
    while len(chosen) < min(COMMON_KEEP, total):
        for k, c in enumerate(by_cat):
            if pos[k] < len(c) and len(chosen) < COMMON_KEEP:
                chosen.add(c[pos[k]])
                pos[k] += 1
    out = [("# Built: " + datetime.now(timezone.utc).strftime("%Y-%m-%d")) if h.startswith("# Built:") else h for h in header] + [""]
    for (title, ns) in cats:
        keep = [n for n in ns if n in chosen]
        if keep:
            out += ([title] if title else []) + keep
    dest = COMMON if not a.limit else os.path.join(work, "names-common.trial.txt")
    with open(dest, "w", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")
    print("kept %d, dropped %d of %d checked (wrote %s)" % (len(chosen), len(todo) - len(chosen), len(todo), dest))
    if len(chosen) < 400 and not a.limit:
        log("warning: fewer than 400 names kept; add candidates to the file")


# ---- pool ----

def wd_labels(qids):
    out = {}
    for i in range(0, len(qids), 50):
        url = WDAPI + "?" + urllib.parse.urlencode(
            {"action": "wbgetentities", "ids": "|".join(qids[i:i + 50]), "props": "labels", "languages": "en", "format": "json"})
        body = fetch(url)
        j = json.loads(body) if body else {}
        for q, e in j.get("entities", {}).items():
            out[q] = e.get("labels", {}).get("en", {}).get("value", "(missing)")
    return out


def sparql(query):
    return fetch(SPARQL + "?" + urllib.parse.urlencode({"query": query}), "text/tab-separated-values")


def cmd_classes():
    live = wd_labels([q for q, _ in CLASSES])
    bad = 0
    print("%-10s %-34s %-34s %s" % ("QID", "expected", "live", "P856 count"))
    for q, want in CLASSES:
        got = live.get(q, "(not found)")
        body = sparql("SELECT (COUNT(?item) AS ?n) WHERE { ?item wdt:P31 wd:%s ; wdt:P856 ?u }" % q)
        n = body.decode().split("\n")[1].strip().strip('"') if body and body.count(b"\n") >= 1 else "?"
        mark = "" if got.lower() == want.lower() else "  MISMATCH"
        bad += bool(mark)
        print("%-10s %-34s %-34s %s%s" % (q, want, got, n, mark), flush=True)
        time.sleep(PAGE_PAUSE)
    if bad:
        sys.exit("%d class label(s) did not match" % bad)


def host_of(url):
    url = url.strip().strip("<>").strip('"')
    if "://" not in url:
        url = "//" + url
    try:
        h = urllib.parse.urlsplit(url).hostname
    except ValueError:
        return None
    if not h:
        return None
    h = h.strip(".").lower()
    if h.startswith("www."):
        h = h[4:]
    try:
        ipaddress.ip_address(h)
        return None
    except ValueError:
        pass
    try:
        return h.encode("idna").decode("ascii")
    except UnicodeError:
        return None


def normalise(url, psl):
    h = host_of(url)
    if not h or not label_ok(h):
        return None
    d = psl.registrable(h)
    if not d or any(d == p or d.endswith("." + p) for p in PLATFORMS):
        return None
    return d


def gather(a, work, psl, common):
    path = os.path.join(work, "candidates-%s.txt" % (a.limit or "full"))
    if os.path.exists(path):
        return read_names(path)
    per_class = max(1, -(-a.candidates // len(CLASSES))) * 2  # raw rows per class, so the big classes do not use it all
    found = {}
    for q, label in CLASSES:
        taken, offset = 0, 0
        while len(found) < a.candidates and taken < per_class:
            n = min(PAGE, a.limit or PAGE)
            body = sparql("SELECT ?url WHERE { ?item wdt:P31 wd:%s ; wdt:P856 ?url } LIMIT %d OFFSET %d" % (q, n, offset))
            time.sleep(PAGE_PAUSE)
            if body is None:
                log("class", q, label, "page failed at offset", offset)
                break
            rows = body.decode("utf-8", "replace").splitlines()[1:]
            for r in rows:
                d = normalise(r, psl)
                if d and d not in common:
                    found[d] = True
            taken += len(rows)
            offset += n
            log("%s %s: %d rows, %d candidates" % (q, label, taken, len(found)))
            if len(rows) < n or (a.limit and len(found) >= a.limit):
                break
        if len(found) >= a.candidates or (a.limit and len(found) >= a.limit):
            break
    names = sorted(found)
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(names) + "\n")
    return names


def cmd_pool(a, work, rate):
    psl = load_psl(work)
    common = set(read_names(COMMON))
    names = [n for n in gather(a, work, psl, common) if label_ok(n) and n not in common]  # the cached candidates may be older than the rules or the common list
    log("%d candidates" % len(names))
    # Liveness at one public resolver, with a progress file so a stopped run can resume.
    progress = os.path.join(work, "pool-progress-%s.tsv" % (a.limit or "full"))
    done = {}
    if os.path.exists(progress):
        for l in open(progress, encoding="utf-8"):
            n, _, ok = l.rstrip("\n").partition("\t")
            done[n] = ok == "1"
    todo = [n for n in names if n not in done]
    lock = threading.Lock()
    pf = open(progress, "a", encoding="utf-8")

    def check(name):
        r = doh(a.doh, name, rate)
        if r is None:
            return  # no answer: leave it undone, a rerun tries again
        ok = r[0] == 0 and bool(public_a(r[1]))
        with lock:
            done[name] = ok
            pf.write("%s\t%d\n" % (name, ok))
            pf.flush()
            if len(done) % 500 == 0:
                log("checked %d of %d" % (len(done), len(names)))

    with ThreadPoolExecutor(8) as ex:
        list(ex.map(check, todo))
    pf.close()
    alive = [n for n in names if done.get(n)]
    rng = random.Random(856)
    rng.shuffle(alive)
    alive = cap_tld(alive[: a.max_names])
    alive.sort()
    now = datetime.now(timezone.utc)
    dest = POOL if not a.limit else os.path.join(work, "names-pool.trial.txt")
    with open(dest, "w", encoding="utf-8") as f:
        f.write("# Names for the Extended test: registrable domains of organisations and places, one per line, sorted.\n")
        f.write("# Source: Wikidata official website values (P856), CC0\n# Licence: CC0 1.0\n")
        f.write("# Built: %s\n# Count: %d\n" % (now.strftime("%Y-%m-%dT%H:%M:%SZ"), len(alive)))
        f.write("\n".join(alive) + "\n")
    print("%d candidates, %d answered, %d written to %s" % (len(names), sum(done.get(n, False) for n in names), len(alive), dest))
    if len(alive) < 50000 and not a.limit:
        log("warning: fewer than 50,000 names; raise --candidates")


def cap_tld(names):
    # No TLD above a quarter of the output: the largest cap c with 4c <= sum(min(count, c)).
    counts = {}
    for n in names:
        t = n.rsplit(".", 1)[1]
        counts[t] = counts.get(t, 0) + 1
    best = 0
    for c in range(1, max(counts.values()) + 1):
        if 4 * c <= sum(min(v, c) for v in counts.values()):
            best = c
    if best == 0:
        log("warning: too few TLDs to cap any at 25%")
        return names
    seen, out = {}, []
    for n in names:
        t = n.rsplit(".", 1)[1]
        seen[t] = seen.get(t, 0) + 1
        if seen[t] <= best:
            out.append(n)
    return out


def main():
    p = argparse.ArgumentParser(description=__doc__ or "Builds the Extended test name lists")
    p.add_argument("cmd", choices=["common", "pool"])
    p.add_argument("--check", action="store_true", help="common: resolve each name and rewrite the file")
    p.add_argument("--classes", action="store_true", help="pool: verify the Wikidata class IDs and exit")
    p.add_argument("--work", default="build-names-work", help="scratch folder for downloads and progress")
    p.add_argument("--rate", type=float, default=20, help="lookups per second")
    p.add_argument("--limit", type=int, default=0, help="trial run size; output goes to the work folder")
    p.add_argument("--doh", default=LIVENESS, help="the pool's liveness resolver (DoH JSON)")
    p.add_argument("--candidates", type=int, default=160000, help="pool: stop collecting at this many candidates")
    p.add_argument("--max-names", type=int, default=100000, help="pool: most names to keep")
    a = p.parse_args()
    os.makedirs(a.work, exist_ok=True)
    rate = Rate(a.rate)
    if a.cmd == "common":
        if not a.check:
            p.error("common needs --check")
        cmd_common(a, a.work, rate)
    elif a.classes:
        cmd_classes()
    else:
        cmd_pool(a, a.work, rate)


if __name__ == "__main__":
    main()
