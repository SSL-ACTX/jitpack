use argon2::Argon2;
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit},
};
use divsufsort::sort_in_place;
use rand_core::{OsRng, RngCore};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

pub const BLOCK_SIZE: usize = 256 * 1024; // 256 KB blocks

#[repr(C)]
pub struct DecompressResult {
    pub bytes_written: u64,
    pub status_code: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum OpCode {
    BranchBit = 0x01,
    EmitMTF = 0x02,
    EmitRunA = 0x03,
    EmitRunB = 0x04,
    Jump = 0x07,
    Terminate = 0x0F,
}

#[derive(Eq, PartialEq)]
pub struct Node {
    pub weight: usize,
    pub token: Option<u16>, // 1-255 are MTF indices, 256 is RunA, 257 is RunB, 258 is EOF
    pub left: Option<Box<Node>>,
    pub right: Option<Box<Node>>,
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        other.weight.cmp(&self.weight) // Min-heap based on weight
    }
}
impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub fn build_huffman_tree(frequencies: &HashMap<u16, usize>) -> Box<Node> {
    let mut heap = BinaryHeap::new();
    for (&token, &weight) in frequencies {
        heap.push(Box::new(Node {
            weight,
            token: Some(token),
            left: None,
            right: None,
        }));
    }

    while heap.len() > 1 {
        let left = heap.pop().unwrap();
        let right = heap.pop().unwrap();
        heap.push(Box::new(Node {
            weight: left.weight + right.weight,
            token: None,
            left: Some(left),
            right: Some(right),
        }));
    }

    heap.pop().unwrap()
}

pub fn generate_bit_codes(
    node: &Node,
    current_path: Vec<bool>,
    codes: &mut HashMap<u16, Vec<bool>>,
) {
    if let Some(token) = node.token {
        codes.insert(token, current_path);
    } else {
        let mut left_path = current_path.clone();
        left_path.push(false); // 0 branch
        generate_bit_codes(node.left.as_ref().unwrap(), left_path, codes);

        let mut right_path = current_path;
        right_path.push(true); // 1 branch
        generate_bit_codes(node.right.as_ref().unwrap(), right_path, codes);
    }
}

pub fn emit_node(node: &Node, ir: &mut Vec<u8>, leaf_jumps: &mut Vec<usize>) -> u32 {
    if let Some(token) = node.token {
        let start_ip = ir.len() as u32;
        if token == 258 {
            ir.push(OpCode::Terminate as u8);
            return start_ip;
        } else if token == 256 {
            ir.push(OpCode::EmitRunA as u8);
        } else if token == 257 {
            ir.push(OpCode::EmitRunB as u8);
        } else {
            ir.push(OpCode::EmitMTF as u8);
            ir.push(token as u8);
        }
        ir.push(OpCode::Jump as u8);
        leaf_jumps.push(ir.len());
        ir.extend_from_slice(&0u32.to_le_bytes());
        return start_ip;
    } else {
        let left_ip = emit_node(node.left.as_ref().unwrap(), ir, leaf_jumps);
        let right_ip = emit_node(node.right.as_ref().unwrap(), ir, leaf_jumps);
        let start_ip = ir.len() as u32;
        ir.push(OpCode::BranchBit as u8);
        ir.extend_from_slice(&left_ip.to_le_bytes());
        ir.extend_from_slice(&right_ip.to_le_bytes());
        return start_ip;
    }
}

/// Optimized BWT using divsufsort (Linear-time Suffix Array construction)
pub fn bwt(input: &[u8]) -> (Vec<u8>, usize) {
    let n = input.len();
    if n == 0 {
        return (vec![], 0);
    }
    if n == 1 {
        return (vec![input[0]], 0);
    }

    let mut sa = vec![0; n];
    sort_in_place(input, &mut sa);

    let mut bwt_data = Vec::with_capacity(n);
    let mut primary_idx = 0;

    for (i, &idx) in sa.iter().enumerate() {
        if idx == 0 {
            primary_idx = i;
            bwt_data.push(input[n - 1]);
        } else {
            bwt_data.push(input[(idx - 1) as usize]);
        }
    }

    (bwt_data, primary_idx)
}

/// Cache-optimized Inverse BWT
pub fn inverse_bwt(bwt: &[u8], primary: usize) -> Vec<u8> {
    let n = bwt.len();
    if n == 0 {
        return vec![];
    }

    let mut counts = [0usize; 256];
    for &b in bwt {
        counts[b as usize] += 1;
    }

    let mut sum = 0;
    let mut starts = [0usize; 256];
    for i in 0..256 {
        starts[i] = sum;
        sum += counts[i];
    }

    let mut next_pos = vec![0usize; n];
    for i in 0..n {
        let b = bwt[i] as usize;
        next_pos[starts[b]] = i;
        starts[b] += 1;
    }

    let mut out = Vec::with_capacity(n);
    let mut curr = next_pos[primary];
    for _ in 0..n {
        out.push(bwt[curr]);
        curr = next_pos[curr];
    }
    out
}

/// Cache-friendly MTF using array copy_within
pub fn mtf(input: &[u8]) -> Vec<u8> {
    let mut table = [0u8; 256];
    for i in 0..256 {
        table[i] = i as u8;
    }

    let mut out = Vec::with_capacity(input.len());
    for &byte in input {
        // Fast search for the byte in the table
        let mut pos = 0;
        while table[pos] != byte {
            pos += 1;
        }

        out.push(pos as u8);

        // Move to front: shift elements up to pos
        if pos > 0 {
            // Using copy_within is much faster than individual removals/insertions
            table.copy_within(0..pos, 1);
            table[0] = byte;
        }
    }
    out
}

pub fn get_canonical_codes(lengths: &[u8]) -> HashMap<u16, Vec<bool>> {
    let mut symbols_by_len: Vec<(u8, u16)> = lengths
        .iter()
        .enumerate()
        .filter(|&(_, &len)| len > 0)
        .map(|(sym, &len)| (len, sym as u16))
        .collect();
    symbols_by_len.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut codes = HashMap::new();
    let mut current_code = 0u32;
    let mut current_len = symbols_by_len.first().map(|x| x.0).unwrap_or(0);
    for (len, sym) in symbols_by_len {
        current_code <<= len - current_len;
        current_len = len;
        let mut path = Vec::with_capacity(len as usize);
        for i in (0..len).rev() {
            path.push((current_code & (1 << i)) != 0);
        }
        codes.insert(sym, path);
        current_code += 1;
    }
    codes
}

pub fn build_canonical_tree(codes: &HashMap<u16, Vec<bool>>) -> Box<Node> {
    let mut root = Box::new(Node {
        weight: 0,
        token: None,
        left: None,
        right: None,
    });
    for (&sym, path) in codes {
        let mut curr = &mut root;
        for &bit in path {
            if bit {
                if curr.right.is_none() {
                    curr.right = Some(Box::new(Node {
                        weight: 0,
                        token: None,
                        left: None,
                        right: None,
                    }));
                }
                curr = curr.right.as_mut().unwrap();
            } else {
                if curr.left.is_none() {
                    curr.left = Some(Box::new(Node {
                        weight: 0,
                        token: None,
                        left: None,
                        right: None,
                    }));
                }
                curr = curr.left.as_mut().unwrap();
            }
        }
        curr.token = Some(sym);
    }
    root
}

pub fn compress_block(chunk: &[u8]) -> (u32, u32, Vec<u8>, Vec<u8>) {
    let (bwt_data, primary_idx) = bwt(chunk);
    let mtf_data = mtf(&bwt_data);
    let mut rle_data = Vec::with_capacity(mtf_data.len());
    let mut zeros = 0;
    for &byte in &mtf_data {
        if byte == 0 {
            zeros += 1;
        } else {
            if zeros > 0 {
                let mut n = zeros;
                while n > 0 {
                    if n % 2 == 1 {
                        rle_data.push(256u16);
                        n = (n - 1) / 2;
                    } else {
                        rle_data.push(257u16);
                        n = (n - 2) / 2;
                    }
                }
                zeros = 0;
            }
            rle_data.push(byte as u16);
        }
    }
    if zeros > 0 {
        let mut n = zeros;
        while n > 0 {
            if n % 2 == 1 {
                rle_data.push(256u16);
                n = (n - 1) / 2;
            } else {
                rle_data.push(257u16);
                n = (n - 2) / 2;
            }
        }
    }
    let mut frequencies: HashMap<u16, usize> = HashMap::new();
    for &token in &rle_data {
        *frequencies.entry(token).or_insert(0) += 1;
    }
    frequencies.insert(258, 1);
    let root = build_huffman_tree(&frequencies);
    let mut bit_codes = HashMap::new();
    generate_bit_codes(&root, Vec::new(), &mut bit_codes);
    let mut lengths = [0u8; 259];
    for (&sym, path) in &bit_codes {
        lengths[sym as usize] = path.len() as u8;
    }
    let canonical_codes = get_canonical_codes(&lengths);
    let mut bitstream = Vec::new();
    let mut current_byte = 0u8;
    let mut bit_count = 0;
    let mut write_bit = |bit: bool| {
        if bit {
            current_byte |= 1 << bit_count;
        }
        bit_count += 1;
        if bit_count == 8 {
            bitstream.push(current_byte);
            current_byte = 0;
            bit_count = 0;
        }
    };
    for &token in &rle_data {
        let path = canonical_codes.get(&token).unwrap();
        for &bit in path {
            write_bit(bit);
        }
    }
    let eof_path = canonical_codes.get(&258).unwrap();
    for &bit in eof_path {
        write_bit(bit);
    }
    if bit_count > 0 {
        bitstream.push(current_byte);
    }
    (
        primary_idx as u32,
        chunk.len() as u32,
        lengths.to_vec(),
        bitstream,
    )
}

// --- Encryption Support ---

pub fn derive_key(password: &str, salt: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    let argon2 = Argon2::default();
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .expect("Argon2 key derivation failed");
    key
}

pub fn encrypt_data(data: &[u8], key: &[u8; 32]) -> (Vec<u8>, [u8; 24]) {
    let mut nonce = [0u8; 24];
    OsRng.fill_bytes(&mut nonce);
    let cipher = XChaCha20Poly1305::new(key.into());
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), data)
        .expect("Encryption failed");
    (ciphertext, nonce)
}

pub fn decrypt_data(
    ciphertext: &[u8],
    key: &[u8; 32],
    nonce: &[u8; 24],
) -> Result<Vec<u8>, String> {
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| "Decryption failed (bad password or corrupted data)".to_string())
}
