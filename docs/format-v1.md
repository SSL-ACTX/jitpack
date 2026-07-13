# JitPack archive format, version 1

This document specifies the byte format emitted and accepted by the current
JitPack implementation. All multi-byte integers are unsigned little-endian.

## Header (56 bytes)

| Offset | Size | Field |
| --- | ---: | --- |
| 0 | 4 | Magic: `7f 4a 50 46` (`\x7FJPF`) |
| 4 | 2 | Format version: `1` |
| 6 | 2 | Target ISA: `0` = x86_64, `1` = AArch64 |
| 8 | 8 | Total uncompressed size |
| 16 | 8 | Number of blocks |
| 24 | 4 | Flags; bit 0 means encrypted |
| 28 | 16 | Argon2 salt; zero when unencrypted |
| 44 | 8 | Metadata-envelope size, excluding the 56-byte header |
| 52 | 4 | Reserved; must be zero |

Unknown flag bits and unsupported versions must be rejected. The decoder also
requires the total of all block output sizes to equal the header's total size.

## Metadata envelope

The envelope immediately follows the header and is exactly the header's
metadata-envelope size.

| Offset | Size | Field |
| --- | ---: | --- |
| 0 | 24 | XChaCha20-Poly1305 nonce; zero when unencrypted |
| 24 | 4 | Metadata body length |
| 28 | variable | Metadata body |

The body is plaintext when bit 0 of flags is clear, otherwise it is an
XChaCha20-Poly1305 ciphertext including its 16-byte authentication tag. For an
encrypted metadata body, the complete 56-byte header is AEAD associated data.
Its plaintext form is a file table: a `u32` file count followed by `u16` path
byte length, UTF-8 path bytes, and `u64` file size for each entry. Paths are
archive relative paths; extractors must reject paths that escape their output
root.

## Block envelope and body

Each block has a 44-byte envelope followed by its body.

| Offset | Size | Field |
| --- | ---: | --- |
| 0 | 24 | XChaCha20-Poly1305 nonce; zero when unencrypted |
| 24 | 4 | Stored body length |
| 28 | 4 | BWT primary index |
| 32 | 4 | Uncompressed block size |
| 36 | 4 | Canonical Huffman code-length table size; currently exactly 259 |
| 40 | 4 | Huffman bitstream size |
| 44 | variable | Stored body |

The plaintext body concatenates the 259-byte canonical Huffman code-length
table and the bitstream. With encryption, the stored body is that plaintext
encrypted using the block nonce and includes the 16-byte tag. Its AEAD
associated data is the complete archive header plus the zero-based block index,
primary index, uncompressed size, code-length size, and bitstream size. A v1
block is at most 256 KiB uncompressed, and its primary index must be inside the
block.

## Compatibility

This is a versioned binary protocol. Any incompatible layout, compression
coding, or authenticated-data change requires a new format version. Readers
must not attempt best-effort decoding of unknown versions.
