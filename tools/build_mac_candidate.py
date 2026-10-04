#!/usr/bin/env python3
"""Build a local Prism import containing the measured M2 Pro launch settings."""

import argparse
import hashlib
import json
from pathlib import Path

try:
    from . import build_pack, runtime
except ImportError:
    import build_pack
    import runtime


def build(collector, mod_jar):
    lock = build_pack.read_lock()
    if collector not in {"g1", "zgc"}:
        raise ValueError("Choose the measured G1 or ZGC launch configuration")
    if lock["minecraft"] != "26.3" or lock["fabric_loader"] != "0.19.5":
        raise ValueError("Revalidate the Prism components before exporting a new game version")
    profile = "m2-metal-batched"
    selected = build_pack.PROFILES[profile]["mods"]
    build_pack.validate_selection(lock, selected)
    mod_bytes = mod_jar.read_bytes()
    name = f"horizon-m2-metal-batched-{collector}-{lock['pack_version']}"
    entries = {"minecraft/" + path: data for path, data in build_pack.overrides(profile, mod_bytes).items()}
    for project in selected:
        pinned = lock["projects"][project]
        source = runtime.fetch(pinned["url"], build_pack.ROOT / ".local/pack-cache" / pinned["filename"],
                               pinned["hashes"]["sha512"], "sha512")
        entries["minecraft/" + build_pack.mod_path(pinned)] = source.read_bytes()
    entries["mmc-pack.json"] = json.dumps({"formatVersion": 1, "components": [
        {"uid": "org.lwjgl3", "version": "3.4.3", "dependencyOnly": True},
        {"uid": "net.minecraft", "version": lock["minecraft"], "important": True},
        {"uid": "net.fabricmc.intermediary", "version": lock["minecraft"], "dependencyOnly": True},
        {"uid": "net.fabricmc.fabric-loader", "version": lock["fabric_loader"]},
    ]}, indent=2).encode()
    flag = {"g1": "-XX:+UseG1GC", "zgc": "-XX:+UseZGC"}[collector]
    entries["instance.cfg"] = (f"InstanceType=OneSix\nname={name}\niconKey=default\n"
        "AutomaticJava=true\nOverrideJavaLocation=false\nOverrideMemory=true\n"
        f"MinMemAlloc=512\nMaxMemAlloc=6144\nOverrideJavaArgs=true\nJvmArgs={flag}\n").encode()
    entries["HORIZON-BUILD.json"] = json.dumps({"profile": profile, "collector": collector,
        "pack_version": lock["pack_version"], "original_mod_sha256": hashlib.sha256(mod_bytes).hexdigest(),
        "mods": {project: lock["projects"][project]["hashes"] for project in selected}}, indent=2).encode()
    output = build_pack.ROOT / "dist" / f"{name}-prism.zip"
    build_pack.write_zip(output, entries)
    script = output.with_name(f"Mac-Import-{collector}.command")
    script.write_text('#!/bin/bash\nset -euo pipefail\ncd "$(dirname "$0")"\n'
        f'open -a "Prism Launcher" --args --import "$PWD/{output.name}"\n')
    script.chmod(0o755)
    print(f"Built local Prism import: {output}\nCollector: {flag}; heap: 512–6144 MiB")
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gc", choices=["g1", "zgc"], required=True)
    parser.add_argument("--mod", type=Path,
                        default=build_pack.ROOT / "dist" / f"horizon-lab-{build_pack.read_lock()['pack_version']}.jar")
    args = parser.parse_args()
    build(args.gc, args.mod)


if __name__ == "__main__":
    main()
