const std = @import("std");
const tk = @import("tokamak");

// Constants
const MAX_READ_SIZE = 2 * 1024 * 1024;

// Standard file system and shell tools for AI agents.
//
// NOTE: tk.ai.AgentTool handles JSON schema generation, input validation, and
// native Zig type support. Return values are formatted via tk.ai.fmt() for LLMs.

pub const ReadFileArgs = struct {
    path: []const u8,
    raw: bool = false,
};

/// Read the contents of a file.
/// If `raw` is false (default), output is prefixed with line numbers (e.g., "1:content").
pub fn readFile(io: std.Io, arena: std.mem.Allocator, args: ReadFileArgs) ![]const u8 {
    const contents = try std.Io.Dir.cwd().readFileAlloc(io, args.path, arena, .limited(MAX_READ_SIZE));
    if (!std.unicode.utf8ValidateSlice(contents)) return error.InvalidUtf8;

    if (args.raw) {
        return contents;
    }

    // Split into lines and format as "N:content"
    var lines = std.mem.splitScalar(u8, contents, '\n');
    var out = try std.ArrayList(u8).initCapacity(arena, contents.len);
    defer out.deinit(arena);
    var line_num: usize = 1;
    while (lines.next()) |line| {
        const formatted = try std.fmt.allocPrint(arena, "{d}:{s}\n", .{ line_num, line });
        try out.appendSlice(arena, formatted);
        line_num += 1;
    }
    return out.toOwnedSlice(arena);
}

pub const WriteFileArgs = struct {
    path: []const u8,
    content: []const u8,
};

/// Write content to a file, creating parent directories if needed.
pub fn writeFile(io: std.Io, _: std.mem.Allocator, args: WriteFileArgs) ![]const u8 {
    // Create parent directories
    if (std.fs.path.dirname(args.path)) |dir| {
        std.Io.Dir.cwd().createDirPath(io, dir) catch |err| switch (err) {
            error.PathAlreadyExists => {},
            else => return err,
        };
    }

    try std.Io.Dir.cwd().writeFile(io, .{ .sub_path = args.path, .data = args.content });

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
pub fn editFile(io: std.Io, arena: std.mem.Allocator, args: EditFileArgs) ![]const u8 {
    // Read current content (without line numbers)
    const content = try std.Io.Dir.cwd().readFileAlloc(io, args.path, arena, .limited(MAX_READ_SIZE));
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
    try std.Io.Dir.cwd().writeFile(io, .{ .sub_path = args.path, .data = new_content });
    return "File edited successfully";
}

pub const RunCommandArgs = struct {
    command: []const u8,
    cwd: ?[]const u8 = null,
};

/// Execute a shell command and return its output.
/// Captures both stdout and stderr. TODO: timeout
pub fn runCommand(io: std.Io, arena: std.mem.Allocator, args: RunCommandArgs) ![]const u8 {
    const res = try std.process.run(arena, io, .{
        .argv = &.{ "sh", "-c", args.command },
        .cwd = if (args.cwd) |p| .{ .path = p } else .inherit,
        .stderr_limit = .limited(MAX_READ_SIZE),
        .stdout_limit = .limited(MAX_READ_SIZE),
    });
    if (!std.unicode.utf8ValidateSlice(res.stdout)) return error.InvalidUtf8;
    if (!std.unicode.utf8ValidateSlice(res.stderr)) return error.InvalidUtf8;

    const exit_code = switch (res.term) {
        .exited => |code| code,
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

pub const LoadSkillArgs = struct {
    skill_name: []const u8,
};

/// Load a skill file and inject its contents as system instructions into the agent's context.
/// Builtin skills (init) take precedence over user-provided skills.
pub fn loadSkill(io: std.Io, arena: std.mem.Allocator, args: LoadSkillArgs) ![]const u8 {
    if (std.mem.eql(u8, args.skill_name, "init")) return @embedFile("skills/init.md");

    // TODO: Check for path traversal
    const path = try std.fmt.allocPrint(arena, "skills/{s}.md", .{args.skill_name});
    return std.Io.Dir.cwd().readFileAlloc(io, path, arena, .limited(MAX_READ_SIZE));
}

/// Register all standard tools with an AgentToolbox.
pub fn registerAllTools(toolbox: *tk.ai.AgentToolbox) !void {
    try toolbox.addTool("read_file", "Read the contents of a file", readFile);
    try toolbox.addTool("write_file", "Write content to a file, creating directories if needed", writeFile);
    try toolbox.addTool("edit_file", "Edit a file by replacing specific content. Set replace_all=true to replace all occurrences", editFile);
    try toolbox.addTool("run_command", "Execute a shell command and return its output", runCommand);
    try toolbox.addTool("load_skill", "Load a set of specialized instructions (a skill) into the current context to improve performance on a specific task.", loadSkill);
}

test runCommand {
    var arena_impl = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena_impl.deinit();
    const arena = arena_impl.allocator();

    const res1 = try runCommand(std.testing.io, arena, .{ .command = "echo hello" });
    try std.testing.expectEqualStrings("hello\n", res1);

    const res2 = try runCommand(std.testing.io, arena, .{ .command = "find . -name build.zig" });
    try std.testing.expect(std.mem.indexOf(u8, res2, "build.zig") != null);

    const res3 = try runCommand(std.testing.io, arena, .{ .command = "grep -r \"\\.addTool()\" --include=\"*.zig\" ." });
    try std.testing.expect(std.mem.indexOf(u8, res3, "tools.zig") != null);
}
