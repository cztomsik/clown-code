const std = @import("std");
const tk = @import("tokamak");

// Standard file system and shell tools for AI agents.

pub const ReadFileArgs = struct {
    path: []const u8,
};

/// Read the contents of a file.
pub fn readFile(arena: std.mem.Allocator, args: ReadFileArgs) ![]const u8 {
    const file = try std.fs.cwd().openFile(args.path, .{});
    defer file.close();

    return file.readToEndAlloc(arena, 1024 * 1024);
}

pub const WriteFileArgs = struct {
    path: []const u8,
    content: []const u8,
};

/// Write content to a file, creating parent directories if needed.
pub fn writeFile(_: std.mem.Allocator, args: WriteFileArgs) ![]const u8 {
    // Create parent directories
    if (std.fs.path.dirname(args.path)) |dir| {
        std.fs.cwd().makePath(dir) catch |err| switch (err) {
            error.PathAlreadyExists => {},
            else => return err,
        };
    }

    const file = try std.fs.cwd().createFile(args.path, .{});
    defer file.close();

    try file.writeAll(args.content);

    return "File written successfully";
}

pub const EditFileArgs = struct {
    path: []const u8,
    old_content: []const u8,
    new_content: []const u8,
    replace_all: bool = false,
};

/// Edit a file by replacing specific content.
/// If replace_all is false (default), old_content must exist exactly once.
pub fn editFile(arena: std.mem.Allocator, args: EditFileArgs) ![]const u8 {
    // Read current content
    const content = try readFile(arena, .{ .path = args.path });
    var new_content = content;

    if (!args.replace_all) {
        // Find old_content
        const pos = std.mem.indexOf(u8, content, args.old_content) orelse {
            return error.ContentNotFound;
        };

        // Check that it only appears once
        if (std.mem.indexOf(u8, content[pos + args.old_content.len ..], args.old_content) != null) {
            return error.AmbiguousMatch;
        }

        // Build new content
        new_content = try std.mem.concat(arena, u8, &.{ content[0..pos], args.new_content, content[pos + args.old_content.len ..] });
    } else {
        new_content = try std.mem.replaceOwned(u8, arena, content, args.old_content, args.new_content);
    }

    // Write back
    const write_file = try std.fs.cwd().createFile(args.path, .{});
    defer write_file.close();
    try write_file.writeAll(new_content);

    return "File edited successfully";
}

pub const RunCommandArgs = struct {
    command: []const u8,
    cwd: ?[]const u8 = null,
};

/// Execute a shell command and return its output.
/// Captures both stdout and stderr. TODO: timeout
pub fn runCommand(arena: std.mem.Allocator, args: RunCommandArgs) ![]const u8 {
    const res = try std.process.Child.run(.{
        .allocator = arena,
        .argv = &.{ "sh", "-c", args.command },
        .cwd = args.cwd,
    });

    const exit_code = switch (res.term) {
        .Exited => |code| code,
        else => return error.CommandFailed,
    };

    if (exit_code != 0) {
        return try std.fmt.allocPrint(
            arena,
            "Command failed with exit code {d}\nStdout:\n{s}\nStderr:\n{s}",
            .{ exit_code, res.stdout, res.stderr },
        );
    }

    if (res.stderr.len > 0) {
        return try std.fmt.allocPrint(
            arena,
            "{s}\n\nStderr:\n{s}",
            .{ res.stdout, res.stderr },
        );
    }

    return res.stdout;
}

/// Register all standard tools with an AgentToolbox.
pub fn registerAllTools(toolbox: *tk.ai.AgentToolbox) !void {
    try toolbox.addTool("read_file", "Read the contents of a file", readFile);
    try toolbox.addTool("write_file", "Write content to a file, creating directories if needed", writeFile);
    try toolbox.addTool("edit_file", "Edit a file by replacing specific content. Set replace_all=true to replace all occurrences", editFile);
    try toolbox.addTool("run_command", "Execute a shell command and return its output", runCommand);
}
