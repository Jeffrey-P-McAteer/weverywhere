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

import zstandard

required_bins = ['cargo', 'zig']

for b in required_bins:
  if not shutil.which(b):
    print(f'[ Fatal Error] required binary "{b}" does not exist. Install and re-run.')
    sys.exit(1)

if not shutil.which('cargo-zigbuild'):
  yn = input(f'Need cargo-zigbuild[.exe] installed, ok to install?').strip().lower()
  if not (yn[:1] in ('y', '1', 't') ):
    print(f'[ Fatal Error] Cannot install cargo-zigbuild, exiting.')
    sys.exit(1)
  subprocess.run([
    'cargo', 'install', '--locked', 'cargo-zigbuild'
  ], check=True)

# (Assuming exec on linux system)
# We must manually download a copy of the mingw build tools which zig calls out to - https://packages.msys2.org/packages/mingw-w64-cross-mingw64-binutils?variant=x86_64

if not shutil.which('x86_64-w64-mingw32-dlltool'):
  mingw32_tools_folder = os.path.join(pathlib.Path.home(), '.cache', 'mingw32_tools_folder')
  os.makedirs(mingw32_tools_folder, exist_ok=True)
  print(f'x86_64-w64-mingw32-dlltool not found, loading from {mingw32_tools_folder}')
  os.environ['PATH'] = os.pathsep.join([
      os.environ['PATH'],
      mingw32_tools_folder,
      os.path.join(mingw32_tools_folder, 'bin'),
      os.path.join(mingw32_tools_folder, 'usr', 'bin'),
      os.path.join(mingw32_tools_folder, 'opt', 'bin'),
      os.path.join(mingw32_tools_folder, 'opt', 'x86_64-w64-mingw32', 'bin'),
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


def find_target_binary(t):
  canidates = [
    os.path.join('target', t, 'release', 'weverywhere.exe'),
    os.path.join('target', t, 'release', 'weverywhere'),
  ]
  for c in canidates:
    if os.path.exists(c):
      return c
  raise Exception(f'Cannot find a binary for {t}')


if 'RUSTFLAGS' in os.environ:
  os.environ.pop('RUSTFLAGS')

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



