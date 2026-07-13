use jitpack_core::*;

static SFX_STUB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sfx_stub"));

use rand_core::{OsRng, RngCore};
use rayon::prelude::*;
use rpassword::read_password;
use std::fs::File;
use std::io::{Read, Write};
use std::time::Instant;
use walkdir::WalkDir;

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
    const BLUE: &str = "\x1b[34m";

    fn color_enabled() -> bool {
        std::env::var("NO_COLOR").is_err()
            && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true)
    }

    macro_rules! c {
        ($code:expr, $text:expr) => {
            if color_enabled() {
                format!("{}{}{}", $code, $text, R)
            } else {
                $text.to_string()
            }
        };
    }

    pub fn banner() {
        println!();
        if color_enabled() {
            println!(
                "  {BOLD}{CYAN}⚡ JitPack{R}  {DIM}·{R}  JIT-Powered Archive  {DIM}v{}{R}",
                env!("CARGO_PKG_VERSION")
            );
        } else {
            println!(
                "  ⚡ JitPack  ·  JIT-Powered Archive  v{}",
                env!("CARGO_PKG_VERSION")
            );
        }
        println!();
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

    pub fn info(label: &str, value: &str) {
        if color_enabled() {
            println!("     {DIM}{label:<13}{R}  {value}");
        } else {
            println!("     {label:<13}  {value}");
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

    pub fn divider() {
        if color_enabled() {
            println!("  {DIM}  {}{R}", "─".repeat(44));
        } else {
            println!("     {}", "─".repeat(44));
        }
    }

    pub fn header(title: &str) {
        if color_enabled() {
            println!("  {BLUE}{BOLD}{title}{R}");
        } else {
            println!("  {title}");
        }
        divider();
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

    pub fn match_hit(filename: &str, offset: usize) {
        if color_enabled() {
            println!("  {GREEN}•{R}  {BOLD}{filename}{R}  {DIM}offset {offset}{R}");
        } else {
            println!("  •  {filename}  offset {offset}");
        }
    }

    pub fn nl() {
        println!();
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

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

// ── Main ───────────────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        return;
    }

    match args[1].as_str() {
        "compress" => {
            if args.len() < 4 {
                ui::error("Missing input or output path.");
                eprintln!("  Usage: jitpack compress <input…> <output.jpf> [--password]");
                return;
            }
            let mut inputs = Vec::new();
            let mut output = String::new();
            let mut password_enabled = false;

            let mut i = 2;
            while i < args.len() {
                if args[i] == "--password" {
                    password_enabled = true;
                } else if i == args.len() - 1
                    || (i == args.len() - 2 && args[args.len() - 1] == "--password")
                {
                    output = args[i].clone();
                } else {
                    inputs.push(args[i].clone());
                }
                i += 1;
            }

            if inputs.is_empty() || output.is_empty() {
                ui::error("Missing input or output path.");
                return;
            }

            let password = prompt_password(password_enabled);
            ui::banner();
            if let Err(e) = compress(&inputs, &output, password.as_deref()) {
                ui::error(&format!("Compression failed: {e}"));
                std::process::exit(1);
            }
        }

        "decompress" => {
            if args.len() < 4 {
                ui::error("Missing input or output path.");
                eprintln!("  Usage: jitpack decompress <input.jpf> <output_dir>");
                return;
            }
            ui::banner();
            if let Err(e) = decompress(&args[2], &args[3]) {
                ui::error(&format!("Decompression failed: {e}"));
                std::process::exit(1);
            }
        }

        "sfx-pack" => {
            if args.len() < 4 {
                ui::error("Missing input or output path.");
                eprintln!("  Usage: jitpack sfx-pack <input> <output_exe> [--password]");
                return;
            }
            let password_enabled = args.contains(&"--password".to_string());
            let password = prompt_password(password_enabled);
            ui::banner();
            if let Err(e) = sfx_pack(&args[2], &args[3], password.as_deref()) {
                ui::error(&format!("SFX creation failed: {e}"));
                std::process::exit(1);
            }
        }

        "query" => {
            if args.len() < 4 {
                ui::error("Missing input or pattern.");
                eprintln!("  Usage: jitpack query <input.jpf> <pattern>");
                return;
            }
            ui::banner();
            if let Err(e) = query(&args[2], &args[3]) {
                ui::error(&format!("Query failed: {e}"));
                std::process::exit(1);
            }
        }

        "help" | "--help" | "-h" => print_usage(),

        cmd => {
            ui::error(&format!("Unknown command '{cmd}'"));
            print_usage();
            std::process::exit(1);
        }
    }
}

fn prompt_password(enabled: bool) -> Option<String> {
    if enabled {
        eprint!("  Password: ");
        std::io::stderr().flush().unwrap();
        Some(read_password().expect("Failed to read password"))
    } else {
        None
    }
}

fn print_usage() {
    ui::banner();
    println!("  Usage:  jitpack <command> [args]");
    ui::nl();
    ui::header("Commands");
    println!("     compress      Compress files or directories into a .jpf archive");
    println!("     decompress    Extract a .jpf archive to a directory");
    println!("     sfx-pack      Bundle a .jpf into a self-extracting executable");
    println!("     query         Search for a pattern inside an archive");
    println!("     help          Show this message");
    ui::nl();
    ui::header("Options");
    println!("     --password    Encrypt with Argon2 + XChaCha20-Poly1305");
    ui::nl();
}

// ── sfx-pack ──────────────────────────────────────────────────────────────────

/// Output layout:
///   [ jitpack-sfx stub (embedded at compile time) ] [ .jpf payload ] [ 8-byte LE offset ]
fn sfx_pack(input_path: &str, output_exe: &str, password: Option<&str>) -> std::io::Result<()> {
    let temp_jpf = format!("{output_exe}.tmp.jpf");

    ui::step("Compressing", input_path);
    let t = Instant::now();
    compress(&[input_path.to_string()], &temp_jpf, password)?;

    let payload_bytes = std::fs::read(&temp_jpf)?;
    let payload_offset: u64 = SFX_STUB.len() as u64;

    ui::step("Assembling", output_exe);
    let mut out_file = File::create(output_exe)?;
    out_file.write_all(SFX_STUB)?;
    out_file.write_all(&payload_bytes)?;
    out_file.write_all(&payload_offset.to_le_bytes())?;
    drop(out_file);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(output_exe, std::fs::Permissions::from_mode(0o755))?;
    }

    if let Err(e) = std::fs::remove_file(&temp_jpf) {
        ui::warn(&format!("Could not remove temp file: {e}"));
    }

    ui::done(
        "Created",
        &format!(
            "{output_exe}  ({})",
            human(SFX_STUB.len() + payload_bytes.len() + 8)
        ),
    );
    ui::info("Stub", &human(SFX_STUB.len()));
    ui::info("Payload", &human(payload_bytes.len()));
    ui::info("Time", &elapsed(t));
    ui::nl();
    Ok(())
}

// ── compress ──────────────────────────────────────────────────────────────────

fn compress(
    input_paths: &[String],
    output_path: &str,
    password: Option<&str>,
) -> std::io::Result<()> {
    let mut all_files = Vec::new();
    for path in input_paths {
        for entry in WalkDir::new(path) {
            let entry = entry.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            if entry.file_type().is_file() {
                all_files.push(entry.path().to_owned());
            }
        }
    }
    all_files.sort();

    if all_files.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No files found",
        ));
    }

    let mut input_data = Vec::new();
    let mut metadata = Vec::new();
    metadata.extend_from_slice(&(all_files.len() as u32).to_le_bytes());

    for file_path in &all_files {
        let mut file = File::open(file_path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;

        let path_str = file_path.to_string_lossy().to_string();
        let path_bytes = path_str.as_bytes();
        metadata.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
        metadata.extend_from_slice(path_bytes);
        metadata.extend_from_slice(&(data.len() as u64).to_le_bytes());

        input_data.extend_from_slice(&data);
    }

    let threads = rayon::current_num_threads();
    let chunks: Vec<&[u8]> = input_data.chunks(BLOCK_SIZE).collect();
    let num_blocks = chunks.len();

    ui::step(
        "Encoding",
        &format!(
            "{} file{}  ·  {} block{}  ·  {} threads  ·  {}",
            all_files.len(),
            if all_files.len() == 1 { "" } else { "s" },
            num_blocks,
            if num_blocks == 1 { "" } else { "s" },
            threads,
            human(input_data.len()),
        ),
    );

    let t = Instant::now();
    let compressed_blocks: Vec<_> = chunks
        .par_iter()
        .map(|&chunk| compress_block(chunk))
        .collect();

    let mut out_file = File::create(output_path)?;
    out_file.write_all(b"\x7FJPF")?;
    out_file.write_all(&2u16.to_le_bytes())?;

    let target_isa: u16 = if cfg!(target_arch = "aarch64") { 1 } else { 0 };
    out_file.write_all(&target_isa.to_le_bytes())?;
    out_file.write_all(&(input_data.len() as u64).to_le_bytes())?;
    out_file.write_all(&(num_blocks as u64).to_le_bytes())?;

    let mut flags = 0u32;
    let mut salt = [0u8; 16];
    if password.is_some() {
        flags |= 1;
        OsRng.fill_bytes(&mut salt);
    }
    out_file.write_all(&flags.to_le_bytes())?;
    out_file.write_all(&salt)?;

    let key = password.map(|p| derive_key(p, &salt));

    let (final_metadata, meta_nonce) = if let Some(ref k) = key {
        let (enc, nonce) = encrypt_data(&metadata, k);
        (enc, nonce)
    } else {
        (metadata, [0u8; 24])
    };

    let meta_total_size = 24 + 4 + final_metadata.len() as u64;
    out_file.write_all(&meta_total_size.to_le_bytes())?;
    out_file.write_all(&[0u8; 4])?;
    out_file.write_all(&meta_nonce)?;
    out_file.write_all(&(final_metadata.len() as u32).to_le_bytes())?;
    out_file.write_all(&final_metadata)?;

    let mut compressed_total = 0usize;
    for (prim, uncomp_len, ir, bits) in &compressed_blocks {
        let mut body = Vec::new();
        body.extend_from_slice(ir);
        body.extend_from_slice(bits);

        let mut block_header = Vec::new();
        block_header.extend_from_slice(&prim.to_le_bytes());
        block_header.extend_from_slice(&uncomp_len.to_le_bytes());
        block_header.extend_from_slice(&(ir.len() as u32).to_le_bytes());
        block_header.extend_from_slice(&(bits.len() as u32).to_le_bytes());

        if let Some(ref k) = key {
            let (enc_body, nonce) = encrypt_data(&body, k);
            out_file.write_all(&nonce)?;
            out_file.write_all(&(enc_body.len() as u32).to_le_bytes())?;
            out_file.write_all(&block_header)?;
            out_file.write_all(&enc_body)?;
            compressed_total += enc_body.len();
        } else {
            out_file.write_all(&[0u8; 24])?;
            out_file.write_all(&(body.len() as u32).to_le_bytes())?;
            out_file.write_all(&block_header)?;
            out_file.write_all(&body)?;
            compressed_total += body.len();
        }
    }

    let ratio = if input_data.is_empty() {
        0.0
    } else {
        (1.0 - compressed_total as f64 / input_data.len() as f64) * 100.0
    };

    ui::done(
        "Compressed",
        &format!(
            "{}  →  {}  ({:.1}% smaller)",
            human(input_data.len()),
            human(compressed_total),
            ratio
        ),
    );
    ui::info(
        "Encryption",
        if password.is_some() {
            "XChaCha20-Poly1305"
        } else {
            "none"
        },
    );
    ui::info("Time", &elapsed(t));
    ui::nl();
    Ok(())
}

// ── decompress ────────────────────────────────────────────────────────────────

fn decompress(input_path: &str, output_dir: &str) -> std::io::Result<()> {
    ui::step("Reading", input_path);
    let mut file = File::open(input_path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    if buffer.len() < 56 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Header too short",
        ));
    }
    if &buffer[0..4] != b"\x7FJPF" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Not a .jpf archive",
        ));
    }

    let _version = u16::from_le_bytes(buffer[4..6].try_into().unwrap());
    let _target_isa = u16::from_le_bytes(buffer[6..8].try_into().unwrap());
    let uncomp_size = u64::from_le_bytes(buffer[8..16].try_into().unwrap());
    let num_blocks = u64::from_le_bytes(buffer[16..24].try_into().unwrap());
    let flags = u32::from_le_bytes(buffer[24..28].try_into().unwrap());
    let salt = &buffer[28..44];
    let meta_total = u64::from_le_bytes(buffer[44..52].try_into().unwrap()) as usize;

    let key = if (flags & 1) != 0 {
        eprint!("  Password: ");
        std::io::stderr().flush().unwrap();
        let pass = read_password().expect("Failed to read password");
        Some(derive_key(&pass, salt))
    } else {
        None
    };

    let meta_nonce: [u8; 24] = buffer[56..80].try_into().unwrap();
    let meta_body_len = u32::from_le_bytes(buffer[80..84].try_into().unwrap()) as usize;
    let meta_body = &buffer[84..84 + meta_body_len];

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

    ui::info(
        "Archive",
        &format!(
            "{}  ·  {} block{}  ·  {}",
            human(uncomp_size as usize),
            num_blocks,
            if num_blocks == 1 { "" } else { "s" },
            if key.is_some() {
                "encrypted"
            } else {
                "no encryption"
            }
        ),
    );

    let mut offset = 56 + meta_total;
    let mut final_output = Vec::with_capacity(uncomp_size as usize);
    let t = Instant::now();

    for block_idx in 0..num_blocks {
        ui::progress(block_idx + 1, num_blocks);

        let nonce: [u8; 24] = buffer[offset..offset + 24].try_into().unwrap();
        let body_len =
            u32::from_le_bytes(buffer[offset + 24..offset + 28].try_into().unwrap()) as usize;
        offset += 28;

        let prim = u32::from_le_bytes(buffer[offset..offset + 4].try_into().unwrap()) as usize;
        let uncomp_len =
            u32::from_le_bytes(buffer[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let ir_len =
            u32::from_le_bytes(buffer[offset + 8..offset + 12].try_into().unwrap()) as usize;
        let bit_len =
            u32::from_le_bytes(buffer[offset + 12..offset + 16].try_into().unwrap()) as usize;
        offset += 16;

        let encrypted_body = &buffer[offset..offset + body_len];
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
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("JIT fault on block {block_idx}"),
            ));
        }
    }

    println!();
    let mut current_pos = 0;
    for (path, size) in files {
        let full_path = std::path::Path::new(output_dir).join(&path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out_file = File::create(&full_path)?;
        out_file.write_all(&final_output[current_pos..current_pos + size])?;
        current_pos += size;
    }

    ui::done(
        "Extracted",
        &format!(
            "{} file{}  →  {output_dir}  ({})",
            num_files,
            if num_files == 1 { "" } else { "s" },
            elapsed(t)
        ),
    );
    ui::nl();
    Ok(())
}

// ── FFI ───────────────────────────────────────────────────────────────────────

#[link(name = "jit_engine")]
unsafe extern "C" {
    fn compile_and_run_jit(
        ir_ptr: *const u8,
        ir_len: u64,
        bitstream_ptr: *const u8,
        output_ptr: *mut u8,
        output_limit: u64,
        mtf_ptr: *mut u8,
    ) -> DecompressResult;

    fn compile_and_run_query(
        ir_ptr: *const u8,
        ir_len: u64,
        bitstream_ptr: *const u8,
        output_ptr: *mut u8,
        output_limit: u64,
        mtf_ptr: *mut u8,
        pattern_ptr: *const u8,
        pattern_len: u64,
        matches_ptr: *mut u64,
        matches_limit: u64,
        primary_idx: u64,
    ) -> DecompressResult;
}

// ── query ─────────────────────────────────────────────────────────────────────

fn query(input_path: &str, pattern: &str) -> std::io::Result<()> {
    ui::step("Searching", &format!("{:?}  in  {input_path}", pattern));

    let mut file = File::open(input_path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    if buffer.len() < 56 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Header too short",
        ));
    }
    if &buffer[0..4] != b"\x7FJPF" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Not a .jpf archive",
        ));
    }

    let _version = u16::from_le_bytes(buffer[4..6].try_into().unwrap());
    let _target_isa = u16::from_le_bytes(buffer[6..8].try_into().unwrap());
    let _uncomp = u64::from_le_bytes(buffer[8..16].try_into().unwrap());
    let num_blocks = u64::from_le_bytes(buffer[16..24].try_into().unwrap());
    let flags = u32::from_le_bytes(buffer[24..28].try_into().unwrap());
    let salt = &buffer[28..44];
    let meta_total = u64::from_le_bytes(buffer[44..52].try_into().unwrap()) as usize;

    let key = if (flags & 1) != 0 {
        eprint!("  Password: ");
        std::io::stderr().flush().unwrap();
        let pass = read_password().expect("Failed to read password");
        Some(derive_key(&pass, salt))
    } else {
        None
    };

    let meta_nonce: [u8; 24] = buffer[56..80].try_into().unwrap();
    let meta_body_len = u32::from_le_bytes(buffer[80..84].try_into().unwrap()) as usize;
    let meta_body = &buffer[84..84 + meta_body_len];

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

    let mut offset = 56 + meta_total;
    let mut current_pos = 0;
    let mut total_matches = 0;

    for block_idx in 0..num_blocks {
        let nonce: [u8; 24] = buffer[offset..offset + 24].try_into().unwrap();
        let body_len =
            u32::from_le_bytes(buffer[offset + 24..offset + 28].try_into().unwrap()) as usize;
        offset += 28;

        let prim = u32::from_le_bytes(buffer[offset..offset + 4].try_into().unwrap()) as usize;
        let uncomp_len =
            u32::from_le_bytes(buffer[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let ir_len =
            u32::from_le_bytes(buffer[offset + 8..offset + 12].try_into().unwrap()) as usize;
        let bit_len =
            u32::from_le_bytes(buffer[offset + 12..offset + 16].try_into().unwrap()) as usize;
        offset += 16;

        let encrypted_body = &buffer[offset..offset + body_len];
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

        let mut bwt_buffer = vec![0u8; uncomp_len];
        let mut mtf_state: Vec<u8> = (0..=255).collect();
        let mut matches = vec![0u64; 1024];

        let result = unsafe {
            compile_and_run_query(
                ir_payload.as_ptr(),
                ir_payload.len() as u64,
                bitstream.as_ptr(),
                bwt_buffer.as_mut_ptr(),
                uncomp_len as u64,
                mtf_state.as_mut_ptr(),
                pattern.as_ptr(),
                pattern.len() as u64,
                matches.as_mut_ptr(),
                matches.len() as u64,
                prim as u64,
            )
        };

        if result.status_code == 0 {
            let n = result.bytes_written as usize;
            for i in 0..n {
                let match_off = matches[i] as usize;
                let mut file_off = 0;
                for (name, size) in &files {
                    if current_pos + match_off >= file_off
                        && current_pos + match_off < file_off + size
                    {
                        ui::match_hit(name, current_pos + match_off - file_off);
                        break;
                    }
                    file_off += size;
                }
            }
            total_matches += n;
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("JIT fault on block {block_idx}"),
            ));
        }
        current_pos += uncomp_len;
    }

    ui::nl();
    ui::done(
        "Found",
        &format!(
            "{total_matches} match{}",
            if total_matches == 1 { "" } else { "es" }
        ),
    );
    ui::nl();
    Ok(())
}
