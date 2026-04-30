const std = @import("std");
const tk = @import("tokamak");
const tools = @import("tools.zig");

const BUF_SIZE = 4096;

pub const TodoItem = struct {
    name: []const u8,
    status: []const u8 = "pending",
};

const Worker = struct {
    pid: std.posix.pid_t,
    pipe: std.fs.File,
    sink: std.ArrayList(u8),
    started_at_ms: i64,
};

const WorkerMsg = union(enum) {
    snapshot: Snapshot,
    err: []const u8,
};

pub const Snapshot = struct {
    messages: []tk.ai.chat.Message,
    todos: []TodoItem,
    total_tokens: u32,
};

pub const Clown = struct {
    agent: tk.ai.Agent,
    todos: std.ArrayList(TodoItem) = .empty,
    worker: ?Worker = null,
    err: ?[]const u8 = null,

    pub fn init(gpa: std.mem.Allocator, agr: *tk.ai.AgentRuntime) !Clown {
        var agent = try agr.createAgent(gpa, .{ .model = "default", .max_completion_tokens = 32 * 1024 });
        errdefer agent.deinit();

        // We do this later because we want it to be scoped with agent.arena.
        var names = tk.iter.map(agr.toolbox.tools.keyIterator(), tk.meta.deref);
        agent.options.tools = try tk.iter.collect(agent.arena, &names);

        const system_prompt = try loadSystemPrompt(agent.arena);
        try agent.addMessage(.{ .role = .system, .content = .{ .text = system_prompt } });

        return .{ .agent = agent };
    }

    pub fn deinit(self: *Clown) void {
        self.stop();
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
        self.stop();
        self.agent.messages.clearRetainingCapacity();
        self.todos.clearRetainingCapacity();
    }

    pub fn send(self: *Clown, msg: []const u8) !void {
        try self.agent.addMessage(.{
            .role = .user,
            .content = .{ .text = try self.agent.arena.dupe(u8, msg) },
        });
        try self.start();
    }

    pub fn retry(self: *Clown) !void {
        self.agent.undo();
        try self.start();
    }

    pub fn sudo(self: *Clown) !void {
        if (self.agent.messages.items.len > 0) {
            // Find and modify the last message to start with some acceptance phrase
            // NOTE: This will only work for non-thinking models in llama.cpp https://github.com/ggml-org/llama.cpp/blob/b5c4227dc653d581d336dd7026ea95b66124fb92/tools/server/server-common.cpp#L1089
            const msg = &self.agent.messages.items[self.agent.messages.items.len - 1];
            msg.content = .{ .text = "Sure, let me answer that! Here's" };

            // Try again
            try self.start();
        }
    }

    pub fn save(self: *Clown) !void {
        const filename = try std.fmt.allocPrint(self.agent.arena, "session-{f}.json", .{tk.time.Time.now()});
        defer self.agent.arena.free(filename);

        const file = try std.fs.cwd().createFile(filename, .{});
        defer file.close();

        var fw = file.writer(&.{});
        var jw = tk.serde.json.Writer.init(&fw.interface, .{ .whitespace = .indent_2 });
        try tk.serde.serialize(&jw, self.makeSnapshot());
    }

    pub fn load(self: *Clown, filename: []const u8) !void {
        self.stop();
        const file = try std.fs.cwd().openFile(filename, .{});
        defer file.close();

        const contents = try file.readToEndAlloc(self.agent.arena, 1024 * 1024);
        self.loadSnapshot(try std.json.parseFromSliceLeaky(Snapshot, self.agent.arena, contents, .{ .allocate = .alloc_always }));
    }

    pub fn @"continue"(self: *Clown) !void {
        const filename = tk.util.trim(try tools.runCommand(self.agent.arena, .{ .command = "ls -1t session-*.json 2>/dev/null | head -1" }));
        if (filename.len == 0) return;
        try self.load(filename);
    }

    pub fn busy(self: *Clown) bool {
        return self.worker != null;
    }

    pub fn elapsed(self: *const Clown) i64 {
        return @divTrunc(std.time.milliTimestamp() - if (self.worker) |w| w.started_at_ms else 0, 1_000);
    }

    fn makeSnapshot(self: *const Clown) Snapshot {
        return .{
            .messages = self.agent.messages.items,
            .todos = self.todos.items,
            .total_tokens = self.agent.total_tokens,
        };
    }

    fn loadSnapshot(self: *Clown, snap: Snapshot) void {
        self.agent.messages.items = snap.messages;
        self.todos.items = snap.todos;
        self.agent.total_tokens = snap.total_tokens;
    }

    fn start(self: *Clown) !void {
        self.stop();
        self.err = null;

        const pipe = try std.posix.pipe();
        errdefer for (pipe) |fd| std.posix.close(fd);

        const pid = try std.posix.fork();
        if (pid == 0) {
            std.posix.close(pipe[0]); // close read
            self.workerMain(.{ .handle = pipe[1] });
            unreachable;
        } else {
            std.posix.close(pipe[1]); // close write & set non-blocking
            const flags = try std.posix.fcntl(pipe[0], std.posix.F.GETFL, 0);
            _ = try std.posix.fcntl(pipe[0], std.posix.F.SETFL, flags | @as(usize, 1 << @bitOffsetOf(std.posix.O, "NONBLOCK")));

            self.worker = .{
                .pid = pid,
                .pipe = .{ .handle = pipe[0] },
                .sink = try .initCapacity(self.agent.arena, 1024),
                .started_at_ms = std.time.milliTimestamp(),
            };
        }
    }

    fn stop(self: *Clown) void {
        if (self.worker) |w| {
            std.posix.kill(w.pid, std.posix.SIG.KILL) catch {};
            w.pipe.close();
            self.worker = null;
        }
    }

    pub fn tick(self: *Clown) !void {
        // Check worker status
        const worker = if (self.worker) |*w| w else return;
        const changed = std.posix.waitpid(worker.pid, std.posix.W.NOHANG);

        // Read all we can (before we branch)
        while (true) {
            var buf: [BUF_SIZE]u8 = undefined;
            const n = worker.pipe.read(&buf) catch 0;
            if (n == 0) break;
            try worker.sink.appendSlice(self.agent.arena, buf[0..n]);
        }

        // Check for any updates
        var updated = false;
        const data = worker.sink.items;
        if (std.mem.lastIndexOf(u8, data, "\n")) |last_nl| {
            const prev_nl = std.mem.lastIndexOf(u8, data[0..last_nl], "\n");
            const line = data[if (prev_nl) |p| p + 1 else 0..last_nl];
            const msg = try std.json.parseFromSliceLeaky(WorkerMsg, self.agent.arena, line, .{ .allocate = .alloc_always });

            switch (msg) {
                .snapshot => |s| self.loadSnapshot(s),
                .err => |e| self.err = e,
            }

            worker.sink.clearRetainingCapacity();
            updated = true;
        }

        // Worker finished
        if (changed.pid != 0) {
            self.worker = null;
            // worker.pipe.close(); // TODO: I think we should close this but I'm getting .BADF
        }
    }

    fn workerMain(self: *Clown, out: std.fs.File) noreturn {
        self.workerInner(out) catch |err| self.workerSend(out, .{ .err = @errorName(err) }) catch |e| @panic(@errorName(e));
        out.close();
        std.posix.exit(0);
    }

    fn workerInner(self: *Clown, out: std.fs.File) !void {
        while (try self.agent.next()) |tcs| {
            try self.agent.acceptAll(tcs);
            try self.workerSend(out, .{ .snapshot = self.makeSnapshot() });
        }

        try self.workerSend(out, .{ .snapshot = self.makeSnapshot() });
    }

    fn workerSend(_: *Clown, out: std.fs.File, msg: WorkerMsg) !void {
        var buf: [BUF_SIZE]u8 = undefined;
        var bw = out.writer(&buf);
        var jw = tk.serde.json.Writer.init(&bw.interface, .{});
        try tk.serde.serialize(&jw, msg);
        try bw.interface.writeAll("\n");
        try bw.interface.flush();
    }
};
