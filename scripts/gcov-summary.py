#!/usr/bin/env python3
"""Summarise gcov coverage of a libuvc build (gcov --json-format).

usage: gcov-summary.py OBJDIR [--functions] [--uncovered FILE.c]
  OBJDIR: directory with the .gcda/.gcno files (e.g. target/libuvc-cov/CMakeFiles/uvc.dir/src)
"""
import gzip, json, os, subprocess, sys

objdir = sys.argv[1]
show_functions = "--functions" in sys.argv
uncovered_file = sys.argv[sys.argv.index("--uncovered") + 1] if "--uncovered" in sys.argv else None

gcdas = sorted(f for f in os.listdir(objdir) if f.endswith(".gcda"))
out = subprocess.run(["gcov", "--json-format", "--stdout", *gcdas], cwd=objdir, capture_output=True, text=True, check=True).stdout
files = {}
for doc in out.splitlines():
    if not doc.strip():
        continue
    for f in json.loads(doc)["files"]:
        name = os.path.basename(f["file"])
        if not name.endswith(".c") or name == "example.c":
            continue
        files[name] = f

tot_l = tot_lc = tot_f = tot_fc = 0
print(f"{'file':16} {'lines':>14} {'functions':>14}")
for name, f in sorted(files.items()):
    lines = [l for l in f["lines"] if not l.get("unexecuted_block", False) or True]
    n = len(lines); c = sum(1 for l in lines if l["count"] > 0)
    fn = f["functions"]; fc = sum(1 for x in fn if x["execution_count"] > 0)
    tot_l += n; tot_lc += c; tot_f += len(fn); tot_fc += fc
    print(f"{name:16} {c:5}/{n:<5} {100*c/max(n,1):5.1f}% {fc:4}/{len(fn):<4} {100*fc/max(len(fn),1):5.1f}%")
    if show_functions:
        for x in sorted(fn, key=lambda x: x["start_line"]):
            if x["execution_count"] == 0:
                print(f"    never called: {x['demangled_name']} (line {x['start_line']})")
print(f"{'TOTAL':16} {tot_lc:5}/{tot_l:<5} {100*tot_lc/max(tot_l,1):5.1f}% {tot_fc:4}/{tot_f:<4} {100*tot_fc/max(tot_f,1):5.1f}%")
if uncovered_file:
    f = files[uncovered_file]
    miss = [l["line_number"] for l in f["lines"] if l["count"] == 0]
    # Compress into ranges.
    ranges, start, prev = [], None, None
    for n in miss:
        if start is None: start = prev = n
        elif n == prev + 1: prev = n
        else: ranges.append((start, prev)); start = prev = n
    if start is not None: ranges.append((start, prev))
    print(f"uncovered lines in {uncovered_file}: " + ", ".join(f"{a}-{b}" if a != b else str(a) for a, b in ranges))
