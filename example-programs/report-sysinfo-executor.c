
// COMPILE: zig cc -Os -target wasm32-wasi -mexec-model=reactor THIS_FILE -o OUT_FILE


// TODO lotsa design work?

__attribute__((export_name("do_report")))
int do_report(char* out, unsigned int out_len) {

  if (out_len > 10) {
    out[0] = 'H';
    out[1] = 'e';
    out[2] = 'l';
    out[3] = 'l';
    out[4] = 'o';
    out[5] = ' ';
    out[6] = 'W';
    out[7] = 'o';
    out[8] = 'r';
    out[9] = 'l';
    out[10] = 'd';
    out[11] = '\0';
  }

  return 0;
}

__attribute__((export_name("_start")))
int _start(char* out, unsigned int out_len) {

  do_report(out, out_len);

  return 0;
}
