use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use jitpack_core::*;
use std::collections::HashMap;
use std::hint::black_box;

pub fn bench_bwt_variants(c: &mut Criterion) {
    let mut group = c.benchmark_group("bwt");
    for size in [1024, 4096, 8192].iter() {
        let data = vec![0u8; *size];
        group.bench_with_input(format!("bwt_zeros_{}b", size), size, |b, _| {
            b.iter(|| bwt(black_box(&data)))
        });

        let mut random_data = vec![0u8; *size];
        getrandom::getrandom(&mut random_data).unwrap();
        group.bench_with_input(format!("bwt_random_{}b", size), size, |b, _| {
            b.iter(|| bwt(black_box(&random_data)))
        });
    }
    group.finish();
}

pub fn bench_mtf(c: &mut Criterion) {
    let mut data = vec![0u8; 1024 * 16];
    getrandom::getrandom(&mut data).unwrap();
    c.bench_function("mtf_16kb", |b| b.iter(|| mtf(black_box(&data))));
}

pub fn bench_huffman(c: &mut Criterion) {
    let mut freq = HashMap::new();
    for i in 0..258 {
        freq.insert(i, (i + 1) as usize);
    }

    c.bench_function("build_huffman_tree_258_symbols", |b| {
        b.iter(|| build_huffman_tree(black_box(&freq)))
    });
}

pub fn bench_encryption_pipeline(c: &mut Criterion) {
    let password = "super_secure_password";
    let salt = [0u8; 16];

    c.bench_function("derive_key_argon2id", |b| {
        b.iter(|| derive_key(black_box(password), black_box(&salt)))
    });

    let key = derive_key(password, &salt);
    let data = vec![0u8; 1024 * 100]; // 100KB

    c.bench_function("encrypt_xchacha20_100kb", |b| {
        b.iter(|| encrypt_data(black_box(&data), black_box(&key)))
    });

    let (ciphertext, nonce) = encrypt_data(&data, &key);
    c.bench_function("decrypt_xchacha20_100kb", |b| {
        b.iter(|| decrypt_data(black_box(&ciphertext), black_box(&key), black_box(&nonce)))
    });
}

pub fn bench_jit_logic(c: &mut Criterion) {
    let mut group = c.benchmark_group("jit_engine");

    // Setup IR
    let data = b"Comprehensive JIT Benchmark Data for Performance Testing".repeat(50);
    let (_prim, uncomp_len, lengths, bitstream) = compress_block(&data);
    let canonical_codes = get_canonical_codes(&lengths);
    let root = build_canonical_tree(&canonical_codes);

    group.bench_function("emit_node_ir_gen", |b| {
        b.iter(|| {
            let mut ir = Vec::new();
            let mut leaf_jumps = Vec::new();
            emit_node(black_box(&root), &mut ir, &mut leaf_jumps)
        })
    });

    let mut ir_payload = Vec::new();
    ir_payload.push(OpCode::Jump as u8);
    ir_payload.extend_from_slice(&0u32.to_le_bytes());
    let mut leaf_jumps = Vec::new();
    let root_ip = emit_node(&root, &mut ir_payload, &mut leaf_jumps);
    ir_payload[1..5].copy_from_slice(&root_ip.to_le_bytes());
    for pos in leaf_jumps {
        ir_payload[pos..pos + 4].copy_from_slice(&root_ip.to_le_bytes());
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

    group.bench_function("jit_decompress_block", |b| {
        b.iter_batched(
            || {
                (
                    vec![0u8; uncomp_len as usize],
                    (0..=255).collect::<Vec<u8>>(),
                )
            },
            |(mut out, mut mtf)| unsafe {
                compile_and_run_jit(
                    ir_payload.as_ptr(),
                    ir_payload.len() as u64,
                    bitstream.as_ptr(),
                    out.as_mut_ptr(),
                    uncomp_len as u64,
                    mtf.as_mut_ptr(),
                )
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_bwt_variants,
    bench_mtf,
    bench_huffman,
    bench_encryption_pipeline,
    bench_jit_logic
);
criterion_main!(benches);
