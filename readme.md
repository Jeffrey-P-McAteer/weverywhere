
# WEverywhere

`weverywhere` is a WASI program management tool supporting the execution of WASI binaries everywhere.

It supports the following capabilities:

 - [ ] List metadata about WASI binaries which you own/have as a file on your machine
 - [ ] List metadata about your current network(s), to include:
    - [ ] What other machines are running `weverywhere` daemons?
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
 - Example WASMI programs are under `./example-programs/` and may be compiled with `uv run compile-example-programs.py` into `./target/example-programs/<NAME>.wasi`










