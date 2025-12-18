
// COMPILE: zig cc -Os -target wasm32-wasi -mexec-model=reactor THIS_FILE -o OUT_FILE


// TODO lotsa design work?

// Imported host function
__attribute__((import_module("host"), import_name("print")))
void host_print(const char* ptr, int len);

__attribute__((export_name("_start")))
void _start() {

  const char* out = "Hello World";
  unsigned int out_len = sizeof(out) - 1;

  host_print(out, out_len);

}
