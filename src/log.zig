const std = @import("std");
const tk = @import("tokamak");

// Custom std.log implementation that writes everything to debug.log.
// Opens the file once and appends, replacing the default stderr logging.
var log_file: ?std.fs.File = null;
var log_mutex = std.Thread.Mutex{};

pub fn debugLog(
    comptime level: std.log.Level,
    comptime scope: anytype,
    comptime format: []const u8,
    args: anytype,
) void {
    log_mutex.lock();
    defer log_mutex.unlock();

    if (log_file == null) {
        const file = std.fs.cwd().createFile("debug.log", .{ .truncate = false }) catch return;
        if (file.stat()) |s| file.seekTo(s.size) catch {} else |_| {}
        log_file = file;
    }

    const file = log_file.?;
    var buf: [4096]u8 = undefined;
    var fbs = std.io.fixedBufferStream(&buf);
    var w = fbs.writer();

    const timestamp = tk.time.Time.now();
    timestamp.format(w) catch return;
    w.print(" {s} {s} ", .{ @tagName(level), @tagName(scope) }) catch return;

    const msg_fmt = format ++ "\n";
    w.print(msg_fmt, args) catch return;

    const written = fbs.getWritten();
    file.writeAll(written) catch {};
}
