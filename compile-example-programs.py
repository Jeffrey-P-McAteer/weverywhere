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

REPO_DIR = os.path.dirname(__file__)

def compile_example_program(example_path, out_dir):
  print(f'[ compile ] {os.path.relpath(example_path, REPO_DIR)}')
  if example_path.endswith('.c'):
    pass
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


