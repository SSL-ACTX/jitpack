const std = @import("std");
const builtin = @import("builtin");
const arch = switch (builtin.cpu.arch) {
    .aarch64 => @import("arch/aarch64.zig"),
    .x86_64 => @import("arch/x86_64.zig"),
    else => @compileError("Unsupported architecture"),
};

pub const os = struct {
    pub const sys_mmap = switch (builtin.cpu.arch) {
        .aarch64 => 222,
        .x86_64 => 9,
        else => @compileError("Unsupported architecture"),
    };
    pub const sys_mprotect = switch (builtin.cpu.arch) {
        .aarch64 => 226,
        .x86_64 => 10,
        else => @compileError("Unsupported architecture"),
    };
    pub const sys_munmap = switch (builtin.cpu.arch) {
        .aarch64 => 215,
        .x86_64 => 11,
        else => @compileError("Unsupported architecture"),
    };
    pub const sys_exit = switch (builtin.cpu.arch) {
        .aarch64 => 93,
        .x86_64 => 60,
        else => @compileError("Unsupported architecture"),
    };

    pub fn syscall2(num: usize, arg1: usize, arg2: usize) usize {
        return switch (builtin.cpu.arch) {
            .aarch64 => asm volatile ("svc #0"
                : [ret] "={x0}" (-> usize),
                : [number] "{x8}" (num),
                  [arg1] "{x0}" (arg1),
                  [arg2] "{x1}" (arg2),
                : .{ .memory = true }
            ),
            .x86_64 => asm volatile ("syscall"
                : [ret] "={rax}" (-> usize),
                : [number] "{rax}" (num),
                  [arg1] "{rdi}" (arg1),
                  [arg2] "{rsi}" (arg2),
                : .{ .rcx = true, .r11 = true, .memory = true }
            ),
            else => @compileError("Unsupported architecture"),
        };
    }

    pub fn syscall3(num: usize, arg1: usize, arg2: usize, arg3: usize) usize {
        return switch (builtin.cpu.arch) {
            .aarch64 => asm volatile ("svc #0"
                : [ret] "={x0}" (-> usize),
                : [number] "{x8}" (num),
                  [arg1] "{x0}" (arg1),
                  [arg2] "{x1}" (arg2),
                  [arg3] "{x2}" (arg3),
                : .{ .memory = true }
            ),
            .x86_64 => asm volatile ("syscall"
                : [ret] "={rax}" (-> usize),
                : [number] "{rax}" (num),
                  [arg1] "{rdi}" (arg1),
                  [arg2] "{rsi}" (arg2),
                  [arg3] "{rdx}" (arg3),
                : .{ .rcx = true, .r11 = true, .memory = true }
            ),
            else => @compileError("Unsupported architecture"),
        };
    }

    pub fn syscall6(
        num: usize,
        arg1: usize,
        arg2: usize,
        arg3: usize,
        arg4: usize,
        arg5: usize,
        arg6: usize,
    ) usize {
        return switch (builtin.cpu.arch) {
            .aarch64 => asm volatile ("svc #0"
                : [ret] "={x0}" (-> usize),
                : [number] "{x8}" (num),
                  [arg1] "{x0}" (arg1),
                  [arg2] "{x1}" (arg2),
                  [arg3] "{x2}" (arg3),
                  [arg4] "{x3}" (arg4),
                  [arg5] "{x4}" (arg5),
                  [arg6] "{x5}" (arg6),
                : .{ .memory = true }
            ),
            .x86_64 => asm volatile ("syscall"
                : [ret] "={rax}" (-> usize),
                : [number] "{rax}" (num),
                  [arg1] "{rdi}" (arg1),
                  [arg2] "{rsi}" (arg2),
                  [arg3] "{rdx}" (arg3),
                  [arg4] "{r10}" (arg4),
                  [arg5] "{r8}" (arg5),
                  [arg6] "{r9}" (arg6),
                : .{ .rcx = true, .r11 = true, .memory = true }
            ),
            else => @compileError("Unsupported architecture"),
        };
    }
};

pub fn panic(msg: []const u8, error_return_trace: ?*std.builtin.StackTrace, ret_addr: ?usize) noreturn {
    _ = msg;
    _ = error_return_trace;
    _ = ret_addr;
    _ = os.syscall2(os.sys_exit, 1, 0);
    while (true) {}
}

pub const DecompressResult = extern struct {
    bytes_written: u64,
    status_code: u32,
};

pub const JitBuffer = struct {
    ptr: [*]align(4096) u8,
    size: usize,

    pub fn init(size: usize) ?JitBuffer {
        const page_size = 4096;
        const rounded_size = (size + page_size - 1) & ~@as(usize, page_size - 1);
        
        const rc = os.syscall6(
            os.sys_mmap,
            0,
            rounded_size,
            1 | 2, // PROT_READ | PROT_WRITE
            2 | 0x20, // MAP_PRIVATE | MAP_ANONYMOUS
            @bitCast(@as(isize, -1)),
            0
        );
        
        const rc_signed: isize = @bitCast(rc);
        if (rc_signed < 0 and rc_signed > -4096) {
            return null;
        }

        return JitBuffer{
            .ptr = @ptrCast(@alignCast(@as(*anyopaque, @ptrFromInt(rc)))),
            .size = rounded_size,
        };
    }

    pub fn makeExecutable(self: *JitBuffer) bool {
        const rc = os.syscall3(
            os.sys_mprotect,
            @intFromPtr(self.ptr),
            self.size,
            1 | 4 // PROT_READ | PROT_EXEC
        );
        
        const rc_signed: isize = @bitCast(rc);
        if (rc_signed < 0 and rc_signed > -4096) {
            return false;
        }

        if (builtin.cpu.arch == .aarch64) {
            const start = @intFromPtr(self.ptr);
            const end = start + self.size;
            const clear_cache = @extern(*const fn (usize, usize) callconv(.c) void, .{ .name = "__clear_cache" });
            clear_cache(start, end);
        }
        return true;
    }

    pub fn deinit(self: *JitBuffer) void {
        _ = os.syscall2(
            os.sys_munmap,
            @intFromPtr(self.ptr),
            self.size
        );
    }
};

export fn compile_and_run_jit(
    ir_ptr: [*]const u8,
    ir_len: u64,
    bitstream_ptr: [*]const u8,
    output_ptr: [*]u8,
    output_limit: u64,
    mtf_ptr: [*]u8,
) DecompressResult {
    const ir = ir_ptr[0..ir_len];
    const written = arch.compile_and_run(ir, bitstream_ptr, output_ptr, output_limit, mtf_ptr) catch {
        return .{ .bytes_written = 0, .status_code = 1 };
    };

    return .{
        .bytes_written = written,
        .status_code = 0,
    };
}

export fn compile_and_run_query(
    ir_ptr: [*]const u8,
    ir_len: u64,
    bitstream_ptr: [*]const u8,
    output_ptr: [*]u8,
    output_limit: u64,
    mtf_ptr: [*]u8,
    pattern_ptr: [*]const u8,
    pattern_len: u64,
    matches_ptr: [*]u64,
    matches_limit: u64,
    primary_idx: u64,
) DecompressResult {
    const ir = ir_ptr[0..ir_len];
    const written = arch.compile_and_run_query(
        ir,
        bitstream_ptr,
        output_ptr,
        output_limit,
        mtf_ptr,
        pattern_ptr,
        pattern_len,
        matches_ptr,
        matches_limit,
        primary_idx,
    ) catch {
        return .{ .bytes_written = 0, .status_code = 1 };
    };
    // Trigger rebuild benchmark

    return .{
        .bytes_written = written,
        .status_code = 0,
    };
}
