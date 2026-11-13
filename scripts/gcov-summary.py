#!/usr/bin/env python3
"""Summarise gcov coverage of a libuvc build (gcov --json-format).

usage: gcov-summary.py OBJDIR [--functions] [--uncovered FILE.c] [--uncovered-branches FILE.c]
                       [--markdown FILE]
  OBJDIR: directory with the .gcda/.gcno files (e.g. target/libuvc-cov/CMakeFiles/uvc.dir/src)
  --uncovered FILE.c           list the never-executed line ranges of FILE.c
  --uncovered-branches FILE.c  list the executed lines of FILE.c with branches (or conditions) never taken
  --markdown FILE              also append the table to FILE as Markdown

Branches are gcc's: every outcome of every conditional jump, including the ones macros
expand to, so 100% is not a realistic goal. The conditions column (masking MC/DC) appears
when libuvc was built with -fcondition-coverage (gcc 14+) and gcov supports --conditions.
$GCOV overrides the gcov binary.
"""
import json, os, subprocess, sys

def arg(flag):
    return sys.argv[sys.argv.index(flag) + 1] if flag in sys.argv else None

objdir = sys.argv[1]
show_functions = "--functions" in sys.argv
uncovered_file = arg("--uncovered")
uncovered_branches_file = arg("--uncovered-branches")
markdown = arg("--markdown")

gcov = os.environ.get("GCOV", "gcov")
gcov_help = subprocess.run([gcov, "--help"], capture_output=True, text=True).stdout
flags = ["--json-format", "--stdout", "--branch-probabilities"]
if "--conditions" in gcov_help:
    flags.append("--conditions")
gcdas = sorted(f for f in os.listdir(objdir) if f.endswith(".gcda"))
out = subprocess.run([gcov, *flags, *gcdas], cwd=objdir, capture_output=True, text=True, check=True).stdout
files = {}
for doc in out.splitlines():
    if not doc.strip():
        continue
    for f in json.loads(doc)["files"]:
        name = os.path.basename(f["file"])
        if not name.endswith(".c") or name == "example.c":
            continue
        files[name] = f

def branches(line):
    return [b for b in line.get("branches", []) if not b.get("throw")]

def stats(f):
    """(covered, total) for lines, functions, branches and condition outcomes."""
    lines, fns = f["lines"], f["functions"]
    br = [b for l in lines for b in branches(l)]
    conds = [c for l in lines for c in l.get("conditions", [])]
    return {
        "lines": (sum(1 for l in lines if l["count"] > 0), len(lines)),
        "functions": (sum(1 for x in fns if x["execution_count"] > 0), len(fns)),
        "branches": (sum(1 for b in br if b["count"] > 0), len(br)),
        "conditions": (sum(c["covered"] for c in conds), sum(c["count"] for c in conds)),
    }

rows = [(name, stats(f)) for name, f in sorted(files.items())]
cols = ["lines", "functions", "branches"]
if any(s["conditions"][1] for _, s in rows):
    cols.append("conditions")
total = {c: tuple(sum(s[c][i] for _, s in rows) for i in (0, 1)) for c in cols}
pct = lambda a, b: f"{100 * a / max(b, 1):.1f}%"

print(f"{'file':16}" + "".join(f" {c:>18}" for c in cols))
for name, s in [*rows, ("TOTAL", total)]:
    print(f"{name:16}" + "".join(f" {s[c][0]:5}/{s[c][1]:<5} {pct(*s[c]):>6}" for c in cols))
    if show_functions and name != "TOTAL":
        for x in sorted(files[name]["functions"], key=lambda x: x["start_line"]):
            if x["execution_count"] == 0:
                print(f"    never called: {x['demangled_name']} (line {x['start_line']})")

if markdown:
    with open(markdown, "a") as md:
        md.write("### libuvc coverage: " + ", ".join(f"{pct(*total[c])} of {c}" for c in cols) + "\n\n")
        md.write("| file | " + " | ".join(cols) + " |\n|---|" + "---:|" * len(cols) + "\n")
        for name, s in rows:
            md.write(f"| `{name}` | " + " | ".join(f"{pct(*s[c])} ({s[c][0]}/{s[c][1]})" for c in cols) + " |\n")
        md.write("| **total** | " + " | ".join(f"**{pct(*total[c])}** ({total[c][0]}/{total[c][1]})" for c in cols) + " |\n\n")

def ranges(nums):
    out, start, prev = [], None, None
    for n in nums:
        if start is None: start = prev = n
        elif n == prev + 1: prev = n
        else: out.append((start, prev)); start = prev = n
    if start is not None: out.append((start, prev))
    return ", ".join(f"{a}-{b}" if a != b else str(a) for a, b in out)

if uncovered_file:
    f = files[uncovered_file]
    print(f"uncovered lines in {uncovered_file}: " + ranges([l["line_number"] for l in f["lines"] if l["count"] == 0]))

if uncovered_branches_file:
    # Executed lines only: on never-executed ones nothing is taken, --uncovered lists those.
    print(f"partially taken branches in {uncovered_branches_file} (executed lines only):")
    for l in files[uncovered_branches_file]["lines"]:
        if l["count"] == 0:
            continue
        br = branches(l)
        missed = sum(1 for b in br if b["count"] == 0)
        conds = l.get("conditions", [])
        cmissed = sum(c["count"] - c["covered"] for c in conds)
        if missed or cmissed:
            what = [f"{missed}/{len(br)} branches untaken"] if missed else []
            if cmissed:
                what.append(f"{cmissed}/{sum(c['count'] for c in conds)} condition outcomes uncovered")
            print(f"  line {l['line_number']:5} ({l.get('function_name', '?')}): " + ", ".join(what))
