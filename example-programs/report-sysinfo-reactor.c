
// COMPILE: zig cc -Os -target wasm32-wasi -mexec-model=command THIS_FILE -o OUT_FILE

#include <stdio.h>

// TODO lotsa design work?
__attribute__((import_module("host"), import_name("trusts_me")))
int host_trusts_me();

int main() {

  // TODO figure out host APIs
  printf("Some output! Hello WASM!\n");

  if (host_trusts_me()) {
    printf("We are a trusted program, yay!\n");
  }
  else {
    printf("Aww, we are not trusted. Calling some functions will not perform tasks.\n");
  }

  return 0;
}
