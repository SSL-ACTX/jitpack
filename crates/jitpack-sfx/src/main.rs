use jitpack_core::*;
use rpassword::read_password;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::time::Instant;

// ── Terminal UI ────────────────────────────────────────────────────────────────

mod ui {
    use std::io::{self, Write};

    const R: &str = "\x1b[0m";
    const BOLD: &str = "\x1b[1m";
    const DIM: &str = "\x1b[2m";
    const CYAN: &str = "\x1b[36m";
    const GREEN: &str = "\x1b[32m";
    const YELLOW: &str = "\x1b[33m";
    const RED: &str = "\x1b[31m";

    fn color_enabled() -> bool {
        std::env::var("NO_COLOR").is_err()
            && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true)
    }

    pub fn banner(encrypted: bool) {
        println!();
        if color_enabled() {
            println!(
                "  {BOLD}{CYAN}⚡ JitPack SFX{R}  {DIM}·  Self-Extracting Archive{}{R}",
                if encrypted {
                    "  ·  🔒 Encrypted"
                } else {
                    ""
                }
            );
        } else {
            println!(
                "  ⚡ JitPack SFX  ·  Self-Extracting Archive{}",
                if encrypted { "  ·  Encrypted" } else { "" }
            );
        }
        println!();
    }

    pub fn info(label: &str, value: &str) {
        if color_enabled() {
            println!("     {DIM}{label:<12}{R}  {value}");
        } else {
            println!("     {label:<12}  {value}");
        }
    }

    pub fn step(label: &str, msg: &str) {
        if color_enabled() {
            println!("  {CYAN}→{R}  {BOLD}{label:<13}{R}  {msg}");
        } else {
            println!("  →  {label:<13}  {msg}");
        }
    }

    pub fn done(label: &str, msg: &str) {
        if color_enabled() {
            println!("  {GREEN}✓{R}  {BOLD}{label:<13}{R}  {msg}");
        } else {
            println!("  ✓  {label:<13}  {msg}");
        }
    }

    pub fn warn(msg: &str) {
        if color_enabled() {
            eprintln!("  {YELLOW}⚠{R}  {DIM}{msg}{R}");
        } else {
            eprintln!("  ⚠  {msg}");
        }
    }

    pub fn error(msg: &str) {
        if color_enabled() {
            eprintln!("\n  {RED}✗{R}  {BOLD}{msg}{R}");
        } else {
            eprintln!("\n  ✗  {msg}");
        }
    }

    pub fn progress(current: u64, total: u64) {
        let width = 28usize;
        let filled = if total == 0 {
            width
        } else {
            (current as usize * width / total as usize).min(width)
        };
        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(width - filled));
        if color_enabled() {
            print!("\r  {CYAN}↓{R}  {DIM}Block{R}  {CYAN}{bar}{R}  {DIM}{current}/{total}{R}   ");
        } else {
            print!("\r  ↓  Block  {bar}  {current}/{total}   ");
        }
        io::stdout().flush().unwrap();
    }

    pub fn nl() {
        println!();
    }
}

fn human(bytes: usize) -> String {
    if bytes < 1_024 {
        format!("{} B", bytes)
    } else if bytes < 1_048_576 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    }
}

fn elapsed(t: Instant) -> String {
    let ms = t.elapsed().as_millis();
    if ms < 1000 {
        format!("{}ms", ms)
    } else {
        format!("{:.1}s", t.elapsed().as_secs_f64())
    }
}

// ── FFI ───────────────────────────────────────────────────────────────────────

unsafe extern "C" {
    fn compile_and_run_jit(
        ir_ptr: *const u8,
        ir_len: u64,
        bitstream_ptr: *const u8,
        output_ptr: *mut u8,
        output_limit: u64,
        mtf_ptr: *mut u8,
    ) -> DecompressResult;
}

// ── Payload loading ───────────────────────────────────────────────────────────

fn load_payload() -> std::io::Result<Vec<u8>> {
    let exe_path = std::fs::read_link("/proc/self/exe").or_else(|_| std::env::current_exe())?;
    let mut exe_file = File::open(&exe_path)?;
    let file_len = exe_file.seek(SeekFrom::End(0))?;

    const TRAILER: u64 = 8;
    if file_len <= TRAILER {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Binary has no appended payload (stub only)",
        ));
    }

    exe_file.seek(SeekFrom::End(-(TRAILER as i64)))?;
    let mut offset_buf = [0u8; 8];
    exe_file.read_exact(&mut offset_buf)?;
    let payload_offset = u64::from_le_bytes(offset_buf);

    if payload_offset >= file_len - TRAILER {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Payload offset in trailer is invalid",
        ));
    }

    exe_file.seek(SeekFrom::Start(payload_offset))?;
    let mut magic = [0u8; 4];
    exe_file.read_exact(&mut magic)?;
    if &magic != b"\x7FJPF" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Embedded payload has invalid magic bytes",
        ));
    }

    let payload_len = (file_len - TRAILER - payload_offset) as usize;
    exe_file.seek(SeekFrom::Start(payload_offset))?;
    let mut payload = vec![0u8; payload_len];
    exe_file.read_exact(&mut payload)?;
    Ok(payload)
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() -> std::io::Result<()> {
    let payload = match load_payload() {
        Ok(p) => p,
        Err(e) => {
            ui::error(&e.to_string());
            eprintln!();
            eprintln!("  This binary is a JitPack SFX stub with no embedded payload.");
            eprintln!("  Create a self-extracting archive with:");
            eprintln!("    jitpack sfx-pack <input> <output_exe>");
            eprintln!();
            std::process::exit(1);
        }
    };

    let payload: &[u8] = &payload;

    if payload.len() < 56 || &payload[0..4] != b"\x7FJPF" {
        ui::error("Embedded payload is invalid or corrupted.");
        std::process::exit(1);
    }

    let version = u16::from_le_bytes(payload[4..6].try_into().unwrap());
    let target_isa = u16::from_le_bytes(payload[6..8].try_into().unwrap());
    let uncompressed = u64::from_le_bytes(payload[8..16].try_into().unwrap());
    let num_blocks = u64::from_le_bytes(payload[16..24].try_into().unwrap());
    let flags = u32::from_le_bytes(payload[24..28].try_into().unwrap());
    let salt = &payload[28..44];
    let meta_total_size = u64::from_le_bytes(payload[44..52].try_into().unwrap()) as usize;
    let encrypted = (flags & 1) != 0;

    ui::banner(encrypted);
    ui::info("Format", &format!("v{version}"));
    ui::info("Target", if target_isa == 1 { "AArch64" } else { "x86_64" });
    ui::info(
        "Size",
        &format!(
            "{}  ·  {} block{}",
            human(uncompressed as usize),
            num_blocks,
            if num_blocks == 1 { "" } else { "s" }
        ),
    );
    ui::nl();

    let key = if encrypted {
        eprint!("  Password: ");
        std::io::stderr().flush().unwrap();
        let pass = read_password().expect("Failed to read password");
        Some(derive_key(&pass, salt))
    } else {
        None
    };

    let meta_nonce: [u8; 24] = payload[56..80].try_into().unwrap();
    let meta_body_len = u32::from_le_bytes(payload[80..84].try_into().unwrap()) as usize;
    let meta_body = &payload[84..84 + meta_body_len];

    let meta_data = if let Some(ref k) = key {
        decrypt_data(meta_body, k, &meta_nonce)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
    } else {
        meta_body.to_vec()
    };

    let mut m = 0;
    let num_files = u32::from_le_bytes(meta_data[m..m + 4].try_into().unwrap());
    m += 4;
    let mut files = Vec::new();
    for _ in 0..num_files {
        let path_len = u16::from_le_bytes(meta_data[m..m + 2].try_into().unwrap()) as usize;
        m += 2;
        let path = String::from_utf8_lossy(&meta_data[m..m + path_len]).to_string();
        m += path_len;
        let size = u64::from_le_bytes(meta_data[m..m + 8].try_into().unwrap()) as usize;
        m += 8;
        files.push((path, size));
    }

    let t = Instant::now();
    let mut offset = 56 + meta_total_size;
    let mut final_output = Vec::with_capacity(uncompressed as usize);

    for block_idx in 0..num_blocks {
        ui::progress(block_idx + 1, num_blocks);

        let nonce: [u8; 24] = payload[offset..offset + 24].try_into().unwrap();
        let body_len =
            u32::from_le_bytes(payload[offset + 24..offset + 28].try_into().unwrap()) as usize;
        offset += 28;

        let prim = u32::from_le_bytes(payload[offset..offset + 4].try_into().unwrap()) as usize;
        let uncomp_len =
            u32::from_le_bytes(payload[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let ir_len =
            u32::from_le_bytes(payload[offset + 8..offset + 12].try_into().unwrap()) as usize;
        let bit_len =
            u32::from_le_bytes(payload[offset + 12..offset + 16].try_into().unwrap()) as usize;
        offset += 16;

        let encrypted_body = &payload[offset..offset + body_len];
        offset += body_len;

        let body = if let Some(ref k) = key {
            decrypt_data(encrypted_body, k, &nonce)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
        } else {
            encrypted_body.to_vec()
        };

        let tree_payload = &body[0..ir_len];
        let bitstream = &body[ir_len..ir_len + bit_len];

        let canonical_codes = get_canonical_codes(tree_payload);
        let root = build_canonical_tree(&canonical_codes);
        let mut ir_payload = Vec::new();
        ir_payload.push(OpCode::Jump as u8);
        ir_payload.extend_from_slice(&0u32.to_le_bytes());
        let mut leaf_jumps = Vec::new();
        let root_ip = emit_node(&root, &mut ir_payload, &mut leaf_jumps);
        ir_payload[1..5].copy_from_slice(&root_ip.to_le_bytes());
        for pos in leaf_jumps {
            ir_payload[pos..pos + 4].copy_from_slice(&root_ip.to_le_bytes());
        }

        let mut output_buffer = vec![0u8; uncomp_len];
        let mut mtf_state: Vec<u8> = (0..=255).collect();

        let result = unsafe {
            compile_and_run_jit(
                ir_payload.as_ptr(),
                ir_payload.len() as u64,
                bitstream.as_ptr(),
                output_buffer.as_mut_ptr(),
                uncomp_len as u64,
                mtf_state.as_mut_ptr(),
            )
        };

        if result.status_code == 0 {
            let final_data = inverse_bwt(&output_buffer[0..result.bytes_written as usize], prim);
            final_output.extend_from_slice(&final_data);
        } else {
            ui::error(&format!(
                "JIT fault on block {block_idx}: code {}",
                result.status_code
            ));
            std::process::exit(1);
        }
    }

    println!();
    let output_dir = "extracted";
    let mut current_pos = 0;
    for (path, size) in files {
        let full_path = std::path::Path::new(output_dir).join(&path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out_file = File::create(full_path)?;
        out_file.write_all(&final_output[current_pos..current_pos + size])?;
        current_pos += size;
    }

    ui::done(
        "Extracted",
        &format!(
            "{} file{}  →  {output_dir}/  ({})",
            num_files,
            if num_files == 1 { "" } else { "s" },
            elapsed(t)
        ),
    );
    ui::nl();
    Ok(())
}
