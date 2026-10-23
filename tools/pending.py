#!/usr/bin/env python3
"""Check the pending fixes in docs/PENDING.md: is a fix out yet?

Each check prints a line and adds a row to the CI run's summary. When a fix can be applied, or a problem is gone, it
also raises a GitHub Actions warning so the run shows it. It never fails the build: the problems are known.

Usage: python3 -I tools/pending.py    (needs cargo and network access to crates.io and the npm registry)
"""

import json
import os
import re
import subprocess
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
UA = "recursa-pending-check (https://github.com/flatshift/recursa)"


def ver(s):
    return tuple(int(x) for x in re.findall(r"\d+", s)[:3])


def locked():
    """Every package in Cargo.lock: name -> list of versions."""
    out = {}
    for name, v in re.findall(r'^name = "([^"]+)"\nversion = "([^"]+)"', (ROOT / "Cargo.lock").read_text(), re.M):
        out.setdefault(name, []).append(v)
    return out


def dry_run():
    """What `cargo update` would change, without changing it."""
    r = subprocess.run(["cargo", "update", "--dry-run"], cwd=ROOT, capture_output=True, text=True)
    return r.stderr


def fetch_json(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=20) as r:
        return json.load(r)


def accepts_major(peer, major):
    """Does the npm range `peer` take some `major`.x release? Each `||` alternative is a set of comparators that must
    all hold: ">=5" with no upper bound takes 7, "^5.7.2" and ">=5 <7" don't."""
    for alt in peer.split("||"):
        # A hyphen range "A - B" is ">=A <=B".
        alt = re.sub(r"(\S+)\s+-\s+(\S+)", r">=\1 <=\2", alt)
        # A prerelease or build tag ("7.0.0-rc.1", "+build.5") isn't more comparators: its numbers would be read as some.
        alt = re.sub(r"(\d)[-+][0-9A-Za-z.-]*", r"\1", alt)
        comparators = re.findall(r"([<>=^~]*)\s*v?(\d+)((?:\.[\dx*]+)*)", alt)
        if not comparators:
            if alt.strip() in ("", "*", "x", "latest"):
                return True
            continue
        ok = True
        for op, m, rest in comparators:
            m = int(m)
            # The minor and patch it names, up to an x: ">7" and ">7.x" exclude all of 7.x, ">7.0" doesn't; "<7" and
            # "<7.0.0" exclude all of 7.x, "<7.1" doesn't.
            parts = []
            for p in rest.strip(".").split("."):
                if not p.isdigit():
                    break
                parts.append(int(p))
            if op in ("", "=", "^", "~"):
                ok &= m == major
            elif op == ">":
                ok &= m < major or (m == major and bool(parts))
            elif op == ">=":
                ok &= m <= major
            elif op == "<":
                ok &= m > major or (m == major and any(parts))
            elif op == "<=":
                ok &= m >= major
        if ok:
            return True
    return False


def quick_xml(lock, plan):
    have = [v for v in lock.get("quick-xml", []) if ver(v) < (0, 41)]
    if not have:
        return "resolved", "quick-xml is 0.41 or later: remove the two ignores in .cargo/audit.toml"
    m = re.search(r"Updating quick-xml v0\.(\d+)\.\d+ -> v(\S+)", plan)
    if m and ver(m.group(2)) >= (0, 41):
        return "available", f"cargo update moves quick-xml to {m.group(2)}"
    rout = lock.get("routinator", ["0"])[0]
    latest = fetch_json("https://crates.io/api/v1/crates/routinator")["crate"]["max_stable_version"]
    if ver(latest) > ver(rout):
        return "available", f"routinator {latest} is out (we have {rout}): check whether it moves to quick-xml 0.41"
    return "pending", f"quick-xml {', '.join(have)} through rpki and routinator {rout}, the newest release"


def tauri_crates(lock, plan):
    names = ["paste", "proc-macro-error", "rustls-pemfile", "zvariant", "zvariant_derive", "zbus_names"]
    left = [n for n in names if n in lock] + [f"glib {v}" for v in lock.get("glib", []) if ver(v) < (0, 19)]
    if not left:
        return "resolved", "none of them is in Cargo.lock any more"
    def removed(n):
        # "glib 0.18.5" is gone only when that version is: removing some other glib proves nothing.
        name, _, v = n.partition(" ")
        version = re.escape(v) + r"(?![\w.])" if v else ""
        return re.search(rf"Removing {re.escape(name)} v{version}", plan)

    gone = [n for n in left if removed(n)]
    if gone:
        return "available", f"cargo update removes {', '.join(gone)}"
    return "pending", f"still in Cargo.lock: {', '.join(left)}"


def typescript(lock, plan):
    pkg = json.loads((ROOT / "web/package.json").read_text())
    have = pkg.get("devDependencies", {}).get("typescript", "")
    if ver(have) >= (7,):
        return "resolved", f"the interface is on typescript {have}"
    peer = fetch_json("https://registry.npmjs.org/svelte-check/latest").get("peerDependencies", {}).get("typescript", "")
    if accepts_major(peer, 7):
        return "available", f"svelte-check accepts typescript {peer}: update TypeScript to 7"
    return "pending", f"svelte-check accepts typescript {peer}"


def data(s):
    """A workflow command's message, escaped as GitHub Actions asks."""
    return s.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def prop(s):
    """A workflow command's property value: a comma or colon would otherwise end it."""
    return data(s).replace(":", "%3A").replace(",", "%2C")


CHECKS = [
    ("quick-xml advisories (RUSTSEC-2026-0194, -0195)", quick_xml),
    ("Unmaintained crates under Tauri", tauri_crates),
    ("TypeScript 7", typescript),
]


def main():
    lock, plan = locked(), dry_run()
    rows = []
    for title, check in CHECKS:
        try:
            state, detail = check(lock, plan)
        except Exception as e:  # A registry that's down is no reason to fail a build.
            state, detail = "unknown", f"check failed: {e}"
        print(f"{state:9} {title}: {detail}")
        if state in ("available", "resolved"):
            print(f"::warning title={prop(f'Pending fix {state}: {title}')}::{data(detail)}. See docs/PENDING.md.")
        rows.append(f"| {title} | {state} | {detail} |")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as f:
            f.write("### Pending fixes (docs/PENDING.md)\n\n| Item | State | Detail |\n|---|---|---|\n")
            f.write("\n".join(rows) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
