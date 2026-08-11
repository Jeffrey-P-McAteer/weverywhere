// COMPILE: zig cc -Os -target wasm32-wasi -mexec-model=reactor THIS_FILE -o OUT_FILE

// chat: the weverywhere interactive chat program. ONE program body with two roles, selected by the
// `mode` argument the launcher (or a replicated copy) passes:
//
//   * mode=ui      - the interactive role. Runs on the local `weverywhere chat` host with a terminal
//                    attached: draws the transcript, reads the keyboard, and on each entered line
//                    sends a copy of ITSELF (mode=deliver) onto the whole fabric via host::replicate.
//   * mode=deliver - the carrier role. Runs on every node that receives the broadcast copy; it reads
//                    the message text from its args and appends it to that node's message store via
//                    host::messages_push. The host stamps the VERIFIED sender name + public key, so a
//                    chat handle cannot be forged - impersonation is prevented by the signed identity
//                    on the request, which is exactly the trust the chat program relies on.
//
// This program does no I/O itself; message passing, the terminal, arguments, and self-replication are
// all host primitives. Freestanding (no libc) so it stays small.

// ---- host imports -----------------------------------------------------------------------------
__attribute__((import_module("host"), import_name("arg_map_get")))
int host_arg_map_get(const char* k, int kl, char* p, int cap);
// Fill [ptr, ptr+len) with cryptographically-secure random bytes (a WASI module has no entropy of its
// own); used to mint a per-message dedup nonce.
__attribute__((import_module("host"), import_name("random")))
int host_random(char* ptr, int len);
// Append a message: id = per-message nonce for dedup, text = body. Host stamps the verified sender.
__attribute__((import_module("host"), import_name("messages_push")))
long long host_messages_push(const char* id, int idlen, const char* text, int textlen);
__attribute__((import_module("host"), import_name("messages_read")))
int host_messages_read(long long after_seq, char* p, int cap);
__attribute__((import_module("host"), import_name("replicate")))
int host_replicate(int scope, const char* args, int len);

__attribute__((import_module("host"), import_name("tty_available")))
int host_tty_available(void);
__attribute__((import_module("host"), import_name("tty_size")))
int host_tty_size(char* p);
__attribute__((import_module("host"), import_name("tty_next_event")))
int host_tty_next_event(char* p, int cap, int timeout_ms);
__attribute__((import_module("host"), import_name("tty_print")))
int host_tty_print(const char* p, int n);
__attribute__((import_module("host"), import_name("tty_move")))
int host_tty_move(int col, int row);
__attribute__((import_module("host"), import_name("tty_clear")))
int host_tty_clear(void);
__attribute__((import_module("host"), import_name("tty_style")))
int host_tty_style(int fg, int bg, int attrs);
__attribute__((import_module("host"), import_name("tty_flush")))
int host_tty_flush(void);

// Event kind tags (mirror crate::tty::event_kind).
enum { EV_CHAR=1, EV_ENTER=2, EV_BACKSPACE=3, EV_LEFT=4, EV_RIGHT=5, EV_UP=6, EV_DOWN=7,
       EV_CTRL_C=8, EV_CTRL_D=9, EV_ESC=10, EV_RESIZE=11 };
// Message record keys (mirror crate::executor::message_keys).
enum { K_SEQ=1, K_NAME=2, K_PUBKEY=3, K_EPOCH=4, K_TEXT=5 };

static int slen(const char* s){ int n=0; while(s[n]) n++; return n; }
static void tprint(const char* s){ host_tty_print(s, slen(s)); }

// ---- small helpers ----------------------------------------------------------------------------
static char argbuf[1024];
static const char* HEX = "0123456789abcdef";

// A fresh 16-hex-char random nonce, used to deduplicate the several network copies of one message.
static char idhex[32];
static void gen_id(void) {
  unsigned char r[8];
  host_random((char*)r, 8);
  for (int i = 0; i < 8; i++) { idhex[2*i] = HEX[r[i] >> 4]; idhex[2*i+1] = HEX[r[i] & 0xf]; }
}

// Decode one CBOR head at p; returns header length, sets *major and *val. (Same decoder as the
// messages selftest; handles the uint/bytes/text/array/map items our records use.)
static int chead(const unsigned char* p, int* major, unsigned long long* val){
  unsigned char ib=p[0]; *major=ib>>5; int ai=ib&0x1f;
  if(ai<24){*val=ai;return 1;}
  if(ai==24){*val=p[1];return 2;}
  if(ai==25){*val=((unsigned long long)p[1]<<8)|p[2];return 3;}
  if(ai==26){*val=((unsigned long long)p[1]<<24)|((unsigned long long)p[2]<<16)|((unsigned long long)p[3]<<8)|p[4];return 5;}
  unsigned long long v=0; for(int i=0;i<8;i++) v=(v<<8)|p[1+i]; *val=v; return 9;
}

// ---- CBOR builder for replicate args ----------------------------------------------------------
static unsigned char cb[1024];
static int cbn;
static void cb_b(unsigned char x){ if(cbn<(int)sizeof(cb)) cb[cbn++]=x; }
static void cb_text(const char* s, int n){
  if(n<24) cb_b(0x60|(unsigned char)n);
  else if(n<256){ cb_b(0x78); cb_b((unsigned char)n); }
  else { cb_b(0x79); cb_b((unsigned char)(n>>8)); cb_b((unsigned char)n); }
  for(int i=0;i<n;i++) cb_b((unsigned char)s[i]);
}

// Broadcast the given text to the fabric by replicating a mode=deliver copy of ourselves. A fresh
// random id rides along so every node collapses the several copies of this one message into one.
static void send_line(const char* text, int tlen){
  gen_id();
  cbn=0;
  cb_b(0xa1);                 // map(1)
  cb_b(0x02);                 // key 2 (arg_map, flattened k,v)
  cb_b(0x86);                 // array(6)
  cb_text("mode",4);
  cb_text("deliver",7);
  cb_text("text",4);
  cb_text(text,tlen);
  cb_text("id",2);
  cb_text(idhex,16);
  host_replicate(0, (const char*)cb, cbn);
}

// ---- transcript state -------------------------------------------------------------------------
#define MAXLINES 256
#define LINEW 200
static char lines[MAXLINES][LINEW];
static int  line_len[MAXLINES];
static int  line_count = 0;               // total appended; storage index is (i % MAXLINES)

static void append_line(const char* s, int n){
  if(n>LINEW) n=LINEW;
  int slot = line_count % MAXLINES;
  for(int i=0;i<n;i++) lines[slot][i]=s[i];
  line_len[slot]=n;
  line_count++;
}

static unsigned char rbuf[16384];

// Read any new messages (seq > *after) and fold them into the transcript. Returns 1 if anything new.
static int poll_messages(long long* after){
  int n = host_messages_read(*after, (char*)rbuf, (int)sizeof(rbuf));
  if(n<=0) return 0;
  int pos=0, major; unsigned long long v;
  pos += chead(rbuf+pos,&major,&v);
  if(major!=4) return 0;
  int count=(int)v, got=0;
  for(int i=0;i<count;i++){
    pos += chead(rbuf+pos,&major,&v);      // map header
    int pairs=(int)v;
    long long seq=0; int nameP=-1,nameL=0,pkP=-1,pkL=0,textP=-1,textL=0;
    for(int j=0;j<pairs;j++){
      unsigned long long key; int km;
      pos += chead(rbuf+pos,&km,&key);
      int vm; unsigned long long vv;
      pos += chead(rbuf+pos,&vm,&vv);
      if(vm==0){ if(key==K_SEQ) seq=(long long)vv; /* epoch ignored */ }
      else {
        if(key==K_NAME){ nameP=pos; nameL=(int)vv; }
        else if(key==K_PUBKEY){ pkP=pos; pkL=(int)vv; }
        else if(key==K_TEXT){ textP=pos; textL=(int)vv; }
        pos += (int)vv;
      }
    }
    // Compose "name (pk8): text" into a single transcript line.
    char comp[LINEW]; int c=0;
    for(int k=0;k<nameL && c<LINEW-2;k++) comp[c++]=rbuf[nameP+k];
    if(pkP>=0 && pkL>0){
      if(c<LINEW-2) comp[c++]=' ';
      if(c<LINEW-2) comp[c++]='(';
      for(int k=0;k<4 && k<pkL && c<LINEW-2;k++){
        unsigned char b=rbuf[pkP+k];
        if(c<LINEW-1) comp[c++]=HEX[b>>4];
        if(c<LINEW-1) comp[c++]=HEX[b&0xf];
      }
      if(c<LINEW-2) comp[c++]=')';
    }
    if(c<LINEW-2){ comp[c++]=':'; comp[c++]=' '; }
    for(int k=0;k<textL && c<LINEW;k++) comp[c++]=rbuf[textP+k];
    append_line(comp,c);
    if(seq>*after) *after=seq;
    got=1;
  }
  return got;
}

// ---- rendering --------------------------------------------------------------------------------
static char input[1024];
static int  input_len = 0;

static void redraw(void){
  char sz[4]; int cols=80, rows=24;
  if(host_tty_size(sz)==4){
    cols = (unsigned char)sz[0] | ((unsigned char)sz[1]<<8);
    rows = (unsigned char)sz[2] | ((unsigned char)sz[3]<<8);
  }
  if(cols<10) cols=10; if(rows<4) rows=4;

  host_tty_clear();
  host_tty_move(0,0);
  host_tty_style(11,-1,1);                 // bright yellow, bold
  tprint("weverywhere chat");
  host_tty_style(-1,-1,0);
  tprint("  —  type a message, Enter to send, Ctrl-C to quit");

  int visible = rows-3;                     // rows 1..rows-2 for transcript
  if(visible<1) visible=1;
  int start = line_count - visible;
  if(start<0) start=0;
  int row=1;
  for(int i=start;i<line_count;i++){
    int slot=i%MAXLINES, n=line_len[slot];
    if(n>cols) n=cols;
    host_tty_move(0,row++);
    host_tty_print(lines[slot], n);
  }

  host_tty_move(0,rows-1);
  host_tty_style(10,-1,0);                  // green prompt
  tprint("> ");
  host_tty_style(-1,-1,0);
  int in=input_len; if(in>cols-2) in=cols-2;
  host_tty_print(input, in);
  host_tty_flush();
}

// ---- roles ------------------------------------------------------------------------------------
static char evbuf[16];

static void ui_loop(void){
  long long after=0;
  redraw();
  for(;;){
    int changed = poll_messages(&after);
    int e = host_tty_next_event(evbuf, (int)sizeof(evbuf), 120);
    if(e>0){
      unsigned char kind=evbuf[0];
      if(kind==EV_CTRL_C || kind==EV_CTRL_D || kind==EV_ESC) break;
      else if(kind==EV_CHAR){
        for(int i=1;i<e && input_len<(int)sizeof(input);i++) input[input_len++]=evbuf[i];
        changed=1;
      }
      else if(kind==EV_BACKSPACE){ if(input_len>0){ input_len--; changed=1; } }
      else if(kind==EV_ENTER){
        if(input_len>0){ send_line(input, input_len); input_len=0; changed=1; }
      }
      else if(kind==EV_RESIZE){ changed=1; }
    }
    if(changed) redraw();
  }
}

__attribute__((export_name("_start")))
void _start(void){
  int ml = host_arg_map_get("mode", 4, argbuf, (int)sizeof(argbuf));
  int is_deliver = (ml==7 && argbuf[0]=='d' && argbuf[1]=='e');
  if(is_deliver){
    int il = host_arg_map_get("id", 2, idhex, (int)sizeof(idhex));
    if(il<0) il=0;
    int tl = host_arg_map_get("text", 4, argbuf, (int)sizeof(argbuf));
    if(tl<0) tl=0;
    host_messages_push(idhex, il, argbuf, tl);
    return;
  }
  // ui role (default): requires a terminal.
  if(!host_tty_available()) return;
  ui_loop();
}
