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

    pub fn done(label: &str, msg: &str) {
        if color_enabled() {
            println!("  {GREEN}✓{R}  {BOLD}{label:<13}{R}  {msg}");
        } else {
            println!("  ✓  {label:<13}  {msg}");
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

    let archive = parse_archive(&payload, ArchiveLimits::default())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let encrypted = archive.is_encrypted();
    ui::banner(encrypted);
    ui::info("Format", &format!("v{}", ARCHIVE_VERSION));
    ui::info(
        "Target",
        if archive.header.target_isa == 1 {
            "AArch64"
        } else {
            "x86_64"
        },
    );
    ui::info(
        "Size",
        &format!(
            "{}  ·  {} block{}",
            human(archive.header.uncompressed_size as usize),
            archive.header.num_blocks,
            if archive.header.num_blocks == 1 {
                ""
            } else {
                "s"
            }
        ),
    );
    ui::nl();

    let key = if encrypted {
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
        .map_err(std::io::Error::other)?
    } else {
        archive.metadata.body.to_vec()
    };

    let files = parse_metadata(&meta_data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let t = Instant::now();
    let output_dir = "extracted";

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
            "{} file{}  →  {output_dir}/  ({})",
            files.len(),
            if files.len() == 1 { "" } else { "s" },
            elapsed(t)
        ),
    );
    ui::nl();
    Ok(())
}
