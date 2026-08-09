
// COMPILE: zig cc -Os -target wasm32-wasi -mexec-model=reactor THIS_FILE -o OUT_FILE

// network-map: the weverywhere discovery program (recursive, signed, CBOR).
//
// weverywhere has NO "who is out there" wire message; discovery is done by sending THIS program onto
// the fabric. Each server runs it and returns a signed CBOR "node record" (a BasicReturnMap). The
// daemon then forwards the same program to that node's own peers and relays their records back up,
// rewriting the request UUID at each hop, so the origin receives one record per node and assembles a
// tree. Depth is bounded by a trust-based budget and a visited-set (keyed by identity pubkey), both
// enforced by the daemon.
//
// This program's job at each hop is just to describe THIS node:
//   * a signed attestation (host::signed_attestation) binding hostname+pubkey+timestamp, so the
//     caller can prove the record represents only this node;
//   * whether this node trusts its caller (host::trusts_me);
//   * this node's depth from the origin (host::depth) and its parent's pubkey (host::caller_pubkey);
//   * a freshly random UUID (seeded from host::random) handed to the daemon for onward forwarding.
//
// Output is a CBOR map with small integer keys (kept in sync with crate::discovery::record_keys):
//   1 => attestation (CBOR byte string)   2 => trusts_caller (uint 0/1)
//   3 => depth (uint)                      4 => parent pubkey (byte string)
//
// Freestanding (no libc/stdio) so the module stays tiny; the record must fit one UDP datagram.

// --- weverywhere host imports (module "host") -------------------------------------------------

// Fill [ptr, ptr+len) with cryptographically secure random bytes; returns bytes written.
__attribute__((import_module("host"), import_name("random")))
int host_random(char* ptr, int len);

// 1 if the server running us trusts the caller's key, else 0.
__attribute__((import_module("host"), import_name("trusts_me")))
int host_trusts_me(void);

// This node's hop distance from the origin (origin's direct responders report 1).
__attribute__((import_module("host"), import_name("depth")))
int host_depth(void);

// Write the caller's (parent's) identity pubkey into [ptr, ptr+cap); returns bytes written.
__attribute__((import_module("host"), import_name("caller_pubkey")))
int host_caller_pubkey(char* ptr, int cap);

// Write this node's signed CBOR attestation into [ptr, ptr+cap); returns bytes written, or -1 if
// this node has no identity key.
__attribute__((import_module("host"), import_name("signed_attestation")))
int host_signed_attestation(char* ptr, int cap);

// Hand our finished CBOR node record to the daemon (sent to the caller as a BasicReturnMap).
__attribute__((import_module("host"), import_name("return_map")))
int host_return_map(const char* ptr, int len);

// Tell the daemon which UUID to stamp on the sub-requests it forwards to our peers.
__attribute__((import_module("host"), import_name("set_forward_uuid")))
int host_set_forward_uuid(const char* ptr, int len);

// --- tiny freestanding CBOR builder -----------------------------------------------------------

static unsigned char out[16384];
static int out_len = 0;

static void put_byte(unsigned char b) {
  if (out_len < (int)sizeof(out)) out[out_len++] = b;
}

static void put_bytes(const unsigned char* src, int n) {
  for (int i = 0; i < n && out_len < (int)sizeof(out); i++) out[out_len++] = src[i];
}

// Write a CBOR type header: major type (0=uint, 2=bytes, 5=map) + argument value.
static void cbor_head(int major, unsigned long long val) {
  unsigned char mt = (unsigned char)(major << 5);
  if (val < 24) {
    put_byte(mt | (unsigned char)val);
  } else if (val < 256ULL) {
    put_byte(mt | 24); put_byte((unsigned char)val);
  } else if (val < 65536ULL) {
    put_byte(mt | 25); put_byte((unsigned char)(val >> 8)); put_byte((unsigned char)val);
  } else if (val < 4294967296ULL) {
    put_byte(mt | 26);
    put_byte((unsigned char)(val >> 24)); put_byte((unsigned char)(val >> 16));
    put_byte((unsigned char)(val >> 8));  put_byte((unsigned char)val);
  } else {
    put_byte(mt | 27);
    for (int s = 56; s >= 0; s -= 8) put_byte((unsigned char)(val >> s));
  }
}

static void cbor_uint(unsigned long long v) { cbor_head(0, v); }
static void cbor_bytes(const unsigned char* b, int n) { cbor_head(2, (unsigned long long)n); put_bytes(b, n); }

// Scratch buffers for host-provided byte strings.
static unsigned char attest[1024];
static unsigned char parent[64];
static unsigned char uuid[16];

__attribute__((export_name("_start")))
void _start(void) {
  // 1. Generate a fresh forwarding UUID from host entropy and hand it to the daemon.
  host_random((char*)uuid, (int)sizeof(uuid));
  host_set_forward_uuid((const char*)uuid, (int)sizeof(uuid));

  // 2. Gather this node's facts.
  int alen = host_signed_attestation((char*)attest, (int)sizeof(attest));
  if (alen < 0) alen = 0;                       // no identity key: empty attestation
  int trusts = host_trusts_me() ? 1 : 0;
  int depth = host_depth(); if (depth < 0) depth = 0;
  int plen = host_caller_pubkey((char*)parent, (int)sizeof(parent));
  if (plen < 0) plen = 0;

  // 3. Encode the CBOR node record: a 4-entry map with integer keys.
  cbor_head(5, 4);                              // map(4)
  cbor_uint(1); cbor_bytes(attest, alen);       // 1 => attestation
  cbor_uint(2); cbor_uint((unsigned)trusts);    // 2 => trusts_caller
  cbor_uint(3); cbor_uint((unsigned)depth);     // 3 => depth
  cbor_uint(4); cbor_bytes(parent, plen);       // 4 => parent pubkey

  host_return_map((const char*)out, out_len);
}
