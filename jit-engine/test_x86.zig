const std = @import("std");
const builtin = @import("builtin");
const jit_engine = @import("jit_engine.zig");
const arch = @import("arch/x86_64.zig");

pub const panic = jit_engine.panic;

fn test_tree_decompress() !void {
    // IR for decoding "A" (bit 0) and "B" (bit 1)
    // Root BranchBit (ip=0) -> left (ip=9), right (ip=11)
    // Left: EmitMTF(0), Terminate (writes MTF[0])
    // Right: EmitMTF(1), Terminate (writes MTF[1])
    const ir = [_]u8{
        0x01, 9, 0, 0, 0, 23, 0, 0, 0, // 0: BranchBit left=9, right=23
        0x02, 0,  // 9: EmitMTF(0)
        0x07, 0, 0, 0, 0, // 11: Jump to 0
        0x02, 1,  // 16: EmitMTF(1)
        0x07, 0, 0, 0, 0, // 18: Jump to 0
        0x0F,     // 23: Terminate
    };
    
    std.debug.print("test_tree_decompress running\n", .{});
    var out: [64]u8 = std.mem.zeroes([64]u8);
    var mtf: [256]u8 = undefined;
    for (0..256) |i| mtf[i] = @intCast(i);
    // Bit 0 = 0 (A), bit 1 = 1 (B), bit 2 = 0 (A) -> 0b00000010 (wait, LSB is bit 0? No, bitstream is shifted right.
    // first bit is bit 0. 0, 1, 0 -> 0x02.
    var src = [16]u8{ 0b00000110, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0 };

    const written = try arch.compile_and_run(&ir, &src, &out, 3, &mtf);
    std.debug.print("written: {}\n", .{written});
    std.debug.print("out: {x} {x} {x}\n", .{out[0], out[1], out[2]});
}

pub fn main() void {
    test_tree_decompress() catch unreachable;
}
