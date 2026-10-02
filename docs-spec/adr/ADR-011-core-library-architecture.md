# ADR-011: Core Library Architecture

## Status

Accepted

## Context

This project provides both CLI and GUI (Tauri) interfaces for P2P audio communication. There was a concern about code duplication between these interfaces.

Analysis revealed:
- Core logic (audio processing, network, protocol) is already shared via the `jamjam` crate
- Session orchestration code (~150 lines each) appears duplicated but has fundamentally different control flows

## Decision

**Maintain the current three-layer architecture:**

```
┌─────────────────────────────────────────────────────┐
│              Interface Layer (CLI / GUI)            │
│  - Session orchestration                            │
│  - User interaction handling                        │
│  - Interface-specific state management              │
└─────────────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────┐
│              Core Library (jamjam crate)            │
│  - audio: AudioEngine, device management, codec     │
│  - network: Connection, latency calculation         │
│  - protocol: Packet format                          │
└─────────────────────────────────────────────────────┘
```

**Do NOT abstract session orchestration into the core library** because:

1. **Control flow differs by interface:**
   - CLI: Synchronous, `Ctrl+C` termination, fixed configuration at startup
   - GUI: Async IPC, command channels, hot-swappable devices, real-time status updates

2. **Complexity cost exceeds benefit:**
   - Abstracting both patterns requires generic code handling all cases
   - Readability decreases as abstraction increases
   - Current duplication is manageable (~150 lines per interface)

3. **Two interfaces do not justify abstraction:**
   - If a third interface is added (e.g., web app), reconsider
   - Until then, maintain simplicity

## Guidelines

### What belongs in Core Library (`src/lib.rs`, `src/audio/`, `src/network/`, `src/protocol/`)

- Audio capture/playback engine
- Network connection management
- Packet encoding/decoding
- Latency calculation
- Device enumeration
- Codec implementation

### What belongs in Interface Layer (`src/main.rs`, `src-tauri/src/`)

- Session lifecycle orchestration (connect → setup → loop → cleanup)
- User input handling (CLI args, IPC commands)
- Output formatting (text for CLI, JSON for GUI)
- Interface-specific state management
- Error presentation to users

### When to move code to Core

Move code to Core Library when:
- The same logic is needed by 3+ interfaces
- The logic is pure computation with no interface-specific control flow
- The logic handles audio/network primitives

Keep code in Interface Layer when:
- Control flow depends on interface type (sync vs async, blocking vs non-blocking)
- State management is interface-specific
- Output format is interface-specific

## Consequences

### Benefits

- Clear separation of concerns
- Each interface can evolve independently
- Core library remains simple and focused
- Easy to understand for new contributors

### Drawbacks

- ~150 lines of similar (not identical) orchestration code in each interface
- Changes to session flow may need updates in multiple places

### Mitigations

- Document the pattern clearly (this ADR)
- When modifying session flow, check both CLI and GUI implementations
