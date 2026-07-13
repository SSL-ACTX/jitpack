const std = @import("std");

threadlocal var next_pos_buffer: [256 * 1024]u32 align(64) = undefined;
threadlocal var counts_buffer: [256]usize = undefined;
threadlocal var starts_buffer: [256]usize = undefined;

pub const Assembler = struct {
    buffer: []u8,
    pos: usize = 0,

    pub fn emit_byte(self: *Assembler, b: u8) void {
        self.buffer[self.pos] = b;
        self.pos += 1;
    }

    pub fn emit_u32(self: *Assembler, val: u32) void {
        std.mem.writeInt(u32, self.buffer[self.pos..][0..4], val, .little);
        self.pos += 4;
    }
};

pub fn compile_and_run(ir: []const u8, bitstream_ptr: [*]const u8, output_ptr: [*]u8, output_limit: u64, mtf_ptr: [*]u8) !u64 {
    const ir_len = ir.len;
    const os = @import("../jit_engine.zig").os;

    // Pass 1: Sizing and Mapping
    const map_size = (ir_len + 1) * 4;
    const rounded_map_size = (map_size + 4095) & ~@as(usize, 4095);
    const map_ptr = os.syscall6(os.sys_mmap, 0, rounded_map_size, 3, 0x22, @bitCast(@as(isize, -1)), 0);
    if (@as(isize, @bitCast(map_ptr)) < 0) return error.MmapFailed;
    const ir_to_mc = @as([*]u32, @ptrCast(@alignCast(@as(*anyopaque, @ptrFromInt(map_ptr)))))[0..(ir_len+1)];
    defer _ = os.syscall2(os.sys_munmap, map_ptr, rounded_map_size);

    var mc_len: u32 = 40; 
    var ip: usize = 0;
    while (ip < ir_len) {
        ir_to_mc[ip] = mc_len;
        const opcode = ir[ip];
        if (opcode == 0x01) { mc_len += 60; ip += 9; }
        else if (opcode == 0x02) { mc_len += 100; ip += 2; }
        else if (opcode == 0x03) { mc_len += 15; ip += 1; }
        else if (opcode == 0x04) { mc_len += 15; ip += 1; }
        else if (opcode == 0x07) { mc_len += 5; ip += 5; }
        else if (opcode == 0x0F) { mc_len += 50; ip += 1; }
        else return error.InvalidOpcode;
    }
    ir_to_mc[ir_len] = mc_len;

    const JitBuffer = @import("../jit_engine.zig").JitBuffer;
    var jit_buf = JitBuffer.init(mc_len) orelse return error.JitBufferInitFailed;
    defer jit_buf.deinit();

    var as = Assembler{ .buffer = jit_buf.ptr[0..jit_buf.size] };

    // Prologue
    as.emit_byte(0x53); // push rbx
    as.emit_byte(0x41); as.emit_byte(0x54); // push r12
    as.emit_byte(0x48); as.emit_byte(0x89); as.emit_byte(0xf3); // mov rbx, rsi
    as.emit_byte(0x41); as.emit_byte(0xb8); as.emit_u32(0); // mov r8d, 0
    as.emit_byte(0x49); as.emit_byte(0xc7); as.emit_byte(0xc1); as.emit_u32(0); // mov r9, 0
    as.emit_byte(0x49); as.emit_byte(0xc7); as.emit_byte(0xc2); as.emit_u32(1); // mov r10, 1

    ip = 0;
    while (ip < ir_len) {
        ir_to_mc[ip] = @intCast(as.pos);
        const opcode = ir[ip];
        if (opcode == 0x01) {
            as.emit_byte(0x45); as.emit_byte(0x85); as.emit_byte(0xc0); 
            const skip_load = as.pos; as.emit_byte(0x75); as.emit_byte(0x00);
            as.emit_byte(0x44); as.emit_byte(0x0f); as.emit_byte(0xb6); as.emit_byte(0x1f); // movzx r11d, byte ptr [rdi]
            as.emit_byte(0x48); as.emit_byte(0xff); as.emit_byte(0xc7); // inc rdi
            as.emit_byte(0x41); as.emit_byte(0xb8); as.emit_u32(8); // mov r8d, 8
            as.buffer[skip_load+1] = @intCast(as.pos - (skip_load+2));
            as.emit_byte(0x44); as.emit_byte(0x89); as.emit_byte(0xda); as.emit_byte(0x83); as.emit_byte(0xe2); as.emit_byte(0x01);
            as.emit_byte(0x41); as.emit_byte(0xd1); as.emit_byte(0xeb); as.emit_byte(0x41); as.emit_byte(0xff); as.emit_byte(0xc8);
            as.emit_byte(0x85); as.emit_byte(0xd2);
            as.emit_byte(0x0f); as.emit_byte(0x84); ir_to_mc[ip+1] = @intCast(as.pos); as.emit_u32(0);
            as.emit_byte(0xe9); ir_to_mc[ip+5] = @intCast(as.pos); as.emit_u32(0);
            ip += 9;
        } else if (opcode == 0x02) {
             as.emit_byte(0x4d); as.emit_byte(0x85); as.emit_byte(0xc9); const skip_run = as.pos; as.emit_byte(0x74); as.emit_byte(0x00);
             as.emit_byte(0x8a); as.emit_byte(0x01); const loop_run = as.pos; as.emit_byte(0x88); as.emit_byte(0x06);
             as.emit_byte(0x48); as.emit_byte(0xff); as.emit_byte(0xc6); as.emit_byte(0x49); as.emit_byte(0xff); as.emit_byte(0xc9);
             as.emit_byte(0x75);
             const diff1 = @as(i8, @truncate(@as(isize, @intCast(loop_run)) - @as(isize, @intCast(as.pos + 1))));
             as.emit_byte(@bitCast(diff1));
             as.emit_byte(0x49); as.emit_byte(0xc7); as.emit_byte(0xc2); as.emit_u32(1);
             as.buffer[skip_run+1] = @intCast(as.pos - (skip_run+2));
             const index = ir[ip+1]; as.emit_byte(0x48); as.emit_byte(0x31); as.emit_byte(0xd2); as.emit_byte(0xb2); as.emit_byte(index);
             as.emit_byte(0x44); as.emit_byte(0x8a); as.emit_byte(0x24); as.emit_byte(0x11);
             as.emit_byte(0x44); as.emit_byte(0x88); as.emit_byte(0x26);
             as.emit_byte(0x48); as.emit_byte(0xff); as.emit_byte(0xc6); as.emit_byte(0x85); as.emit_byte(0xd2); const skip_mtf = as.pos; as.emit_byte(0x74); as.emit_byte(0x00);
             const loop_mtf = as.pos; as.emit_byte(0x8a); as.emit_byte(0x44); as.emit_byte(0x11); as.emit_byte(0xff); as.emit_byte(0x88); as.emit_byte(0x04); as.emit_byte(0x11);
             as.emit_byte(0x48); as.emit_byte(0xff); as.emit_byte(0xca); as.emit_byte(0x75);
             const diff2 = @as(i8, @truncate(@as(isize, @intCast(loop_mtf)) - @as(isize, @intCast(as.pos + 1))));
             as.emit_byte(@bitCast(diff2));
             as.emit_byte(0x44); as.emit_byte(0x88); as.emit_byte(0x21); as.buffer[skip_mtf+1] = @intCast(as.pos - (skip_mtf+2));
             ip += 2;
        } else if (opcode == 0x03) {
            as.emit_byte(0x4d); as.emit_byte(0x01); as.emit_byte(0xd1); as.emit_byte(0x4d); as.emit_byte(0x01); as.emit_byte(0xd2); ip += 1;
        } else if (opcode == 0x04) {
            as.emit_byte(0x4c); as.emit_byte(0x89); as.emit_byte(0xd0); // mov rax, r10
            as.emit_byte(0x48); as.emit_byte(0x01); as.emit_byte(0xc0); // add rax, rax
            as.emit_byte(0x49); as.emit_byte(0x01); as.emit_byte(0xc1); // add r9, rax
            as.emit_byte(0x4d); as.emit_byte(0x01); as.emit_byte(0xd2); // add r10, r10
            ip += 1;
        } else if (opcode == 0x07) {
            as.emit_byte(0xe9); ir_to_mc[ip+1] = @intCast(as.pos); as.emit_u32(0); ip += 5;
        } else if (opcode == 0x0F) {
             as.emit_byte(0x4d); as.emit_byte(0x85); as.emit_byte(0xc9); const skip_run = as.pos; as.emit_byte(0x74); as.emit_byte(0x00);
             as.emit_byte(0x8a); as.emit_byte(0x01); const loop_run = as.pos; as.emit_byte(0x88); as.emit_byte(0x06);
             as.emit_byte(0x48); as.emit_byte(0xff); as.emit_byte(0xc6); as.emit_byte(0x49); as.emit_byte(0xff); as.emit_byte(0xc9);
             as.emit_byte(0x75);
             const diff2 = @as(i8, @truncate(@as(isize, @intCast(loop_run)) - @as(isize, @intCast(as.pos + 1))));
             as.emit_byte(@bitCast(diff2));
             as.buffer[skip_run+1] = @intCast(as.pos - (skip_run+2));
             as.emit_byte(0x48); as.emit_byte(0x89); as.emit_byte(0xf0); as.emit_byte(0x48); as.emit_byte(0x29); as.emit_byte(0xd8);
             as.emit_byte(0x41); as.emit_byte(0x5c); // pop r12
             as.emit_byte(0x5b); // pop rbx
             as.emit_byte(0xc3); // ret
             ip += 1;
        }
    }

    // Pass 2: Fixups
    ip = 0;
    while (ip < ir_len) {
        const opcode = ir[ip];
        if (opcode == 0x01) {
            const lbl0 = @as(u32, ir[ip+1]) | (@as(u32, ir[ip+2])<<8) | (@as(u32, ir[ip+3])<<16) | (@as(u32, ir[ip+4])<<24);
            const lbl1 = @as(u32, ir[ip+5]) | (@as(u32, ir[ip+6])<<8) | (@as(u32, ir[ip+7])<<16) | (@as(u32, ir[ip+8])<<24);
            const pos0 = ir_to_mc[ip+1]; const target0 = ir_to_mc[lbl0];
            std.mem.writeInt(u32, as.buffer[pos0..][0..4], @bitCast(@as(i32, @intCast(target0)) - @as(i32, @intCast(pos0 + 4))), .little);
            const pos1 = ir_to_mc[ip+5]; const target1 = ir_to_mc[lbl1];
            std.mem.writeInt(u32, as.buffer[pos1..][0..4], @bitCast(@as(i32, @intCast(target1)) - @as(i32, @intCast(pos1 + 4))), .little);
            ip += 9;
        } else if (opcode == 0x07) {
            const lbl = @as(u32, ir[ip+1]) | (@as(u32, ir[ip+2])<<8) | (@as(u32, ir[ip+3])<<16) | (@as(u32, ir[ip+4])<<24);
            const pos = ir_to_mc[ip+1]; const target = ir_to_mc[lbl];
            std.mem.writeInt(u32, as.buffer[pos..][0..4], @bitCast(@as(i32, @intCast(target)) - @as(i32, @intCast(pos + 4))), .little);
            ip += 5;
        } else if (opcode == 0x02) { ip += 2; }
        else { ip += 1; }
    }

    if (!jit_buf.makeExecutable()) return error.MakeExecutableFailed;
    const JitFn = *const fn (src: [*]const u8, dest: [*]u8, limit: [*]u8, mtf: [*]u8) callconv(.c) u64;
    const func: JitFn = @ptrCast(jit_buf.ptr);
    return func(bitstream_ptr, output_ptr, output_ptr + output_limit, mtf_ptr);
}

pub fn compile_and_run_query(
    ir: []const u8,
    bitstream_ptr: [*]const u8,
    output_ptr: [*]u8,
    output_limit: u64,
    mtf_ptr: [*]u8,
    pattern_ptr: [*]const u8,
    pattern_len: u64,
    matches_ptr: [*]u64,
    matches_limit: u64,
    primary_idx: u64,
) !u64 {
    const written = try compile_and_run(ir, bitstream_ptr, output_ptr, output_limit, mtf_ptr);
    const bwt_data = output_ptr[0..written];

    const counts = counts_buffer[0..256];
    @memset(counts, 0);
    for (bwt_data) |b| counts[b] += 1;

    const starts = starts_buffer[0..256];
    @memset(starts, 0);
    var sum: usize = 0;
    for (0..256) |i| {
        starts[i] = sum;
        sum += counts[i];
    }

    if (written > next_pos_buffer.len) return error.BlockSizeTooLarge;
    const next_pos = next_pos_buffer[0..written];
    for (bwt_data, 0..) |b, i| {
        next_pos[starts[b]] = @intCast(i);
        starts[b] += 1;
    }

    var curr = primary_idx;
    var state: usize = 0;
    var match_count: u64 = 0;
    var pos: u64 = 0;

    const p = pattern_ptr[0..pattern_len];

    for (0..written) |_| {
        curr = next_pos[@intCast(curr)];
        const char = bwt_data[@intCast(curr)];

        if (char == p[state]) {
            state += 1;
            if (state == pattern_len) {
                if (match_count < matches_limit) {
                    matches_ptr[match_count] = pos + 1 - pattern_len;
                    match_count += 1;
                } else {
                    break;
                }
                state = 0;
            }
        } else {
            state = 0;
            if (char == p[0]) {
                state = 1;
            }
        }
        pos += 1;
    }

    return match_count;
}

