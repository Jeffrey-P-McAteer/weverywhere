// COMPILE: zig cc -Os -target wasm32-wasi -mexec-model=reactor THIS_FILE -o OUT_FILE

// chat: the weverywhere interactive chat program. A SINGLE interactive role - there is no longer a
// "deliver" carrier copy, and the program never replicates itself. It:
//
//   * draws the transcript and reads the keyboard on the local `weverywhere chat` host, and
//   * on each entered line broadcasts a SIGNED message onto the fabric via host::messages_send.
//
// A message is NOT a copy of this program: host::messages_send transmits a small signed record (the
// host stamps + signs our VERIFIED identity, so a chat handle can't be forged), so we no longer ship
// the whole wasm binary per keystroke. The payload is a CBOR *list* - never a bare string - so one
// primitive carries structured data; a lone line of text is sent as a one-element list [ "text" ].
// A second, optional list element carries a small "kind" uint distinguishing system notices from
// ordinary chat: kind 0 (or absent) = chat, 1 = join, 2 = leave. On start we broadcast a join notice
// and on quit a leave notice, so peers see who comes and goes; both are rendered as "*** name ...".
// Received messages land in this node's message store (host verifies the signature first) and we read
// them back with host::messages_read, decoding each stored payload's first list element to display.
//
// This program does no I/O itself; message passing, the terminal, and arguments are host primitives.
// Freestanding (no libc) so it stays small.

// ---- host imports -----------------------------------------------------------------------------
__attribute__((import_module("host"), import_name("arg_map_get")))
int host_arg_map_get(const char* k, int kl, char* p, int cap);
// Broadcast a signed message: `p` is a CBOR list or map (NOT a bare string). The host mints a dedup
// nonce, signs it with our verified identity, and fans it out onto the fabric. Returns 0 on success.
__attribute__((import_module("host"), import_name("messages_send")))
int host_messages_send(const char* p, int n);
__attribute__((import_module("host"), import_name("messages_read")))
int host_messages_read(long long after_seq, char* p, int cap);

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
// Payload "kind" (the optional 2nd list element): a plain line vs. a join/leave system notice.
enum { KIND_CHAT=0, KIND_JOIN=1, KIND_LEAVE=2 };

static int slen(const char* s){ int n=0; while(s[n]) n++; return n; }
static void tprint(const char* s){ host_tty_print(s, slen(s)); }

// ---- small helpers ----------------------------------------------------------------------------
static const char* HEX = "0123456789abcdef";

// Decode one CBOR head at p; returns header length, sets *major and *val. (Handles the uint / bytes /
// text / array / map items our records use.)
static int chead(const unsigned char* p, int* major, unsigned long long* val){
  unsigned char ib=p[0]; *major=ib>>5; int ai=ib&0x1f;
  if(ai<24){*val=ai;return 1;}
  if(ai==24){*val=p[1];return 2;}
  if(ai==25){*val=((unsigned long long)p[1]<<8)|p[2];return 3;}
  if(ai==26){*val=((unsigned long long)p[1]<<24)|((unsigned long long)p[2]<<16)|((unsigned long long)p[3]<<8)|p[4];return 5;}
  unsigned long long v=0; for(int i=0;i<8;i++) v=(v<<8)|p[1+i]; *val=v; return 9;
}

// ---- CBOR builder for the outgoing message list -----------------------------------------------
static unsigned char cb[1024];
static int cbn;
static void cb_b(unsigned char x){ if(cbn<(int)sizeof(cb)) cb[cbn++]=x; }
static void cb_text(const char* s, int n){
  if(n<24) cb_b(0x60|(unsigned char)n);
  else if(n<256){ cb_b(0x78); cb_b((unsigned char)n); }
  else { cb_b(0x79); cb_b((unsigned char)(n>>8)); cb_b((unsigned char)n); }
  for(int i=0;i<n;i++) cb_b((unsigned char)s[i]);
}

// Broadcast a signed CBOR list [ "text" ] (chat) or [ "text", kind ] (join/leave). The host signs +
// fans it out; we never transmit a copy of the program. A fresh dedup nonce is minted host-side.
static void send_msg(const char* text, int tlen, int kind){
  cbn=0;
  if(kind==KIND_CHAT){
    cb_b(0x81);            // array(1): [ text ] - keeps plain chat back-compatible
    cb_text(text,tlen);   // element 0: the message text
  } else {
    cb_b(0x82);           // array(2): [ text, kind ] - a system notice
    cb_text(text,tlen);   // element 0: descriptive text (unused when a kind is present)
    cb_b((unsigned char)kind); // element 1: small uint kind (< 24, so a bare byte)
  }
  host_messages_send((const char*)cb, cbn);
}
static void send_line(const char* text, int tlen){ send_msg(text, tlen, KIND_CHAT); }
static void send_join(void){ send_msg("joined", 6, KIND_JOIN); }
static void send_leave(void){ send_msg("left", 4, KIND_LEAVE); }

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

// Append "name (pk8)" (sender identity) from the current record in rbuf into comp at *c.
static void put_ident(char* comp, int* c, int nameP, int nameL, int pkP, int pkL){
  for(int k=0;k<nameL && *c<LINEW-2;k++) comp[(*c)++]=rbuf[nameP+k];
  if(pkP>=0 && pkL>0){
    if(*c<LINEW-2) comp[(*c)++]=' ';
    if(*c<LINEW-2) comp[(*c)++]='(';
    for(int k=0;k<4 && k<pkL && *c<LINEW-2;k++){
      unsigned char b=rbuf[pkP+k];
      if(*c<LINEW-1) comp[(*c)++]=HEX[b>>4];
      if(*c<LINEW-1) comp[(*c)++]=HEX[b&0xf];
    }
    if(*c<LINEW-2) comp[(*c)++]=')';
  }
}

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
    // The stored TEXT is the sender's CBOR payload (a list). Display its first element (the text), and
    // read an optional 2nd element (a small uint "kind") marking a join/leave notice. If the payload
    // isn't a list-with-a-leading-string, fall back to showing the raw payload bytes.
    int dispP=textP, dispL=textL, kind=KIND_CHAT;
    if(textP>=0 && textL>=1){
      int pmaj; unsigned long long pv2;
      int h = chead(rbuf+textP,&pmaj,&pv2);
      if(pmaj==4 && pv2>=1 && h<textL){
        int emaj; unsigned long long ev;
        int eh = chead(rbuf+textP+h,&emaj,&ev);
        if((emaj==3 || emaj==2) && h+eh+(int)ev<=textL){ dispP=textP+h+eh; dispL=(int)ev; }
        // Element 1 (if present) is the kind uint, right after element 0.
        if(pv2>=2){
          int off = h+eh+(int)ev;
          if(off < textL){
            int kmaj; unsigned long long kv;
            chead(rbuf+textP+off,&kmaj,&kv);
            if(kmaj==0) kind=(int)kv;
          }
        }
      }
    }
    // Compose the transcript line: a system notice "*** name (pk8) joined/left the chat" for join/leave,
    // otherwise the ordinary "name (pk8): text".
    char comp[LINEW]; int c=0;
    if(kind==KIND_JOIN || kind==KIND_LEAVE){
      const char* pre="*** ";
      for(int k=0;pre[k] && c<LINEW;k++) comp[c++]=pre[k];
      put_ident(comp,&c,nameP,nameL,pkP,pkL);
      const char* verb=(kind==KIND_JOIN)?" joined the chat":" left the chat";
      for(int k=0;verb[k] && c<LINEW;k++) comp[c++]=verb[k];
    } else {
      put_ident(comp,&c,nameP,nameL,pkP,pkL);
      if(c<LINEW-2){ comp[c++]=':'; comp[c++]=' '; }
      for(int k=0;k<dispL && c<LINEW;k++) comp[c++]=rbuf[dispP+k];
    }
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

// ---- main loop --------------------------------------------------------------------------------
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
  // Interactive role only: requires a terminal. (Delivery is now a host primitive, not a program.)
  if(!host_tty_available()) return;
  send_join();     // announce our arrival to the fabric
  ui_loop();
  send_leave();    // announce our departure before the host tears the send sink down
}
