#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "pycdlib>=1.14",
# ]
# ///
"""
Build the weverywhere test VM disk images by unattended install under qemu.

For each VM in testbed/vm_config.py this produces a ready-to-boot qcow2 under
testbed/vms/<hostname>/disk.qcow2:

  * Linux (fedora/ubuntu): a copy of the distro's cloud qcow2 image, provisioned
    on first boot by cloud-init from a generated NoCloud seed ISO - user, SSH
    key, hard-coded static IP, and a systemd service that runs the weverywhere
    binary. cloud-init powers the VM off when it finishes, which ends the build.

  * Windows: a fresh install from an ISO you provide, driven by a generated
    autounattend.xml on a config ISO. A first-logon script installs OpenSSH,
    drops the SSH key, sets the static IP, registers a boot task for the
    weverywhere binary, then shuts down to end the build.

A VM is (re)built only when its inputs change - the VM's config entry, the
global settings, the base image, or the weverywhere binary (see build_hash in
_common.py). Networking during the install uses qemu user-mode networking, so
this script never creates a host interface and has nothing to tear down.

Usage (from the repo root):
    uv run testbed/create-test-vm-images.py                 # build all, as needed
    uv run testbed/create-test-vm-images.py win-test01      # only named VM(s)
    uv run testbed/create-test-vm-images.py --force         # rebuild even if current
    uv run testbed/create-test-vm-images.py --prepare-only  # make ISOs/keys, don't boot
    uv run testbed/create-test-vm-images.py --debug         # watch install in a GTK window
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import time

import _common as c


def build_linux(vm: dict, args) -> None:
    base_url = c.vm_config.IMAGE_SOURCES.get(vm["os"].lower())
    if not base_url:
        sys.exit(f"[testbed] no image source for os={vm['os']!r}; "
                 f"add one to IMAGE_SOURCES in vm_config.py")

    base = c.download(base_url, c.CACHE_DIR / base_url.rsplit("/", 1)[1])

    # Fresh copy of the base image, grown to the configured size. A plain copy
    # (not a backing overlay) keeps each VM self-contained under testbed/vms/.
    disk = c.disk_path(vm)
    c.log(f"creating {disk.relative_to(c.REPO_ROOT)} from {base.name}")
    c.run(["qemu-img", "convert", "-O", "qcow2", str(base), str(disk)])
    c.run(["qemu-img", "resize", str(disk), f"{c.vm_config.DISK_SIZE_GB}G"])

    c.build_cloud_init_seed(vm, c.vm_dir(vm) / "seed.iso")

    if args.prepare_only:
        c.log(f"{vm['hostname']}: --prepare-only, skipping provisioning boot")
        return

    c.log(f"{vm['hostname']}: booting for cloud-init provisioning "
          "(powers off automatically when done) ...")
    _run_install_boot(vm, args, timeout=1800)


def build_windows(vm: dict, args) -> None:
    # The config ISO and blank disk need no external ISO, so build them first.
    c.build_windows_config(vm, c.vm_dir(vm) / "config.iso")

    disk = c.disk_path(vm)
    disk.unlink(missing_ok=True)  # start from a clean disk on every (re)build
    c.log(f"creating blank {disk.relative_to(c.REPO_ROOT)} "
          f"({c.vm_config.DISK_SIZE_GB}G)")
    c.run(["qemu-img", "create", "-f", "qcow2", str(disk),
           f"{c.vm_config.DISK_SIZE_GB}G"])

    if args.prepare_only:
        c.log(f"{vm['hostname']}: --prepare-only, skipping install boot")
        return

    # Only now do we need the (user-provided) Windows install ISO.
    _require_windows_iso(c.CACHE_DIR / c.vm_config.WINDOWS_ISO_NAME)
    c.log(f"{vm['hostname']}: booting Windows unattended install "
          "(shuts down automatically when done) ...")
    _run_install_boot(vm, args, timeout=3600)


def _require_windows_iso(win_iso) -> None:
    """Windows ISOs are not redistributable - prompt the user and wait."""
    if win_iso.exists() and win_iso.stat().st_size > 0:
        return
    c.CACHE_DIR.mkdir(parents=True, exist_ok=True)
    print()
    print("=" * 72)
    print(" A Windows x64 installation ISO is required and cannot be downloaded")
    print(" automatically (Microsoft requires an interactive download).")
    print()
    print("  1. Download a Windows 10 or 11 x64 ISO, e.g. from")
    print("       https://www.microsoft.com/software-download/windows11")
    print("       (or the Media Creation Tool / a Windows 10 eval ISO)")
    print(f"  2. Save it as:  {win_iso}")
    print()
    print(" I will wait here until that file exists. Press Ctrl-C to abort.")
    print("=" * 72)
    try:
        while not (win_iso.exists() and win_iso.stat().st_size > 0):
            time.sleep(3)
    except KeyboardInterrupt:
        sys.exit("\n[testbed] aborted while waiting for the Windows ISO.")
    c.log(f"found {win_iso.name} ({c.human_bytes(win_iso.stat().st_size)})")


def _run_install_boot(vm: dict, args, *, timeout: int) -> None:
    if args.debug:
        c.log(f"{vm['hostname']}: --debug set, opening a qemu GTK window so you "
              "can watch the install")
    cmd = c.qemu_install_cmd(vm, disp=c.display_args(gui=args.debug))
    c.log("qemu: " + " ".join(cmd))
    start = time.time()
    proc = subprocess.Popen(cmd)
    try:
        proc.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        proc.kill()
        sys.exit(f"[testbed] {vm['hostname']}: install did not finish within "
                 f"{timeout}s. Check {c.vm_dir(vm) / 'serial.log'} and re-run "
                 f"with --display to watch it.")
    if proc.returncode not in (0, None):
        sys.exit(f"[testbed] {vm['hostname']}: qemu exited {proc.returncode} "
                 f"during install.")
    c.log(f"{vm['hostname']}: install finished in "
          f"{int(time.time() - start)}s")


def main() -> None:
    ap = argparse.ArgumentParser(description="Build weverywhere test VM images")
    ap.add_argument("vms", nargs="*", help="hostnames to build (default: all)")
    ap.add_argument("--force", action="store_true",
                    help="rebuild even if inputs are unchanged")
    ap.add_argument("--prepare-only", action="store_true",
                    help="generate keys/seed/config ISOs but do not boot qemu")
    ap.add_argument("--debug", action="store_true",
                    help="diagnostic mode: open a qemu GTK window to watch/interact "
                         "with the install instead of running headless "
                         "(needs a local X/Wayland session)")
    args = ap.parse_args()

    vms = c.select_vms(args.vms)
    c.log(f"weverywhere version: {c.WEVERYWHERE_VERSION}")

    built, skipped = [], []
    for vm in vms:
        # The install bakes the weverywhere binary into the image, so make sure
        # it exists - building it automatically if a matching versioned artifact
        # is not already staged under dist/. Needed for --prepare-only too, since
        # the seed/config ISOs embed the binary.
        c.ensure_weverywhere_binary(vm)

        if not args.force and not c.needs_rebuild(vm):
            c.log(f"{vm['hostname']}: up to date, skipping "
                  f"(use --force to rebuild)")
            skipped.append(vm["hostname"])
            continue

        c.log(f"=== building {vm['hostname']} ({vm['os']}) ===")
        c.ensure_ssh_key(vm)
        if c.is_windows(vm):
            build_windows(vm, args)
        else:
            build_linux(vm, args)

        if not args.prepare_only:
            c.write_manifest(vm)
        built.append(vm["hostname"])

    print()
    c.log(f"done. built: {built or 'none'} | skipped: {skipped or 'none'}")
    if built and not args.prepare_only:
        c.log("boot them with: uv run testbed/run-test-vm-network.py")


if __name__ == "__main__":
    main()
