# JitPack

**A Research Archive Format with JIT-Specialized Decompression**

[![Rust](https://img.shields.io/badge/Rust-2024-orange.svg)](https://www.rust-lang.org/)
[![Zig](https://img.shields.io/badge/Zig-required-f7a41d.svg)](https://ziglang.org/)
[![Status](https://img.shields.io/badge/Status-Experimental-red.svg)]()

> [!IMPORTANT]
> JitPack is an experimental archive toolchain. It executes generated native
> code while decoding archive blocks, and its format and implementation are
> still evolving. Do not use it for production data or as a security boundary.

---

JitPack is a Rust and Zig research project that explores archive decoding
specialized to each archive's canonical Huffman coding. The Rust workspace
handles compression, validated archive framing, encryption, and the command
line interface; the Zig runtime generates and runs host-native decode paths.

## Table of Contents

- [Research Objectives](#research-objectives)
- [Architecture](#architecture)
- [Documentation](#documentation)
- [Getting Started](#getting-started)
- [Command Interface](#command-interface)
- [Security Model](#security-model)
- [License](#license)

---

## Research Objectives

1. **Specialized decoding:** Explore replacing generic entropy-decoder loops
   with native branches generated for an archive's code tree.
2. **Portable archives:** Store archive data and coding metadata, not native
   machine code; generate host-specific code at extraction time.
3. **Defensive parsing:** Validate archive structure, resource limits, and file
   paths before decompression or filesystem writes.
4. **Authenticated encryption:** Support Argon2-derived keys and
   XChaCha20-Poly1305 encryption with archive headers and block parameters
   authenticated as associated data.

---

## Architecture

```mermaid
flowchart LR
    Input[Input files] --> CLI[Rust CLI]
    CLI --> Encoder[Block encoder\nBWT → MTF → RLE → Huffman]
    Encoder --> Archive[JPF v1 archive]
    Archive --> Parser[Checked Rust parser]
    Parser --> JIT[Zig JIT runtime]
    JIT --> Output[Extracted files / query results]
```

The workspace contains three crates:

- `jitpack-core` — compression primitives, archive framing, encryption, and
  shared validation.
- `jitpack-cli` — archive creation, extraction, querying, and SFX packaging.
- `jitpack-sfx` — self-extracting archive stub.

---

## Documentation

- [JPF v1 format specification](docs/format-v1.md)
- [Original design plan](docs/plan.md)

---

## Getting Started

### Prerequisites

- Rust 1.85.0+ (with Edition 2024 support)
- Zig 0.17.0-dev, available on `PATH`
- A supported Linux/Android `x86_64` or `aarch64` target

### Build

```bash
cargo build --release -p jitpack-cli
```

---

## Command Interface

```bash
# Create an archive
cargo run --release -p jitpack-cli -- compress <input...> <output.jpf>

# Create an encrypted archive
cargo run --release -p jitpack-cli -- compress <input...> <output.jpf> --password

# Extract an archive
cargo run --release -p jitpack-cli -- decompress <input.jpf> <output_dir>

# Search an archive
cargo run --release -p jitpack-cli -- query <input.jpf> <pattern>

# Build a self-extracting archive
cargo run --release -p jitpack-cli -- sfx-pack <input> <output_exe>
```

---

## Security Model

JPF v1 uses bounded parsing, validates metadata and block framing, blocks path
traversal during extraction, and authenticates encrypted metadata and block
parameters. The JIT remains experimental: malformed or hostile archives should
be treated with caution until the runtime has broader fuzzing and platform
testing coverage.

---

## License

Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0). See
[LICENSE](LICENSE).

---

<div align="center">

Built with 🦀 & ⚡ by [Seuriin](https://github.com/SSL-ACTX)

</div>
