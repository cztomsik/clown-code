const std = @import("std");
const tk = @import("tokamak");

// TODO: This shouldn't be hard-coded
const tool_names: []const []const u8 = &.{ "todos_update", "file_read", "file_write", "file_edit", "run_command", "scrape", "hacker_news", "reddit" };

pub const TodoItem = struct {
    name: []const u8,
    status: []const u8 = "pending",
};

pub const Clown = struct {
    agent: tk.ai.Agent,
    todos: std.ArrayList(TodoItem) = .empty,
    busy: bool = false,

    pub fn init(gpa: std.mem.Allocator, agr: *tk.ai.AgentRuntime) !Clown {
        var agent = try agr.createAgent(gpa, .{ .model = "", .tools = tool_names, .max_completion_tokens = 32 * 1024 });
        errdefer agent.deinit();

        const system_prompt = try loadSystemPrompt(agent.arena);
        try agent.addMessage(.{ .role = .system, .content = .{ .text = system_prompt } });

        return .{ .agent = agent };
    }

    pub fn deinit(self: *Clown) void {
        self.agent.deinit();
    }

    fn loadSystemPrompt(arena: std.mem.Allocator) ![]const u8 {
        const file = std.fs.cwd().openFile("CLOWN.md", .{}) catch |err| switch (err) {
            error.FileNotFound => return @embedFile("CLOWN.md"),
            else => return err,
        };
        defer file.close();
        return file.readToEndAlloc(arena, 1024 * 1024);
    }

    pub fn clear(self: *Clown) void {
        self.agent.messages.clearRetainingCapacity();
    }

    pub fn send(self: *Clown, msg: []const u8) !void {
        try self.agent.addMessage(.{
            .role = .user,
            .content = .{ .text = try self.agent.arena.dupe(u8, msg) },
        });

        self.agent.result = null;
        self.busy = true;
    }

    pub fn retry(self: *Clown) !void {
        while (self.agent.messages.pop()) |msg| {
            if (msg.role == .assistant) break;
        }

        self.agent.result = null;
        self.busy = true;
    }

    pub fn tick(self: *Clown) !void {
        if (try self.agent.next()) |tcs| try self.agent.acceptAll(tcs);
        self.busy = self.agent.result == null;
    }

    pub fn @"continue"(self: *Clown) !void {
        const filename = try self.exec("ls -1 session-*.json 2>/dev/null | head -1");
        try self.load(filename orelse return);
    }

    pub fn load(self: *Clown, filename: []const u8) !void {
        const file = try std.fs.cwd().openFile(filename, .{});
        defer file.close();

        const contents = try file.readToEndAlloc(self.agent.arena, 1024 * 1024);
        self.agent.messages.items = try std.json.parseFromSliceLeaky([]tk.ai.chat.Message, self.agent.arena, contents, .{});
    }

    pub fn save(self: *Clown) !void {
        const filename = try std.fmt.allocPrint(self.agent.arena, "session-{f}.json", .{tk.time.Time.now()});
        defer self.agent.arena.free(filename);

        const file = try std.fs.cwd().createFile(filename, .{});
        defer file.close();

        var fw = file.writer(&.{});
        var jw = tk.serde.json.Writer.init(&fw.interface, .{ .whitespace = .indent_2 });
        try tk.serde.serialize(&jw, self.agent.messages.items);
    }

    pub fn exec(self: *Clown, cmd: []const u8) !?[]const u8 {
        const res = try std.process.Child.run(.{
            .allocator = self.agent.arena,
            .argv = &.{ "sh", "-c", cmd },
        });
        const out = tk.util.trim(res.stdout);

        return if (out.len > 0) out else null;
    }
};
