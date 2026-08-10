// COMPILE: zig cc -Os -target wasm32-wasi -mexec-model=reactor THIS_FILE -o OUT_FILE

// args-echo: prints back the program arguments it was launched with. A tiny demonstration (and test)
// of the host::arg_* imports: a program's behaviour is parameterised by a positional arg_list and a
// named arg_map, chosen by whoever launched (or replicated) it. Freestanding (no libc) so it stays
// tiny, like network-map.c; output goes back to the caller via host::print.

__attribute__((import_module("host"), import_name("print")))
void host_print(const char* ptr, int len);

// Number of positional arguments (arg_list).
__attribute__((import_module("host"), import_name("arg_len")))
int host_arg_len(void);
// Write positional argument `index` into [ptr, ptr+cap); returns bytes written, or -1 if out of range.
__attribute__((import_module("host"), import_name("arg_get")))
int host_arg_get(int index, char* ptr, int cap);
// Number of named arguments (arg_map key/value pairs).
__attribute__((import_module("host"), import_name("arg_map_len")))
int host_arg_map_len(void);
// Write the KEY of named argument `index` into [ptr, ptr+cap); returns bytes written, or -1.
__attribute__((import_module("host"), import_name("arg_map_key")))
int host_arg_map_key(int index, char* ptr, int cap);
// Look up a named argument by key; writes its value into [ptr, ptr+cap); returns bytes written, or -1.
__attribute__((import_module("host"), import_name("arg_map_get")))
int host_arg_map_get(const char* key_ptr, int key_len, char* ptr, int cap);

static int slen(const char* s) { int n = 0; while (s[n]) n++; return n; }
static void puts_(const char* s) { host_print(s, slen(s)); }

static char valbuf[512];
static char keybuf[256];

__attribute__((export_name("_start")))
void _start(void) {
  puts_("args-echo: positional args:\n");
  int n = host_arg_len();
  for (int i = 0; i < n; i++) {
    int l = host_arg_get(i, valbuf, (int)sizeof(valbuf));
    if (l < 0) l = 0;
    puts_("  - ");
    host_print(valbuf, l);
    puts_("\n");
  }

  puts_("args-echo: named args:\n");
  int m = host_arg_map_len();
  for (int i = 0; i < m; i++) {
    int kl = host_arg_map_key(i, keybuf, (int)sizeof(keybuf));
    if (kl < 0) kl = 0;
    int vl = host_arg_map_get(keybuf, kl, valbuf, (int)sizeof(valbuf));
    if (vl < 0) vl = 0;
    puts_("  ");
    host_print(keybuf, kl);
    puts_(" = ");
    host_print(valbuf, vl);
    puts_("\n");
  }
}
