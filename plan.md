# RFC: JitPack File Format and JIT Decompression Engine Specification (JCDF-1)

* **Status:** Draft / Proposal
* **Target Stack:** Rust (Orchestration & IR Parsing), Zig (Low-Level Memory Allocations & Code Generation Assembler)
* **Extension:** `.jpf` (JitPack File)

---

## 1. Introduction & Design Goals

Traditional compression formats (such as Deflate/zlib or Zstandard) rely on a static interpreter loop inside the decompressor to parse trees, resolve back-references, and write literal bytes. This generic parser design introduces significant CPU branch misprediction overhead and constant memory dereferences.

**JitPack** shifts the decompression logic from a static loop to a dynamically compiled native machine-code sequence specialized *specifically* for the archived file's internal Huffman and LZ77 patterns. 

### Core Objectives:
* **Zero-Interpreter Overhead:** Compile data trees (Huffman/Shannon-Fano) directly into CPU hardware branches (`JNZ`, `JZ`).
* **Direct SIMD Register Operations:** Compile sliding window copies (LZ77) into unrolled, size-specific SIMD memory moves.
* **Memory Safety:** Enforce strict bounds-checking at compile-time within the JIT-emitted machine code to prevent arbitrary code execution vulnerabilities.

---

## 2. High-Level Architecture Flow

```
[ Compression Phase (Rust) ]
Source File ---> Analyze Layout ---> Generate Specialized JP-IR ---> Serialize to .jpf

[ Extraction Phase (Rust + Zig) ]
.jpf File ---> Parse JP-IR ---> JIT Compiler Engine ---> RX Memory (Zig) ---> Run ---> Output File
```

---

## 3. The Binary Format Specification (`.jpf`)

The layout of a `.jpf` file is designed for rapid parsing of metadata and the immediate streaming of the Intermediate Representation (IR) to the JIT compiler.

### Byte Layout

| Offset (Bytes) | Field Name | Type | Description |
| :--- | :--- | :--- | :--- |
| `0x00 - 0x03` | `MAGIC_BYTES` | `[4]u8` | `\x7F` `J` `P` `F` |
| `0x04 - 0x05` | `VERSION` | `u16` | Format version (e.g., `0x0001`) |
| `0x06 - 0x07` | `TARGET_ISA` | `u16` | target architecture (e.g., `0 = x86_64`, `1 = AArch64`) |
| `0x08 - 0x0F` | `UNCOMPRESSED_SIZE`| `u64` | Expected output buffer allocation size |
| `0x10 - 0x17` | `METADATA_SIZE` | `u64` | Byte size of the JSON/binary file metadata section |
| `0x18 - 0x1F` | `IR_PAYLOAD_SIZE` | `u64` | Byte size of the specialized IR instructions segment |
| `0x20 - 0x27` | `BITSTREAM_SIZE` | `u64` | Byte size of the raw compressed bitstream |
| `0x28 - 0x47` | `HASH_SHA256` | `[32]u8`| Integrity check hash of the original uncompressed file |
| `0x48 - ...` | `METADATA` | Variable | File paths, permissions, and directory structure |
| `... - ...` | `IR_PAYLOAD` | Variable | The array of JP-IR instructions defining the decompressor |
| `... - EOF` | `BITSTREAM` | Variable | The raw bitstream processed by the compiled machine code |

---

## 4. The Virtual Instruction Set (JP-IR)

Instead of compiling directly to machine code during compression, the compressor emits a specialized Intermediate Representation (**JP-IR**). This guarantees that the `.jpf` file remains platform-independent. The extractor’s JIT compiler translates this IR into host-native assembly (x86_64 or ARM64) at runtime.

The virtual machine operates on three logical pointers:
* `src_ptr`: Points to the input compressed bitstream.
* `dest_ptr`: Points to the allocated output buffer.
* `dest_end`: Points to the end of the output buffer (for hardware-enforced bounds checking).

### Instruction Set Architecture (ISA)

| Opcode (u8) | Instruction | Arguments | Description |
| :--- | :--- | :--- | :--- |
| `0x01` | `BRANCH_BIT` | `lbl_zero: u32, lbl_one: u32` | Read 1 bit from `src_ptr`. If 0, jump to IP offset `lbl_zero`, else jump to `lbl_one`. |
| `0x02` | `EMIT_LITERAL` | `val: u8` | Write byte `val` to `dest_ptr`, then increment `dest_ptr`. |
| `0x03` | `EMIT_RAW_BITS`| `bits: u8` | Read `bits` from `src_ptr` and write directly to `dest_ptr`. |
| `0x04` | `COPY_WINDOW` | `offset: u32, len: u32` | Copy `len` bytes from `dest_ptr - offset` to `dest_ptr` (LZ77 back-reference). |
| `0x05` | `LOOP_BEGIN` | `counter_reg: u8, count: u32` | Initialize a loop register with `count`. |
| `0x06` | `LOOP_END` | `counter_reg: u8, target: u32` | Decrement `counter_reg` and jump to `target` if non-zero. |
| `0x0F` | `TERMINATE` | None | Stop execution and return total decompressed bytes. |

---

## 5. Implementation Strategy: Rust & Zig Integration

This architecture divides tasks between Rust and Zig to utilize the strengths of both languages.

```
+-------------------------------------------------------------+
|                         RUST CLI                            |
|  - Parses command-line args                                 |
|  - Reads and decodes .jpf File Headers                      |
|  - Manages File I/O operations                              |
+------------------------------+------------------------------+
                               |
                        Passes JP-IR Payload
                               v
+-------------------------------------------------------------+
|                   ZIG JIT RUNTIME ENGINE                    |
|  - Allocates RX Memory via OS-specific Syscalls             |
|  - Implements lightweight, dependency-free Assembler        |
|  - Compiles JP-IR to native Machine Code (x86_64/AArch64)   |
|  - Executes JIT segment and returns control to Rust         |
+-------------------------------------------------------------+
```

### 1. Zig JIT Allocator & Assembler (Low-Level Core)
Zig handles platform-specific virtual memory manipulation. It creates the execution sandbox and compiles the bytecode.

```zig
// jit_engine.zig
const std = @import("std");
const mem = std.mem;

pub const JitBuffer = struct {
    ptr: [*]align(mem.page_size) u8,
    size: usize,

    pub fn init(size: usize) !JitBuffer {
        const rounded_size = mem.alignForward(usize, size, mem.page_size);
        
        // Allocate page-aligned memory as Readable/Writable
        const ptr = try std.os.mmap(
            null,
            rounded_size,
            std.os.PROT.READ | std.os.PROT.WRITE,
            std.os.MAP.PRIVATE | std.os.MAP.ANONYMOUS,
            -1,
            0
        );

        return JitBuffer{
            .ptr = ptr.ptr,
            .size = rounded_size,
        };
    }

    pub fn makeExecutable(self: *JitBuffer) !void {
        // Transition memory block from RW to RX
        try std.os.mprotect(
            self.ptr[0..self.size],
            std.os.PROT.READ | std.os.PROT.EXEC
        );
    }

    pub fn deinit(self: *JitBuffer) void {
        std.os.munmap(self.ptr[0..self.size]);
    }
};
```

### 2. Rust Code Generation & Parsing (Orchestrator)
Rust processes the raw bytecode and uses FFI to drive the Zig-backed JIT assembler.

```rust
// jit_compiler.rs
#[repr(C)]
pub struct DecompressResult {
    pub bytes_written: u64,
    pub status_code: u32,
}

extern "C" {
    // FFI call into our Zig JIT engine
    fn compile_and_run_jit(
        ir_ptr: *const u8,
        ir_len: u64,
        bitstream_ptr: *const u8,
        output_ptr: *mut u8,
        output_limit: u64,
    ) -> DecompressResult;
}

pub fn decompress_archive(ir_payload: &[u8], compressed_data: &[u8], output_size: usize) -> Vec<u8> {
    let mut out_buffer = vec![0u8; output_size];
    
    unsafe {
        let result = compile_and_run_jit(
            ir_payload.as_ptr(),
            ir_payload.len() as u64,
            compressed_data.as_ptr(),
            out_buffer.as_mut_ptr(),
            output_size as u64,
        );
        
        assert_eq!(result.status_code, 0, "Decompression engine encountered an execution fault.");
    }
    
    out_buffer
}
```

---

## 6. JIT Compilation Mapping (x86_64 Reference)

To maintain high performance, the compiler maps JP-IR operations directly to hardware registers.

### Register Mapping Convention (System V AMD64 ABI):
* `RDI` : Current `src_ptr` (Input Bitstream Reader)
* `RSI` : Current `dest_ptr` (Output Buffer Writer)
* `RDX` : `dest_end` (Strict boundary checking limit)
* `RCX` : Bit-shifter read buffer

### Example IR Transformation: `COPY_WINDOW`
The JP-IR instruction `COPY_WINDOW (offset=4, len=8)` represents copying an 8-byte sequence located 4 bytes backward in the history window. The JIT translates this into direct memory movement:

```assembly
; JP-IR Instruction: COPY_WINDOW 4, 8
; Safety Check: Verify dest_ptr + 8 <= dest_end
mov  r8, rsi
add  r8, 8
cmp  r8, rdx
ja   .out_of_bounds_error

; Perform specialized copy using direct pointer offsets
mov  r9, rsi
sub  r9, 4       ; r9 points to source window (dest_ptr - 4)
mov  r10, [r9]   ; Load 8 bytes (since len=8, we can do one QWORD move)
mov  [rsi], r10  ; Store 8 bytes directly into output
add  rsi, 8      ; Increment dest_ptr by 8
```

---

## 7. Security & Sandboxing Model

Executing dynamically generated machine code carries inherent risks. The JIT compiler must implement hard constraints:

1. **Strict Buffer Boundaries:** Prior to every write sequence (`EMIT` or `COPY_WINDOW`), the JIT-emitted code must perform hardware-level bounds checks against the maximum allocated boundary (`RDX`). If a boundary violation is detected, execution aborts immediately, preventing heap corruption.
2. **Deterministic Halt:** Infinite loops are mitigated by tracking loop bounds strictly inside native CPU registers rather than stack variables that could be manipulated.
3. **W^X Compliance:** The execution environment explicitly ensures memory pages are never concurrently writable and executable. The JIT engine populates the buffer in raw `RW` mode, and changes the page protections to `RX` before initiating execution.
