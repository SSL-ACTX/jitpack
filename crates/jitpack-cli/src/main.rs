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

    pub fn banner() {
        println!();
        let codename = option_env!("GIT_CODENAME").unwrap_or("unknown");
        let hash = option_env!("GIT_HASH").unwrap_or("unknown");
        let arch = std::env::consts::ARCH;
        let os = std::env::consts::OS;

        if color_enabled() {
            println!(
                "  {BOLD}{CYAN}⚡ JitPack{R}  {DIM}·{R}  JIT-Powered Archive  {DIM}v{}{R}",
                env!("CARGO_PKG_VERSION")
            );
            println!(
                "  {DIM}Author: {}{R}  {DIM}·{R}  {DIM}Build: {} ({}) [{}-{}]{R}",
                env!("CARGO_PKG_AUTHORS"),
                codename,
                hash,
                arch,
                os
            );
        } else {
            println!(
                "  ⚡ JitPack  ·  JIT-Powered Archive  v{}",
                env!("CARGO_PKG_VERSION")
            );
            println!(
                "  Author: {}  ·  Build: {} ({}) [{}-{}]",
                env!("CARGO_PKG_AUTHORS"),
                codename,
                hash,
                arch,
                os
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
    let bin_name = args
        .first()
        .and_then(|s| std::path::Path::new(s).file_name().and_then(|n| n.to_str()))
        .unwrap_or("jitpack");
    if args.len() < 2 {
        print_usage(bin_name);
        return;
    }

    match args[1].as_str() {
        "compress" => {
            if args.len() < 4 {
                ui::error("Missing input or output path.");
                eprintln!("  Usage: {bin_name} compress <input…> <output.jpf> [--password]");
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
                eprintln!("  Usage: {bin_name} decompress <input.jpf> <output_dir>");
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
                eprintln!("  Usage: {bin_name} sfx-pack <input> <output_exe> [--password]");
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
                eprintln!("  Usage: {bin_name} query <input.jpf> <pattern>");
                return;
            }
            ui::banner();
            if let Err(e) = query(&args[2], &args[3]) {
                ui::error(&format!("Query failed: {e}"));
                std::process::exit(1);
            }
        }

        "list" | "ls" => {
            if args.len() < 3 {
                ui::error("Missing archive path.");
                eprintln!("  Usage: {bin_name} list <input.jpf>");
                return;
            }
            if let Err(e) = list_archive_contents(&args[2]) {
                ui::error(&format!("List failed: {e}"));
                std::process::exit(1);
            }
        }

        "tree" => {
            if args.len() < 3 {
                ui::error("Missing archive path.");
                eprintln!("  Usage: {bin_name} tree <input.jpf>");
                return;
            }
            if let Err(e) = tree_archive_contents(&args[2]) {
                ui::error(&format!("Tree failed: {e}"));
                std::process::exit(1);
            }
        }

        "info" => {
            if args.len() < 3 {
                ui::error("Missing archive path.");
                eprintln!("  Usage: {bin_name} info <input.jpf>");
                return;
            }
            if let Err(e) = info_archive_contents(&args[2]) {
                ui::error(&format!("Info failed: {e}"));
                std::process::exit(1);
            }
        }

        "cat" => {
            if args.len() < 4 {
                ui::error("Missing archive or file path.");
                eprintln!("  Usage: {bin_name} cat <input.jpf> <file_path>");
                return;
            }
            if let Err(e) = cat_file(&args[2], &args[3]) {
                ui::error(&format!("Cat failed: {e}"));
                std::process::exit(1);
            }
        }

        "help" | "--help" | "-h" => print_usage(bin_name),

        cmd => {
            ui::error(&format!("Unknown command '{cmd}'"));
            print_usage(bin_name);
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

fn print_usage(bin_name: &str) {
    ui::banner();
    println!("  Usage:  {bin_name} <command> [args]");
    ui::nl();
    ui::header("Commands");
    println!("     compress      Compress files or directories into a .jpf archive");
    println!("     decompress    Extract a .jpf archive to a directory");
    println!("     sfx-pack      Bundle a .jpf into a self-extracting executable");
    println!("     query         Search for a pattern inside an archive");
    println!("     list / ls     List files inside an archive without extracting");
    println!("     tree          Show a visual directory tree structure of the archive");
    println!("     info          Show detailed structural layout of the archive");
    println!("     cat           Decompress a single file to stdout");
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
            let entry = entry.map_err(std::io::Error::other)?;
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

    let target_isa: u16 = if cfg!(target_arch = "aarch64") { 1 } else { 0 };
    let mut flags = 0u32;
    let mut salt = [0u8; 16];
    if password.is_some() {
        flags |= ENCRYPTION_FLAG;
        OsRng.fill_bytes(&mut salt);
    }
    let key = password.map(|p| derive_key(p, &salt));
    let metadata_body_len = metadata.len() + usize::from(key.is_some()) * 16;
    let meta_total_size = 24 + 4 + metadata_body_len as u64;
    let mut header = Vec::with_capacity(ARCHIVE_HEADER_SIZE);
    header.extend_from_slice(ARCHIVE_MAGIC);
    header.extend_from_slice(&ARCHIVE_VERSION.to_le_bytes());
    header.extend_from_slice(&target_isa.to_le_bytes());
    header.extend_from_slice(&(input_data.len() as u64).to_le_bytes());
    header.extend_from_slice(&(num_blocks as u64).to_le_bytes());
    header.extend_from_slice(&flags.to_le_bytes());
    header.extend_from_slice(&salt);
    header.extend_from_slice(&meta_total_size.to_le_bytes());
    header.extend_from_slice(&[0u8; 4]);
    debug_assert_eq!(header.len(), ARCHIVE_HEADER_SIZE);

    let (final_metadata, meta_nonce) = if let Some(ref k) = key {
        let (enc, nonce) = encrypt_data_with_aad(&metadata, k, &header);
        (enc, nonce)
    } else {
        (metadata, [0u8; 24])
    };

    debug_assert_eq!(final_metadata.len(), metadata_body_len);
    let mut out_file = File::create(output_path)?;
    out_file.write_all(&header)?;
    out_file.write_all(&meta_nonce)?;
    out_file.write_all(&(final_metadata.len() as u32).to_le_bytes())?;
    out_file.write_all(&final_metadata)?;

    let mut compressed_total = 0usize;
    for (block_index, (prim, uncomp_len, ir, bits)) in compressed_blocks.iter().enumerate() {
        let mut body = Vec::new();
        body.extend_from_slice(ir);
        body.extend_from_slice(bits);

        let mut block_header = Vec::new();
        block_header.extend_from_slice(&prim.to_le_bytes());
        block_header.extend_from_slice(&uncomp_len.to_le_bytes());
        block_header.extend_from_slice(&(ir.len() as u32).to_le_bytes());
        block_header.extend_from_slice(&(bits.len() as u32).to_le_bytes());

        if let Some(ref k) = key {
            let aad = block_aad(
                &header,
                block_index as u64,
                *prim,
                *uncomp_len,
                ir.len() as u32,
                bits.len() as u32,
            );
            let (enc_body, nonce) = encrypt_data_with_aad(&body, k, &aad);
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

    let archive = parse_archive(&buffer, ArchiveLimits::default())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let key = if archive.is_encrypted() {
        eprint!("  Password: ");
        std::io::stderr().flush().unwrap();
        let pass = read_password().expect("Failed to read password");
        Some(derive_key(&pass, &archive.header.salt))
    } else {
        None
    };

    let meta_data = if let Some(ref k) = key {
        decrypt_data_with_aad(
            archive.metadata.body,
            k,
            &archive.metadata.nonce,
            archive.header_bytes,
        )
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
    } else {
        if archive.is_encrypted() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Archive is encrypted",
            ));
        }
        archive.metadata.body.to_vec()
    };
    let files = parse_metadata(&meta_data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    ui::info(
        "Archive",
        &format!(
            "{}  ·  {} block{}  ·  {}",
            human(archive.header.uncompressed_size as usize),
            archive.header.num_blocks,
            if archive.header.num_blocks == 1 {
                ""
            } else {
                "s"
            },
            if key.is_some() {
                "encrypted"
            } else {
                "no encryption"
            }
        ),
    );

    let t = Instant::now();
    extract_archive(
        &archive,
        key.as_ref(),
        std::path::Path::new(output_dir),
        |curr, total| {
            ui::progress(curr, total);
        },
    )
    .map_err(std::io::Error::other)?;

    println!();
    ui::done(
        "Extracted",
        &format!(
            "{} file{}  →  {output_dir}  ({})",
            files.len(),
            if files.len() == 1 { "" } else { "s" },
            elapsed(t)
        ),
    );
    ui::nl();
    Ok(())
}

// ── query ─────────────────────────────────────────────────────────────────────

fn query(input_path: &str, pattern: &str) -> std::io::Result<()> {
    ui::step("Searching", &format!("{:?}  in  {input_path}", pattern));

    let mut file = File::open(input_path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let archive = parse_archive(&buffer, ArchiveLimits::default())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let key = if archive.is_encrypted() {
        eprint!("  Password: ");
        std::io::stderr().flush().unwrap();
        let pass = read_password().expect("Failed to read password");
        Some(derive_key(&pass, &archive.header.salt))
    } else {
        None
    };

    let matches = query_archive(&archive, key.as_ref(), pattern).map_err(std::io::Error::other)?;

    for m in &matches {
        ui::match_hit(&m.path, m.offset);
    }

    ui::nl();
    ui::done(
        "Found",
        &format!(
            "{} match{}",
            matches.len(),
            if matches.len() == 1 { "" } else { "es" }
        ),
    );
    ui::nl();
    Ok(())
}

// ── info ──────────────────────────────────────────────────────────────────────

fn info_archive_contents(input_path: &str) -> std::io::Result<()> {
    let mut file = File::open(input_path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let archive = parse_archive(&buffer, ArchiveLimits::default())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let key = if archive.is_encrypted() {
        eprint!("  Password: ");
        std::io::stderr().flush().unwrap();
        let pass = read_password().expect("Failed to read password");
        Some(derive_key(&pass, &archive.header.salt))
    } else {
        None
    };

    let files = inspect_metadata(&archive, key.as_ref())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    ui::banner();
    println!("  Archive Layout Info:");
    ui::divider();
    ui::info("Format version", &format!("v{}", ARCHIVE_VERSION));
    ui::info(
        "Target ISA",
        if archive.header.target_isa == 1 {
            "AArch64"
        } else {
            "x86_64"
        },
    );
    ui::info(
        "Encryption",
        if archive.is_encrypted() {
            "XChaCha20-Poly1305 (Argon2 salt present)"
        } else {
            "no encryption"
        },
    );
    ui::info(
        "Metadata size",
        &format!(
            "{} (envelope: {})",
            human(archive.metadata.body.len()),
            human(archive.metadata.body.len() + 28)
        ),
    );
    ui::info(
        "Data blocks",
        &format!(
            "{} block{}",
            archive.header.num_blocks,
            if archive.header.num_blocks == 1 {
                ""
            } else {
                "s"
            }
        ),
    );
    ui::info(
        "Uncompressed",
        &human(archive.header.uncompressed_size as usize),
    );
    ui::info(
        "Files count",
        &format!(
            "{} file{}",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        ),
    );
    ui::nl();

    Ok(())
}

// ── list ──────────────────────────────────────────────────────────────────────

fn list_archive_contents(input_path: &str) -> std::io::Result<()> {
    let mut file = File::open(input_path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let archive = parse_archive(&buffer, ArchiveLimits::default())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let key = if archive.is_encrypted() {
        eprint!("  Password: ");
        std::io::stderr().flush().unwrap();
        let pass = read_password().expect("Failed to read password");
        Some(derive_key(&pass, &archive.header.salt))
    } else {
        None
    };

    let files = inspect_metadata(&archive, key.as_ref())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let max_path_len = files.iter().map(|f| f.path.len()).max().unwrap_or(20);
    // Cap path width to a maximum of 40 to avoid breaking mobile terminals
    let path_width = max_path_len.clamp(10, 40);
    let divider_len = path_width + 4 + 12;

    ui::banner();
    println!("  {:<width$}    {:>12}", "Path", "Size", width = path_width);
    if std::env::var("NO_COLOR").is_err() {
        println!("  \x1b[2m{}\x1b[0m", "─".repeat(divider_len));
    } else {
        println!("  {}", "─".repeat(divider_len));
    }

    let mut total_size = 0u64;
    for entry in &files {
        let path_str = if entry.path.len() > path_width {
            let half = (path_width - 3) / 2;
            format!(
                "{}...{}",
                &entry.path[..half],
                &entry.path[entry.path.len() - half..]
            )
        } else {
            entry.path.clone()
        };
        println!(
            "  {:<width$}    {:>12}",
            path_str,
            human(entry.size as usize),
            width = path_width
        );
        total_size += entry.size;
    }

    if std::env::var("NO_COLOR").is_err() {
        println!("  \x1b[2m{}\x1b[0m", "─".repeat(divider_len));
    } else {
        println!("  {}", "─".repeat(divider_len));
    }
    println!(
        "  Total: {} file{} ({})",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        human(total_size as usize)
    );
    ui::nl();

    Ok(())
}

// ── tree ──────────────────────────────────────────────────────────────────────

struct TreeNode {
    name: String,
    size: Option<u64>,
    children: std::collections::BTreeMap<String, TreeNode>,
}

fn build_tree(files: &[FileEntry]) -> TreeNode {
    let mut root = TreeNode {
        name: ".".to_string(),
        size: None,
        children: std::collections::BTreeMap::new(),
    };

    for entry in files {
        let parts: Vec<&str> = entry.path.split('/').filter(|s| !s.is_empty()).collect();
        let mut current = &mut root;
        for (i, part) in parts.iter().enumerate() {
            let is_last = i == parts.len() - 1;
            current = current
                .children
                .entry(part.to_string())
                .or_insert_with(|| TreeNode {
                    name: part.to_string(),
                    size: if is_last { Some(entry.size) } else { None },
                    children: std::collections::BTreeMap::new(),
                });
        }
    }

    root
}

fn print_tree_node(node: &TreeNode, prefix: &str, is_last: bool) {
    if node.name != "." {
        let connector = if is_last { "└── " } else { "├── " };
        let size_str = match node.size {
            Some(sz) => format!(" ({})", human(sz as usize)),
            None => "/".to_string(),
        };
        println!("{}{}{}{}", prefix, connector, node.name, size_str);
    } else {
        println!(".");
    }

    let next_prefix = if node.name == "." {
        "".to_string()
    } else if is_last {
        format!("{}    ", prefix)
    } else {
        format!("{}│   ", prefix)
    };

    let children_list: Vec<&TreeNode> = node.children.values().collect();
    for (i, child) in children_list.iter().enumerate() {
        let child_is_last = i == children_list.len() - 1;
        print_tree_node(child, &next_prefix, child_is_last);
    }
}

fn tree_archive_contents(input_path: &str) -> std::io::Result<()> {
    let mut file = File::open(input_path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let archive = parse_archive(&buffer, ArchiveLimits::default())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let key = if archive.is_encrypted() {
        eprint!("  Password: ");
        std::io::stderr().flush().unwrap();
        let pass = read_password().expect("Failed to read password");
        Some(derive_key(&pass, &archive.header.salt))
    } else {
        None
    };

    let files = inspect_metadata(&archive, key.as_ref())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    ui::banner();

    let root = build_tree(&files);
    print_tree_node(&root, "", true);

    ui::nl();
    Ok(())
}

// ── cat ───────────────────────────────────────────────────────────────────────

fn cat_file(input_path: &str, target_file_path: &str) -> std::io::Result<()> {
    let mut file = File::open(input_path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let archive = parse_archive(&buffer, ArchiveLimits::default())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let key = if archive.is_encrypted() {
        eprint!("  Password: ");
        std::io::stderr().flush().unwrap();
        let pass = read_password().expect("Failed to read password");
        Some(derive_key(&pass, &archive.header.salt))
    } else {
        None
    };

    let file_bytes = extract_file(&archive, key.as_ref(), target_file_path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    std::io::stdout().write_all(&file_bytes)?;
    std::io::stdout().flush()?;

    Ok(())
}
