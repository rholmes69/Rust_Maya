#!/usr/bin/env python3
"""Write a pxr-flattened `<name>.flat.usda` next to every USD file under a folder.

The `usd_corpus` test (USD_CORPUS=<dir> cargo test --release usd_corpus -- --ignored --nocapture)
compares Rust Maya's own composition of each file with pxr's flattened stage.
Needs Pixar's USD: pip install usd-core
"""
import os
import sys

from pxr import Usd

root = sys.argv[1] if len(sys.argv) > 1 else "."
done = failed = 0
for d, _, files in os.walk(root):
    for f in files:
        if f.endswith((".usd", ".usda", ".usdc", ".usdz")) and not f.endswith(".flat.usda"):
            p = os.path.join(d, f)
            try:
                Usd.Stage.Open(p).Flatten().Export(os.path.splitext(p)[0] + ".flat.usda")
                done += 1
            except Exception as e:  # unreadable test files are reported and skipped
                failed += 1
                print("skip", p, str(e).splitlines()[0][:120])
print(f"{done} flattened, {failed} skipped")
