const std = @import("std");
const tk = @import("tokamak");
const Clown = @import("model.zig").Clown;
const TodoItem = @import("model.zig").TodoItem;

// Standard file system and shell tools for AI agents.
//
// NOTE: tk.ai.AgentTool handles JSON schema generation, input validation, and
// native Zig type support. Return values are formatted via tk.ai.fmt() for LLMs.

pub const ReadFileArgs = struct {
    path: []const u8,
};

/// Read the contents of a file.
pub fn readFile(arena: std.mem.Allocator, args: ReadFileArgs) ![]const u8 {
    const file = try std.fs.cwd().openFile(args.path, .{});
    defer file.close();

    const raw = try file.readToEndAlloc(arena, 1024 * 1024);

    // Split into lines and format as "N: line"
    var lines = std.mem.splitScalar(u8, raw, '\n');
    var out = try std.ArrayList(u8).initCapacity(arena, raw.len);
    defer out.deinit(arena);
    var line_num: usize = 1;
    while (lines.next()) |line| {
        const formatted = try std.fmt.allocPrint(arena, "{d}: {s}\n", .{ line_num, line });
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
    // Read current content (without line numbers)
    const file = try std.fs.cwd().openFile(args.path, .{});
    defer file.close();
    const content = try file.readToEndAlloc(arena, 1024 * 1024);
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
        .max_output_bytes = 1024 * 1024,
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

pub const ScrapeArgs = struct {
    url: []const u8,
    query_selector: ?[]const u8,
};

/// Scrape a web page and convert it to markdown. Optionally filter to a CSS selector.
pub fn scrape(http_client: *tk.http.Client, arena: std.mem.Allocator, args: ScrapeArgs) ![]const u8 {
    const res = try http_client.request(arena, .{ .url = args.url });

    const doc = try tk.dom.Document.parseFromSlice(arena, res.body);
    defer doc.deinit();

    var node = &doc.node;

    if (args.query_selector) |sel| {
        if (try doc.querySelector(sel)) |el| node = &el.node;
    }

    return try tk.html2md.html2md(arena, node, .{});
}

pub const HackerNewsArgs = struct {
    sort: enum { top, new, best },
    limit: u9 = 10,
};

/// Get stories from Hacker News.
pub fn hackerNews(http_client: *tk.http.Client, arena: std.mem.Allocator, args: HackerNewsArgs) ![]const tk.ext.hackernews.Story {
    var client = tk.ext.hackernews.Client{ .http_client = http_client };

    return switch (args.sort) {
        .top => try client.getTopStories(arena, args.limit),
        .new => try client.getNewStories(arena, args.limit),
        .best => try client.getBestStories(arena, args.limit),
    };
}

pub const RedditArgs = struct {
    subreddit: []const u8,
    sort: enum { hot, new, top },
    limit: u32 = 10,
};

/// Get posts from a Reddit subreddit.
pub fn reddit(http_client: *tk.http.Client, arena: std.mem.Allocator, args: RedditArgs) ![]const tk.ext.reddit.Post {
    var client = tk.ext.reddit.Client{ .http_client = http_client };

    return switch (args.sort) {
        .hot => try client.getHotPosts(arena, args.subreddit, args.limit),
        .new => try client.getNewPosts(arena, args.subreddit, args.limit),
        .top => try client.getTopPosts(arena, args.subreddit, args.limit),
    };
}

pub const UpdateTodosArgs = struct {
    upsert: []const TodoItem,
};

pub fn updateTodos(clown: *Clown, arena: std.mem.Allocator, args: UpdateTodosArgs) ![]const TodoItem {
    next: for (args.upsert) |ch| {
        for (clown.todos.items) |*it| {
            if (std.mem.eql(u8, it.name, ch.name)) {
                it.* = ch;
                continue :next;
            }
        } else {
            try clown.todos.append(arena, ch);
        }
    }

    return clown.todos.items;
}

pub const LoadSkillArgs = struct {
    skill_name: []const u8,
};

/// Load a skill file and inject its contents as system instructions into the agent's context.
/// Builtin skills (init, compact) take precedence over user-provided skills.
/// The `init` skill is concatenated with the canonical system prompt (CLOWN.md) at compile time.
pub fn loadSkill(arena: std.mem.Allocator, args: LoadSkillArgs) ![]const u8 {
    if (std.mem.eql(u8, args.skill_name, "init")) return @embedFile("skills/init.md") ++ @embedFile("CLOWN.md");
    if (std.mem.eql(u8, args.skill_name, "compact")) return @embedFile("skills/compact.md");

    // TODO: Check for path traversal
    const path = try std.fmt.allocPrint(arena, "skills/{s}.md", .{args.skill_name});
    const file = try std.fs.cwd().openFile(path, .{});
    defer file.close();

    return file.readToEndAlloc(arena, 1024 * 1024);
}

/// Register all standard tools with an AgentToolbox.
pub fn registerAllTools(toolbox: *tk.ai.AgentToolbox) !void {
    try toolbox.addTool("todos_update", "Create/update todo item(s)", updateTodos);
    try toolbox.addTool("file_read", "Read the contents of a file", readFile);
    try toolbox.addTool("file_write", "Write content to a file, creating directories if needed", writeFile);
    try toolbox.addTool("file_edit", "Edit a file by replacing specific content. Set replace_all=true to replace all occurrences", editFile);
    try toolbox.addTool("run_command", "Execute a shell command and return its output", runCommand);
    try toolbox.addTool("load_skill", "Load a set of specialized instructions (a skill) into the current context to improve performance on a specific task.", loadSkill);
    try toolbox.addTool("scrape", "Scrape a web page and convert it to markdown. Optionally filter to a CSS selector", scrape);
    try toolbox.addTool("hacker_news", "Get stories from Hacker News", hackerNews);
    try toolbox.addTool("reddit", "Get posts from a Reddit subreddit", reddit);
}

test runCommand {
    var arena_impl = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena_impl.deinit();
    const arena = arena_impl.allocator();

    const res1 = try runCommand(arena, .{ .command = "echo hello" });
    try std.testing.expectEqualStrings("hello\n", res1);

    const res2 = try runCommand(arena, .{ .command = "find . -name build.zig" });
    try std.testing.expect(std.mem.indexOf(u8, res2, "build.zig") != null);

    const res3 = try runCommand(arena, .{ .command = "grep -r \"\\.addTool()\" --include=\"*.zig\" ." });
    try std.testing.expect(std.mem.indexOf(u8, res3, "tools.zig") != null);
}
