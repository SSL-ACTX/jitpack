const std = @import("std");

threadlocal var ir_to_mc_buffer: [8192]u32 align(64) = undefined;
threadlocal var next_pos_buffer: [256 * 1024]u32 align(64) = undefined;
threadlocal var counts_buffer: [256]usize = undefined;
threadlocal var starts_buffer: [256]usize = undefined;

pub const Assembler = struct {
    buffer: []u8,
    pos: usize = 0,

    pub fn emit(self: *Assembler, instr: u32) void {
        self.buffer[self.pos] = @truncate(instr);
        self.buffer[self.pos + 1] = @truncate(instr >> 8);
        self.buffer[self.pos + 2] = @truncate(instr >> 16);
        self.buffer[self.pos + 3] = @truncate(instr >> 24);
        self.pos += 4;
    }

    pub fn b_cond(self: *Assembler, cond: u4, target_mc: u32) void {
        const current_pc = self.pos;
        const offset_bytes = @as(i64, target_mc) - @as(i64, @intCast(current_pc));
        const offset_instrs = @as(i32, @intCast(@divExact(offset_bytes, 4)));
        const imm19 = @as(u32, @bitCast(offset_instrs)) & 0x7FFFF;
        self.emit(0x54000000 | (imm19 << 5) | cond);
    }

    pub fn b_uncond(self: *Assembler, target_mc: u32) void {
        const current_pc = self.pos;
        const offset_bytes = @as(i64, target_mc) - @as(i64, @intCast(current_pc));
        const offset_instrs = @as(i32, @intCast(@divExact(offset_bytes, 4)));
        const imm26 = @as(u32, @bitCast(offset_instrs)) & 0x03FFFFFF;
        self.emit(0x14000000 | imm26);
    }

    pub fn mov_imm_w(self: *Assembler, rd: u5, imm: u16) void {
        self.emit(0x52800000 | (@as(u32, imm) << 5) | rd);
    }
};

pub fn compile_and_run(ir: []const u8, bitstream_ptr: [*]const u8, output_ptr: [*]u8, output_limit: u64, mtf_ptr: [*]u8) !u64 {
    return compile_internal(ir, bitstream_ptr, output_ptr, output_limit, mtf_ptr, null, 0, null, 0, 0);
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
    return compile_internal(ir, bitstream_ptr, output_ptr, output_limit, mtf_ptr, pattern_ptr, pattern_len, matches_ptr, matches_limit, primary_idx);
}

fn compile_internal(
    ir: []const u8,
    bitstream_ptr: [*]const u8,
    output_ptr: [*]u8,
    output_limit: u64,
    mtf_ptr: [*]u8,
    pattern_ptr: ?[*]const u8,
    pattern_len: u64,
    matches_ptr: ?[*]u64,
    matches_limit: u64,
    primary_idx: u64,
) !u64 {
    const ir_len = ir.len;

    var mc_len: u32 = 64; 
    var ip: usize = 0;
    while (ip < ir_len) {
        const opcode = ir[ip];
        if (opcode == 0x01) { mc_len += 44; ip += 9; }
        else if (opcode == 0x02) { mc_len += 80; ip += 2; }
        else if (opcode == 0x03) { mc_len += 8; ip += 1; }
        else if (opcode == 0x04) { mc_len += 12; ip += 1; }
        else if (opcode == 0x07) { mc_len += 4; ip += 5; }
        else if (opcode == 0x0F) { mc_len += 36; ip += 1; }
        else return error.InvalidOpcode;
    }

    const JitBuffer = @import("../jit_engine.zig").JitBuffer;
    var jit_buf = JitBuffer.init(mc_len) orelse return error.JitBufferInitFailed;
    defer jit_buf.deinit();

    var as = Assembler{ .buffer = jit_buf.ptr[0..jit_buf.size] };
    if (ir_len + 1 > ir_to_mc_buffer.len) return error.IrTooLong;
    const ir_to_mc = ir_to_mc_buffer[0..(ir_len + 1)];

    as.emit(0xaa0103e7); // mov x7, x1
    as.emit(0x52800009); // mov w9, #0
    as.emit(0xd280000c); // mov x12, #0
    as.emit(0xd280002d); // mov x13, #1

    ip = 0;
    while (ip < ir_len) {
        ir_to_mc[ip] = @intCast(as.pos);
        const opcode = ir[ip];
        if (opcode == 0x01) {
            as.emit(0x7100013f); as.emit(0x54000061); as.emit(0x38401408); as.emit(0x52800109); as.emit(0x12000104);
            as.emit(0x53017d08); as.emit(0x51000529); as.emit(0x7100009f);
            as.b_cond(1, @as(u32, @intCast(as.pos)) + 8);
            as.emit(0xd503201f); as.emit(0xd503201f); // placeholders
            ip += 9;
        } else if (opcode == 0x02) {
            as.emit(0xf100019f); const skip_run = as.pos; as.emit(0x0); as.emit(0x3940006e); 
            const loop_run = as.pos; as.emit(0x3800142e); as.emit(0xf100058c); as.b_cond(1, @as(u32, @intCast(loop_run)));
            as.emit(0xd280002d);
            const skip_run_target = @as(u32, @intCast(as.pos)); const old_pos_run = as.pos; as.pos = skip_run; as.b_cond(0, skip_run_target); as.pos = old_pos_run;
            const index = ir[ip+1]; as.mov_imm_w(4, index); as.emit(0x8b244066); as.emit(0x394000c5); as.emit(0x38001425); as.emit(0x7100009f);
            const skip_loop = as.pos; as.emit(0x0);
            const loop_start = as.pos;
            as.emit(0x385ffccb); // ldrb w11, [x6, #-1]!
            as.emit(0x380018cb); // strb w11, [x6, #1]
            as.emit(0xeb0300df); // cmp x6, x3
            as.b_cond(1, @as(u32, @intCast(loop_start)));
            const done_pos = @as(u32, @intCast(as.pos)); const old_pos = as.pos; as.pos = skip_loop; as.b_cond(0, done_pos); as.pos = old_pos;
            as.emit(0x39000065); ip += 2;
        } else if (opcode == 0x03) {
            as.emit(0x8b0d018c); as.emit(0x8b0d01ad); ip += 1;
        } else if (opcode == 0x04) {
            as.emit(0x8b0d01ae); as.emit(0x8b0e018c); as.emit(0x8b0d01ad); ip += 1;
        } else if (opcode == 0x07) {
            as.emit(0xd503201f); ip += 5;
        } else if (opcode == 0x0F) {
            as.emit(0xf100019f); const skip_run = as.pos; as.emit(0x0); as.emit(0x3940006e); 
            const loop_run = as.pos; as.emit(0x3800142e); as.emit(0xf100058c); as.b_cond(1, @as(u32, @intCast(loop_run)));
            as.emit(0xd280002d);
            const skip_run_target = @as(u32, @intCast(as.pos)); const old_pos_run = as.pos; as.pos = skip_run; as.b_cond(0, skip_run_target); as.pos = old_pos_run;
            as.emit(0xcb070020); as.emit(0xd65f03c0); ip += 1;
        }
    }

    ip = 0;
    while (ip < ir_len) {
        const opcode = ir[ip];
        if (opcode == 0x01) {
            const lbl0 = @as(u32, ir[ip+1]) | (@as(u32, ir[ip+2])<<8) | (@as(u32, ir[ip+3])<<16) | (@as(u32, ir[ip+4])<<24);
            const lbl1 = @as(u32, ir[ip+5]) | (@as(u32, ir[ip+6])<<8) | (@as(u32, ir[ip+7])<<16) | (@as(u32, ir[ip+8])<<24);
            var temp_as = Assembler{ .buffer = jit_buf.ptr[0..jit_buf.size], .pos = ir_to_mc[ip] + 36 };
            temp_as.b_uncond(ir_to_mc[lbl0]);
            temp_as.b_uncond(ir_to_mc[lbl1]);
            ip += 9;
        } else if (opcode == 0x07) {
            const lbl = @as(u32, ir[ip+1]) | (@as(u32, ir[ip+2])<<8) | (@as(u32, ir[ip+3])<<16) | (@as(u32, ir[ip+4])<<24);
            var temp_as = Assembler{ .buffer = jit_buf.ptr[0..jit_buf.size], .pos = ir_to_mc[ip] };
            temp_as.b_uncond(ir_to_mc[lbl]);
            ip += 5;
        } else if (opcode == 0x02) { ip += 2; }
        else { ip += 1; }
    }

    if (!jit_buf.makeExecutable()) return error.MakeExecutableFailed;
    const JitFn = *const fn (src: [*]const u8, dest: [*]u8, limit: [*]u8, mtf: [*]u8) callconv(.c) u64;
    const func: JitFn = @ptrCast(jit_buf.ptr);

    if (pattern_ptr == null) {
        return func(bitstream_ptr, output_ptr, output_ptr + output_limit, mtf_ptr);
    } else {
        const written = func(bitstream_ptr, output_ptr, output_ptr + output_limit, mtf_ptr);
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

        var matcher_buf = JitBuffer.init(16384 + pattern_len * 512) orelse return error.JitBufferInitFailed;
        defer matcher_buf.deinit();
        var mas = Assembler{ .buffer = matcher_buf.ptr[0..matcher_buf.size] };

        // x0 = np, x1 = bwt, x2 = matches, x3 = n, x4 = pri, x5 = lim
        mas.emit(0xaa0403e6); // mov x6, x4
        mas.emit(0xd2800008); // mov x8, #0 (state)
        mas.emit(0xd2800009); // mov x9, #0 (match count)
        mas.emit(0xd280000a); // mov x10, #0 (pos)

        const loop_top = mas.pos;
        mas.emit(0xd37ef4cb); // lsl x11, x6, #2
        mas.emit(0xb86b680b); // ldr w11, [x0, x11]
        mas.emit(0xaa0b03e6); // mov x6, x11
        mas.emit(0x386b682c); // ldrb w12, [x1, x11]

        const p = pattern_ptr.?[0..pattern_len];
        var end_jumps: [1024]usize = undefined;
        var end_jumps_len: usize = 0;

        for (p, 0..) |char, i| {
            mas.emit(0xf1000000 | (@as(u32, @intCast(i)) << 10) | (8 << 5) | 0x1f); // cmp x8, i
            const skip_this_state = mas.pos; mas.emit(0x54000001); // b.ne
            
            mas.emit(0x71000000 | (@as(u32, char) << 10) | (12 << 5) | 0x1f); // cmp w12, char
            const mismatch = mas.pos; mas.emit(0x54000001); // b.ne
            
            mas.emit(0x91000508); // add x8, x8, #1
            if (i == pattern_len - 1) {
                mas.emit(0xeb05013f); // cmp x9, x5
                const skip_ret = mas.pos; mas.emit(0);
                mas.emit(0xaa0903e0); // mov x0, x9
                mas.emit(0xd65f03c0); // ret
                const target = @as(u32, @intCast(mas.pos));
                const old_pos = mas.pos; mas.pos = skip_ret; mas.b_cond(3, target); mas.pos = old_pos;

                mas.emit(0xd100000b | (@as(u32, @intCast(pattern_len - 1)) << 10) | (10 << 5)); // sub x11, x10, #len-1
                mas.emit(0xf800844b); // str x11, [x2], #8
                mas.emit(0x91000529); // add x9, x9, #1
                mas.emit(0xd2800008); // mov x8, #0
            }
            if (end_jumps_len >= 1024) return error.TooManyJumps;
            end_jumps[end_jumps_len] = mas.pos;
            end_jumps_len += 1;
            mas.emit(0x14000000); // b to done_iter
            
            const m_target = @as(u32, @intCast(mas.pos));
            const old_m = mas.pos; mas.pos = mismatch; mas.b_cond(1, m_target); mas.pos = old_m;
            
            mas.emit(0xd2800008); // mov x8, #0
            mas.emit(0x71000000 | (@as(u32, p[0]) << 10) | (12 << 5) | 0x1f); // cmp w12, p[0]
            const not_p0 = mas.pos; mas.emit(0x54000001); // b.ne
            if (pattern_len > 1) mas.emit(0xd2800028); // mov x8, #1
            const np0_target = @as(u32, @intCast(mas.pos));
            const old_np0 = mas.pos; mas.pos = not_p0; mas.b_cond(1, np0_target); mas.pos = old_np0;
            
            if (end_jumps_len >= 1024) return error.TooManyJumps;
            end_jumps[end_jumps_len] = mas.pos;
            end_jumps_len += 1;
            mas.emit(0x14000000); // b to done_iter
            
            const sts_target = @as(u32, @intCast(mas.pos));
            const old_sts = mas.pos; mas.pos = skip_this_state; mas.b_cond(1, sts_target); mas.pos = old_sts;
        }

        const done_iter_target = @as(u32, @intCast(mas.pos));
        for (end_jumps[0..end_jumps_len]) |pos| {
            var temp_as = Assembler{ .buffer = matcher_buf.ptr[0..matcher_buf.size], .pos = pos };
            temp_as.b_uncond(done_iter_target);
        }

        mas.emit(0x9100054a); // add x10, x10, #1
        mas.emit(0xf1000463); // subs x3, x3, #1
        mas.b_cond(1, @as(u32, @intCast(loop_top)));

        mas.emit(0xaa0903e0); // mov x0, x9
        mas.emit(0xd65f03c0); // ret

        if (!matcher_buf.makeExecutable()) return error.MakeExecutableFailed;
        const MatcherFn = *const fn (np: [*]const u32, bwt: [*]const u8, m: [*]u64, n: u64, pri: u64, lim: u64) callconv(.c) u64;
        const matcher: MatcherFn = @ptrCast(matcher_buf.ptr);
        return matcher(next_pos.ptr, bwt_data.ptr, matches_ptr.?, written, primary_idx, matches_limit);
    }
}
