
// COMPILE: zig cc -Os -target wasm32-wasi -mexec-model=reactor THIS_FILE -o OUT_FILE

// network-map: the weverywhere discovery program.
//
// weverywhere deliberately has NO "who is out there" wire message. Instead, discovery is done by
// sending THIS program onto the fabric: multicast delivers it to every reachable server at once,
// each server runs it, and each server reports back (over the normal stdout-forwarding path) the
// facts it knows locally -- its hostname, whether it trusts the caller, and the peers it has
// passively observed. The `weverywhere netmap` client collects every server's report and draws the
// network as a tree. Because topology data is produced by a program (not baked into the protocol),
// future needs -- richer telemetry, filtering, multi-hop forwarding -- can ship as different
// programs without any protocol change.
//
// This is intentionally freestanding (no libc/stdio) so the compiled module stays tiny; the whole
// ExecuteRequest that carries it must fit in a single UDP datagram.
//
// Output format (tab-separated, whole report emitted in a single host_print):
//   NODE\t<hostname>\t<trusts_me 0|1>\n
//   PEER\t<name>\t<addr>\t<trusted 0|1>\t<pubkey_hex>\n   (repeated per observed peer)

// --- weverywhere host imports (module "host") -------------------------------------------------

// Forward bytes back to the caller (arrives as this program's stdout).
__attribute__((import_module("host"), import_name("print")))
void host_print(const char* ptr, int len);

// 1 if the server running us trusts the caller's key, else 0.
__attribute__((import_module("host"), import_name("trusts_me")))
int host_trusts_me(void);

// Write this server's hostname into [ptr, ptr+cap); returns bytes written.
__attribute__((import_module("host"), import_name("hostname")))
int host_hostname(char* ptr, int cap);

// How many peers this server has passively observed on the fabric.
__attribute__((import_module("host"), import_name("peer_count")))
int host_peer_count(void);

// Write peer #index as "name\taddr\ttrusted(0|1)\tpubkey_hex" into [ptr, ptr+cap);
// returns bytes written, or -1 if index is out of range.
__attribute__((import_module("host"), import_name("peer_report")))
int host_peer_report(int index, char* ptr, int cap);

// --- tiny freestanding buffer builder ---------------------------------------------------------

static char out[16384];   // full report, sent in one datagram
static int  out_len = 0;

static void put_bytes(const char* src, int n) {
  for (int i = 0; i < n && out_len < (int)sizeof(out); i++) {
    out[out_len++] = src[i];
  }
}

static void put_cstr(const char* s) {
  int n = 0;
  while (s[n]) n++;
  put_bytes(s, n);
}

static void put_char(char c) {
  if (out_len < (int)sizeof(out)) out[out_len++] = c;
}

// Ask the host to fill a scratch buffer, then copy the returned bytes into `out`.
static char scratch[1024];
static void put_host_bytes(int n) {
  if (n < 0) n = 0;
  if (n > (int)sizeof(scratch)) n = (int)sizeof(scratch);
  put_bytes(scratch, n);
}

// --- report assembly --------------------------------------------------------------------------

__attribute__((export_name("_start")))
void _start(void) {
  // NODE line: NODE \t <hostname> \t <trusts_me> \n
  put_cstr("NODE\t");
  put_host_bytes(host_hostname(scratch, (int)sizeof(scratch)));
  put_char('\t');
  put_char(host_trusts_me() ? '1' : '0');
  put_char('\n');

  // One PEER line per observed neighbour. peer_report already emits the tab-separated fields.
  int n = host_peer_count();
  for (int i = 0; i < n; i++) {
    if (out_len >= (int)sizeof(out) - 1) break; // report full; stop cleanly
    int pl = host_peer_report(i, scratch, (int)sizeof(scratch));
    if (pl < 0) continue;                         // index vanished; skip
    put_cstr("PEER\t");
    put_host_bytes(pl);
    put_char('\n');
  }

  host_print(out, out_len);
}
