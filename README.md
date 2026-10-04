# jamuru

P2P Audio Communication for Musicians

Low-latency peer-to-peer audio communication application for macOS, Windows, and Linux.

## Documentation

- [Storybook](https://koedame.github.io/jamuru-client/) - UI component library and design system

### In-Repository Development Docs

- [docs/](./docs/README.md) - Installation, quick start, troubleshooting, privacy, and development guides
- [docs-spec/](./docs-spec/README.md) - Specifications (single source of truth for the implementation; ADRs are append-only)
- [AGENTS.md](./AGENTS.md) - Development guide: workflow, commands, and the development rules index (commit conventions, secrets policy, etc.)
- [Plans.md](./Plans.md) - Task tracking

## Features

- Cross-platform support (macOS, Windows, Linux)
- Low-latency audio streaming
- Peer-to-peer connection (no central server required for audio)
- Multiple audio codec support (Opus, PCM)

## Development

This section is for the copyright holder's development work. The [LICENSE](./LICENSE) does not permit anyone else to build or run the software from source; use the official binaries from GitHub Releases.

See [docs/development/](./docs/development/building.md) for detailed development guides.

### Quick Start

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Clone the repository
git clone https://github.com/koedame/jamuru-client.git
cd jamuru-client

# Build core library
cargo build

# Run tests
cargo test
```

### Running the Desktop App (Tauri)

```bash
# Run from project root (not src-tauri directory)

# Development mode (frontend server starts automatically)
cargo tauri dev

# Production build (pass the server the app asks for its signaling server)
JAMJAM_SERVER_URL=https://<jamuru server> cargo tauri build
```

The built application will be in `src-tauri/target/release/bundle/`.

## License

**Source Available License (Not Open Source)**

This software is released under a custom Source Available license. The source code is publicly available for transparency purposes, but usage is restricted:

- **Permitted**: Viewing source code, using official binaries
- **Not Permitted**: Modification, redistribution, providing it as a service to others, building from source, creating competing products

See [LICENSE](./LICENSE) for full terms and [THIRD_PARTY_LICENSES.md](./THIRD_PARTY_LICENSES.md) for third-party component licenses.

## Trademarks

ASIO is a trademark of Steinberg Media Technologies GmbH. jamuru is not affiliated with or endorsed by Steinberg. All other product names are the property of their respective owners.
