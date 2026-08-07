#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# ///
"""
weverywhere - single source of truth for the build version.

Version scheme: YYYY.MM.H where H = whole hours elapsed since the start of the
current month (UTC). ~744 distinct values per month (31d x 24h) - enough for
many releases per day, monotonic within a month, and needs no stored file or
hard-coded version string anywhere in the tree.
Examples: 2026.08.0 (Aug 1 00:xx UTC), 2026.08.375 (Aug 16 15:xx UTC).

--- Why an env-var override -----------------------------------------------------
The version is time-derived, so recomputing it in each process risks an
hour-boundary mismatch: publish.py could tag v2026.08.55 while the build a
minute later bakes 2026.08.56 into the binary.

To avoid that, resolve_version() prefers the WEVERYWHERE_VERSION environment
variable and only computes (and then exports) a fresh value when it is unset. The
first process in a pipeline - scripts/publish.py or scripts/build.py - resolves
it once; every child process it spawns (cargo zigbuild -> build.rs) inherits the
identical string through the environment. build.rs reads the same variable, so
the version baked into the binary, the git tag, and the GitHub asset names can
never drift across an hour boundary.

--- Usage ----------------------------------------------------------------------
    from _version import resolve_version
    version = resolve_version()        # env override, else compute + export

    uv run scripts/_version.py         # print the resolved version
"""

from __future__ import annotations

import os
from datetime import datetime, timezone

# Environment variable that pins the version across a build pipeline. build.rs
# reads the same name, so keep them in sync.
ENV_VAR = "WEVERYWHERE_VERSION"


def compute_version() -> str:
    """YYYY.MM.H, H = whole hours elapsed since the start of the month (UTC)."""
    now = datetime.now(timezone.utc)
    hours_in_month = (now.day - 1) * 24 + now.hour
    return f"{now.year}.{now.month:02d}.{hours_in_month}"


def resolve_version() -> str:
    """
    Return the build version, preferring $WEVERYWHERE_VERSION.

    When the env var is unset/empty, compute a fresh version AND export it into
    os.environ so any child process this one later spawns inherits the identical
    value instead of recomputing it (and possibly crossing an hour boundary).
    """
    existing = os.environ.get(ENV_VAR, "").strip()
    if existing:
        return existing

    version = compute_version()
    os.environ[ENV_VAR] = version
    return version


if __name__ == "__main__":
    print(resolve_version())
