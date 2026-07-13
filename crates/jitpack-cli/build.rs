use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = Path::new(&manifest_dir).parent().unwrap().parent().unwrap();

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();

    // Compile the Zig JIT engine for the CLI itself.
    let zig_file = "../../jit-engine/jit_engine.zig";
    let lib_name = "jit_engine";
    let output_file = format!("{}/lib{}.a", out_dir, lib_name);

    println!("cargo:rerun-if-changed=../../jit-engine/jit_engine.zig");
    println!("cargo:rerun-if-changed=../../jit-engine/arch/aarch64.zig");
    println!("cargo:rerun-if-changed=../../jit-engine/arch/x86_64.zig");

    let zig_target = match (target_arch.as_str(), target_os.as_str()) {
        ("aarch64", "android") => "aarch64-linux-android",
        ("aarch64", "linux") => "aarch64-linux-gnu",
        ("x86_64", "linux") => "x86_64-linux-gnu",
        (arch, os) => panic!("Unsupported target: {}-{}", arch, os),
    };

    let status = Command::new("zig")
        .args([
            "build-lib",
            zig_file,
            "-O",
            "ReleaseSafe",
            "-fPIC",
            "-fno-stack-check",
            &format!("-femit-bin={}", output_file),
            "-target",
            zig_target,
        ])
        .status()
        .expect("Failed to run zig build-lib");

    if !status.success() {
        panic!("Zig compilation failed");
    }

    if target_arch == "aarch64" && target_os == "android" {
        let clang_builtins_dir = "/data/data/com.termux/files/usr/lib/clang/21/lib/linux";
        println!("cargo:rustc-link-search=native={}", clang_builtins_dir);
        println!("cargo:rustc-link-lib=static=clang_rt.builtins-aarch64-android");
    }

    println!("cargo:rustc-link-search=native={}", out_dir);
    println!("cargo:rustc-link-lib=static={}", lib_name);

    // Build the jitpack-sfx stub and embed it into the CLI.
    //
    // We use a dedicated target-dir inside OUT_DIR to avoid conflicting with
    // Cargo's own lock on the workspace target directory.
    let sfx_target_dir = format!("{}/sfx_build", out_dir);

    // Determine the cargo target triple from the env vars Cargo sets for us.
    let cargo_target = env::var("TARGET").unwrap();

    // Rerun if the sfx source changes.
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root
            .join("crates/jitpack-sfx/src/main.rs")
            .display()
    );

    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "-p",
            "jitpack-sfx",
            "--target",
            &cargo_target,
            "--target-dir",
            &sfx_target_dir,
        ])
        .current_dir(workspace_root)
        .status()
        .expect("Failed to build jitpack-sfx stub");

    if !status.success() {
        panic!("jitpack-sfx stub build failed");
    }

    // Copy the compiled stub into OUT_DIR so include_bytes! can find it.
    let stub_src = Path::new(&sfx_target_dir)
        .join(&cargo_target)
        .join("release")
        .join("jitpack-sfx");
    let stub_dst = Path::new(&out_dir).join("sfx_stub");

    std::fs::copy(&stub_src, &stub_dst).unwrap_or_else(|e| {
        panic!(
            "Failed to copy sfx stub from {} to {}: {}",
            stub_src.display(),
            stub_dst.display(),
            e
        )
    });

    // Extract Git commit hash and message for build codename.
    let commit_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let codename = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| commit_hash.clone());

    println!("cargo:rustc-env=GIT_HASH={}", commit_hash);
    println!("cargo:rustc-env=GIT_CODENAME={}", codename);
}
