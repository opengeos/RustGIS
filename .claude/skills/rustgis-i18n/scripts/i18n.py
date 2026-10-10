#!/usr/bin/env python3
"""RustGIS interface catalogs: extract keys, audit coverage, and merge translations.

Keys are the English strings in the code. Catalogs are crates/ui-egui/src/i18n/<code>.tsv with
one `English<TAB>Translation` per line (`\\n` / `\\t` escapes, `#` comments).

  i18n.py keys                       # every translatable English string, one per line
  i18n.py audit [--keys-file F]      # coverage, missing, stale and placeholder problems per locale
  i18n.py missing <code|all> -o out.json   # missing keys as {code: {english: ""}} to translate
  i18n.py insert translations.json   # merge {code: {english: translation}} into the catalogs

Run from the repository root (or anywhere inside it).
"""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True).stdout.strip())
I18N = ROOT / "crates/ui-egui/src/i18n"
UI = ROOT / "crates/ui-egui/src"

LIT = r'"((?:[^"\\]|\\.)*)"'

def unrust(s: str) -> str:
    """A Rust string literal's body to its value."""
    return s.encode("utf-8").decode("unicode_escape").encode("latin-1").decode("utf-8") if "\\" in s else s

def looks_like_text(s: str) -> bool:
    if not s or len(s) < 2 or s.strip() != s and not s.endswith(" "):
        return False
    if re.fullmatch(r"[a-z0-9_.:\-/#{}\[\]*%+?=]+", s):  # ids, icon names, keys, paths
        return False
    if re.fullmatch(r"[\W\d_]+", s):  # punctuation and numbers only
        return False
    if s.startswith(("http", "#", "{", "rustgis", "esri", "ui.", "gp:")) or "://" in s:
        return False
    if re.search(r"\{\}|\{:[^}]*\}|\{[a-z_]+:[^}]*\}", s):  # format!() positional templates
        return False
    if re.search(r"[A-Za-z]", s) is None:
        return False
    if " " not in s and "\n" not in s and s[0].islower():  # identifiers: convexHull, layerId, clip
        return False
    return True

SKIP_CONTEXT = re.compile(r"(log::\w+!|Id::new|id_salt|from_id_salt|load_texture|panic!|expect\(|#\[|debug_assert)")

def ui_keys() -> set[str]:
    keys: set[str] = set()
    for f in sorted(UI.glob("*.rs")):
        if f.name in ("icons.rs", "theme.rs"):
            continue
        text = f.read_text(encoding="utf-8")
        # Drop test modules.
        text = re.split(r"#\[cfg\(test\)\]", text)[0]
        for line in text.splitlines():
            code = line.split("//")[0] if not re.search(r'"[^"]*//', line) else line
            if SKIP_CONTEXT.search(code):
                continue
            # Arguments to run()/json!() are command ids and parameters, not text.
            code = re.sub(r'(app|self|s)\.run\("[^"]*"', "", code)
            code = re.sub(r'json!\(\{[^}]*\}\)', "", code)
            # format!() builds messages that are never looked up (only i18n::fmt templates are).
            code = re.sub(r'(?<!i18n::)format!\(\s*"(?:[^"\\]|\\.)*"', "", code)
            for m in re.finditer(LIT, code):
                v = unrust(m.group(1))
                if looks_like_text(v):
                    keys.add(v)
    return keys

def engine_keys() -> set[str]:
    keys: set[str] = set()
    # Commands: C::new("id", "Label", "Location", …)
    for f in (ROOT / "crates/engine/src/cmd").glob("*.rs"):
        for m in re.finditer(r'C::new\(\s*"[^"]+",\s*' + LIT + r',\s*' + LIT, f.read_text(encoding="utf-8")):
            keys.add(unrust(m.group(1)))
            keys.update(seg for seg in unrust(m.group(2)).split(" › ") if seg)
    # Tool labels, undo labels: Tool::label and friends.
    for f in [ROOT / "crates/engine/src/lib.rs", ROOT / "crates/map/src/lib.rs", ROOT / "crates/map/src/symbology.rs",
              ROOT / "crates/map/src/color.rs", ROOT / "crates/data/src/geometry.rs", ROOT / "crates/data/src/value.rs"]:
        text = f.read_text(encoding="utf-8")
        for body in re.findall(r"fn (?:label|kind_label)\(self\) -> &'static str \{(.*?)\n    \}", text, re.S):
            keys.update(unrust(v) for v in re.findall(r'=> ' + LIT, body))
        for body in re.findall(r"pub fn kind_label\(&self\) -> &'static str \{(.*?)\n    \}", text, re.S):
            keys.update(unrust(v) for v in re.findall(r'=> ' + LIT, body))
    # Geoprocessing tools.
    tools = (ROOT / "crates/analysis/src/tools.rs").read_text(encoding="utf-8")
    for field in ("name", "summary", "label"):
        keys.update(unrust(v) for v in re.findall(field + r': ' + LIT, tools))
    for tb in re.findall(r'toolbox: ' + LIT, tools):
        keys.update(seg for seg in unrust(tb).split(" › ") if seg)
    for lst in re.findall(r'Choice\(&\[(.*?)\]\)', tools) + re.findall(r'const UNITS: &\[&str\] = &\[(.*?)\];', tools):
        keys.update(unrust(v) for v in re.findall(LIT, lst))
    return {k for k in keys if k and looks_like_text(k)}

def all_keys() -> list[str]:
    return sorted(ui_keys() | engine_keys(), key=lambda s: (s.lower(), s))

def esc(s: str) -> str:
    return s.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n")

def unesc(s: str) -> str:
    out, i = [], 0
    while i < len(s):
        c = s[i]
        if c == "\\" and i + 1 < len(s):
            n = s[i + 1]
            out.append({"n": "\n", "t": "\t", "\\": "\\"}.get(n, "\\" + n))
            i += 2
        else:
            out.append(c)
            i += 1
    return "".join(out)

def locales() -> list[str]:
    return sorted(p.stem for p in I18N.glob("*.tsv"))

def read_catalog(code: str) -> tuple[list[str], dict[str, str]]:
    header, entries = [], {}
    for line in (I18N / f"{code}.tsv").read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.startswith("#"):
            if not entries:
                header.append(line)
            continue
        if "\t" in line:
            en, tr = line.split("\t", 1)
            entries[unesc(en)] = unesc(tr)
    return header, entries

def write_catalog(code: str, header: list[str], entries: dict[str, str]) -> None:
    lines = list(header) or [f"# {code} interface catalog: English<TAB>translation"]
    for en in sorted(entries, key=lambda s: (s.lower(), s)):
        lines.append(f"{esc(en)}\t{esc(entries[en])}")
    (I18N / f"{code}.tsv").write_text("\n".join(lines) + "\n", encoding="utf-8")

def placeholders(s: str) -> list[str]:
    return sorted(re.findall(r"\{([a-z_]+)\}", s))

def cmd_keys(_a) -> None:
    print("\n".join(esc(k) for k in all_keys()))

def cmd_audit(a) -> int:
    keys = set(all_keys())
    bad = 0
    print(f"{len(keys)} translatable strings")
    for code in locales():
        _, cat = read_catalog(code)
        have = keys & cat.keys()
        missing = sorted(keys - cat.keys())
        stale = sorted(cat.keys() - keys)
        ph = [k for k in have if placeholders(k) != placeholders(cat[k])]
        nl = [k for k in have if ("\n" in k) and ("\n" not in cat[k])]
        pct = 100 * len(have) // max(len(keys), 1)
        print(f"{code:4} {pct:3}%  translated {len(have):4}  missing {len(missing):4}  stale {len(stale):3}  placeholder {len(ph)}  line-break {len(nl)}")
        if a.verbose:
            for k in missing[: a.limit]:
                print(f"      missing: {esc(k)}")
            for k in stale[: a.limit]:
                print(f"      stale:   {esc(k)}")
        for k in ph:
            print(f"      PLACEHOLDER {code}: {esc(k)} -> {esc(cat[k])}")
        bad += len(ph)
    return 1 if bad else 0

def cmd_missing(a) -> None:
    keys = all_keys()
    codes = locales() if a.code == "all" else [a.code]
    out = {}
    for code in codes:
        _, cat = read_catalog(code)
        out[code] = {k: "" for k in keys if k not in cat}
    text = json.dumps(out, ensure_ascii=False, indent=1)
    if a.output:
        pathlib.Path(a.output).write_text(text + "\n", encoding="utf-8")
        print(f"wrote {sum(len(v) for v in out.values())} missing entries to {a.output}")
    else:
        print(text)

def cmd_insert(a) -> int:
    data = json.loads(pathlib.Path(a.file).read_text(encoding="utf-8"))
    keys = set(all_keys())
    problems = 0
    for code, entries in data.items():
        if not (I18N / f"{code}.tsv").exists():
            print(f"skip {code}: no catalog (add {code}.tsv and a LANGUAGES row first)")
            continue
        header, cat = read_catalog(code)
        added = 0
        for en, tr in entries.items():
            if not tr or not tr.strip():
                continue
            if en not in keys:
                print(f"  {code}: not a current key, skipped: {esc(en)}")
                continue
            if placeholders(en) != placeholders(tr):
                print(f"  {code}: placeholder mismatch, skipped: {esc(en)} -> {esc(tr)}")
                problems += 1
                continue
            if a.overwrite or en not in cat:
                added += en not in cat
                cat[en] = tr
        if a.prune:
            for k in [k for k in cat if k not in keys]:
                del cat[k]
        write_catalog(code, header, cat)
        print(f"{code}: +{added}")
    return 1 if problems else 0

def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("keys").set_defaults(fn=cmd_keys)
    au = sub.add_parser("audit")
    au.add_argument("-v", "--verbose", action="store_true")
    au.add_argument("--limit", type=int, default=40)
    au.set_defaults(fn=cmd_audit)
    mi = sub.add_parser("missing")
    mi.add_argument("code")
    mi.add_argument("-o", "--output")
    mi.set_defaults(fn=cmd_missing)
    ins = sub.add_parser("insert")
    ins.add_argument("file")
    ins.add_argument("--overwrite", action="store_true", help="replace existing translations")
    ins.add_argument("--prune", action="store_true", help="drop entries whose key no longer exists")
    ins.set_defaults(fn=cmd_insert)
    a = p.parse_args()
    return a.fn(a) or 0

if __name__ == "__main__":
    sys.exit(main())
