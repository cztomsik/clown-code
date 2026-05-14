const std = @import("std");
const tk = @import("tokamak");

const FILE = "debug.log";

// Custom std.log implementation that append-writes everything to a file.
pub fn debugLog(
    comptime level: std.log.Level,
    comptime scope: anytype,
    comptime format: []const u8,
    args: anytype,
) void {
    // NOTE: This could still race but I don't want to waste any more time with legacy I/O
    const file = std.fs.cwd().openFile(FILE, .{ .mode = .read_write, .lock = .exclusive }) catch std.fs.cwd().createFile(FILE, .{}) catch return;
    defer file.close();

    file.seekFromEnd(0) catch {};

    var buf: [4096]u8 = undefined;
    var bw = file.writerStreaming(&buf); // NOTE: writer() ignores seeking entirely, and it took me a while to figure it out!
    var w = &bw.interface;
    defer w.flush() catch {};

    w.print("{f} {s} {s} ", .{ tk.time.Time.now(), @tagName(level), @tagName(scope) }) catch @panic("PRINT");

    const msg_fmt = format ++ "\n";
    w.print(msg_fmt, args) catch {};
}
