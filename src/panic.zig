const std = @import("std");

// Custom panic handler that writes everything to error.log file.
pub fn panic(msg: []const u8, err_trace: ?*std.builtin.StackTrace, ra: ?usize) noreturn {
    const file = std.fs.cwd().createFile("error.log", .{ .truncate = true }) catch std.posix.abort();
    errdefer file.close();

    var fw = file.writer(&.{});
    const w = &fw.interface;

    // Write panic message
    w.print("panic: {s}\n", .{msg}) catch {};

    if (std.debug.getSelfDebugInfo() catch null) |info| {
        // Write error return trace (if available)
        if (err_trace) |t| {
            std.debug.writeStackTrace(t.*, w, info, .no_color) catch {};
        }

        // Write current stack trace
        std.debug.writeCurrentStackTrace(w, info, .no_color, ra orelse @returnAddress()) catch {};
    } else {
        w.writeAll("(could not open debug info)\n") catch {};
    }

    // Flush, close, abort
    w.flush() catch {};
    file.close();
    std.posix.abort();
}
