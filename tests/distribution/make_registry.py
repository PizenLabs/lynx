#!/usr/bin/env python3
"""Build a Cargo registry index from packaged ``.crate`` archives.

``cargo_install_test.sh`` uses this to prove the published package graph
installs from a registry instead of the source checkout. It reads the archives
produced by ``cargo package`` (which already carry Cargo's normalized manifest),
derives one index entry per crate, and writes:

    <out>/config.json                  the download template (sparse mode only)
    <out>/<prefix>/<name>              newline-delimited JSON index entries

The index path layout follows the crates.io convention that Cargo enforces for
both git and sparse registries.

Two modes:

- **crates.io replacement** (used by `cargo_install_test.sh`): omit `--dl-base`,
  pass `--index-url ""`. Same-registry dependencies omit the `registry` field,
  exactly like crates.io. The `--out` directory is the registry index; place the
  `.crate` archives one level up for a Cargo `local-registry` source.
- **Sparse registry**: pass `--dl-base` (a `dl` template, e.g.
  ``http://127.0.0.1:8080/{crate}-{version}.crate``) and `--index-url` (this
  registry's own index URL) to serve a standalone sparse registry.

Usage:
    make_registry.py --crates-dir DIR --out DIR [--dl-base URL] [--index-url URL]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import sys
import tarfile
import tomllib


def index_path(name: str) -> str:
    """crates.io index path for a crate name."""
    n = name.lower()
    if len(n) == 1:
        return f"1/{n}"
    if len(n) == 2:
        return f"2/{n}"
    if len(n) == 3:
        return f"3/{n[0]}/{n}"
    return f"{n[:2]}/{n[2:4]}/{n}"


def _dep_entries(manifest: dict, local: set[str], index_url: str) -> list[dict]:
    """Flatten dependencies (including target- and kind-specific ones) into the
    crates.io index dependency shape."""
    entries: list[dict] = []

    def add(name: str, spec: dict, kind: str, target: str | None) -> None:
        if isinstance(spec, str):
            spec = {"version": spec}
        if "version" not in spec:
            raise SystemExit(
                f"dependency `{name}` ({kind}{'' if target is None else ', ' + target}) "
                "has no version requirement"
            )
        real_name = spec.get("package") or name
        entry: dict = {
            "name": name,
            "req": spec["version"],
            "features": spec.get("features", []),
            "optional": bool(spec.get("optional", False)),
            "default_features": bool(spec.get("default-features", True)),
            "target": target,
            "kind": kind,
        }
        # crates.io omits `registry` for same-registry deps and only sets
        # `package` when a rename is in play. For deps served by this same
        # registry we either omit (crates.io replacement) or point at the local
        # index; external deps point at crates.io.
        if real_name in local and index_url:
            entry["registry"] = index_url
        if spec.get("package"):
            entry["package"] = spec["package"]
        entries.append(entry)

    sections = (
        ("dependencies", "normal"),
        ("build-dependencies", "build"),
        ("dev-dependencies", "dev"),
    )
    for section, kind in sections:
        for name, spec in manifest.get(section, {}).items():
            add(name, spec, kind, None)
    for cfg, table in manifest.get("target", {}).items():
        for section, kind in sections:
            for name, spec in table.get(section, {}).items():
                add(name, spec, kind, cfg)
    return entries


def crate_entry(crate: pathlib.Path, local: set[str], index_url: str) -> tuple[str, str, dict]:
    with tarfile.open(crate) as tar:
        names = [n for n in tar.getnames() if n.endswith("/Cargo.toml") and n.count("/") == 1]
        if not names:
            raise SystemExit(f"{crate}: no normalized Cargo.toml found")
        manifest = tomllib.load(tar.extractfile(names[0]))

    pkg = manifest["package"]
    name, vers = pkg["name"], pkg["version"]
    entry = {
        "name": name,
        "vers": vers,
        "deps": _dep_entries(manifest, local, index_url),
        "features": dict(manifest.get("features", {})),
        "cksum": hashlib.sha256(crate.read_bytes()).hexdigest(),
        "yanked": False,
        "rust_version": pkg.get("rust-version"),
        "v": 2,
    }
    if pkg.get("links"):
        entry["links"] = pkg["links"]
    return name, vers, entry


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--crates-dir", required=True, type=pathlib.Path)
    ap.add_argument("--out", required=True, type=pathlib.Path)
    ap.add_argument("--dl-base", default=None,
                    help="Sparse registry download template; omit for a local-registry source")
    ap.add_argument("--index-url", default="",
                    help="Index URL for same-registry deps (empty -> omitted, i.e. crates.io replacement)")
    args = ap.parse_args()

    crates = sorted(args.crates_dir.glob("*.crate"))
    if not crates:
        print(f"no .crate archives in {args.crates_dir}", file=sys.stderr)
        return 2

    grouped: dict[str, list[tuple[str, dict]]] = {}
    # First pass: learn which crate names this registry provides, so that
    # internal dependencies are marked as coming from the same registry.
    local: set[str] = set()
    for crate in crates:
        with tarfile.open(crate) as tar:
            manifest_name = next(
                n for n in tar.getnames() if n.endswith("/Cargo.toml") and n.count("/") == 1
            )
            local.add(tomllib.load(tar.extractfile(manifest_name))["package"]["name"])

    for crate in crates:
        name, vers, entry = crate_entry(crate, local, args.index_url)
        grouped.setdefault(index_path(name), []).append((vers, entry))

    for path, items in grouped.items():
        target = args.out / path
        target.parent.mkdir(parents=True, exist_ok=True)
        items.sort(key=lambda item: item[0])
        body = "\n".join(json.dumps(entry, separators=(",", ":")) for _, entry in items)
        target.write_text(body + "\n")

    if args.dl_base is not None:
        (args.out / "config.json").write_text(json.dumps({"dl": args.dl_base, "api": None}))
    print(f"registry: {len(crates)} crate(s), {len(grouped)} index entr(ies) -> {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
