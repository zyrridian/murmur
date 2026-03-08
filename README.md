# Murmur

[![Build Status](https://github.com/zyrridian/murmur/workflows/Build%20and%20Release/badge.svg)](https://github.com/zyrridian/murmur/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Murmur is a decentralized, terminal-based P2P chat node written in Rust. It operates entirely without central servers, utilizing `libp2p` for mesh networking and message propagation.

## Overview

- **Networking:** `libp2p` (Gossipsub, mDNS)
- **Encryption:** Noise protocol
- **UI:** `ratatui` / `crossterm`
- **Runtime:** `tokio` asynchronous event loop

## Installation

### Pre-built Binaries
Download the latest standalone executable for your platform (Linux, macOS, Windows) from the [Releases](https://github.com/zyrridian/murmur/releases) page.

### Build from Source
Ensure you have the latest stable Rust toolchain installed, then run:

```bash
git clone [https://github.com/zyrridian/murmur.git](https://github.com/zyrridian/murmur.git)
cd murmur
cargo build --release
```
The compiled binary will be located at `target/release/cli`.

## Usage

Start the node by passing your desired display name as an argument:

```bash
./murmur <username>
```

Once running, the node will automatically discover and connect to other Murmur instances on the local network via mDNS. Type `/ip4/...` commands to dial external peers manually.

## Architecture

This project is structured as a Cargo workspace to decouple the core networking logic from the presentation layer:

- `crates/chat-core`: P2P networking engine, swarm configuration, and isolated event loops.
- `crates/chat-protocol`: Serialization and shared data definitions.
- `apps/cli`: The terminal user interface.

## Contributing
See [CONTRIBUTING.md](CONTRIBUTING.md) for architectural details and PR guidelines.

## License
Dual-licensed under MIT or Apache 2.0.