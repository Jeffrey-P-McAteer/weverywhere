#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "zstandard",
# ]
# ///

import os
import sys
import shutil
import subprocess
import pathlib
import urllib.request
import re
import tarfile
import io
import json
import re

import zstandard

required_bins = ['cargo', 'zig', 'git']

if 'RUSTFLAGS' in os.environ:
  os.environ.pop('RUSTFLAGS')

for b in required_bins:
  if not shutil.which(b):
    print(f'[ Fatal Error] required binary "{b}" does not exist. Install and re-run.')
    sys.exit(1)

if not shutil.which('cargo-zigbuild'):
  yn = input(f'Need cargo-zigbuild[.exe] installed, ok to install? ').strip().lower()
  if not (yn[:1] in ('y', '1', 't') ):
    print(f'[ Fatal Error] Cannot install cargo-zigbuild, exiting.')
    sys.exit(1)
  subprocess.run([
    'cargo', 'install', '--locked', 'cargo-zigbuild'
  ], check=True)

def fetch_json(url):
    with urllib.request.urlopen(url) as resp:
        return json.load(resp)

def clone_macos_sdks(sdk_cache_dir):
    """Clone the macOS SDK repository if it doesn't exist."""
    if os.path.exists(os.path.join(sdk_cache_dir, '.git')):
        if not os.path.exists('/tmp/zig-build-all-targets-checked-macos'): # Check Flag file
          print(f"SDK cache directory already exists at: {sdk_cache_dir}")
          print("Pulling latest changes...")
          try:
              subprocess.run(["git", "pull"], cwd=str(sdk_cache_dir))
          except subprocess.CalledProcessError as e:
              print(f"Warning: git pull failed: {e}")
              print("Continuing with existing SDKs...")
          with open('/tmp/zig-build-all-targets-checked-macos', 'w') as fd: # Set Flag file
            fd.write('Done!')
    else:
        print(f"Cloning macOS SDKs to: {sdk_cache_dir}")
        # sdk_cache_dir.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run([
            "git", "clone",
            "https://github.com/alexey-lysiuk/macos-sdk.git",
            str(sdk_cache_dir)
        ])


def find_sdk_directories(sdk_cache_dir):
    sdk_pattern = re.compile(r'MacOSX(\d+)\.(\d+)(?:\.(\d+))?\.sdk')
    sdks = []
    for item in pathlib.Path(sdk_cache_dir).iterdir():
        if item.is_dir():
            match = sdk_pattern.match(item.name)
            if match:
                major = int(match.group(1))
                minor = int(match.group(2))
                patch = int(match.group(3)) if match.group(3) else 0
                version = (major, minor, patch)
                sdks.append((version, item))
    return sdks

def get_most_recent_sdk(sdk_cache_dir):
    sdks = find_sdk_directories(sdk_cache_dir)
    if not sdks:
        print("No SDK directories found!")
        return None
    # Sort by version tuple (newest first)
    sdks.sort(reverse=True, key=lambda x: x[0])
    # Print all found SDKs
    # print("\nFound SDKs:")
    # for version, path in sdks:
    #     version_str = f"{version[0]}.{version[1]}.{version[2]}" if version[2] else f"{version[0]}.{version[1]}"
    #     print(f"  - macOS {version_str}: {path.name}")
    most_recent_version, most_recent_path = sdks[0]
    version_str = f"{most_recent_version[0]}.{most_recent_version[1]}.{most_recent_version[2]}" if most_recent_version[2] else f"{most_recent_version[0]}.{most_recent_version[1]}"
    #print(f"\nUsing most recent SDK: macOS {version_str} at {most_recent_path}")
    return most_recent_path



# (Assuming exec on x86_64 linux system)
# We must manually download a copy of the mingw build tools which zig calls out to - https://packages.msys2.org/packages/mingw-w64-cross-mingw64-binutils?variant=x86_64

if True or not shutil.which('x86_64-w64-mingw32-dlltool'): # Always use the downloaded copy!
  mingw32_tools_folder = os.path.join(pathlib.Path.home(), '.cache', 'mingw32_tools_folder')
  os.makedirs(mingw32_tools_folder, exist_ok=True)
  print(f'x86_64-w64-mingw32-dlltool not found, loading from {mingw32_tools_folder}')
  os.environ['PATH'] = os.pathsep.join([
      mingw32_tools_folder,
      os.path.join(mingw32_tools_folder, 'bin'),
      os.path.join(mingw32_tools_folder, 'usr', 'bin'),
      os.path.join(mingw32_tools_folder, 'opt', 'bin'),
      os.path.join(mingw32_tools_folder, 'opt', 'x86_64-w64-mingw32', 'bin'),
      os.environ['PATH'],
  ])
  if not shutil.which('x86_64-w64-mingw32-dlltool'):
    # with urllib.request.urlopen('https://packages.msys2.org/packages/mingw-w64-cross-mingw64-binutils?variant=x86_64') as response:
    #   html = response.read().decode()
    # matches = re.findall(r'https://[^>]*mingw64.binutils[^>]*.x86_64.pkg.tar.zst', html, flags=re.MULTILINE)
    # first_link = matches[0]
    first_link = 'https://archlinux.org/packages/extra/x86_64/mingw-w64-binutils/download/'
    print(f'Downloading {first_link}')
    with urllib.request.urlopen(first_link) as response:
      zst_data = response.read()
    dctx = zstandard.ZstdDecompressor()
    #decompressed = dctx.decompress(zst_data)
    zst_stream = io.BytesIO(zst_data)
    with dctx.stream_reader(zst_stream) as reader:
      decompressed_tar_data = io.BytesIO(reader.read())
    #tar_buffer = io.BytesIO(decompressed_tar_data)
    with tarfile.open(fileobj=decompressed_tar_data, mode='r:') as tar:
      tar.extractall(path=mingw32_tools_folder)

print('Using binary', shutil.which('x86_64-w64-mingw32-dlltool'))

macos_sdk_folder = os.path.join(pathlib.Path.home(), '.cache', 'macos_sdk_folder')
os.makedirs(macos_sdk_folder, exist_ok=True)
clone_macos_sdks(macos_sdk_folder)
macos_sdk_path = get_most_recent_sdk(macos_sdk_folder)
macos_framework_path = os.path.join(macos_sdk_path, "System", "Library", "Frameworks")
macos_lib_path = os.path.join(macos_sdk_path, "usr", "lib")
# Construct the linker flags for Zig
# -isysroot tells Zig where the SDK root is
# -F adds framework search paths
# -L adds library search paths
zig_link_args = [
    f"-C link-arg=-isysroot", f"-C link-arg={macos_sdk_path}",
    f"-C link-arg=-F{macos_framework_path}",
    f"-C link-arg=-L{macos_lib_path}",
]

os.environ['CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS'] = ' '.join(zig_link_args)
os.environ['CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS'] = ' '.join(zig_link_args)

print('Using MacOS SDK at ', macos_sdk_folder)


def find_target_binary(t):
  canidates = [
    os.path.join('target', t, 'release', 'weverywhere.exe'),
    os.path.join('target', t, 'release', 'weverywhere'),
  ]
  for c in canidates:
    if os.path.exists(c):
      return c
  raise Exception(f'Cannot find a binary for {t}')

print()

# rustc --print target-list
targets = [
  'x86_64-pc-windows-gnu',
  'x86_64-unknown-linux-gnu',
  'x86_64-apple-darwin',

  # TODO future R&D stuff
  'aarch64-pc-windows-gnullvm',
  'aarch64-unknown-linux-gnu',
  'aarch64-apple-darwin',
]

for t in targets:
  print(f'Building for "{t}"')
  subprocess.run([
    'cargo', 'zigbuild', '--release', '--target', f'{t}'
  ], check=True)
  out_bin_path = os.path.abspath(find_target_binary(t))
  print(f'[ Built ] {out_bin_path}')



