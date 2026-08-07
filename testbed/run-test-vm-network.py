#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "pycdlib>=1.14",
# ]
# ///
"""
Boot the weverywhere test VMs on a shared host network and SSH into them.

This brings up one Linux bridge (holding the host's gateway IP) with a tap
device per VM, boots every built VM attached to its tap with its hard-coded
static IP, and prints the exact `ssh` command to reach each one using the key
generated under testbed/vms/<hostname>/.

Every interface created here is torn down on exit - normal exit, Ctrl-C, or a
kill signal - so no bridge or tap devices are ever left behind. The VMs are
asked to shut down cleanly (ACPI via QMP) before the interfaces are removed.

Requires root for the `ip` commands (invoked via sudo). The install phase does
NOT use this script - it runs on qemu user-mode networking - so the only host
networking that ever exists is created and destroyed here.

Usage (from the repo root):
    uv run testbed/run-test-vm-network.py                # boot all built VMs
    uv run testbed/run-test-vm-network.py linux-test01   # boot only named VM(s)
    uv run testbed/run-test-vm-network.py --debug         # open a GTK window per VM
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import time

import _common as c


def _print_access_banner(vms: list[dict]) -> None:
    print()
    print("=" * 72)
    print(" VMs are booting. Host bridge:", c.vm_config.HOST_BRIDGE_CIDR)
    print(" (Static IPs take effect once each guest finishes booting.)")
    print()
    for vm in vms:
        key = c.vm_dir(vm) / "id_ed25519"
        ip = c.static_ip(vm).ip
        print(f"  {vm['hostname']:<14} {vm['os']:<8} {ip}")
        print(f"       ssh -i {key.relative_to(c.REPO_ROOT)} "
              f"{vm['username']}@{ip}")
    print()
    print(" Press Ctrl-C to shut the VMs down and remove all network interfaces.")
    print("=" * 72)
    print()


def main() -> None:
    ap = argparse.ArgumentParser(description="Boot weverywhere test VMs")
    ap.add_argument("vms", nargs="*", help="hostnames to boot (default: all built)")
    ap.add_argument("--debug", action="store_true",
                    help="diagnostic mode: open a qemu GTK window per VM instead "
                         "of running headless (needs a local X/Wayland session)")
    args = ap.parse_args()

    vms = c.select_vms(args.vms)

    missing = [vm["hostname"] for vm in vms if not c.disk_path(vm).exists()]
    if missing:
        sys.exit(
            f"[testbed] no disk image for: {', '.join(missing)}\n"
            f"          build them first: uv run testbed/create-test-vm-images.py"
        )

    procs: list[tuple[dict, subprocess.Popen, "c.Path"]] = []

    # HostNetwork sets up the bridge + taps and guarantees teardown on any exit
    # (context manager + atexit + SIGINT/SIGTERM/SIGHUP). We boot the VMs inside
    # it so the interfaces always outlive the guests and are always cleaned up.
    with c.HostNetwork():
        for vm in vms:
            tap = c.tap_name(vm)
            qmp = c.vm_dir(vm) / "qmp.sock"
            if qmp.exists():
                qmp.unlink()
            cmd = c.qemu_run_cmd(vm, tap, qmp, disp=c.display_args(gui=args.debug))
            c.log(f"booting {vm['hostname']} on {tap} ({c.static_ip(vm).ip})")
            c.log("qemu: " + " ".join(cmd))
            procs.append((vm, subprocess.Popen(cmd), qmp))

        _print_access_banner(vms)

        try:
            # Stay alive while the VMs run; exit if they all quit on their own.
            while any(p.poll() is None for _, p, _ in procs):
                time.sleep(2)
            c.log("all VMs have exited.")
        except KeyboardInterrupt:
            c.log("Ctrl-C: asking VMs to shut down cleanly ...")

        _shutdown_vms(procs)

    c.log("done - all VMs stopped and host interfaces removed.")


def _shutdown_vms(procs) -> None:
    # Request a clean ACPI shutdown via each VM's QMP socket, then wait, then
    # force-kill any stragglers. (HostNetwork teardown runs after this.)
    for vm, proc, qmp in procs:
        if proc.poll() is None:
            c.qmp_powerdown(qmp)

    deadline = time.time() + 60
    for vm, proc, qmp in procs:
        remaining = max(0, deadline - time.time())
        try:
            proc.wait(timeout=remaining)
        except subprocess.TimeoutExpired:
            c.log(f"{vm['hostname']}: did not power off in time, killing.")
            proc.kill()

    for _, _, qmp in procs:
        if qmp.exists():
            qmp.unlink()


if __name__ == "__main__":
    main()
