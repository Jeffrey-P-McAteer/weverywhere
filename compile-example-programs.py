#!/usr/bin/env -S uv run --script
# /// script
# dependencies = [
#
# ]
# ///

import os
import sys
import subprocess
import shutil
import traceback
import shlex

REPO_DIR = os.path.dirname(__file__)

def check_req_bins():
  req_bins = [
    ('zig', 'Zig performs compilation of C code to .wasm executable files.')
  ]
  missing_one = False
  for b, reason_txt in req_bins:
    if not shutil.which(b):
      print(f'The required program "{b}" is mising on your system! This is required because: {reason_txt}')
      missing_one = True
  if missing_one:
    raise Exception(f'At least one required program is missing! Ensure it is installed and on your PATH.')

def read_compile_command_from_c_source(c_source_file):
  # Default
  compile_cmd = [
    'zig', 'cc',
      '-target', 'wasm32-wasi',
      '-O2',
      '-o', 'OUT_FILE',
      'THIS_FILE'
  ]
  with open(c_source_file, 'r', encoding='UTF-8') as fd:
    while line := fd.readline():
      if line.startswith('// COMPILE:'):
        compile_cmd = shlex.split( line[len('// COMPILE:'):].strip() )
  return compile_cmd

def template_compile_command(cmd, template_dict):
  return [ template_dict.get(c, c) for c in cmd ]

def run_cmd(cmd):
  print(f'> {" ".join(cmd)}')
  subprocess.run(cmd, check=True)

def compile_example_program(example_path, out_dir):
  print(f'[ compile ] {os.path.relpath(example_path, REPO_DIR)}')

  example_path_stem = os.path.splitext(os.path.basename(example_path))[0] # name w/o extension
  compile_cmd_dict = {
    'THIS_FILE': example_path,
    'OUT_FILE': os.path.join(out_dir, f'{example_path_stem}.wasm'),
  }

  if example_path.endswith('.c'):
    bare_cmd = read_compile_command_from_c_source(example_path)
    cmd = template_compile_command(bare_cmd, compile_cmd_dict)
    run_cmd(cmd)
  else:
    raise Exception(f'[ compile_example_program ] We do not know how to compile the example program at {example_path}! Please add an implementation to compile_example_program().')


def main():
  example_programs = os.path.join(REPO_DIR, 'example-programs')
  example_programs_out = os.path.join(REPO_DIR, 'target', 'example-programs')

  os.makedirs(example_programs_out, exist_ok=True)

  for program_name in os.listdir(example_programs):
    program_path = os.path.join(example_programs, program_name)
    compile_example_program(program_path, example_programs_out)

  print(f'Done! Outputs are in {os.path.relpath(example_programs_out, REPO_DIR)}')

if __name__ == '__main__':
  main()


