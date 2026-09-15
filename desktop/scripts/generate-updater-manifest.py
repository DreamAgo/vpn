#!/usr/bin/env python3
"""Build a complete Tauri v2 update manifest from normalized signed assets."""

import argparse
import json
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import quote


ASSETS = {
    "windows-x86_64": "vpn-gui-windows-amd64-setup-{tag}.exe",
    "darwin-x86_64": "vpn-gui-macos-amd64-{tag}.app.tar.gz",
    "darwin-aarch64": "vpn-gui-macos-arm64-{tag}.app.tar.gz",
}


def generate_manifest(assets_dir: Path, tag: str, base_url: str) -> dict:
    platforms = {}
    for target, pattern in ASSETS.items():
        asset = assets_dir / pattern.format(tag=tag)
        if not asset.is_file() or asset.stat().st_size == 0:
            raise ValueError(f"Missing or empty update package: {asset}")
        signature_path = Path(str(asset) + ".sig")
        if not signature_path.is_file():
            raise ValueError(f"Missing update signature: {signature_path}")
        signature = signature_path.read_text(encoding="utf-8").strip()
        if not signature:
            raise ValueError(f"Empty update signature: {signature_path}")
        platforms[target] = {
            "signature": signature,
            "url": f"{base_url.rstrip('/')}/{quote(asset.name, safe='')}",
        }
    return {
        "version": tag.removeprefix("v"),
        "notes": f"VPN Client {tag.removeprefix('v')}",
        "pub_date": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "platforms": platforms,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repository", required=True, help="GitHub owner/repository")
    parser.add_argument("--assets-dir", type=Path, default=Path("release-assets"))
    parser.add_argument("--base-url", help="Optional mirror URL containing the update packages")
    args = parser.parse_args()
    base_url = args.base_url or (
        f"https://github.com/{args.repository}/releases/download/{quote(args.tag, safe='')}"
    )
    try:
        manifest = generate_manifest(args.assets_dir, args.tag, base_url)
    except ValueError as error:
        parser.error(str(error))
    output = args.assets_dir / "latest.json"
    temporary = output.with_suffix(".json.tmp")
    temporary.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    temporary.replace(output)


if __name__ == "__main__":
    main()
