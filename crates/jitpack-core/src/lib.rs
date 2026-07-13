use argon2::Argon2;
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use divsufsort::sort_in_place;
use rand_core::{OsRng, RngCore};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::fmt;
use std::fs::File;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

pub const BLOCK_SIZE: usize = 256 * 1024; // 256 KB blocks

pub const ARCHIVE_MAGIC: &[u8; 4] = b"\x7FJPF";
pub const ARCHIVE_VERSION: u16 = 1;
pub const ARCHIVE_HEADER_SIZE: usize = 56;
pub const BLOCK_ENVELOPE_SIZE: usize = 44;
pub const ENCRYPTION_FLAG: u32 = 1;

/// Limits applied before an archive is decoded or passed to the JIT.
///
/// These are deliberately independent from the host's available memory: an
/// archive header is untrusted input and must not be allowed to request an
/// arbitrary allocation.
#[derive(Debug, Clone, Copy)]
pub struct ArchiveLimits {
    pub max_uncompressed_size: u64,
    pub max_blocks: u64,
    pub max_metadata_size: usize,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_uncompressed_size: 4 * 1024 * 1024 * 1024,
            max_blocks: 1_000_000,
            max_metadata_size: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    Truncated(&'static str),
    InvalidMagic,
    UnsupportedVersion(u16),
    UnsupportedFlags(u32),
    LimitExceeded(&'static str),
    Invalid(&'static str),
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(section) => write!(f, "archive is truncated in {section}"),
            Self::InvalidMagic => write!(f, "not a JitPack archive"),
            Self::UnsupportedVersion(version) => write!(f, "unsupported archive version {version}"),
            Self::UnsupportedFlags(flags) => write!(f, "unsupported archive flags {flags:#x}"),
            Self::LimitExceeded(what) => write!(f, "archive limit exceeded: {what}"),
            Self::Invalid(what) => write!(f, "invalid archive: {what}"),
        }
    }
}

impl std::error::Error for ArchiveError {}

#[derive(Debug, Clone, Copy)]
pub struct ArchiveHeader {
    pub target_isa: u16,
    pub uncompressed_size: u64,
    pub num_blocks: u64,
    pub flags: u32,
    pub salt: [u8; 16],
}

#[derive(Debug, Clone, Copy)]
pub struct MetadataView<'a> {
    pub nonce: [u8; 24],
    pub body: &'a [u8],
}

#[derive(Debug, Clone, Copy)]
pub struct BlockView<'a> {
    pub nonce: [u8; 24],
    pub primary_index: u32,
    pub uncompressed_size: u32,
    pub code_lengths_len: u32,
    pub bitstream_len: u32,
    pub body: &'a [u8],
}

#[derive(Debug, Clone, Copy)]
pub struct FileEntryView<'a> {
    pub path: &'a str,
    pub size: u64,
}

#[derive(Debug)]
pub struct ArchiveView<'a> {
    pub header: ArchiveHeader,
    pub header_bytes: &'a [u8],
    pub metadata: MetadataView<'a>,
    pub blocks: Vec<BlockView<'a>>,
}

impl<'a> ArchiveView<'a> {
    pub fn is_encrypted(&self) -> bool {
        self.header.flags & ENCRYPTION_FLAG != 0
    }
}

/// Parses and validates archive framing. The returned payloads borrow the
/// input; decryption and decompression remain explicit caller operations.
pub fn parse_archive(input: &[u8], limits: ArchiveLimits) -> Result<ArchiveView<'_>, ArchiveError> {
    if input.len() < ARCHIVE_HEADER_SIZE {
        return Err(ArchiveError::Truncated("header"));
    }
    if &input[..4] != ARCHIVE_MAGIC {
        return Err(ArchiveError::InvalidMagic);
    }

    let version = read_u16(input, 4, "version")?;
    if version != ARCHIVE_VERSION {
        return Err(ArchiveError::UnsupportedVersion(version));
    }
    let target_isa = read_u16(input, 6, "target ISA")?;
    let uncompressed_size = read_u64(input, 8, "uncompressed size")?;
    let num_blocks = read_u64(input, 16, "block count")?;
    let flags = read_u32(input, 24, "flags")?;
    if flags & !ENCRYPTION_FLAG != 0 {
        return Err(ArchiveError::UnsupportedFlags(flags));
    }
    if slice_at(input, 52, 4, "reserved header")? != [0; 4] {
        return Err(ArchiveError::Invalid("reserved header bytes"));
    }
    if uncompressed_size > limits.max_uncompressed_size {
        return Err(ArchiveError::LimitExceeded("uncompressed size"));
    }
    if num_blocks == 0 || num_blocks > limits.max_blocks {
        return Err(ArchiveError::LimitExceeded("block count"));
    }

    let mut salt = [0u8; 16];
    salt.copy_from_slice(slice_at(input, 28, 16, "salt")?);
    let metadata_size = read_u64(input, 44, "metadata size")?;
    let metadata_size =
        usize::try_from(metadata_size).map_err(|_| ArchiveError::LimitExceeded("metadata size"))?;
    if metadata_size < 28 || metadata_size > limits.max_metadata_size {
        return Err(ArchiveError::LimitExceeded("metadata size"));
    }

    let metadata_offset = ARCHIVE_HEADER_SIZE;
    let metadata = slice_at(input, metadata_offset, metadata_size, "metadata")?;
    let mut metadata_nonce = [0u8; 24];
    metadata_nonce.copy_from_slice(&metadata[..24]);
    let metadata_body_len = read_u32(metadata, 24, "metadata body length")? as usize;
    if metadata_body_len.checked_add(28) != Some(metadata_size) {
        return Err(ArchiveError::Invalid(
            "metadata size does not match its envelope",
        ));
    }

    let header = ArchiveHeader {
        target_isa,
        uncompressed_size,
        num_blocks,
        flags,
        salt,
    };
    let encrypted = flags & ENCRYPTION_FLAG != 0;
    let mut offset = metadata_offset + metadata_size;
    let mut total_uncompressed = 0u64;
    let mut blocks = Vec::with_capacity(usize::try_from(num_blocks).unwrap_or(usize::MAX));

    for _ in 0..num_blocks {
        let envelope = slice_at(input, offset, BLOCK_ENVELOPE_SIZE, "block envelope")?;
        let mut nonce = [0u8; 24];
        nonce.copy_from_slice(&envelope[..24]);
        let body_len = read_u32(envelope, 24, "block body length")? as usize;
        let primary_index = read_u32(envelope, 28, "block primary index")?;
        let block_uncompressed_size = read_u32(envelope, 32, "block uncompressed size")?;
        let code_lengths_len = read_u32(envelope, 36, "code lengths length")?;
        let bitstream_len = read_u32(envelope, 40, "bitstream length")?;
        if block_uncompressed_size == 0 || block_uncompressed_size as usize > BLOCK_SIZE {
            return Err(ArchiveError::Invalid("block uncompressed size"));
        }
        if primary_index >= block_uncompressed_size {
            return Err(ArchiveError::Invalid("block primary index"));
        }
        if code_lengths_len != 259 {
            return Err(ArchiveError::Invalid("code lengths length"));
        }
        let plaintext_len = (code_lengths_len as usize)
            .checked_add(bitstream_len as usize)
            .ok_or(ArchiveError::Invalid("block plaintext length"))?;
        let expected_body_len = if encrypted {
            plaintext_len.checked_add(16)
        } else {
            Some(plaintext_len)
        };
        if expected_body_len != Some(body_len) {
            return Err(ArchiveError::Invalid("block body length"));
        }

        offset = offset
            .checked_add(BLOCK_ENVELOPE_SIZE)
            .ok_or(ArchiveError::Invalid("block offset"))?;
        let body = slice_at(input, offset, body_len, "block body")?;
        offset = offset
            .checked_add(body_len)
            .ok_or(ArchiveError::Invalid("block offset"))?;
        total_uncompressed = total_uncompressed
            .checked_add(block_uncompressed_size as u64)
            .ok_or(ArchiveError::LimitExceeded("uncompressed size"))?;
        if total_uncompressed > limits.max_uncompressed_size {
            return Err(ArchiveError::LimitExceeded("uncompressed size"));
        }
        blocks.push(BlockView {
            nonce,
            primary_index,
            uncompressed_size: block_uncompressed_size,
            code_lengths_len,
            bitstream_len,
            body,
        });
    }
    if offset != input.len() {
        return Err(ArchiveError::Invalid("trailing data"));
    }
    if total_uncompressed != uncompressed_size {
        return Err(ArchiveError::Invalid(
            "uncompressed size does not match blocks",
        ));
    }

    Ok(ArchiveView {
        header,
        header_bytes: &input[..ARCHIVE_HEADER_SIZE],
        metadata: MetadataView {
            nonce: metadata_nonce,
            body: &metadata[28..],
        },
        blocks,
    })
}

/// Parses the decrypted metadata table and verifies that it consumes the whole
/// metadata body. Path safety is intentionally handled by extraction policy.
pub fn parse_metadata(input: &[u8]) -> Result<Vec<FileEntryView<'_>>, ArchiveError> {
    let file_count = read_u32(input, 0, "file count")? as usize;
    let max_files = input.len().saturating_sub(4) / 10;
    if file_count > max_files {
        return Err(ArchiveError::Invalid("file count"));
    }
    let mut offset = 4;
    let mut files = Vec::with_capacity(file_count);
    for _ in 0..file_count {
        let path_len = read_u16(input, offset, "file path length")? as usize;
        offset = offset
            .checked_add(2)
            .ok_or(ArchiveError::Truncated("file path length"))?;
        let path_bytes = slice_at(input, offset, path_len, "file path")?;
        let path = std::str::from_utf8(path_bytes)
            .map_err(|_| ArchiveError::Invalid("file path is not UTF-8"))?;
        offset = offset
            .checked_add(path_len)
            .ok_or(ArchiveError::Truncated("file path"))?;
        let size = read_u64(input, offset, "file size")?;
        offset = offset
            .checked_add(8)
            .ok_or(ArchiveError::Truncated("file size"))?;
        files.push(FileEntryView { path, size });
    }
    if offset != input.len() {
        return Err(ArchiveError::Invalid("trailing metadata"));
    }
    Ok(files)
}

/// Resolves an archive path below `output_root` without permitting it to escape
/// through absolute, parent-directory, or platform-prefix components.
pub fn extraction_path(output_root: &Path, archive_path: &str) -> Result<PathBuf, ArchiveError> {
    let path = Path::new(archive_path);
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => relative.push(segment),
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            Component::ParentDir => {
                return Err(ArchiveError::Invalid("unsafe extraction path"));
            }
        }
    }
    if relative.as_os_str().is_empty() {
        return Err(ArchiveError::Invalid("empty extraction path"));
    }
    Ok(output_root.join(relative))
}

fn slice_at<'a>(
    input: &'a [u8],
    offset: usize,
    len: usize,
    section: &'static str,
) -> Result<&'a [u8], ArchiveError> {
    let end = offset
        .checked_add(len)
        .ok_or(ArchiveError::Truncated(section))?;
    input
        .get(offset..end)
        .ok_or(ArchiveError::Truncated(section))
}

fn read_u16(input: &[u8], offset: usize, section: &'static str) -> Result<u16, ArchiveError> {
    Ok(u16::from_le_bytes(
        slice_at(input, offset, 2, section)?
            .try_into()
            .expect("slice length checked"),
    ))
}

fn read_u32(input: &[u8], offset: usize, section: &'static str) -> Result<u32, ArchiveError> {
    Ok(u32::from_le_bytes(
        slice_at(input, offset, 4, section)?
            .try_into()
            .expect("slice length checked"),
    ))
}

fn read_u64(input: &[u8], offset: usize, section: &'static str) -> Result<u64, ArchiveError> {
    Ok(u64::from_le_bytes(
        slice_at(input, offset, 8, section)?
            .try_into()
            .expect("slice length checked"),
    ))
}

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
        start_ip
    } else {
        let left_ip = emit_node(node.left.as_ref().unwrap(), ir, leaf_jumps);
        let right_ip = emit_node(node.right.as_ref().unwrap(), ir, leaf_jumps);
        let start_ip = ir.len() as u32;
        ir.push(OpCode::BranchBit as u8);
        ir.extend_from_slice(&left_ip.to_le_bytes());
        ir.extend_from_slice(&right_ip.to_le_bytes());
        start_ip
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

    // Double the input to simulate cyclic suffix sorting
    let mut doubled = Vec::with_capacity(2 * n);
    doubled.extend_from_slice(input);
    doubled.extend_from_slice(input);

    let mut sa2 = vec![0; 2 * n];
    sort_in_place(&doubled, &mut sa2);

    let mut bwt_data = Vec::with_capacity(n);
    let mut primary_idx = 0;

    for &idx in &sa2 {
        let idx = idx as usize;
        if idx < n {
            if idx == 0 {
                primary_idx = bwt_data.len();
                bwt_data.push(input[n - 1]);
            } else {
                bwt_data.push(input[idx - 1]);
            }
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

    let mut lf = vec![0usize; n];
    for i in 0..n {
        let b = bwt[i] as usize;
        lf[i] = starts[b];
        starts[b] += 1;
    }

    let mut out = vec![0u8; n];
    let mut curr = primary;
    for i in (0..n).rev() {
        out[i] = bwt[curr];
        curr = lf[curr];
    }
    out
}

/// Cache-friendly MTF using array copy_within
pub fn mtf(input: &[u8]) -> Vec<u8> {
    let mut table = [0u8; 256];
    for (i, val) in table.iter_mut().enumerate() {
        *val = i as u8;
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
    encrypt_data_with_aad(data, key, &[])
}

pub fn encrypt_data_with_aad(data: &[u8], key: &[u8; 32], aad: &[u8]) -> (Vec<u8>, [u8; 24]) {
    let mut nonce = [0u8; 24];
    OsRng.fill_bytes(&mut nonce);
    let cipher = XChaCha20Poly1305::new(key.into());
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: data, aad })
        .expect("Encryption failed");
    (ciphertext, nonce)
}

pub fn decrypt_data(
    ciphertext: &[u8],
    key: &[u8; 32],
    nonce: &[u8; 24],
) -> Result<Vec<u8>, String> {
    decrypt_data_with_aad(ciphertext, key, nonce, &[])
}

pub fn decrypt_data_with_aad(
    ciphertext: &[u8],
    key: &[u8; 32],
    nonce: &[u8; 24],
    aad: &[u8],
) -> Result<Vec<u8>, String> {
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| "Decryption failed (bad password or corrupted data)".to_string())
}

/// Associated data for an encrypted block in JPF v3. It binds the archive
/// header and every unencrypted block field used to decode the ciphertext.
pub fn block_aad(
    header: &[u8],
    block_index: u64,
    primary_index: u32,
    uncompressed_size: u32,
    code_lengths_len: u32,
    bitstream_len: u32,
) -> Vec<u8> {
    let mut aad = Vec::with_capacity(header.len() + 24);
    aad.extend_from_slice(header);
    aad.extend_from_slice(&block_index.to_le_bytes());
    aad.extend_from_slice(&primary_index.to_le_bytes());
    aad.extend_from_slice(&uncompressed_size.to_le_bytes());
    aad.extend_from_slice(&code_lengths_len.to_le_bytes());
    aad.extend_from_slice(&bitstream_len.to_le_bytes());
    aad
}

#[link(name = "jit_engine")]
unsafe extern "C" {
    pub fn compile_and_run_jit(
        ir_ptr: *const u8,
        ir_len: u64,
        bitstream_ptr: *const u8,
        output_ptr: *mut u8,
        output_limit: u64,
        mtf_ptr: *mut u8,
    ) -> DecompressResult;

    pub fn compile_and_run_query(
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

pub fn decompress_block(
    block: &BlockView<'_>,
    key: Option<&[u8; 32]>,
    block_idx: usize,
    header_bytes: &[u8],
) -> Result<Vec<u8>, String> {
    let body = if let Some(k) = key {
        decrypt_data_with_aad(
            block.body,
            k,
            &block.nonce,
            &block_aad(
                header_bytes,
                block_idx as u64,
                block.primary_index,
                block.uncompressed_size,
                block.code_lengths_len,
                block.bitstream_len,
            ),
        )?
    } else {
        block.body.to_vec()
    };

    if body.len() < block.code_lengths_len as usize {
        return Err("block body is shorter than Huffman code lengths".to_string());
    }
    let tree_payload = &body[..block.code_lengths_len as usize];
    let bitstream = &body[block.code_lengths_len as usize..];

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

    let mut output_buffer = vec![0u8; block.uncompressed_size as usize];
    let mut mtf_state: Vec<u8> = (0..=255).collect();

    let result = unsafe {
        compile_and_run_jit(
            ir_payload.as_ptr(),
            ir_payload.len() as u64,
            bitstream.as_ptr(),
            output_buffer.as_mut_ptr(),
            block.uncompressed_size as u64,
            mtf_state.as_mut_ptr(),
        )
    };

    if result.status_code == 0 {
        let bytes_written = usize::try_from(result.bytes_written)
            .map_err(|_| "JIT output length overflow".to_string())?;
        if bytes_written > output_buffer.len() {
            return Err("JIT reported output beyond the supplied buffer".to_string());
        }
        let final_data = inverse_bwt(
            &output_buffer[..bytes_written],
            block.primary_index as usize,
        );
        Ok(final_data)
    } else {
        Err(format!(
            "JIT fault on block {block_idx}: code {}",
            result.status_code
        ))
    }
}

pub fn extract_archive(
    archive: &ArchiveView<'_>,
    key: Option<&[u8; 32]>,
    output_dir: &Path,
    mut progress_callback: impl FnMut(u64, u64),
) -> Result<(), String> {
    let meta_data = if let Some(k) = key {
        decrypt_data_with_aad(
            archive.metadata.body,
            k,
            &archive.metadata.nonce,
            archive.header_bytes,
        )?
    } else {
        if archive.is_encrypted() {
            return Err("archive is encrypted but no decryption key was provided".to_string());
        }
        archive.metadata.body.to_vec()
    };

    let files = parse_metadata(&meta_data).map_err(|e| format!("failed to parse metadata: {e}"))?;

    let metadata_size = files
        .iter()
        .try_fold(0u64, |total, entry| total.checked_add(entry.size))
        .ok_or_else(|| "metadata size overflow".to_string())?;
    if metadata_size != archive.header.uncompressed_size {
        return Err("metadata size does not match archive size".to_string());
    }

    let output_capacity = usize::try_from(archive.header.uncompressed_size)
        .map_err(|_| "archive is too large for this platform".to_string())?;
    let mut final_output = Vec::with_capacity(output_capacity);

    for (block_idx, block) in archive.blocks.iter().enumerate() {
        progress_callback((block_idx + 1) as u64, archive.header.num_blocks);
        let decompressed = decompress_block(block, key, block_idx, archive.header_bytes)?;
        final_output.extend_from_slice(&decompressed);
    }

    if final_output.len() != output_capacity {
        return Err("decompressed output size does not match archive header".to_string());
    }

    let mut current_pos: usize = 0;
    for entry in &files {
        let size = usize::try_from(entry.size)
            .map_err(|_| "file is too large for this platform".to_string())?;
        let full_path = extraction_path(output_dir, entry.path)
            .map_err(|e| format!("unsafe extraction path: {e}"))?;
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
        }
        let mut out_file = File::create(&full_path)
            .map_err(|e| format!("failed to create file {}: {e}", full_path.display()))?;
        let next_pos = current_pos
            .checked_add(size)
            .ok_or_else(|| "file size overflow".to_string())?;
        let data = final_output
            .get(current_pos..next_pos)
            .ok_or_else(|| "file metadata exceeds output".to_string())?;
        out_file
            .write_all(data)
            .map_err(|e| format!("failed to write to file {}: {e}", full_path.display()))?;
        current_pos = next_pos;
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub struct QueryMatch {
    pub path: String,
    pub offset: usize,
}

pub fn query_archive(
    archive: &ArchiveView<'_>,
    key: Option<&[u8; 32]>,
    pattern: &str,
) -> Result<Vec<QueryMatch>, String> {
    let meta_data = if let Some(k) = key {
        decrypt_data_with_aad(
            archive.metadata.body,
            k,
            &archive.metadata.nonce,
            archive.header_bytes,
        )?
    } else {
        if archive.is_encrypted() {
            return Err("archive is encrypted but no decryption key was provided".to_string());
        }
        archive.metadata.body.to_vec()
    };

    let files = parse_metadata(&meta_data).map_err(|e| format!("failed to parse metadata: {e}"))?;

    let mut query_matches = Vec::new();
    let mut current_pos = 0usize;

    for (block_idx, block) in archive.blocks.iter().enumerate() {
        let body = if let Some(k) = key {
            decrypt_data_with_aad(
                block.body,
                k,
                &block.nonce,
                &block_aad(
                    archive.header_bytes,
                    block_idx as u64,
                    block.primary_index,
                    block.uncompressed_size,
                    block.code_lengths_len,
                    block.bitstream_len,
                ),
            )?
        } else {
            block.body.to_vec()
        };

        if body.len() < block.code_lengths_len as usize {
            return Err("block body is shorter than Huffman code lengths".to_string());
        }
        let tree_payload = &body[..block.code_lengths_len as usize];
        let bitstream = &body[block.code_lengths_len as usize..];

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

        let mut bwt_buffer = vec![0u8; block.uncompressed_size as usize];
        let mut mtf_state: Vec<u8> = (0..=255).collect();
        let mut matches = vec![0u64; 1024];

        let result = unsafe {
            compile_and_run_query(
                ir_payload.as_ptr(),
                ir_payload.len() as u64,
                bitstream.as_ptr(),
                bwt_buffer.as_mut_ptr(),
                block.uncompressed_size as u64,
                mtf_state.as_mut_ptr(),
                pattern.as_ptr(),
                pattern.len() as u64,
                matches.as_mut_ptr(),
                matches.len() as u64,
                block.primary_index as u64,
            )
        };

        if result.status_code == 0 {
            let n = result.bytes_written as usize;
            for &m in matches.iter().take(n) {
                let match_off = m as usize;
                let mut file_off = 0;
                for entry in &files {
                    let size = usize::try_from(entry.size)
                        .map_err(|_| "file is too large for this platform".to_string())?;
                    if current_pos + match_off >= file_off
                        && current_pos + match_off < file_off + size
                    {
                        query_matches.push(QueryMatch {
                            path: entry.path.to_string(),
                            offset: current_pos + match_off - file_off,
                        });
                        break;
                    }
                    file_off += size;
                }
            }
        } else {
            return Err(format!(
                "JIT fault on query block {block_idx}: code {}",
                result.status_code
            ));
        }

        current_pos += block.uncompressed_size as usize;
    }

    Ok(query_matches)
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
}

pub fn inspect_metadata(
    archive: &ArchiveView<'_>,
    key: Option<&[u8; 32]>,
) -> Result<Vec<FileEntry>, String> {
    let meta_data = if let Some(k) = key {
        decrypt_data_with_aad(
            archive.metadata.body,
            k,
            &archive.metadata.nonce,
            archive.header_bytes,
        )?
    } else {
        if archive.is_encrypted() {
            return Err("archive is encrypted but no decryption key was provided".to_string());
        }
        archive.metadata.body.to_vec()
    };

    let files_view =
        parse_metadata(&meta_data).map_err(|e| format!("failed to parse metadata: {e}"))?;

    let files = files_view
        .into_iter()
        .map(|f| FileEntry {
            path: f.path.to_string(),
            size: f.size,
        })
        .collect();

    Ok(files)
}

pub fn extract_file(
    archive: &ArchiveView<'_>,
    key: Option<&[u8; 32]>,
    target_path: &str,
) -> Result<Vec<u8>, String> {
    let meta_data = if let Some(k) = key {
        decrypt_data_with_aad(
            archive.metadata.body,
            k,
            &archive.metadata.nonce,
            archive.header_bytes,
        )?
    } else {
        if archive.is_encrypted() {
            return Err("archive is encrypted but no decryption key was provided".to_string());
        }
        archive.metadata.body.to_vec()
    };

    let files = parse_metadata(&meta_data).map_err(|e| format!("failed to parse metadata: {e}"))?;

    let mut file_start = 0usize;
    let mut target_entry = None;
    for entry in &files {
        let size = usize::try_from(entry.size)
            .map_err(|_| "file is too large for this platform".to_string())?;
        if entry.path == target_path {
            target_entry = Some((file_start, file_start + size));
            break;
        }
        file_start += size;
    }

    let (file_start, file_end) = match target_entry {
        Some(range) => range,
        None => return Err(format!("file not found in archive: {target_path}")),
    };

    let mut file_data = Vec::with_capacity(file_end - file_start);
    let mut current_block_start = 0usize;

    for (block_idx, block) in archive.blocks.iter().enumerate() {
        let block_len = block.uncompressed_size as usize;
        let current_block_end = current_block_start + block_len;

        if current_block_start < file_end && current_block_end > file_start {
            let decompressed = decompress_block(block, key, block_idx, archive.header_bytes)?;
            let start_in_block = file_start.saturating_sub(current_block_start);
            let end_in_block = if file_end < current_block_end {
                file_end - current_block_start
            } else {
                block_len
            };
            file_data.extend_from_slice(&decompressed[start_in_block..end_in_block]);
        }

        current_block_start = current_block_end;
        if current_block_start >= file_end {
            break;
        }
    }

    Ok(file_data)
}

#[cfg(test)]
mod archive_tests {
    use super::*;

    fn sample_archive() -> Vec<u8> {
        let metadata = [1u8, 0, 0, 0, 1, 0, b'a', 1, 0, 0, 0, 0, 0, 0, 0];
        let code_lengths = vec![0u8; 259];
        let bitstream = [0u8];
        let block_body = [code_lengths, bitstream.to_vec()].concat();
        let metadata_size = 28 + metadata.len();
        let mut archive = Vec::new();
        archive.extend_from_slice(ARCHIVE_MAGIC);
        archive.extend_from_slice(&ARCHIVE_VERSION.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&1u64.to_le_bytes());
        archive.extend_from_slice(&1u64.to_le_bytes());
        archive.extend_from_slice(&0u32.to_le_bytes());
        archive.extend_from_slice(&[0u8; 16]);
        archive.extend_from_slice(&(metadata_size as u64).to_le_bytes());
        archive.extend_from_slice(&[0u8; 4]);
        archive.extend_from_slice(&[0u8; 24]);
        archive.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
        archive.extend_from_slice(&metadata);
        archive.extend_from_slice(&[0u8; 24]);
        archive.extend_from_slice(&(block_body.len() as u32).to_le_bytes());
        archive.extend_from_slice(&0u32.to_le_bytes());
        archive.extend_from_slice(&1u32.to_le_bytes());
        archive.extend_from_slice(&259u32.to_le_bytes());
        archive.extend_from_slice(&1u32.to_le_bytes());
        archive.extend_from_slice(&block_body);
        archive
    }

    #[test]
    fn parses_valid_archive_framing() {
        let archive = sample_archive();
        let parsed = parse_archive(&archive, ArchiveLimits::default()).unwrap();
        assert_eq!(parsed.header.uncompressed_size, 1);
        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.blocks[0].body.len(), 260);
        assert_eq!(parse_metadata(parsed.metadata.body).unwrap()[0].path, "a");

        let inspected = inspect_metadata(&parsed, None).unwrap();
        assert_eq!(inspected.len(), 1);
        assert_eq!(inspected[0].path, "a");
        assert_eq!(inspected[0].size, 1);
    }

    #[test]
    fn test_bwt_roundtrip() {
        let original = b"Hello, this is a longer text to test the BWT and inverse BWT roundtrip. Let's make sure it doesn't scramble anything!";
        let (bwt_data, prim) = bwt(original);
        let inverted = inverse_bwt(&bwt_data, prim);
        assert_eq!(original.to_vec(), inverted);
    }

    #[test]
    fn rejects_truncated_and_inconsistent_archives() {
        let archive = sample_archive();
        assert!(matches!(
            parse_archive(
                &archive[..ARCHIVE_HEADER_SIZE - 1],
                ArchiveLimits::default()
            ),
            Err(ArchiveError::Truncated(_))
        ));

        let mut invalid = archive;
        invalid[44..52].copy_from_slice(&28u64.to_le_bytes());
        assert!(matches!(
            parse_archive(&invalid, ArchiveLimits::default()),
            Err(ArchiveError::Invalid(_)) | Err(ArchiveError::Truncated(_))
        ));

        let mut invalid_reserved = sample_archive();
        invalid_reserved[52] = 1;
        assert!(matches!(
            parse_archive(&invalid_reserved, ArchiveLimits::default()),
            Err(ArchiveError::Invalid("reserved header bytes"))
        ));
    }

    #[test]
    fn rejects_archive_above_uncompressed_limit() {
        let mut archive = sample_archive();
        archive[8..16].copy_from_slice(&2u64.to_le_bytes());
        let limits = ArchiveLimits {
            max_uncompressed_size: 1,
            ..ArchiveLimits::default()
        };
        assert!(matches!(
            parse_archive(&archive, limits),
            Err(ArchiveError::LimitExceeded("uncompressed size"))
        ));
    }

    #[test]
    fn extraction_paths_stay_below_output_root() {
        let root = Path::new("output");
        assert_eq!(
            extraction_path(root, "nested/file.txt").unwrap(),
            PathBuf::from("output/nested/file.txt")
        );
        assert_eq!(
            extraction_path(root, "/absolute").unwrap(),
            PathBuf::from("output/absolute")
        );
        for path in ["../escape", "", "."] {
            assert!(
                extraction_path(root, path).is_err(),
                "{path} should be rejected"
            );
        }
    }

    #[test]
    fn authenticated_encryption_rejects_changed_associated_data() {
        let key = [7u8; 32];
        let (ciphertext, nonce) = encrypt_data_with_aad(b"payload", &key, b"header");
        assert_eq!(
            decrypt_data_with_aad(&ciphertext, &key, &nonce, b"header").unwrap(),
            b"payload"
        );
        assert!(decrypt_data_with_aad(&ciphertext, &key, &nonce, b"changed").is_err());
    }
}
