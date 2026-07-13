#[cfg(test)]
mod tests {
    use jitpack_core::*;

    #[test]
    fn test_jit_stability_small() {
        let data = b"STABILITY";
        let (prim, uncomp_len, lengths, bitstream) = compress_block(data);

        let canonical_codes = get_canonical_codes(&lengths);
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

        #[repr(C)]
        pub struct DecompressResult {
            pub bytes_written: u64,
            pub status_code: u32,
        }

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
        }

        let mut out = vec![0u8; uncomp_len as usize];
        let mut mtf_table: Vec<u8> = (0..=255).collect();

        println!("Starting JIT execution...");
        let result = unsafe {
            compile_and_run_jit(
                ir_payload.as_ptr(),
                ir_payload.len() as u64,
                bitstream.as_ptr(),
                out.as_mut_ptr(),
                uncomp_len as u64,
                mtf_table.as_mut_ptr(),
            )
        };

        assert_eq!(result.status_code, 0, "JIT status code should be 0");
        assert_eq!(
            result.bytes_written, uncomp_len as u64,
            "JIT should write full block"
        );

        let unbwt = inverse_bwt(&out, prim as usize);
        assert_eq!(&unbwt, data, "Decompressed data mismatch!");
        println!("JIT Stability Test Passed!");
    }

    #[test]
    fn test_jit_query_small() {
        let data = b"STABILITY_TEST_STABILITY";
        let (prim, uncomp_len, lengths, bitstream) = compress_block(data);

        let canonical_codes = get_canonical_codes(&lengths);
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

        #[repr(C)]
        pub struct DecompressResult {
            pub bytes_written: u64,
            pub status_code: u32,
        }

        #[link(name = "jit_engine")]
        unsafe extern "C" {
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

        let mut out = vec![0u8; uncomp_len as usize];
        let mut mtf_table: Vec<u8> = (0..=255).collect();
        let mut matches = vec![0u64; 4];
        let pattern = b"STAB";
        let result = unsafe {
            compile_and_run_query(
                ir_payload.as_ptr(),
                ir_payload.len() as u64,
                bitstream.as_ptr(),
                out.as_mut_ptr(),
                uncomp_len as u64,
                mtf_table.as_mut_ptr(),
                pattern.as_ptr(),
                pattern.len() as u64,
                matches.as_mut_ptr(),
                matches.len() as u64,
                prim as u64,
            )
        };

        assert_eq!(result.status_code, 0, "JIT status code should be 0");
        assert_eq!(result.bytes_written, 2, "Should find exactly 2 matches");
        assert_eq!(matches[0], 0, "First match should be at index 0");
        assert_eq!(matches[1], 15, "Second match should be at index 15");

        // Verify limit bounds checking: limit to 1 match
        let mut matches_limit1 = vec![0u64; 1];
        let mut mtf_table2: Vec<u8> = (0..=255).collect();
        let result_limit1 = unsafe {
            compile_and_run_query(
                ir_payload.as_ptr(),
                ir_payload.len() as u64,
                bitstream.as_ptr(),
                out.as_mut_ptr(),
                uncomp_len as u64,
                mtf_table2.as_mut_ptr(),
                pattern.as_ptr(),
                pattern.len() as u64,
                matches_limit1.as_mut_ptr(),
                matches_limit1.len() as u64,
                prim as u64,
            )
        };

        assert_eq!(result_limit1.status_code, 0, "JIT status code should be 0");
        assert_eq!(
            result_limit1.bytes_written, 1,
            "Should truncate to 1 match due to limit"
        );
        assert_eq!(matches_limit1[0], 0, "First match should still be found");
    }

    #[test]
    fn test_decompress_block_and_extract_file() {
        let data = b"STABILITY";
        let (prim, uncomp_len, lengths, bitstream) = compress_block(data);
        let mut block_body = Vec::new();
        block_body.extend_from_slice(&lengths);
        block_body.extend_from_slice(&bitstream);

        let block = BlockView {
            nonce: [0u8; 24],
            primary_index: prim,
            uncompressed_size: uncomp_len,
            code_lengths_len: lengths.len() as u32,
            bitstream_len: bitstream.len() as u32,
            body: &block_body,
        };

        let decompressed = decompress_block(&block, None, 0, &[]).unwrap();
        assert_eq!(&decompressed, data);
    }
}
