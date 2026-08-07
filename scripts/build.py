#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "zstandard",
# ]
# ///
"""
weverywhere - cross-compile + package for every supported platform.

Cross-compiles the Rust binary on a Linux x86_64 host to all six release
targets using cargo-zigbuild (zig as the cross-linker), then stages the
resulting artifacts under dist/ where scripts/publish.py can find them.

Usage (from the repo root):
    uv run scripts/build.py                       # all six targets
    uv run scripts/build.py linux-x64 macos-arm64 # only the named targets

Outputs (every file names its version and target; <ver> = YYYY.MM.<hours>):
    dist/linux-x64/weverywhere-<ver>-linux-x64
    dist/linux-arm64/weverywhere-<ver>-linux-arm64
    dist/windows-x64/weverywhere-<ver>-windows-x64.exe
    dist/windows-x64/weverywhere-<ver>-windows-x64.zip      (contains the .exe above)
    dist/windows-arm64/weverywhere-<ver>-windows-arm64.exe
    dist/windows-arm64/weverywhere-<ver>-windows-arm64.zip  (contains the .exe above)
    dist/macos-x64/weverywhere-<ver>-macos-x64
    dist/macos-arm64/weverywhere-<ver>-macos-arm64

The build version (YYYY.MM.<hours-into-month>, UTC) is resolved once here and
exported as WEVERYWHERE_VERSION so build.rs bakes the identical string into every
binary. See scripts/_version.py for the single source of truth.
"""

from __future__ import annotations

import io
import os
import pathlib
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import urllib.request
import zipfile

import zstandard

from _version import resolve_version

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
DIST_DIR = REPO_ROOT / "dist"

# Platform name  ->  Rust target triple. Platform names match publish.py and the
# GitHub asset suffixes.
TARGETS: dict[str, str] = {
    "linux-x64":     "x86_64-unknown-linux-gnu",
    "linux-arm64":   "aarch64-unknown-linux-gnu",
    "windows-x64":   "x86_64-pc-windows-gnu",
    "windows-arm64": "aarch64-pc-windows-gnullvm",
    "macos-x64":     "x86_64-apple-darwin",
    "macos-arm64":   "aarch64-apple-darwin",
}

REQUIRED_BINS = ["cargo", "zig", "git"]

# Minimum cargo-zigbuild that correctly links zig's compiler-rt builtins for the
# *-windows-gnullvm targets. 0.20.0 and older leave __chkstk / __divti3 /
# __udivti3 / __umodti3 / __floatuntidf undefined when linking
# aarch64-pc-windows-gnullvm; 0.23.0 is verified-good with zig 0.16.0.
MIN_CARGO_ZIGBUILD = (0, 23, 0)


# -- toolchain bootstrap -------------------------------------------------------

def check_required_bins() -> None:
    for b in REQUIRED_BINS:
        if not shutil.which(b):
            sys.exit(f'[fatal] required binary "{b}" not found on PATH. Install it and re-run.')


def _cargo_zigbuild_bin() -> str | None:
    # cargo resolves `cargo zigbuild` from a cargo-zigbuild binary on PATH or in
    # $CARGO_HOME/bin (~/.cargo/bin), the latter often absent from PATH under
    # `uv run`, so check both.
    found = shutil.which("cargo-zigbuild")
    if found:
        return found
    cargo_home = pathlib.Path(os.environ.get("CARGO_HOME", pathlib.Path.home() / ".cargo"))
    candidate = cargo_home / "bin" / "cargo-zigbuild"
    return str(candidate) if candidate.exists() else None


def _cargo_zigbuild_version(binary: str) -> tuple[int, ...] | None:
    out = subprocess.run([binary, "--version"], capture_output=True, text=True)
    m = re.search(r"(\d+)\.(\d+)\.(\d+)", out.stdout)
    return tuple(int(g) for g in m.groups()) if m else None


def ensure_cargo_zigbuild() -> None:
    binary = _cargo_zigbuild_bin()
    version = _cargo_zigbuild_version(binary) if binary else None

    if version is not None and version >= MIN_CARGO_ZIGBUILD:
        return

    want = ".".join(map(str, MIN_CARGO_ZIGBUILD))
    if version is None:
        print(f"cargo-zigbuild is required for cross-compilation; installing >= {want} ...")
    else:
        have = ".".join(map(str, version))
        # 0.20.0 and older mislink the *-windows-gnullvm targets, so force the
        # upgrade rather than silently building broken binaries.
        print(f"cargo-zigbuild {have} is too old (need >= {want}); upgrading ...")

    subprocess.run(
        ["cargo", "install", "--locked", "--force", "cargo-zigbuild"], check=True
    )


def ensure_rust_targets(triples: list[str]) -> None:
    """Install any rustup std targets that aren't present yet."""
    if not shutil.which("rustup"):
        return  # non-rustup toolchains manage std some other way; assume present
    installed = subprocess.run(
        ["rustup", "target", "list", "--installed"],
        capture_output=True, text=True,
    ).stdout.split()
    missing = [t for t in triples if t not in installed]
    if missing:
        print(f"Installing rust std for: {', '.join(missing)}")
        subprocess.run(["rustup", "target", "add", *missing], check=True)


def setup_mingw_tools() -> None:
    """
    x86_64-pc-windows-gnu linking calls out to the mingw-w64 binutils
    (dlltool, etc.), which are not present on a bare Linux host. Download the
    Arch Linux mingw-w64-binutils package into a cache dir and prepend it to
    PATH. Idempotent: skipped once the tools are on PATH.
    """
    cache = pathlib.Path.home() / ".cache" / "mingw32_tools_folder"
    cache.mkdir(parents=True, exist_ok=True)
    os.environ["PATH"] = os.pathsep.join([
        str(cache),
        str(cache / "bin"),
        str(cache / "usr" / "bin"),
        str(cache / "opt" / "bin"),
        str(cache / "opt" / "x86_64-w64-mingw32" / "bin"),
        os.environ["PATH"],
    ])
    if shutil.which("x86_64-w64-mingw32-dlltool"):
        print(f"Using mingw dlltool: {shutil.which('x86_64-w64-mingw32-dlltool')}")
        return

    url = "https://archlinux.org/packages/extra/x86_64/mingw-w64-binutils/download/"
    print(f"Downloading mingw-w64 binutils from {url}")
    with urllib.request.urlopen(url) as resp:
        zst_data = resp.read()
    with zstandard.ZstdDecompressor().stream_reader(io.BytesIO(zst_data)) as reader:
        tar_bytes = io.BytesIO(reader.read())
    with tarfile.open(fileobj=tar_bytes, mode="r:") as tar:
        tar.extractall(path=cache)

    if not shutil.which("x86_64-w64-mingw32-dlltool"):
        sys.exit("[fatal] mingw-w64 dlltool still not found after download.")
    print(f"Using mingw dlltool: {shutil.which('x86_64-w64-mingw32-dlltool')}")


def setup_macos_sdk() -> None:
    """
    Linking the *-apple-darwin targets needs the macOS SDK (system frameworks
    and libs). Clone a prebuilt SDK collection and point the darwin targets'
    RUSTFLAGS at the newest SDK via -isysroot / -F / -L.
    """
    cache = pathlib.Path.home() / ".cache" / "macos_sdk_folder"
    if (cache / ".git").exists():
        print(f"Using cached macOS SDK checkout at {cache}")
    else:
        print(f"Cloning macOS SDKs to {cache} (shallow) ...")
        cache.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run([
            "git", "clone", "--depth", "1",
            "https://github.com/alexey-lysiuk/macos-sdk.git",
            str(cache),
        ], check=True)

    sdk = _newest_sdk(cache)
    if sdk is None:
        sys.exit(f"[fatal] no MacOSX*.sdk directory found under {cache}")
    print(f"Using macOS SDK: {sdk.name}")

    frameworks = sdk / "System" / "Library" / "Frameworks"
    libs = sdk / "usr" / "lib"
    link_args = " ".join([
        "-C", "link-arg=-isysroot", "-C", f"link-arg={sdk}",
        "-C", f"link-arg=-F{frameworks}",
        "-C", f"link-arg=-L{libs}",
    ])
    os.environ["CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS"] = link_args
    os.environ["CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS"] = link_args


def _newest_sdk(cache: pathlib.Path) -> pathlib.Path | None:
    pattern = re.compile(r"MacOSX(\d+)\.(\d+)(?:\.(\d+))?\.sdk")
    best: tuple[tuple[int, int, int], pathlib.Path] | None = None
    for item in cache.iterdir():
        if not item.is_dir():
            continue
        m = pattern.match(item.name)
        if not m:
            continue
        ver = (int(m.group(1)), int(m.group(2)), int(m.group(3) or 0))
        if best is None or ver > best[0]:
            best = (ver, item)
    return best[1] if best else None


# -- build + package -----------------------------------------------------------

def find_target_binary(triple: str) -> pathlib.Path:
    for name in ("weverywhere.exe", "weverywhere"):
        candidate = REPO_ROOT / "target" / triple / "release" / name
        if candidate.exists():
            return candidate
    raise FileNotFoundError(f"no built binary found for target {triple}")


def build_target(platform: str, triple: str) -> pathlib.Path:
    print(f"\n=== Building {platform}  ({triple}) ===")
    # The repo's .cargo/config.toml sets `[profile.release] debug = true` as a
    # local dev convenience, which balloons the release binaries past 300 MB.
    # Override it here so distributed artifacts are stripped and lean (~20 MB)
    # without changing the checked-in dev config.
    subprocess.run(
        [
            "cargo", "zigbuild", "--release", "--target", triple,
            "--config", 'profile.release.strip="symbols"',
            "--config", "profile.release.debug=false",
        ],
        cwd=REPO_ROOT, check=True,
    )
    binary = find_target_binary(triple)
    print(f"[built] {binary}")
    return binary


def package_target(
    platform: str, binary: pathlib.Path, version: str
) -> list[pathlib.Path]:
    """
    Stage the built binary under dist/<platform>/ and return the artifact(s).

    Every file is named weverywhere-<version>-<platform>[.ext] so its target and
    version are obvious at a glance. Windows ships both the raw .exe and a .zip
    of that same .exe (the archived entry keeps the identical full name).
    """
    # Rebuild the platform dir from scratch so stale, differently-versioned
    # artifacts from an earlier run don't linger alongside the current ones.
    out_dir = DIST_DIR / platform
    if out_dir.exists():
        shutil.rmtree(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    base = f"weverywhere-{version}-{platform}"
    artifacts: list[pathlib.Path] = []

    if platform.startswith("windows"):
        exe = out_dir / f"{base}.exe"
        shutil.copy2(binary, exe)
        artifacts.append(exe)

        archive = out_dir / f"{base}.zip"
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as zf:
            zf.write(exe, exe.name)  # entry keeps the full weverywhere-<ver>-<plat>.exe name
        artifacts.append(archive)
    else:
        # Linux and macOS ship the raw, executable binary.
        out_bin = out_dir / base
        shutil.copy2(binary, out_bin)
        out_bin.chmod(out_bin.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        artifacts.append(out_bin)

    for a in artifacts:
        print(f"[packaged] {a}")
    return artifacts


# -- main ----------------------------------------------------------------------

def main() -> None:
    # RUSTFLAGS from the ambient shell would clobber the per-target darwin flags
    # we set below, so drop it.
    os.environ.pop("RUSTFLAGS", None)

    selected = sys.argv[1:] or list(TARGETS)
    unknown = [t for t in selected if t not in TARGETS]
    if unknown:
        sys.exit(
            f"[fatal] unknown target(s): {', '.join(unknown)}\n"
            f"        valid targets: {', '.join(TARGETS)}"
        )

    version = resolve_version()
    print(f"weverywhere build version: {version}")

    check_required_bins()
    ensure_cargo_zigbuild()
    ensure_rust_targets([TARGETS[t] for t in selected])

    # Only bootstrap the heavy per-OS toolchains we actually need.
    if any(t.startswith("windows") for t in selected):
        setup_mingw_tools()
    if any(t.startswith("macos") for t in selected):
        setup_macos_sdk()

    artifacts: list[pathlib.Path] = []
    for platform in selected:
        binary = build_target(platform, TARGETS[platform])
        artifacts.extend(package_target(platform, binary, version))

    print(f"\nDone. {len(artifacts)} artifact(s) staged under {DIST_DIR.relative_to(REPO_ROOT)}/:")
    for a in artifacts:
        print(f"  {a.relative_to(REPO_ROOT)}")


if __name__ == "__main__":
    main()
