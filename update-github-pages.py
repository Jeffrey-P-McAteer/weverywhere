# /// script
# requires-python = ">=3.12"
# dependencies = [
#   "GitPython",
# ]
# ///

import os
import sys
import subprocess
import tempfile
import traceback
import shutil
import webbrowser
import time
import datetime
import getpass
import socket
import tomllib
import zlib
import pathlib
import textwrap

import git

BUILD_TIMESTAMP = datetime.datetime.now().strftime('%Y-%m-%d %H:%M')

r = git.Repo('.')
h = r.head.commit.hexsha[:7]
dirty = r.is_dirty()
if dirty:
    added,deleted = 0,0
    for diff in r.index.diff(None):
        for (a,b) in [(diff.a_blob, diff.b_blob)]:
            pass
    # simplified: call git directly for numstat
    out = subprocess.getoutput('git diff --numstat')
    added = sum(int(l.split()[0]) for l in out.splitlines() if l.split()[0].isdigit())
    deleted = sum(int(l.split()[1]) for l in out.splitlines() if l.split()[1].isdigit())
    GIT_HASH_AND_DELTAS = f"{h}-dirty+{added}-{deleted}"
else:
    GIT_HASH_AND_DELTAS = h

# HTML template for the release download page
INDEX_HTML_TEMPLATE = """<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>weverywhere - Download</title>
    <style>
        body {
            font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            margin: 0;
            padding: 80pt 0pt 0pt 0pt;
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }
        h1, h2, h3 {
            font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
        }
        .container {
            background: white;
            border-radius: 12px;
            box-shadow: 0 10px 30px rgba(0,0,0,0.2);
            padding: 40px;
            max-width: 600px;
            text-align: center;
        }
        h1 {
            color: #333;
            margin-bottom: 10px;
            font-size: 2.5em;
        }
        .subtitle {
            color: #666;
            margin-bottom: 30px;
            font-size: 1.1em;
        }
        .download-section {
            margin: 30px 0;
        }
        .download-button-holder {
            display: flex;
            flex-direction: row;
        }
        .download-button {
            /*display: inline-block;*/
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
            text-decoration: none;
            padding: 15px 30px;
            border-radius: 8px;
            margin: 10px;
            font-weight: bold;
            font-size: 1.1em;
            transition: transform 0.2s, box-shadow 0.2s;
            min-width: 200px;
            display: flex;
            align-items: center;   /* Vertically centers items */
            justify-content: center; /* Horizontally centers items (optional) */
            text-decoration: none; /* Removes underline */
            gap: 8px; /* Space between image and text */
            flex-direction: row;
            width: 35%;
        }
        .download-button:hover {
            transform: translateY(-2px);
            box-shadow: 0 5px 15px rgba(0,0,0,0.2);
        }
        .platform-icon {
            margin-right: 8px;
            font-size: 1.2em;
        }
        .file-info {
            font-size: 0.9em;
            color: #888;
            margin-top: 5px;
        }
        .controls {
            background: #f8f9fa;
            border-radius: 8px;
            padding: 20px;
            margin-top: 30px;
            text-align: left;
        }
        .controls h3 {
            margin-top: 0;
            color: #333;
        }
        .controls ul {
            margin: 0;
            padding-left: 20px;
        }
        .controls li {
            margin: 5px 0;
            color: #555;
        }
        .project-status {
            background: #f8f9fa;
            border-radius: 8px;
            padding: 20px;
            margin-top: 30px;
            text-align: left;
        }
        .project-status h3 {
            margin-top: 0;
            color: #333;
        }
        .chart-container {
            margin: 20px 0;
            max-width: 100%;
            overflow: hidden;
        }
        .chart-container img {
            max-width: 100%;
            height: auto;
            border-radius: 8px;
            box-shadow: 0 2px 10px rgba(0,0,0,0.1);
        }
        .footer {
            margin-top: 30px;
            padding-top: 20px;
            border-top: 1px solid #eee;
            color: #888;
            font-size: 0.9em;
        }
        /* Mobile Style Adjustments */
        @media (max-aspect-ratio: 1/1) {
            .download-button-holder {
                flex-direction: column;
            }
            .download-button {
                width: 80%;
            }
        }
    </style>
</head>
<body>
    <div class="container">
        <h1>weverywhere</h1>
        <p class="subtitle">
             TODO docs et al
        </p>

        <div class="download-section">
            <h2>Download Latest Release</h2>
            <div class="download-button-holder">
                <!-- <a href="FullCrisis3.linux.x64" class="download-button">
                    <img class="platform-icon" src="linux-icon.png" width="64" height="64" />
                    <span>Linux x64</span>
                </a>

                <a href="FullCrisis3.win.x64.exe" class="download-button">
                    <img class="platform-icon" src="windows-icon.png" width="64" height="64" />
                    <span>Windows x64</span>
                </a> -->
            </div>
        </div>


        <div class="project-status">
            <h3>Code</h3>
            <p>
                Code for this project resides at
                <a href="https://github.com/Jeffrey-P-McAteer/weverywhere">github.com/Jeffrey-P-McAteer/weverywhere</a>.
            </p>
        </div>

        <div class="footer">
            <p>Built at """+BUILD_TIMESTAMP+""" from """+GIT_HASH_AND_DELTAS+"""</p>
        </div>
    </div>
</body>
</html>"""

# AI-generated utility
def get_last_commit_sha_message(repo_path="."):
    git_dir = os.path.join(repo_path, ".git")

    # Read the HEAD file to find the current branch
    with open(os.path.join(git_dir, "HEAD"), "r") as f:
        ref_line = f.readline().strip()

    if ref_line.startswith("ref: "):
        ref_path = os.path.join(git_dir, ref_line[5:])
        with open(ref_path, "r") as f:
            commit_hash = f.readline().strip()
    else:
        # Detached HEAD (ref_line contains the commit hash directly)
        commit_hash = ref_line

    # Get the object file for the commit
    obj_path = os.path.join(git_dir, "objects", commit_hash[:2], commit_hash[2:])
    with open(obj_path, "rb") as f:
        compressed_data = f.read()

    decompressed_data = zlib.decompress(compressed_data)

    # Convert to string
    commit_data = decompressed_data.decode()

    # Find the commit message (starts after two newlines)
    message_index = commit_data.find("\n\n")
    if message_index != -1:
        return (commit_hash, commit_data[message_index+2:].strip())
    else:
        return (commit_hash, "(No commit message found)")

def wrap_text(text, width=76):
    return textwrap.fill(text, width=width, break_long_words=False, break_on_hyphens=False)

def render_file_size(td, file_path):
  file_path = os.path.join(td, file_path)
  file_bytes = 0
  if os.path.exists(file_path):
    if os.path.isfile(file_path):
      file_bytes = os.path.getsize(file_path)
    else:
      # Recurse sum directory contents
      for root, dirs, files in os.walk(file_path):
        for file in files:
          child_file_path = os.path.join(root, file)
          file_bytes += os.path.getsize(child_file_path)

  return f'{file_bytes//1_000_000}mb'

def run_command(cmd, cwd=None, description="", capture_output=True):
    """Run a shell command and handle errors"""
    print(f"Running: {description}")
    print(f"Command: {' '.join(cmd) if isinstance(cmd, list) else cmd}")

    try:
        result = subprocess.run(cmd, cwd=cwd, check=True, capture_output=capture_output, text=True)
        if capture_output and result.stdout:
            print(f"Output: {result.stdout.strip()}")
        return True
    except subprocess.CalledProcessError as e:
        print(f"ERROR: {e}")
        if capture_output:
            if e.stdout:
                print(f"Stdout: {e.stdout}")
            if e.stderr:
                print(f"Stderr: {e.stderr}")
        return False

def create_pages_branch(temp_dir):
    """Create orphan pages branch and copy files"""
    temp_path = pathlib.Path(temp_dir)

    # Initialize git repo in temp directory
    if not run_command(["git", "init"], cwd=temp_path, description="Initializing temporary git repository"):
        return False

    # Copy release files
    repo_dir = pathlib.Path(os.path.join(__file__))

    # release_dir = repo_dir / "release"

    # linux_exe = release_dir / "FullCrisis3.linux.x64"
    # windows_exe = release_dir / "FullCrisis3.win.x64.exe"

    # shutil.copy2(linux_exe, temp_path / "FullCrisis3.linux.x64")
    # shutil.copy2(windows_exe, temp_path / "FullCrisis3.win.x64.exe")

    # linux_icon_png = repo_dir / "graphics" / "linux-icon.png"
    # windows_icon_png = repo_dir / "graphics" / "windows-icon.png"

    # shutil.copy2(linux_icon_png, temp_path / "linux-icon.png")
    # shutil.copy2(windows_icon_png, temp_path / "windows-icon.png")

    # letter_gothic_ttf = repo_dir / "thirdparty-assets" / "fonts" / "Letter-Gothic.ttf"
    # rockwell_ttf = repo_dir / "thirdparty-assets" / "fonts" / "Rockwell.ttf"

    # shutil.copy2(letter_gothic_ttf, temp_path / "Letter-Gothic.ttf")
    # shutil.copy2(rockwell_ttf, temp_path / "Rockwell.ttf")

    # Create the CNAME file, used by github itself for custom domains
    with open(temp_path / "CNAME", 'w') as fd:
        fd.write('weverywhere.jmcateer.com\n')

    # Create index.html
    index_file = temp_path / "index.html"
    with open(index_file, 'w', encoding='utf-8') as f:
        html_content = INDEX_HTML_TEMPLATE
        f.write(html_content)

    print("SUCCESS: Files copied to temporary directory")

    if 'preview' in sys.argv:
        webbrowser.open(f'file:///{str(index_file)}')
        input(f'Pausing to allow user to inspect page at {index_file}')
        input('Press enter to continue...')
    else:
        print('Pushing directly to remote because "preview" not passed as an argument')

    # Add and commit files
    if not run_command(["git", "add", "."], cwd=temp_path, description="Adding files to git"):
        return False

    if not run_command(["git", "commit", "-m", "Release files"], cwd=temp_path, description="Creating initial commit"):
        return False

    print("SUCCESS: Initial commit created")
    return True

def push_to_pages_branch(temp_dir):
    """Push the pages branch to origin"""
    temp_path = pathlib.Path(temp_dir)

    # Get the current repository's remote URL
    result = subprocess.run(["git", "remote", "get-url", "origin"], capture_output=True, text=True)
    if result.returncode != 0:
        print("ERROR: Could not get origin remote URL")
        return False

    remote_url = result.stdout.strip()
    print(f"Remote URL: {remote_url}")

    # Add remote to temp repo
    if not run_command(["git", "remote", "add", "origin", remote_url], cwd=temp_path, description="Adding remote origin"):
        return False

    # Force push to pages branch (this will overwrite any existing pages branch)
    if not run_command(["git", "push", "-f", "origin", "HEAD:pages"], cwd=temp_path, description="Force pushing to pages branch"):
        return False

    print("SUCCESS: Pages branch published")
    return True

git_repo = os.path.dirname(__file__)
os.chdir(git_repo)

last_commit_sha, last_commit_msg = get_last_commit_sha_message(git_repo)

git_remote_origin_url = subprocess.check_output(['git', 'remote', 'get-url', 'origin']).decode('utf-8').strip()

open_preview = any('preview' in arg for arg in sys.argv)
noninteractive = any('noninteractive' in arg for arg in sys.argv)

version = '0.0.0'
if os.path.exists(os.path.join(git_repo, 'Cargo.toml')):
  with open(os.path.join(git_repo, 'Cargo.toml'), 'rb') as fd:
      data = tomllib.load(fd)
      version = data["package"]["version"]

# Printed above download links in monospace
build_message = ' '.join([
  'Version', version, 'built at', datetime.datetime.now().strftime('%Y-%m-%d %H:%M'),
  'by', getpass.getuser(),
  'on', socket.gethostname(),

  '\nfrom commit', last_commit_sha, 'with the message:',

  '\n'+wrap_text(last_commit_msg)
])

with tempfile.TemporaryDirectory(prefix='weverywhere-github-pages') as temp_dir:
  print(f'Building pages for {git_repo} at {temp_dir}')

  # Create pages branch content
  if not create_pages_branch(temp_dir):
      print("ERROR: Failed to create pages branch content")
      sys.exit(1)

  # Push to remote pages branch
  if not push_to_pages_branch(temp_dir):
      print("ERROR: Failed to push pages branch")
      sys.exit(1)



