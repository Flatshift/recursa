#!/usr/bin/env python3
"""Write the third-party notices that ship inside the app and the CLI.

MIT, Apache-2.0, BSD, ISC, Zlib, Unicode and the OFL require their notices to travel with
binary copies. This collects them for every Rust crate the CLI or the app links (normal
dependencies, on every platform we build for) and for the npm packages bundled into the UI,
and writes web/src/legal/third-party-notices.txt, which the UI shows and the app bundles.

Run it whenever Cargo.lock or web/package-lock.json changes (build.sh does):
    python3 -I tools/notices.py
"""

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "web/src/legal/third-party-notices.txt"
TARGETS = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-apple-ios",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
]
# Workspace packages that build something we distribute.
SHIPPED = {"recursa", "recursa-desktop"}
# npm packages whose code or files end up in web/dist.
NPM = ["svelte", "clsx", "esm-env", "@tauri-apps/api"]
# For a choice of licenses ("MIT OR Apache-2.0"), use the first we can, in this order.
PREFER = ["MIT", "Apache-2.0", "BSD-3-Clause", "BSD-2-Clause", "ISC", "Zlib", "Unlicense", "0BSD", "MIT-0", "CC0-1.0", "BSL-1.0"]
# How each license's file is usually named in a package.
FILE_HINT = {
    "MIT": "MIT", "MIT-0": "MIT", "Apache-2.0": "APACHE", "BSD-3-Clause": "BSD", "BSD-2-Clause": "BSD",
    "ISC": "ISC", "Zlib": "ZLIB", "Unlicense": "UNLICENSE", "BSL-1.0": "BOOST", "CC0-1.0": "CC0",
}
LICENSE_FILE = re.compile(r"(?i)^(licen[cs]e|copying|unlicense|ofl)")
NOTICE_FILE = re.compile(r"(?i)^notice")
COPYRIGHT_LINE = re.compile(r"(?i)^\s*(copyright\b|©|\(c\)\s)")
# Title lines that vary between otherwise identical texts ("MIT License", "The MIT License (MIT)", "====").
TITLE_LINE = re.compile(r"(?i)^\s*((the\s+)?mit\s+license(\s*\(mit\))?|=+)\s*$")

MIT_TEXT = """Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
"""


def read(path):
    return path.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n").strip("\n") + "\n"


def cargo_packages():
    cmd = ["cargo", "metadata", "--format-version", "1", "--locked", "--manifest-path", str(ROOT / "Cargo.toml")]
    for t in TARGETS:
        cmd += ["--filter-platform", t]
    meta = json.loads(subprocess.run(cmd, check=True, capture_output=True, text=True).stdout)
    pkgs = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    todo = [i for i in meta["workspace_members"] if pkgs[i]["name"] in SHIPPED]
    seen = set(todo)
    while todo:
        for dep in nodes[todo.pop()]["deps"]:
            # Normal dependencies only: build scripts and tests don't ship.
            if any(k["kind"] is None for k in dep["dep_kinds"]) and dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                todo.append(dep["pkg"])
    out = []
    for i in seen:
        p = pkgs[i]
        if p["source"] is None:  # our own crates
            continue
        out.append((p["name"], p["version"], p.get("license") or "", Path(p["manifest_path"]).parent))
    return out


def npm_packages():
    out = []
    for name in NPM:
        d = ROOT / "web/node_modules" / name
        pj = json.loads((d / "package.json").read_text())
        out.append((name, pj["version"], pj.get("license") or "", d))
    return out


def choose(expr, files):
    """The license to use and the files that carry it."""
    simple = expr and "(" not in expr and " AND " not in expr and " WITH " not in expr
    options = [o.strip() for o in re.split(r" OR |/", expr)] if simple else []
    if len(options) > 1:
        for want in [o for o in PREFER if o in options] + [o for o in options if o not in PREFER]:
            hint = FILE_HINT.get(want, want.upper())
            hit = [f for f in files if hint in f.name.upper()]
            if hit:
                return want, hit
        generic = [f for f in files if not any(h in f.name.upper() for h in FILE_HINT.values())]
        if len(generic) == 1:
            return options[0], generic
        pick = next((o for o in PREFER if o in options), options[0])
        return pick, []
    return expr, files


def strip_copyright(text):
    """The license text without its copyright lines, which are listed per package instead."""
    kept = [l for l in text.splitlines() if not COPYRIGHT_LINE.match(l)]
    return re.sub(r"\n{3,}", "\n\n", "\n".join(kept)).strip("\n") + "\n"


def body_key(text):
    kept = [l for l in text.splitlines() if not COPYRIGHT_LINE.match(l) and not TITLE_LINE.match(l)]
    flat = " ".join(kept).translate(str.maketrans("“”‘’", "\"\"''"))
    return re.sub(r"\s+", " ", flat).strip().lower()


def main():
    groups = {}  # key -> {"license", "body", "members": [(label, copyright lines)]}
    notices = []  # (label, NOTICE text)
    missing = []
    entries = [(n, v, l, d, "crate") for n, v, l, d in cargo_packages()] + [(n, v, l, d, "npm") for n, v, l, d in npm_packages()]
    # The Rust standard library is linked into every binary.
    entries.append(("Rust standard library", "", "MIT OR Apache-2.0", None, "std"))
    # The UI's typeface ships from web/src/fonts, not from an npm package.
    entries.append(("Recursive (font)", "", "OFL-1.1", ROOT / "web/src/fonts", "font"))
    for name, version, expr, folder, kind in sorted(entries, key=lambda e: (e[0].lower(), e[1])):
        label = f"{name} {version}".strip() + (" (npm)" if kind == "npm" else "")
        files = sorted(f for f in folder.iterdir() if f.is_file() and LICENSE_FILE.match(f.name)) if folder else []
        if folder:
            notices += [(label, read(f)) for f in sorted(folder.iterdir()) if f.is_file() and NOTICE_FILE.match(f.name)]
        lic, chosen = choose(expr, files)
        texts = [read(f) for f in chosen]
        if not texts:
            if lic in ("MIT", "MIT-0") or (kind == "std"):
                lic = "MIT"
                texts = [MIT_TEXT]
            else:
                # No file in the package: use the same license's text from another package.
                same = next((g for g in groups.values() if g["license"] == lic), None)
                if same:
                    same["members"].append((label + " (no license file in the package)", []))
                else:
                    missing.append((label, expr, lic))
                continue
        body = "\n\n".join(texts)
        copyright = sorted({l.strip() for t in texts for l in t.splitlines() if COPYRIGHT_LINE.match(l)})
        if kind == "std":
            copyright = ["Copyright (c) The Rust Project Contributors"]
        key = (lic, body_key(body))
        g = groups.setdefault(key, {"license": lic, "body": strip_copyright(body), "members": []})
        g["members"].append((label, copyright))

    # Packages without a file whose license only appeared later in the alphabet.
    for label, expr, lic in list(missing):
        same = next((g for g in groups.values() if g["license"] == lic), None)
        if same:
            same["members"].append((label + " (no license file in the package)", []))
            missing.remove((label, expr, lic))
    missing = [f"{label}: {expr or 'no license field'}" for label, expr, _ in missing]

    rule = "-" * 79
    lines = [
        "Third-party notices",
        "",
        "This program includes the software listed below. Each part is distributed under its own",
        "license, reproduced here as that license requires. The program itself is licensed under",
        "the GNU General Public License, version 3 or later; see the LICENSE file.",
        "",
        "Generated by tools/notices.py from Cargo.lock and web/package-lock.json.",
        "Where a package offers a choice of licenses, the one used is named.",
        "",
    ]
    order = sorted(groups.values(), key=lambda g: (-len(g["members"]), g["license"]))
    for g in order:
        lines += [rule, f"{g['license']}: {len(g['members'])} package{'s' if len(g['members']) != 1 else ''}", rule, ""]
        for label, cr in g["members"]:
            lines.append(f"  {label}")
            lines += [f"      {c}" for c in cr]
        lines += ["", g["body"].rstrip("\n"), ""]
    if notices:
        lines += [rule, "NOTICE files (Apache-2.0 section 4(d))", rule, ""]
        for label, text in notices:
            lines += [f"  {label}", "", text.rstrip("\n"), ""]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text("\n".join(lines).rstrip("\n") + "\n", encoding="utf-8")
    total = sum(len(g["members"]) for g in groups.values())
    print(f"{OUT.relative_to(ROOT)}: {total} packages in {len(groups)} license texts, {len(notices)} NOTICE files, {OUT.stat().st_size // 1024} KB")
    if missing:
        print("No license text found for:\n  " + "\n  ".join(missing), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
