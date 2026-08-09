
# WEverywhere

`weverywhere` is a WASI program management tool supporting the execution of WASI binaries everywhere.

It supports the following capabilities:

 - [ ] List metadata about WASI binaries which you own/have as a file on your machine
 - [ ] List metadata about your current network(s), to include:
    - [x] What other machines are running `weverywhere` daemons? (see **Network discovery** below — `weverywhere netmap`)
    - [ ] What Libraries\*/Services\* are exposed by the machines on these networks?
 - [ ] Run a `weverywhere` Daemon which performs the following tasks:
    - [ ] Reads a configuration file allowing the host to specify: (likely `/etc/weverywhere/weverywhere.conf` and a /etc/weverywhere/weverywhere.d/\*.conf` included directory)
        - [ ] Resource Quotas: how many CPU cores / bytes of RAM / network traffic is allowed to be consumed in Total, by Signature Groups.
        - [ ] Signature Groups: lists of public keys which are trusted by the host for privileged hostoperations or different quotas (likely a `/etc/weverywhere/groups/<Group-Name>/*.pub-key.pem` directory)
    - [ ] Listens on ipv4 and ipv6 UDP multicast address+ports (TODO decide which) for Network Messages\*\*.
        - [ ] Within Quota limits, perform the requested tasks to include:
            - [ ] Return metadata (see above)
            - [ ] Return executable material (WASI modules and functions)
            - [ ] Execute WASI modules + functions
    - [ ] Listen on a Unix socket for Network Messages which are privledged and can control the server's operation as-if the config files had been modified and the server re-started.
    - [ ] The Unix Socket will also have an event-notification capability to list events from executing WASI programs to include the PKI details of requestors for data and execution events. This is designed to allow future cloud providers to bill customers for CPU/RAM/Resource usage with their own management programs.

A stark contrast to other scale-out platforms is the lack of any capability observation plans; the underlying implementation may contain some default WASI modules,
and these can be combined with a client sending their own WASI module to execute on the server to read free RAM/CPU/whatever, which can then return values to the client in whatever format the client is interested in.

High-level tools for observation will be constructed with these primitives.


# Libraries

Inactive components of the `weverywhere` network. These contain meta-data suitable to allow the
transmission of selected library executable material from host to host.

executable material is identified as WASI modules and functions. Because WASI functions declare their input types,
a primitive amount of type checking and input validity is possible when combining libraries and their functions.


# Services

Active components of the `weverywhere` network.


# Network Messages

At the point where a Service's function call graph reaches from one host to another,
Network Messages are required.

# Repository Design

This is a single rust binary.

 - Source code lives under `./src/*`
 - Example server configuration lies under `./etc/*` and is embedded into the binary; sample config may be extracted to your system with a sub-command (see `weverywhere --help` for details).
 - Build and packaging scripts live under `./scripts/*` and are self-contained `uv run` scripts.
 - Example WASMI programs are under `./example-programs/` and may be compiled with `uv run scripts/compile-example-programs.py` into `./target/example-programs/<NAME>.wasi`. The subset named in `./example-programs/embedded.list` is additionally compiled by `build.rs` and embedded into the binary (see *Embedded programs*).
 - `scripts/update-github-pages.py` does what it says on the tin, and is currently a big mess copied from another project.
 - `scripts/build.py` cross-compiles the rust code on a Linux x86_64 host to all six release targets (linux/windows/macos x64 and arm64) and stages the artifacts under `./dist/`.
 - `scripts/publish.py` builds (via `scripts/build.py`) and publishes a versioned GitHub release with all six platform artifacts attached. Run `uv run scripts/publish.py --init-creds` once to set up a token.

## Versioning

The build version is `YYYY.MM.<hours>` where `<hours>` is the whole hours elapsed since the start of the current month (UTC). It is derived purely from the clock — there is no `version.txt` and no hard-coded version string. `scripts/_version.py` is the single source of truth; `build.rs` bakes the same value into the binary (visible via `weverywhere --version`) via the `WEVERYWHERE_VERSION` environment variable so the git tag, the GitHub asset names, and the compiled-in version can never drift.

# Installing and running as a daemon

`weverywhere` can install and manage itself as a background daemon on Linux, macOS, and Windows.

1. Stage the binary and config templates onto a machine with `install-to`, which extracts the
   embedded `etc/` templates and copies the running binary into `<root>/bin`:

   ```bash
   weverywhere install-to /usr/local                       # linux/macos -> /usr/local/{bin,etc}
   weverywhere install-to "C:\Program Files\weverywhere"    # windows
   ```

   Override the sub-paths with `--install-etc` / `--install-bin` if needed.

2. Register and start the daemon (it runs `weverywhere serve` at boot). Run as root / Administrator:

   ```bash
   sudo /usr/local/bin/weverywhere daemon install
   ```

   The backend is systemd on Linux, launchd on macOS, and the Task Scheduler on Windows, but the
   lifecycle verbs are identical everywhere:

   ```bash
   weverywhere daemon status | start | stop | restart | uninstall
   ```

# Client mode

By default the client talks to the **local daemon** on this machine (a loopback unicast, so it works
on every platform without multicast on the LAN):

```bash
weverywhere run ./program.wasi
```

Pass `--fabric` to instead broadcast the request to the multicast fabric (the whole LAN):

```bash
weverywhere run --fabric ./program.wasi
```

# Network discovery

`weverywhere` has **no dedicated "who is out there" wire message**, and deliberately so. Discovery
is performed by *sending a program* onto the fabric — the same mechanism as any other work. This
keeps the wire protocol minimal and means future topology/telemetry needs can ship as different
programs without a protocol change.

```bash
# Draw a trust-annotated map of the whole multicast fabric:
weverywhere netmap

# Only ask the daemon on this machine (no LAN traffic):
weverywhere netmap --local

# Use your own discovery/observation program instead of the bundled one:
weverywhere netmap --program ./my-topology-probe.wasi
```

By default `netmap` uses the discovery program **embedded in the binary** (see *Embedded programs*
below), so a copied binary needs no external `.wasm` file. `--program` overrides it with your own.

How it works:

1. `netmap` signs and broadcasts the discovery program (`example-programs/network-map.c`, compiled
   and embedded at build time; overridable with `--program`) to the fabric. Multicast delivers it to
   every reachable Executor at once — that is the "jump to all servers".
2. Each Executor runs the program, which calls host imports to report what it knows *locally*:
    - `host::hostname(ptr, cap)` — the node's OS hostname.
    - `host::trusts_me()` — whether that node trusts **you** (the caller's signing key).
    - `host::peer_count()` / `host::peer_report(i, ptr, cap)` — the neighbours that node has
      **passively observed** (identities seen on inbound fabric traffic), each tagged with whether
      that node trusts the peer. This is observation, not a discovery protocol: Executors simply
      remember who has talked to them (see `Executor::note_peer`).
3. Each node prints one tab-separated report, which is forwarded back over the normal stdout path.
   `netmap` collects every report and renders a tree, annotating each node with `<3` (this host
   trusts you) or `x` (it does not), and each neighbour with `trusted` / `untrusted`.

Example:

```
weverywhere network map
  you = my-laptop
  legend:  <3 = this host trusts you    x = this host does NOT trust you

(you) my-laptop
|-- fileserver @ 192.168.1.10:2240   [<3]
|   `-- peer: my-laptop @ 192.168.1.5:41888  [trusted]  key:e680d5c4
`-- guest-pi @ 192.168.1.23:2240   [x]
    `-- peer: my-laptop @ 192.168.1.5:41888  [untrusted]  key:e680d5c4
```

To design a richer view (services exposed, load, RAM, multi-hop forwarding, ...), write a different
WASI program and hand it to `netmap --program` — no changes to `weverywhere` itself are required.

# Embedded programs

Selected example programs are compiled and **baked into the binary** at build time so that commands
which ship a bundled program (currently `netmap`) work with no external `.wasm` files — important
for fast, self-contained deployments where the binary is copied around.

- The set to embed is listed, one program stem per line, in `example-programs/embedded.list`
  (the single source of truth).
- `build.rs` compiles each with zig (honouring the source's `// COMPILE:` line) and generates the
  `EMBEDDED_PROGRAMS` table that `src/embedded_programs.rs` exposes. If zig is unavailable the build
  still succeeds — it just embeds nothing and emits a warning, and callers fall back as below.
- Resolution order for a bundled program: `--program <FILE>` override → embedded bytes → the on-disk
  compiled example (`target/example-programs/<name>.wasm`, a dev convenience).
- Dump every embedded program back to disk (e.g. to inspect with `weverywhere info` or pass via
  `--program`) with:

  ```bash
  weverywhere extract-programs ./out-dir
  ```

# Project-level Missing Pieces and TODOs

 - [ ] Binary signing
 - [ ] `update-github-pages.py` website rendering
    - [ ] icons for architectures needed
    - [ ] download buttons
    - [ ] documentation rendering would be great

# Vocabulary

`weverywhere` uses a lot of existing design ideas, but we narrow them to more specific terms
and try to re-use these terms in commands, configuration files, and documentation to avoid confusion.

 - Program
    - WASM or WASI binary which has a single indented entry point (`_start`)
 - Library
    - WASM binary which has many smaller entry points designed to be used by Programs
 - Executor / Server
    - Refers to a single machine running `weverywhere serve` as a long-running daemon which executes Programs passing through the Fabric.
 - Controller / Client
    - Refers to a single machine or originating service which issues a Program to be executed by the `weverywhere` Fabric.
 - Fabric
    - Refers to the set of connected Executors and Controllers. Typically this is synonymous with an IP network, but Executors may forward Programs and Messages as they see fit which breaks this analogy.
 - Message
    - Refers to serialized `weverywhere` structures

TODO document trust things once we see what topologies will be necessary



# Misc One-Liners for testing

```bash
openssl genpkey -algorithm ed25519 -out /tmp/weverywhere-test.pem


```







