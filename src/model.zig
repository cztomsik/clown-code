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
    pipe: std.Io.File,
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
    io: std.Io,
    agent: tk.ai.Agent,
    todos: std.ArrayList(TodoItem) = .empty,
    worker: ?Worker = null,
    compacting: bool = false,
    err: ?[]const u8 = null,

    pub fn init(io: std.Io, gpa: std.mem.Allocator, agr: *tk.ai.AgentRuntime) !Clown {
        var agent = try agr.createAgent(gpa, .{ .model = "default", .max_completion_tokens = 32 * 1024 });
        errdefer agent.deinit();

        // We do this later because we want it to be scoped with agent.arena.
        // TODO: reconsider if we shouldn't drop tool filtering entirely
        // TODO: consider implementing our own hashmap with saner API (header.keys are there, but private)
        var names: std.ArrayList([]const u8) = .empty;
        var keys = agr.toolbox.tools.keyIterator();
        while (keys.next()) |key| try names.append(agent.arena, key.*);
        agent.options.tools = try names.toOwnedSlice(agent.arena);

        const system_prompt = try loadSystemPrompt(io, agent.arena);
        try agent.addMessage(.{ .role = .system, .content = .{ .text = system_prompt } });

        return .{ .io = io, .agent = agent };
    }

    pub fn deinit(self: *Clown) void {
        self.stop();
        self.agent.deinit();
    }

    fn loadSystemPrompt(io: std.Io, arena: std.mem.Allocator) ![]const u8 {
        const prefix = @embedFile("PREFIX.md");

        const project_context = std.Io.Dir.cwd().readFileAlloc(io, "CLOWN.md", arena, .limited(1024 * 1024)) catch |err| switch (err) {
            error.FileNotFound => "",
            else => return err,
        };

        const today: tk.time.Date = .today();
        const cwd_path = try std.Io.Dir.cwd().realPathFileAlloc(io, ".", arena);

        return try std.fmt.allocPrint(
            arena,
            "{s}{s}{s}\n\nCurrent date: {f}\nCurrent working directory: {s}\n",
            .{ prefix, if (project_context.len > 0) "\n\n" else "", project_context, today, cwd_path },
        );
    }

    pub fn clear(self: *Clown) void {
        self.stop();
        self.todos.clearRetainingCapacity();
        self.agent.messages.shrinkRetainingCapacity(1); // Keep the system msg
    }

    pub fn clearTools(self: *Clown) void {
        self.stop();

        // Remove all tool results but keep system, user, and assistant messages
        var i: usize = 0;
        for (self.agent.messages.items) |msg| {
            if (msg.role != .tool) {
                self.agent.messages.items[i] = msg;
                i += 1;
            }
        }

        self.agent.messages.shrinkRetainingCapacity(i);
    }

    pub fn compact(self: *Clown) !void {
        // Step 1: Ask the model to summarize the conversation
        self.compacting = true;
        try self.send(
            \\Please provide a concise summary of the conversation so far.
            \\Include:
            \\- Current state of any ongoing tasks
            \\- Key decisions and their rationale
            \\- Important file paths and code snippets
            \\- Anything that is absolutely neccessary in order to continue the work
            \\Keep it under 2000 characters. After providing the summary, stop.
        );
    }

    fn finishCompact(self: *Clown) !void {
        self.compacting = false;
        const last = self.agent.messages.getLast() orelse return;
        if (last.role != .assistant) return;

        const summary = last.content.?.text;
        const fmt =
            \\The conversation history has been compacted to save context space. Acknowledge this and ask user what they want to do next.
            \\
            \\<compacted summary>
            \\{s}
            \\</compacted summary>
        ;

        self.clear();
        try self.send(try std.fmt.allocPrint(self.agent.arena, fmt, .{summary}));
    }

    pub fn send(self: *Clown, msg: []const u8) !void {
        try self.agent.addMessage(.{
            .role = .user,
            .content = .{ .text = try self.agent.arena.dupe(u8, msg) },
        });
        try self.start();
    }

    pub fn undo(self: *Clown, buf: []u8, msg_len: *usize) void {
        if (self.agent.undo()) |msg| {
            if (msg.content) |c| {
                if (c.text.len < buf.len) {
                    @memcpy(buf[0..c.text.len], c.text);
                    msg_len.* = c.text.len;
                }
            }
        }
    }

    pub fn retry(self: *Clown) !void {
        // Strip only assistant/tool messages, keep the user message
        while (self.agent.messages.getLast()) |msg| {
            if (msg.role != .assistant and msg.role != .tool) break;
            _ = self.agent.messages.pop();
        }
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

        const file = try std.Io.Dir.cwd().createFile(self.io, filename, .{});
        defer file.close(self.io);

        var fw = file.writer(self.io, &.{});
        var jw = tk.serde.json.Writer.init(&fw.interface, .{ .whitespace = .indent_2 });
        try tk.serde.serialize(&jw, self.makeSnapshot());
    }

    pub fn load(self: *Clown, filename: []const u8) !void {
        self.stop();

        const contents = try std.Io.Dir.cwd().readFileAlloc(self.io, filename, self.agent.arena, .limited(1024 * 1024));
        self.loadSnapshot(try std.json.parseFromSliceLeaky(Snapshot, self.agent.arena, contents, .{ .allocate = .alloc_always }));
    }

    pub fn @"continue"(self: *Clown) !void {
        const filename = tk.util.trim(try tools.runCommand(self.io, self.agent.arena, .{ .command = "ls -1t session-*.json 2>/dev/null | head -1" }));
        if (filename.len == 0) return;
        try self.load(filename);
    }

    pub fn busy(self: *Clown) bool {
        return self.worker != null;
    }

    pub fn elapsed(self: *Clown) i64 {
        return @divTrunc(tk.time.milliTimestamp() - if (self.worker) |w| w.started_at_ms else 0, 1_000);
    }

    fn makeSnapshot(self: *Clown) Snapshot {
        return .{
            .messages = self.agent.messages.items,
            .todos = self.todos.items,
            .total_tokens = self.agent.total_tokens,
        };
    }

    fn loadSnapshot(self: *Clown, snap: Snapshot) void {
        // NOTE: assigning slice is wrong here, not because of pointers, but because we also need to restore capacity
        self.agent.messages = .fromOwnedSlice(snap.messages);
        self.todos = .fromOwnedSlice(snap.todos);
        self.agent.total_tokens = snap.total_tokens;
    }

    fn start(self: *Clown) !void {
        self.stop();
        self.err = null;

        var pipe: [2]c_int = undefined;
        const pipe_res = std.c.pipe(&pipe);
        if (pipe_res != 0) return error.PipeFailed;
        errdefer {
            _ = std.c.close(pipe[0]);
            _ = std.c.close(pipe[1]);
        }

        const pid = std.c.fork();
        if (pid == -1) return error.ForkFailed;
        if (pid == 0) {
            _ = std.c.close(pipe[0]); // close read
            self.workerMain(.{ .handle = pipe[1], .flags = .{ .nonblocking = false } });
            unreachable;
        } else {
            _ = std.c.close(pipe[1]); // close write & set non-blocking
            const flags = std.c.fcntl(pipe[0], std.posix.F.GETFL, @as(c_int, 0));
            if (flags == -1) return error.FnctlFailed;
            _ = std.c.fcntl(pipe[0], std.posix.F.SETFL, flags | @as(c_int, 1 << @bitOffsetOf(std.posix.O, "NONBLOCK")));

            self.worker = .{
                .pid = pid,
                .pipe = .{ .handle = pipe[0], .flags = .{ .nonblocking = true } },
                .sink = try .initCapacity(self.agent.arena, 1024),
                .started_at_ms = tk.time.milliTimestamp(),
            };
        }
    }

    pub fn stop(self: *Clown) void {
        if (self.worker) |w| {
            _ = std.c.kill(w.pid, std.posix.SIG.KILL);
            w.pipe.close(self.io);
            self.worker = null;
        }
    }

    pub fn tick(self: *Clown) !void {
        // Check worker status
        const worker = if (self.worker) |*w| w else return;
        const changed = std.c.waitpid(worker.pid, null, @as(c_int, std.posix.W.NOHANG));

        // Read all we can (before we branch)
        while (true) {
            var buf: [BUF_SIZE]u8 = undefined;
            const n = worker.pipe.readStreaming(self.io, &.{buf[0..]}) catch 0;
            if (n == 0) break;
            try worker.sink.appendSlice(self.agent.arena, buf[0..n]);
        }

        // Check for any updates
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
        }

        // Worker finished
        if (changed != 0) {
            self.worker = null;
            // worker.pipe.close(); // TODO: I think we should close this but I'm getting .BADF

            if (self.compacting) {
                try self.finishCompact();
                return;
            }
        }
    }

    fn workerMain(self: *Clown, out: std.Io.File) noreturn {
        self.workerInner(out) catch |err| {
            std.log.err("worker error: {s}", .{@errorName(err)});
            _ = self.workerSend(out, .{ .err = @errorName(err) }) catch |e| @panic(@errorName(e));
        };
        out.close(self.io);
        std.c.exit(0);
    }

    fn workerInner(self: *Clown, out: std.Io.File) !void {
        while (try self.agent.next()) |tcs| {
            try self.agent.acceptAll(tcs);
            try self.workerSend(out, .{ .snapshot = self.makeSnapshot() });
        }

        try self.workerSend(out, .{ .snapshot = self.makeSnapshot() });
    }

    fn workerSend(self: *Clown, out: std.Io.File, msg: WorkerMsg) !void {
        var buf: [BUF_SIZE]u8 = undefined;
        var bw = out.writer(self.io, &buf);
        var jw = tk.serde.json.Writer.init(&bw.interface, .{});
        try tk.serde.serialize(&jw, msg);
        try bw.interface.writeAll("\n");
        try bw.interface.flush();
    }
};
