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
    uv run testbed/create-test-vm-images.py --jobs 2        # cap parallelism (default 8)
    uv run testbed/create-test-vm-images.py --prepare-only  # make ISOs/keys, don't boot
    uv run testbed/create-test-vm-images.py --debug         # watch install in a GTK window

Installs run in parallel (up to --jobs at once) - safe because the install phase
uses qemu user-mode networking (no shared host interfaces) and every output file
is per-VM. Shared writes (the built binary, cached downloads) are locked.
"""

from __future__ import annotations

import argparse
import contextlib
import functools
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed

import _common as c


class BuildError(Exception):
    """A single VM failed to build. Raised (instead of sys.exit) so that parallel
    workers surface the failure per-VM without tearing the whole process down."""


# In-flight qemu install processes, so a Ctrl-C can kill them all promptly instead
# of waiting on blocked proc.wait() calls in worker threads.
_LIVE_PROCS: set[subprocess.Popen] = set()
_LIVE_PROCS_LOCK = threading.Lock()


def _register_proc(proc: subprocess.Popen) -> None:
    with _LIVE_PROCS_LOCK:
        _LIVE_PROCS.add(proc)


def _unregister_proc(proc: subprocess.Popen) -> None:
    with _LIVE_PROCS_LOCK:
        _LIVE_PROCS.discard(proc)


def _kill_all_live_procs() -> None:
    with _LIVE_PROCS_LOCK:
        procs = list(_LIVE_PROCS)
    for proc in procs:
        with contextlib.suppress(Exception):
            proc.kill()

def timed(func):
    @functools.wraps(func)
    def wrapper(*args, **kwargs):
        start = time.perf_counter()
        result = func(*args, **kwargs)
        duration = int(time.perf_counter() - start)

        minutes, seconds = divmod(duration, 60)

        print(f"{func.__name__} took {minutes}m {seconds:02d}s")
        return result

    return wrapper

@timed
def build_linux(vm: dict, args) -> None:
    base_url = c.vm_config.IMAGE_SOURCES.get(vm["os"].lower())
    if not base_url:
        raise BuildError(f"{vm['hostname']}: no image source for os={vm['os']!r}; "
                         f"add one to IMAGE_SOURCES in vm_config.py")

    base = c.download(base_url, c.CACHE_DIR / base_url.rsplit("/", 1)[1])

    # Fresh copy of the base image, grown to the configured size. A plain copy
    # (not a backing overlay) keeps each VM self-contained under testbed/vms/.
    disk = c.disk_path(vm)
    c.log(f"creating {disk.relative_to(c.REPO_ROOT)} from {base.name}")
    c.run(["qemu-img", "convert", "-O", "qcow2", str(base), str(disk)])
    c.run(["qemu-img", "resize", str(disk), f"{c.vm_config.DISK_SIZE_GB}G"])

    c.build_cloud_init_seed(vm, c.seed_iso_path(vm))

    if args.prepare_only:
        c.log(f"{vm['hostname']}: --prepare-only, skipping provisioning boot")
        return

    c.log(f"{vm['hostname']}: booting for cloud-init provisioning "
          "(powers off automatically when done) ...")
    _run_install_boot(vm, args, timeout=1800)


@timed
def build_windows(vm: dict, args) -> None:
    # The config ISO and blank disk need no external ISO, so build them first.
    c.build_windows_config(vm, c.config_iso_path(vm))

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
            time.sleep(1)
    except KeyboardInterrupt:
        sys.exit("\n[testbed] aborted while waiting for the Windows ISO.")
    c.log(f"found {win_iso.name} ({c.human_bytes(win_iso.stat().st_size)})")


def _run_install_boot(vm: dict, args, *, timeout: int) -> None:
    if args.debug:
        c.log(f"{vm['hostname']}: --debug set, opening a qemu GTK window so you "
              "can watch the install")
    cmd = c.qemu_install_cmd(vm, disp=c.display_args(gui=args.debug))
    c.log(f"{vm['hostname']}: qemu: " + " ".join(cmd))
    start = time.time()
    proc = subprocess.Popen(cmd)
    _register_proc(proc)
    try:
        proc.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        proc.kill()
        raise BuildError(
            f"{vm['hostname']}: install did not finish within {timeout}s. "
            f"Check {c.vm_dir(vm) / 'serial.log'} and re-run with --debug to watch it."
        )
    finally:
        _unregister_proc(proc)
    if proc.returncode not in (0, None):
        raise BuildError(
            f"{vm['hostname']}: qemu exited {proc.returncode} during install."
        )
    c.log(f"{vm['hostname']}: install finished in "
          f"{int(time.time() - start)}s")


def _build_one(vm: dict, args) -> None:
    """Build a single VM end to end. Runs in its own worker thread under --jobs.
    Safe to run concurrently: install-phase networking is qemu user-mode (no shared
    host interfaces), every output file is per-VM, and the only cross-VM shared
    writes (binary build, cached downloads) are locked in _common.py."""
    c.log(f"=== building {vm['hostname']} ({vm['os']}) ===")
    c.ensure_ssh_key(vm)
    if c.is_windows(vm):
        build_windows(vm, args)
    else:
        build_linux(vm, args)
    if not args.prepare_only:
        c.write_manifest(vm)


def _build_parallel(to_build: list[dict], args, jobs: int,
                    built: list[str], failed: list[str]) -> None:
    """Run _build_one across up to `jobs` worker threads, recording per-VM results.
    A Ctrl-C kills the in-flight qemus and shuts the pool down without blocking on
    them."""
    ex = ThreadPoolExecutor(max_workers=min(jobs, len(to_build)))
    futures = {ex.submit(_build_one, vm, args): vm for vm in to_build}
    completed_cleanly = False
    try:
        for fut in as_completed(futures):
            vm = futures[fut]
            try:
                fut.result()
                built.append(vm["hostname"])
            except Exception as e:  # noqa: BLE001 - report per-VM, keep others going
                failed.append(vm["hostname"])
                c.log(f"{vm['hostname']}: BUILD FAILED: {e}")
        completed_cleanly = True
    finally:
        if not completed_cleanly:
            # Interrupt / unexpected error: kill running qemus so the worker threads'
            # proc.wait() returns, then shut down without waiting on them.
            _kill_all_live_procs()
        ex.shutdown(wait=completed_cleanly, cancel_futures=not completed_cleanly)


@timed
def main() -> None:
    ap = argparse.ArgumentParser(description="Build weverywhere test VM images")
    ap.add_argument("vms", nargs="*", help="hostnames to build (default: all)")
    ap.add_argument("--force", action="store_true",
                    help="rebuild even if inputs are unchanged")
    ap.add_argument("--prepare-only", action="store_true",
                    help="generate keys/seed/config ISOs but do not boot qemu")
    ap.add_argument("--jobs", "-j", type=int, default=8,
                    help="max VM installs to run in parallel (default: 8). Each "
                         "install boots its own qemu on user-mode networking, so "
                         "they don't share host interfaces; the practical ceiling is "
                         "host RAM/CPU. Use --jobs 1 to build serially.")
    ap.add_argument("--debug", action="store_true",
                    help="diagnostic mode: open a qemu GTK window to watch/interact "
                         "with the install instead of running headless "
                         "(needs a local X/Wayland session)")
    args = ap.parse_args()

    vms = c.select_vms(args.vms)
    c.log(f"weverywhere version: {c.WEVERYWHERE_VERSION}")

    # --- Prepare (serial) ---------------------------------------------------
    # Stage everything shared BEFORE fanning out, so parallel workers never race on
    # it: the per-platform binary build (baked into every image; also feeds
    # build_hash) and the decision of which VMs actually need rebuilding.
    to_build, skipped = [], []
    for vm in vms:
        c.ensure_weverywhere_binary(vm)
        if not args.force and not c.needs_rebuild(vm):
            c.log(f"{vm['hostname']}: up to date, skipping (use --force to rebuild)")
            skipped.append(vm["hostname"])
        else:
            to_build.append(vm)

    if not to_build:
        print()
        c.log(f"done. built: none | skipped: {skipped or 'none'}")
        return

    # Windows installs need the (user-provided) ISO. Wait for it once, up front, so
    # parallel workers don't each print the interactive prompt.
    if not args.prepare_only and any(c.is_windows(vm) for vm in to_build):
        _require_windows_iso(c.CACHE_DIR / c.vm_config.WINDOWS_ISO_NAME)

    # --- Build (parallel) ---------------------------------------------------
    jobs = max(1, args.jobs)
    c.log(f"building {len(to_build)} VM(s) with up to {jobs} parallel job(s)")

    built, failed = [], []
    try:
        if jobs == 1 or len(to_build) == 1:
            for vm in to_build:
                try:
                    _build_one(vm, args)
                    built.append(vm["hostname"])
                except Exception as e:  # noqa: BLE001 - report per-VM, keep going
                    failed.append(vm["hostname"])
                    c.log(f"{vm['hostname']}: BUILD FAILED: {e}")
        else:
            _build_parallel(to_build, args, jobs, built, failed)
    except KeyboardInterrupt:
        _kill_all_live_procs()
        sys.exit("[testbed] aborted; killed any in-flight qemu install(s).")

    print()
    c.log(f"done. built: {built or 'none'} | skipped: {skipped or 'none'} "
          f"| failed: {failed or 'none'}")
    if built and not args.prepare_only:
        c.log("boot them with: uv run testbed/run-test-vm-network.py")
    if failed:
        sys.exit(f"[testbed] {len(failed)} VM(s) failed to build: "
                 f"{', '.join(failed)}")


if __name__ == "__main__":
    main()
