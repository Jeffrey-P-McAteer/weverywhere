#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# ///
"""
SSH into a weverywhere test VM by hostname, using the key generated for it.

    uv run testbed/vm-shell.py linux-test01            # interactive shell
    uv run testbed/vm-shell.py linux-test01 uptime     # one-off command, prints output
    uv run testbed/vm-shell.py win-test01 whoami

Any arguments after the hostname are forwarded to ssh as the remote command, so
`vm-shell.py <host> <cmd...>` runs <cmd> on the VM and prints its output. With no
command you get an interactive shell. Connection details (key path, user, static
IP, and the StrictHostKeyChecking=no / UserKnownHostsFile=/dev/null options) come
from _common.ssh_base_cmd, the same ones run-test-vm-network.py prints.
"""

from __future__ import annotations

import os
import sys

import _common as c


def _usage(exit_code: int) -> None:
    hosts = ", ".join(vm["hostname"] for vm in c.all_vms())
    print(__doc__.strip())
    print(f"\nknown hosts: {hosts}")
    sys.exit(exit_code)


def main() -> None:
    args = sys.argv[1:]
    if not args or args[0] in ("-h", "--help"):
        _usage(0 if args else 2)

    hostname, forwarded = args[0], args[1:]
    vm = c.select_vms([hostname])[0]  # validates the hostname, lists known ones

    key = c.vm_dir(vm) / "id_ed25519"
    if not key.exists():
        sys.exit(
            f"[testbed] no SSH key for {hostname} at {key.relative_to(c.REPO_ROOT)}\n"
            f"          build the VM first: uv run testbed/create-test-vm-images.py {hostname}"
        )

    # ssh_base_cmd already carries -i <key>, StrictHostKeyChecking=no,
    # UserKnownHostsFile=/dev/null, and user@static-ip. Forwarded args land after
    # the destination, i.e. ssh runs them as the remote command.
    cmd = c.ssh_base_cmd(vm) + forwarded
    os.execvp(cmd[0], cmd)  # replace this process so the TTY is handed to ssh


if __name__ == "__main__":
    main()
