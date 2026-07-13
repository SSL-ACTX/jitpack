#[cfg(test)]
mod tests {
    use jitpack_core::*;

    fn create_test_archive(
        magic: &[u8; 4],
        version: u16,
        target_isa: u16,
        uncomp_size: u64,
        block_count: u64,
        flags: u32,
        reserved: u32,
    ) -> Vec<u8> {
        let mut archive = Vec::new();
        archive.extend_from_slice(magic); // 0..4
        archive.extend_from_slice(&version.to_le_bytes()); // 4..6
        archive.extend_from_slice(&target_isa.to_le_bytes()); // 6..8
        archive.extend_from_slice(&uncomp_size.to_le_bytes()); // 8..16
        archive.extend_from_slice(&block_count.to_le_bytes()); // 16..24
        archive.extend_from_slice(&flags.to_le_bytes()); // 24..28
        archive.extend_from_slice(&[0u8; 16]); // salt (28..44)
        archive.extend_from_slice(&28u64.to_le_bytes()); // metadata size = 28 (44..52)
        archive.extend_from_slice(&reserved.to_le_bytes()); // reserved (52..56)

        // Metadata envelope (28 bytes)
        archive.extend_from_slice(&[0u8; 24]); // nonce
        archive.extend_from_slice(&0u32.to_le_bytes()); // body length = 0
        archive
    }

    #[test]
    fn test_invalid_magic() {
        let archive = create_test_archive(b"XJPF", 1, 0, 100, 1, 0, 0);
        let res = parse_archive(&archive, ArchiveLimits::default());
        assert!(matches!(res, Err(ArchiveError::InvalidMagic)));
    }

    #[test]
    fn test_unsupported_version() {
        let archive = create_test_archive(b"\x7FJPF", 99, 0, 100, 1, 0, 0);
        let res = parse_archive(&archive, ArchiveLimits::default());
        assert!(matches!(res, Err(ArchiveError::UnsupportedVersion(99))));
    }

    #[test]
    fn test_unsupported_flags() {
        let archive = create_test_archive(b"\x7FJPF", 1, 0, 100, 1, 0x8000, 0);
        let res = parse_archive(&archive, ArchiveLimits::default());
        assert!(matches!(res, Err(ArchiveError::UnsupportedFlags(0x8000))));
    }

    #[test]
    fn test_reserved_bytes() {
        let archive = create_test_archive(b"\x7FJPF", 1, 0, 100, 1, 0, 0xFFFF);
        let res = parse_archive(&archive, ArchiveLimits::default());
        assert!(matches!(
            res,
            Err(ArchiveError::Invalid("reserved header bytes"))
        ));
    }

    #[test]
    fn test_uncompressed_limit_exceeded() {
        let archive = create_test_archive(b"\x7FJPF", 1, 0, 10_000_000, 1, 0, 0);
        let limits = ArchiveLimits {
            max_uncompressed_size: 5_000_000,
            ..Default::default()
        };
        let res = parse_archive(&archive, limits);
        assert!(matches!(
            res,
            Err(ArchiveError::LimitExceeded("uncompressed size"))
        ));
    }

    #[test]
    fn test_block_count_limit_exceeded() {
        let archive = create_test_archive(b"\x7FJPF", 1, 0, 100, 100, 0, 0);
        let limits = ArchiveLimits {
            max_blocks: 50,
            ..Default::default()
        };
        let res = parse_archive(&archive, limits);
        assert!(matches!(
            res,
            Err(ArchiveError::LimitExceeded("block count"))
        ));
    }

    #[test]
    fn test_truncated_header() {
        let archive = create_test_archive(b"\x7FJPF", 1, 0, 100, 1, 0, 0);
        let res = parse_archive(
            &archive[..ARCHIVE_HEADER_SIZE - 5],
            ArchiveLimits::default(),
        );
        assert!(matches!(res, Err(ArchiveError::Truncated("header"))));
    }
}
