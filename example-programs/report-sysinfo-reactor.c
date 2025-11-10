
// COMPILE: zig cc -Os -target wasm32-wasi -mexec-model=command THIS_FILE -o OUT_FILE

#include <stdio.h>

// TODO lotsa design work?

int main() {

  // TODO figure out host APIs
  printf("Some output! Hello WASM!\n");

  return 0;
}
