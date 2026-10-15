#!/usr/bin/env python3
"""Generate a synthetic multi-language repository for stress measurement.

Usage:
    gen_repo.py <dest> --files N --symbols-per-file M [--lang rust|go|ts|py]

Each file declares a `module` pseudo-symbol plus M named symbols, giving the
engine roughly N*(M+1) indexed symbols. Deterministic output for reproducible
benchmarks.
"""
import argparse
import os
import random


def rust_file(idx: int, m: int) -> str:
    lines = [f"// synthetic module {idx}"]
    for s in range(m):
        lines.append(f"pub fn func_{idx}_{s}(x: i64) -> i64 {{")
        lines.append(f"    let v = x + {s};")
        lines.append("    v * 2")
        lines.append("}")
    return "\n".join(lines) + "\n"


def go_file(idx: int, m: int) -> str:
    lines = [f"package pkg{idx % 32}", ""]
    for s in range(m):
        lines.append(f"func Func{idx}_{s}(x int64) int64 {{")
        lines.append(f"\tv := x + {s}")
        lines.append("\treturn v * 2")
        lines.append("}")
    return "\n".join(lines) + "\n"


def ts_file(idx: int, m: int) -> str:
    lines = [f"// synthetic module {idx}"]
    for s in range(m):
        lines.append(f"export function func{idx}_{s}(x: number): number {{")
        lines.append(f"  const v = x + {s};")
        lines.append("  return v * 2;")
        lines.append("}")
    return "\n".join(lines) + "\n"


def py_file(idx: int, m: int) -> str:
    lines = [f"# synthetic module {idx}"]
    for s in range(m):
        lines.append(f"def func_{idx}_{s}(x):")
        lines.append(f"    v = x + {s}")
        lines.append("    return v * 2")
        lines.append("")
    return "\n".join(lines) + "\n"


WRITERS = {"rust": rust_file, "go": go_file, "ts": ts_file, "py": py_file}
EXT = {"rust": "rs", "go": "go", "ts": "ts", "py": "py"}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("dest")
    ap.add_argument("--files", type=int, default=100)
    ap.add_argument("--symbols-per-file", type=int, default=10)
    ap.add_argument("--lang", choices=list(WRITERS), default="rust")
    ap.add_argument("--seed", type=int, default=0)
    args = ap.parse_args()

    random.seed(args.seed)
    writer = WRITERS[args.lang]
    ext = EXT[args.lang]
    for i in range(args.files):
        d = os.path.join(args.dest, f"pkg{i % 64}")
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, f"mod_{i}.{ext}"), "w") as fh:
            fh.write(writer(i, args.symbols_per_file))
    print(f"generated {args.files} files x {args.symbols_per_file} symbols under {args.dest}")


if __name__ == "__main__":
    main()
