const std = @import("std");

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
            as.emit_byte(0x8a); as.emit_byte(0x17); as.emit_byte(0x48); as.emit_byte(0xff); as.emit_byte(0xc7);
            as.emit_byte(0x44); as.emit_byte(0x88); as.emit_byte(0xd3); as.emit_byte(0x41); as.emit_byte(0xb8); as.emit_u32(8);
            as.buffer[skip_load+1] = @intCast(as.pos - (skip_load+2));
            as.emit_byte(0x44); as.emit_byte(0x89); as.emit_byte(0xda); as.emit_byte(0x83); as.emit_byte(0xe2); as.emit_byte(0x01);
            as.emit_byte(0x41); as.emit_byte(0xd1); as.emit_byte(0xeb); as.emit_byte(0x41); as.emit_byte(0xff); as.emit_byte(0xc8);
            as.emit_byte(0x85); as.emit_byte(0xd2);
            as.emit_byte(0x0f); as.emit_byte(0x84); ir_to_mc[ip+1] = @intCast(as.pos); as.emit_u32(0);
            as.emit_byte(0xe9); ir_to_mc[ip+5] = @intCast(as.pos); as.emit_u32(0);
            ip += 9;
        } else if (opcode == 0x02) {
             as.emit_byte(0x4d); as.emit_byte(0x85); as.emit_byte(0xc9); const skip_run = as.pos; as.emit_byte(0x74); as.emit_byte(0x00);
             as.emit_byte(0x44); as.emit_byte(0x8a); as.emit_byte(0x01); const loop_run = as.pos; as.emit_byte(0x44); as.emit_byte(0x88); as.emit_byte(0x06);
             as.emit_byte(0x48); as.emit_byte(0xff); as.emit_byte(0xc6); as.emit_byte(0x49); as.emit_byte(0xff); as.emit_byte(0xc9);
             as.emit_byte(0x75); as.emit_byte(@intCast(@as(i8, @intCast(loop_run)) - @as(i8, @intCast(as.pos + 1))));
             as.emit_byte(0x49); as.emit_byte(0xc7); as.emit_byte(0xc2); as.emit_u32(1);
             as.buffer[skip_run+1] = @intCast(as.pos - (skip_run+2));
             const index = ir[ip+1]; as.emit_byte(0x48); as.emit_byte(0x31); as.emit_byte(0xd2); as.emit_byte(0xb2); as.emit_byte(index);
             as.emit_byte(0x44); as.emit_byte(0x8a); as.emit_byte(0x04); as.emit_byte(0x11); as.emit_byte(0x44); as.emit_byte(0x88); as.emit_byte(0x06);
             as.emit_byte(0x48); as.emit_byte(0xff); as.emit_byte(0xc6); as.emit_byte(0x85); as.emit_byte(0xd2); const skip_mtf = as.pos; as.emit_byte(0x74); as.emit_byte(0x00);
             const loop_mtf = as.pos; as.emit_byte(0x8a); as.emit_byte(0x44); as.emit_byte(0x11); as.emit_byte(0xff); as.emit_byte(0x88); as.emit_byte(0x04); as.emit_byte(0x11);
             as.emit_byte(0x48); as.emit_byte(0xff); as.emit_byte(0xca); as.emit_byte(0x75); as.emit_byte(@intCast(@as(i8, @intCast(loop_mtf)) - @as(i8, @intCast(as.pos + 1))));
             as.emit_byte(0x44); as.emit_byte(0x88); as.emit_byte(0x01); as.buffer[skip_mtf+1] = @intCast(as.pos - (skip_mtf+2));
             ip += 2;
        } else if (opcode == 0x03) {
            as.emit_byte(0x4d); as.emit_byte(0x01); as.emit_byte(0xd1); as.emit_byte(0x4d); as.emit_byte(0x01); as.emit_byte(0xd2); ip += 1;
        } else if (opcode == 0x04) {
            as.emit_byte(0x4d); as.emit_byte(0x89); as.emit_byte(0xd3); as.emit_byte(0x4d); as.emit_byte(0x01); as.emit_byte(0xdb);
            as.emit_byte(0x4d); as.emit_byte(0x01); as.emit_byte(0xd9); as.emit_byte(0x4d); as.emit_byte(0x01); as.emit_byte(0xd2); ip += 1;
        } else if (opcode == 0x07) {
            as.emit_byte(0xe9); ir_to_mc[ip+1] = @intCast(as.pos); as.emit_u32(0); ip += 5;
        } else if (opcode == 0x0F) {
             as.emit_byte(0x4d); as.emit_byte(0x85); as.emit_byte(0xc9); const skip_run = as.pos; as.emit_byte(0x74); as.emit_byte(0x00);
             as.emit_byte(0x44); as.emit_byte(0x8a); as.emit_byte(0x01); const loop_run = as.pos; as.emit_byte(0x44); as.emit_byte(0x88); as.emit_byte(0x06);
             as.emit_byte(0x48); as.emit_byte(0xff); as.emit_byte(0xc6); as.emit_byte(0x49); as.emit_byte(0xff); as.emit_byte(0xc9);
             as.emit_byte(0x75); as.emit_byte(@intCast(@as(i8, @intCast(loop_run)) - @as(i8, @intCast(as.pos + 1))));
             as.buffer[skip_run+1] = @intCast(as.pos - (skip_run+2));
             as.emit_byte(0x48); as.emit_byte(0x89); as.emit_byte(0xf0); as.emit_byte(0x48); as.emit_byte(0x29); as.emit_byte(0xd8); as.emit_byte(0x5b); as.emit_byte(0xc3);
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
