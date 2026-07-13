#![no_main]
use libfuzzer_sys::fuzz_target;
use jitpack_core::{parse_archive, ArchiveLimits};

fuzz_target!(|data: &[u8]| {
    // Fuzz the archive framing parser with default limits
    let _ = parse_archive(data, ArchiveLimits::default());
});
