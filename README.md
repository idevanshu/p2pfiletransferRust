# Pushpak

Peer-to-peer file transfer built in Rust. Send files and folders directly between machines — **no port forwarding, no cloud, no accounts**.

Powered by [libp2p](https://libp2p.io/), pushpak handles NAT traversal automatically using hole punching (DCUtR) and relay fallback, so it works across the internet even when both peers are behind routers.

```
Sender                          Receiver
  |                                |
  |  1. listen + print peer addr   |
  |                                |
  |    2. dial sender address      |
  |<-------------------------------|
  |                                |
  |  3. stream file data           |
  |------------------------------->|
  |                                |
  |  [done]                        |
```

---

## Features

- **NAT traversal** — hole punching via DCUtR + relay fallback, no port forwarding needed
- **LAN discovery** — mDNS auto-discovers peers on the same network
- **End-to-end encrypted** — Noise protocol on every connection
- **Files and folders** — send a single file or an entire directory tree
- **Progress bar** — live speed, ETA, and byte counters
- **Low-end friendly** — single-threaded async runtime, ~6 MB binary, minimal memory
- **Safe** — path traversal protection, symlink skipping, premature EOF detection

---

## Prerequisites

- [Rust](https://rustup.rs/) 1.75+
- That's it. All dependencies are pulled via Cargo.

---

## Install

```bash
git clone https://github.com/idevanshu/p2pfiletransferRust.git
cd p2pfiletransferRust
cargo build --release
```

The binary is at `target/release/pushpak` (~6 MB, stripped).

---

## Quick Start

### Send a file

```bash
# Sender machine
pushpak -m send -f photo.jpg
```

Output:

```
──────────────────────────────────
  pushpak sender
  peer: 12D3KooWAbCdEf...
──────────────────────────────────
[listen] /ip4/192.168.1.5/tcp/43210/p2p/12D3KooWAbCdEf...
```

Copy the full `/ip4/.../p2p/...` address and give it to the receiver.

### Receive a file

```bash
# Receiver machine
pushpak -m receive -a /ip4/192.168.1.5/tcp/43210/p2p/12D3KooWAbCdEf...
```

The file is saved to the `received/` directory.

### Send a folder

```bash
pushpak -m send -f ./my-project/
```

The entire directory tree is transferred recursively.

---

## NAT Traversal

This is the main reason pushpak uses libp2p. When both peers are behind NAT (home routers, CGNAT), direct connections normally fail. Pushpak solves this automatically:

### How it works

```
  Sender (behind NAT)              Relay Server              Receiver (behind NAT)
       |                               |                            |
  1.   |-------- connect ------------->|                            |
  2.   |                               |<-------- connect ---------|
  3.   |                               |                            |
       |   relay reserves a circuit    |   receiver dials sender    |
       |         for sender            |   via relay circuit        |
  4.   |<========= relayed connection ============================>|
       |                               |                            |
  5.   |<---------- DCUtR hole punch (tries direct) -------------->|
       |                               |                            |
  6.   |<============ direct connection (if punch works) =========>|
```

1. Both peers connect to a relay server
2. Receiver dials the sender through the relay circuit
3. Data flows through the relay initially
4. DCUtR (Direct Connection Upgrade through Relay) attempts to hole-punch a direct path
5. If hole punching succeeds, data flows directly (fast). If not, relay continues to work (still functional)

### Using a relay

You need a libp2p relay server. You can run one yourself using the [rust-libp2p relay example](https://github.com/libp2p/rust-libp2p/tree/master/protocols/relay), or use any public libp2p relay.

**Sender (behind NAT):**

```bash
pushpak -m send -f data.zip -r /ip4/RELAY_IP/tcp/4001/p2p/RELAY_PEER_ID
```

The sender will print a circuit address like:

```
[listen] /ip4/RELAY_IP/tcp/4001/p2p/RELAY_PEER_ID/p2p-circuit/p2p/SENDER_PEER_ID
```

**Receiver (behind NAT):**

```bash
pushpak -m receive \
  -a /ip4/RELAY_IP/tcp/4001/p2p/RELAY_PEER_ID/p2p-circuit/p2p/SENDER_PEER_ID \
  -r /ip4/RELAY_IP/tcp/4001/p2p/RELAY_PEER_ID
```

### LAN (no relay needed)

On the same local network, pushpak uses mDNS to discover peers automatically. Just use the direct address printed by the sender — no relay flag needed.

---

## CLI Reference

```
pushpak [OPTIONS] --mode <mode>

Options:
  -m, --mode <mode>        send or receive
  -f, --file <file>        Path to file or folder (required for send)
  -a, --address <address>  Sender multiaddr (required for receive)
  -r, --relay <relay>      Relay server multiaddr for NAT traversal
  -p, --port <port>        TCP listen port, 0 = random [default: 0]
  -h, --help               Print help
  -V, --version            Print version
```

### Address format

Pushpak uses [multiaddr](https://multiformats.io/multiaddr/) format:

| Scenario | Address format |
|---|---|
| Direct (LAN or port-forwarded) | `/ip4/192.168.1.5/tcp/43210/p2p/12D3KooW...` |
| Via relay | `/ip4/RELAY_IP/tcp/4001/p2p/RELAY_ID/p2p-circuit/p2p/SENDER_ID` |

### Environment variables

| Variable | Effect |
|---|---|
| `RUST_LOG=debug` | Enable verbose libp2p debug logging |
| `RUST_LOG=info` | Show connection-level events |

---

## Wire Protocol

Pushpak uses a custom binary protocol (`/pushpak/transfer/1`) over libp2p streams.

### Single file

```
[1 byte]    mode = 0x00
[2 bytes]   filename length (u16 LE)
[N bytes]   filename (UTF-8)
[8 bytes]   file size (u64 LE)
[...]       file data in 256 KB chunks
```

### Directory

```
[1 byte]    mode = 0x01
[2 bytes]   folder name length (u16 LE)
[N bytes]   folder name (UTF-8)
[4 bytes]   file count (u32 LE)

For each file:
  [2 bytes]   relative path length (u16 LE)
  [N bytes]   relative path (UTF-8)
  [8 bytes]   file size (u64 LE)
  [...]       file data in 256 KB chunks
```

---

## Architecture

```
main()
 |
 +-- build_swarm()         Create libp2p swarm with all protocols
 |    |-- TCP + Noise      Encrypted transport
 |    |-- Yamux            Stream multiplexing
 |    |-- Relay client     NAT traversal via relay
 |    |-- DCUtR            Hole punching upgrade
 |    |-- mDNS             LAN peer discovery
 |    +-- Stream protocol  Custom file transfer streams
 |
 +-- send()                Sender mode
 |    |-- validate path
 |    |-- listen on TCP
 |    |-- connect to relay (optional)
 |    |-- accept transfer streams
 |    +-- send_payload()
 |         |-- send_name()    Write filename
 |         |-- send_file()    mmap + chunked write
 |         +-- collect_files() Iterative dir traversal
 |
 +-- receive()             Receiver mode
      |-- dial sender (with 30s timeout)
      |-- open stream (with 10s timeout)
      |-- recv_name()      Read filename
      |-- recv_file()      Chunked read + progress
      +-- safe_join()      Path traversal protection
```

### Key constants

| Constant | Value | Purpose |
|---|---|---|
| `BUF` | 256 KB | Chunk size for file I/O |
| `CONNECT_TIMEOUT` | 30s | Max wait for peer connection |
| `STREAM_TIMEOUT` | 10s | Max wait for stream negotiation |
| `IDLE_TIMEOUT` | 300s | Close idle connections after 5 min |
| `MAX_NAME` | 240 | Max filename component length |

---

## Security

| Threat | Protection |
|---|---|
| Eavesdropping | Noise protocol encrypts all connections |
| Path traversal (`../../etc/passwd`) | `safe_join()` strips `..`, `/`, root components; validates result stays under base dir |
| Symlink attacks | `collect_files()` skips symlinks entirely |
| Truncated transfers | `recv_file()` detects premature EOF and returns error with byte counts |
| Oversized filenames | Rejects names > 240 bytes on receive, > 65535 bytes on send |
| Control characters in filenames | Stripped on both send and receive |

---

## Performance

Optimized for speed on both fast and low-end hardware:

- **Single-threaded async** — `tokio::current_thread` runtime uses 1 OS thread, minimal memory overhead
- **Memory-mapped I/O** — sender uses `mmap` for zero-copy file reads; OS handles paging for large files
- **256 KB chunks** — balances syscall overhead vs memory usage
- **TCP_NODELAY** — disables Nagle's algorithm for low-latency sends
- **Buffered I/O** — `BufReader`/`BufWriter` on both sides reduce syscall count
- **Stripped release binary** — LTO + strip + codegen-units=1 produces ~6 MB binary

### Building for minimum size

```bash
cargo build --release
```

The `[profile.release]` in `Cargo.toml` already includes:

```toml
opt-level = 3
lto = "thin"
strip = true
codegen-units = 1
```

---

## Troubleshooting

### "connection timed out"

- Check that the sender is running and the address is correct
- If both peers are behind NAT, you need to use a relay (`-r` flag)
- Verify the sender's firewall isn't blocking the port

### "stream open timed out"

- The connection was established but the transfer protocol failed to negotiate
- Usually means a version mismatch — make sure both peers run the same version

### "address must end with /p2p/PEER_ID"

- The receiver address must include the sender's peer ID
- Copy the full address from the sender's output, including the `/p2p/12D3KooW...` part

### "transfer interrupted: got X/Y bytes"

- The sender disconnected mid-transfer
- Check network stability and try again

### No output from sender

- The sender waits silently for connections after printing the address
- Press Ctrl+C to shut down gracefully

### Debug logging

```bash
RUST_LOG=debug pushpak -m send -f file.txt
```

This shows all libp2p events including mDNS, relay, DCUtR negotiations.

---

## Comparison with Previous Version

| | v3 (raw TCP) | v5 (libp2p) |
|---|---|---|
| NAT traversal | Manual port forwarding or ngrok | Automatic (DCUtR + relay) |
| Encryption | None | Noise protocol |
| LAN discovery | Manual IP entry | mDNS auto-discovery |
| Port | Hardcoded 8000 | Random (or `-p PORT`) |
| Runtime | Multi-threaded tokio | Single-threaded (lighter) |
| Empty file handling | mmap crash (UB) | Safe (skipped) |
| Path traversal | Partial sanitization | Full protection |
| Symlinks | Followed (loop risk) | Skipped |
| EOF detection | Silent truncation | Error with byte counts |
| Ctrl+C | Abrupt kill | Graceful shutdown |
| Binary size | ~8 MB | ~6 MB (stripped + LTO) |

---

## License

MIT
