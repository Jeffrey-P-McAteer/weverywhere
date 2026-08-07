#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "requests>=2.31",
# ]
# ///
"""
Publish a weverywhere release to GitHub.

--- Credentials ----------------------------------------------------------------
Stored in ~/.weverywhere-publish-creds.json:

  {
    "github": {
      "token": "ghp_...",
      "owner": "Jeffrey-P-McAteer",
      "repo":  "weverywhere"
    }
  }

A GITHUB_TOKEN / GH_TOKEN environment variable (with the "repo" scope) is used as
a fallback for the token if the creds file is absent, in which case owner/repo
default to the values below.

Generate a Personal Access Token at https://github.com/settings/tokens
Required scope: repo  (or public_repo for public repositories)

Run once to create the template:
  uv run scripts/publish.py --init-creds

--- Workflow --------------------------------------------------------------------
  1. Compute version YYYY.MM.H (hours elapsed so far in month, UTC)
  2. Build all dist/ artifacts via build.py         (skip with --skip-build)
  3. Create and push an annotated git tag  v{version}
  4. Create a GitHub Release
  5. Upload every artifact found in dist/

--- Usage -----------------------------------------------------------------------
  uv run scripts/publish.py                 # build then publish
  uv run scripts/publish.py --skip-build    # publish existing dist/ files
  uv run scripts/publish.py --dry-run       # preview without changing anything
  uv run scripts/publish.py --draft         # create as a draft (not public yet)
  uv run scripts/publish.py --prerelease    # mark as pre-release
  uv run scripts/publish.py --force         # replace existing release for this version
  uv run scripts/publish.py --init-creds    # write credentials template then exit
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

import requests

from _version import resolve_version

REPO_ROOT = Path(__file__).resolve().parent.parent
DIST_DIR = REPO_ROOT / "dist"
CREDS_FILE = Path.home() / ".weverywhere-publish-creds.json"
APP_NAME = "weverywhere"

DEFAULT_OWNER = "Jeffrey-P-McAteer"
DEFAULT_REPO = "weverywhere"

# -- credentials ---------------------------------------------------------------

CREDS_TEMPLATE = {
    "github": {
        "token": "ghp_YOUR_PERSONAL_ACCESS_TOKEN",
        "owner": DEFAULT_OWNER,
        "repo":  DEFAULT_REPO,
    }
}


def init_creds() -> None:
    if CREDS_FILE.exists():
        print(f"Credentials file already exists: {CREDS_FILE}")
        print("Edit it to add your token, or delete it to regenerate.")
        return

    CREDS_FILE.write_text(json.dumps(CREDS_TEMPLATE, indent=2) + "\n")
    CREDS_FILE.chmod(0o600)
    print(f"Created {CREDS_FILE}")
    print()
    print("Next steps:")
    print("  1. Generate a token at https://github.com/settings/tokens")
    print("     Required scope: repo  (or public_repo for public repos)")
    print(f"  2. Edit {CREDS_FILE} and replace the placeholder token")
    print("  3. Run:  uv run scripts/publish.py --dry-run")


def load_creds() -> dict:
    if CREDS_FILE.exists():
        try:
            raw = json.loads(CREDS_FILE.read_text())
        except json.JSONDecodeError as e:
            print(f"ERROR: Cannot parse {CREDS_FILE}: {e}")
            sys.exit(1)

        gh = raw.get("github", {})
        missing = [k for k in ("token", "owner", "repo") if not gh.get(k)]
        if missing:
            print(f"ERROR: Missing keys in 'github' section of {CREDS_FILE}: {', '.join(missing)}")
            sys.exit(1)

        if gh["token"].startswith("ghp_YOUR"):
            print("ERROR: Credentials file contains the placeholder token.")
            print(f"       Edit {CREDS_FILE} and add a real GitHub Personal Access Token.")
            sys.exit(1)

        return gh

    # No creds file: fall back to an environment token.
    env_token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if env_token:
        return {
            "token": env_token,
            "owner": os.environ.get("GITHUB_OWNER", DEFAULT_OWNER),
            "repo":  os.environ.get("GITHUB_REPO", DEFAULT_REPO),
        }

    print(f"ERROR: Credentials file not found: {CREDS_FILE}")
    print("       and no GITHUB_TOKEN / GH_TOKEN environment variable is set.")
    print()
    print("Run this to create a template:")
    print("  uv run scripts/publish.py --init-creds")
    sys.exit(1)


# -- git -----------------------------------------------------------------------

def git_run(*args: str, capture: bool = True) -> str:
    result = subprocess.run(
        ["git", *args],
        capture_output=capture,
        text=True,
        cwd=REPO_ROOT,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"`git {' '.join(args)}` failed:\n{result.stderr.strip()}"
        )
    return result.stdout.strip()


def check_git_state() -> dict:
    try:
        commit = git_run("rev-parse", "--short", "HEAD")
    except RuntimeError:
        print("ERROR: Not a git repository or no commits yet.")
        sys.exit(1)

    branch = git_run("rev-parse", "--abbrev-ref", "HEAD")

    # Check for unpushed commits
    try:
        unpushed = git_run("rev-list", "@{u}..HEAD", "--count")
    except RuntimeError:
        unpushed = "?"     # no upstream configured

    dirty = bool(subprocess.run(
        ["git", "status", "--porcelain"],
        capture_output=True, text=True, cwd=REPO_ROOT,
    ).stdout.strip())

    # Check that origin is configured
    remotes = git_run("remote").splitlines()
    if "origin" not in remotes:
        print("ERROR: No 'origin' remote configured.")
        print("       Run: git remote add origin <url>")
        sys.exit(1)

    return {
        "commit":   commit,
        "branch":   branch,
        "dirty":    dirty,
        "unpushed": unpushed,
    }


def tag_exists_locally(tag: str) -> bool:
    result = subprocess.run(
        ["git", "tag", "-l", tag],
        capture_output=True, text=True, cwd=REPO_ROOT,
    )
    return bool(result.stdout.strip())


def create_and_push_tag(
    tag: str, message: str, dry_run: bool, creds: dict | None = None
) -> None:
    if tag_exists_locally(tag):
        if not dry_run:
            print(f"  Local tag {tag} already exists; deleting it ...")
            git_run("tag", "-d", tag)

    if dry_run:
        print(f"  [dry-run] git tag -a {tag} -m {message!r}")
        print(f"  [dry-run] git push origin {tag}")
        return

    git_run("tag", "-a", tag, "-m", message)
    print(f"  Created local tag {tag}")

    try:
        git_run("push", "origin", tag)
        print(f"  Pushed {tag} -> origin")
        return
    except RuntimeError as e:
        # `origin` is commonly an SSH URL (git@github.com:...); when no SSH key
        # is configured the push fails. Fall back to an HTTPS push authenticated
        # with the release token so publishing works in token-only environments.
        if not creds:
            raise
        print(f"  git push origin failed ({str(e).splitlines()[0]}); "
              "retrying over HTTPS with the release token ...")

    https_url = (
        f"https://x-access-token:{creds['token']}@github.com/"
        f"{creds['owner']}/{creds['repo']}.git"
    )
    git_run("push", https_url, tag)
    print(f"  Pushed {tag} -> {creds['owner']}/{creds['repo']} (HTTPS)")


# -- GitHub API ----------------------------------------------------------------

class GitHub:
    API    = "https://api.github.com"
    UPLOAD = "https://uploads.github.com"

    def __init__(self, token: str, owner: str, repo: str) -> None:
        self.owner = owner
        self.repo  = repo
        self.sess  = requests.Session()
        self.sess.headers.update({
            "Authorization":        f"Bearer {token}",
            "Accept":               "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
        })

    def _url(self, path: str) -> str:
        # GitHub API returns 404 for any URL with a trailing slash,
        # so omit the separator when path is empty.
        base = f"{self.API}/repos/{self.owner}/{self.repo}"
        return f"{base}/{path}" if path else base

    def verify(self) -> dict:
        r = self.sess.get(self._url(""))

        if r.status_code == 200:
            return r.json()

        if r.status_code == 401:
            msg = r.json().get("message", "") if r.content else ""
            raise RuntimeError(
                f"GitHub token rejected (401 Unauthorized): {msg}\n"
                "  - Check the token hasn't expired or been revoked.\n"
                "  - Fine-grained tokens must be granted 'Contents' read/write access.\n"
                "  - Classic tokens need the 'repo' scope (or 'public_repo' for public repos).\n"
                "  Generate a new classic token at: https://github.com/settings/tokens?type=classic"
            )

        if r.status_code == 403:
            msg = r.json().get("message", "") if r.content else ""
            raise RuntimeError(
                f"GitHub access forbidden (403): {msg}\n"
                "  If the repo is inside an organisation that requires SSO,\n"
                "  authorise your token for that org at https://github.com/settings/tokens"
            )

        if r.status_code == 404:
            # GitHub returns 404 (not 403) when a token exists but cannot see
            # the repo - common with fine-grained PATs that don't list the repo
            # explicitly, even for *public* repos.
            probe = requests.get(
                f"https://api.github.com/repos/{self.owner}/{self.repo}",
                timeout=10,
            )
            if probe.status_code == 200:
                info       = probe.json()
                visibility = "private" if info.get("private") else "public"
                scope      = "repo" if info.get("private") else "public_repo"
                raise RuntimeError(
                    f"Repository {self.owner}/{self.repo} exists ({visibility}) "
                    "but your token cannot access it (GitHub returned 404).\n\n"
                    "This is almost always a fine-grained PAT scope issue:\n"
                    "  GitHub fine-grained PATs must explicitly list every repo they can\n"
                    "  access - even public ones.  Use a classic PAT instead:\n\n"
                    "  1. Go to https://github.com/settings/tokens?type=classic\n"
                    "  2. Click 'Generate new token (classic)'\n"
                    f"  3. Tick the '{scope}' scope\n"
                    f"  4. Paste the new token (starts with ghp_) into {CREDS_FILE}"
                )
            raise RuntimeError(
                f"Repository {self.owner}/{self.repo} not found (checked both "
                "authenticated and unauthenticated).\n"
                f"Verify 'owner' and 'repo' in {CREDS_FILE}"
            )

        # Unexpected status - surface the raw GitHub message.
        try:
            msg = r.json().get("message", r.text[:200])
        except Exception:
            msg = r.text[:200]
        raise RuntimeError(f"GitHub API returned HTTP {r.status_code}: {msg}")

    def get_release_by_tag(self, tag: str) -> dict | None:
        r = self.sess.get(self._url(f"releases/tags/{tag}"))
        if r.status_code == 404:
            return None
        r.raise_for_status()
        return r.json()

    def delete_release(self, release_id: int) -> None:
        r = self.sess.delete(self._url(f"releases/{release_id}"))
        r.raise_for_status()

    def delete_remote_tag(self, tag: str) -> None:
        r = self.sess.delete(self._url(f"git/refs/tags/{tag}"))
        if r.status_code not in (204, 422):
            r.raise_for_status()

    def create_release(
        self,
        tag:        str,
        name:       str,
        body:       str,
        draft:      bool = False,
        prerelease: bool = False,
    ) -> dict:
        r = self.sess.post(self._url("releases"), json={
            "tag_name":         tag,
            "name":             name,
            "body":             body,
            "draft":            draft,
            "prerelease":       prerelease,
            "generate_release_notes": False,
        })
        r.raise_for_status()
        return r.json()

    def upload_asset(
        self,
        release_id:  int,
        path:        Path,
        upload_name: str,
    ) -> dict:
        content_type = (
            mimetypes.guess_type(path.name)[0] or "application/octet-stream"
        )
        url = (
            f"{self.UPLOAD}/repos/{self.owner}/{self.repo}"
            f"/releases/{release_id}/assets"
        )
        file_size = path.stat().st_size
        r = self.sess.post(
            url,
            params={"name": upload_name},
            headers={
                "Content-Type":   content_type,
                "Content-Length": str(file_size),
            },
            data=_ProgressReader(path),
            stream=True,
        )
        print()   # newline after progress bar
        r.raise_for_status()
        return r.json()


class _ProgressReader:
    """File-like wrapper that prints a simple progress bar while being read."""

    def __init__(self, path: Path) -> None:
        self._f     = open(path, "rb")
        self._total = path.stat().st_size
        self._done  = 0

    def read(self, size: int = -1) -> bytes:
        chunk = self._f.read(size)
        self._done += len(chunk)
        if self._total > 0:
            pct    = self._done * 100 // self._total
            done_m = self._done  // 1_048_576
            tot_m  = self._total // 1_048_576
            bar    = "#" * (pct // 5) + "-" * (20 - pct // 5)
            print(f"\r    [{bar}] {pct:3d}%  {done_m}/{tot_m} MB",
                  end="", flush=True)
        return chunk

    def __len__(self) -> int:
        return self._total

    def close(self) -> None:
        self._f.close()


# -- artifacts -----------------------------------------------------------------

# The six platforms scripts/build.py stages under dist/. Windows ships both the
# raw .exe and a .zip of it; every other platform ships a single raw binary.
_PLATFORMS = [
    "linux-x64",
    "linux-arm64",
    "windows-x64",
    "windows-arm64",
    "macos-x64",
    "macos-arm64",
]


def _artifact_names(version: str, platform: str) -> list[str]:
    base = f"{APP_NAME}-{version}-{platform}"
    if platform.startswith("windows"):
        return [f"{base}.exe", f"{base}.zip"]
    return [base]


def find_artifacts(version: str) -> list[tuple[Path, str]]:
    """Return (local_path, upload_name) for every artifact that exists in dist/.

    build.py already names each file weverywhere-<version>-<platform>[.ext], so
    the on-disk filename is used verbatim as the GitHub asset name.
    """
    result = []
    for platform in _PLATFORMS:
        for name in _artifact_names(version, platform):
            local = DIST_DIR / platform / name
            if local.exists():
                result.append((local, name))
    return result


# -- release notes -------------------------------------------------------------

def make_release_body(version: str, git_info: dict) -> str:
    now = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC")
    dirty_note = "  [!] built with uncommitted changes" if git_info["dirty"] else ""

    return "\n".join([
        f"**Version:** `{version}`  ",
        f"**Commit:** `{git_info['commit']}`{dirty_note}  ",
        f"**Branch:** `{git_info['branch']}`  ",
        f"**Built:** {now}",
        "",
        "### Installing",
        "",
        "| Platform | Instructions |",
        "|---|---|",
        "| **Linux x64 / arm64** | `chmod +x weverywhere-*-linux-*` then run directly |",
        "| **Windows x64 / arm64** | Run the `.exe` directly, or extract the `.zip` (same binary inside) |",
        "| **macOS (Apple Silicon / Intel)** | `chmod +x weverywhere-*-macos-*` then run directly |",
    ])


# -- main ----------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Publish a weverywhere GitHub release",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--init-creds",  action="store_true",
        help=f"Create a credentials template at {CREDS_FILE} then exit",
    )
    parser.add_argument(
        "--dry-run",     action="store_true",
        help="Show what would happen without making any changes",
    )
    parser.add_argument(
        "--skip-build",  action="store_true",
        help="Use existing dist/ artifacts; skip running build.py",
    )
    parser.add_argument(
        "--force",       action="store_true",
        help="Delete and recreate the release if this version already exists",
    )
    parser.add_argument(
        "--draft",       action="store_true",
        help="Create as a draft (not publicly visible until published)",
    )
    parser.add_argument(
        "--prerelease",  action="store_true",
        help="Mark the release as a pre-release",
    )
    args = parser.parse_args()

    # -- --init-creds ----------------------------------------------------------
    if args.init_creds:
        init_creds()
        return

    sep = "-" * 60

    # -- credentials + API connection ------------------------------------------
    print(sep)
    print("Credentials")
    print(sep)
    creds = load_creds()
    gh    = GitHub(creds["token"], creds["owner"], creds["repo"])

    if not args.dry_run:
        print("Connecting to GitHub ...")
        try:
            repo_info = gh.verify()
        except RuntimeError as e:
            print(f"ERROR: {e}")
            sys.exit(1)
        visibility = "private" if repo_info.get("private") else "public"
        print(f"  OK  {creds['owner']}/{creds['repo']}  ({visibility})")
        print(f"     {repo_info.get('html_url', '')}")
    else:
        print(f"  [dry-run] Would connect to {creds['owner']}/{creds['repo']}")

    # -- version + git state ---------------------------------------------------
    print()
    print(sep)
    print("Version & git state")
    print(sep)
    # Resolve once here (this also exports WEVERYWHERE_VERSION), so the build.py
    # subprocess and its `cargo zigbuild` -> build.rs bake the SAME version -
    # the git tag, GitHub asset names, and the baked-in --version output can no
    # longer drift across an hour boundary.
    version  = resolve_version()
    tag      = f"v{version}"
    git_info = check_git_state()

    print(f"  Version:  {version}  ->  tag {tag}")
    print(f"  Commit:   {git_info['commit']}  [{git_info['branch']}]")

    if git_info["dirty"]:
        print("  [!]  Working tree has uncommitted changes - the release will be")
        print("     tagged at the current HEAD, not including these changes.")

    unpushed = git_info["unpushed"]
    if unpushed not in ("0", "?", ""):
        print(f"  [!]  {unpushed} local commit(s) not yet pushed to origin.")
        print("     Consumers downloading the source archive won't see them.")

    # -- check for existing release --------------------------------------------
    if not args.dry_run:
        existing = gh.get_release_by_tag(tag)
        if existing:
            if args.force:
                print(f"\n  Force mode: deleting existing release {tag} ...")
                gh.delete_release(existing["id"])
                gh.delete_remote_tag(tag)
                print(f"  Deleted release and remote tag {tag}")
            else:
                print(
                    f"\nERROR: A release for {tag} already exists:\n"
                    f"       {existing['html_url']}\n\n"
                    "       Wait until the next hour for a new version,\n"
                    "       or use --force to overwrite this release."
                )
                sys.exit(1)

    # -- build -----------------------------------------------------------------
    print()
    print(sep)
    print("Build" if not args.skip_build else "Artifacts  (--skip-build)")
    print(sep)

    if not args.skip_build:
        cmd = ["uv", "run", "scripts/build.py"]
        if args.dry_run:
            print(f"  [dry-run] {' '.join(cmd)}")
        else:
            print("  Running build.py ...")
            result = subprocess.run(cmd, cwd=REPO_ROOT)
            if result.returncode != 0:
                print("ERROR: build.py failed - see output above.", file=sys.stderr)
                sys.exit(1)
    else:
        print("  Skipping build.")

    artifacts = find_artifacts(version)
    if not artifacts:
        print(
            f"\nERROR: No artifacts found in {DIST_DIR.relative_to(REPO_ROOT)}/\n"
            "       Run `uv run scripts/build.py` or drop --skip-build."
        )
        sys.exit(1)

    print(f"\n  {len(artifacts)} artifact(s) ready:")
    for path, upload_name in artifacts:
        size_mb = path.stat().st_size / 1_048_576
        print(f"    {upload_name:<50}  {size_mb:5.0f} MB")

    # -- git tag ---------------------------------------------------------------
    print()
    print(sep)
    print("Git tag")
    print(sep)
    try:
        create_and_push_tag(
            tag,
            message=f"weverywhere {version}",
            dry_run=args.dry_run,
            creds=creds,
        )
    except RuntimeError as e:
        print(f"ERROR: {e}", file=sys.stderr)
        sys.exit(1)

    # -- create release --------------------------------------------------------
    print()
    print(sep)
    print("GitHub release")
    print(sep)

    release_name = f"weverywhere {version}"
    body         = make_release_body(version, git_info)
    draft_note   = "  (draft)" if args.draft else ""
    pre_note     = "  (pre-release)" if args.prerelease else ""

    if args.dry_run:
        print(f"  [dry-run] POST /repos/{creds['owner']}/{creds['repo']}/releases")
        print(f"    tag:        {tag}")
        print(f"    name:       {release_name}")
        print(f"    draft:      {args.draft}{draft_note}")
        print(f"    prerelease: {args.prerelease}{pre_note}")
        release_id  = 0
        release_url = f"https://github.com/{creds['owner']}/{creds['repo']}/releases/tag/{tag}"
    else:
        try:
            release = gh.create_release(
                tag=tag,
                name=release_name,
                body=body,
                draft=args.draft,
                prerelease=args.prerelease,
            )
        except requests.HTTPError as e:
            print(f"ERROR: Could not create release: {e}", file=sys.stderr)
            sys.exit(1)
        release_id  = release["id"]
        release_url = release["html_url"]
        print(f"  OK  {release_url}{draft_note}{pre_note}")

    # -- upload artifacts ------------------------------------------------------
    print()
    print(sep)
    print(f"Uploading {len(artifacts)} artifact(s)")
    print(sep)

    failed = []
    for path, upload_name in artifacts:
        size_mb = path.stat().st_size / 1_048_576
        print(f"  {upload_name}  ({size_mb:.0f} MB)")
        if args.dry_run:
            print("    [dry-run] POST asset")
            continue
        try:
            asset = gh.upload_asset(release_id, path, upload_name)
            print(f"    OK  {asset['browser_download_url']}")
        except (requests.HTTPError, RuntimeError) as e:
            print(f"    X  FAILED: {e}", file=sys.stderr)
            failed.append(upload_name)

    # -- summary ---------------------------------------------------------------
    print()
    print(sep)
    if args.dry_run:
        print("DRY-RUN complete - no changes were made.")
    elif failed:
        print(f"Published with errors - {len(failed)} upload(s) failed:")
        for name in failed:
            print(f"  X  {name}")
        print(f"\n  Release URL: {release_url}")
        sys.exit(1)
    else:
        uploaded = len(artifacts)
        print(f"OK  Published {tag}  ({uploaded} artifact(s))")
        print(f"   {release_url}")
    print(sep)


if __name__ == "__main__":
    main()
