const std = @import("std");
const io = std.Options.debug_io;

// Custom panic handler that writes everything to error.log file.
pub const panic = std.debug.FullPanic(panicFn);

fn panicFn(msg: []const u8, ra: ?usize) noreturn {
    const file = std.Io.Dir.cwd().createFile(io, "error.log", .{ .truncate = true }) catch std.process.abort();
    errdefer file.close(io);

    var fw = file.writer(io, &.{});
    const term: std.Io.Terminal = .{ .writer = &fw.interface, .mode = .no_color };

    // Write panic message
    fw.interface.print("panic: {s}\n", .{msg}) catch {};

    // Write error return trace (if available)
    if (@errorReturnTrace()) |t| {
        std.debug.writeErrorReturnTrace(t, term) catch {};
    }

    // Write current stack trace
    std.debug.writeCurrentStackTrace(.{ .first_address = ra }, term) catch {};

    // Flush, close, abort
    fw.interface.flush() catch {};
    file.close(io);
    std.process.abort();
}
