use std::env;
use std::process::Command;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let zig_file = "../../jit-engine/jit_engine.zig";
    let lib_name = "jit_engine";
    let output_file = format!("{}/lib{}.a", out_dir, lib_name);

    println!("cargo:rerun-if-changed=../../jit-engine");

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();

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
    // Note: We don't link here because core is a library, but downstream will need to.
    // However, for benchmarks in core itself, we might need it.
    println!("cargo:rustc-link-lib=static={}", lib_name);
}
