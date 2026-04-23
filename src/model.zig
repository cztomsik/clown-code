const std = @import("std");
const tk = @import("tokamak");
const tools = @import("tools.zig");

const BUF_SIZE = 512;

pub const TodoItem = struct {
    name: []const u8,
    status: []const u8 = "pending",
};

const Worker = struct {
    pid: std.posix.pid_t,
    pipe_read: std.fs.File,
    pipe_res: std.ArrayList(u8),
    started_at_ms: i64,
};

const WorkerSnapshot = struct {
    messages: []tk.ai.chat.Message,
    todos: []TodoItem,
};

const TickRes = enum { idle, busy, finished };

pub const Clown = struct {
    agent: tk.ai.Agent,
    todos: std.ArrayList(TodoItem) = .empty,
    worker: ?Worker = null,

    pub fn init(gpa: std.mem.Allocator, agr: *tk.ai.AgentRuntime) !Clown {
        var agent = try agr.createAgent(gpa, .{ .model = "", .max_completion_tokens = 32 * 1024 });
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

    pub fn save(self: *Clown) !void {
        const filename = try std.fmt.allocPrint(self.agent.arena, "session-{f}.json", .{tk.time.Time.now()});
        defer self.agent.arena.free(filename);

        const file = try std.fs.cwd().createFile(filename, .{});
        defer file.close();

        var fw = file.writer(&.{});
        var jw = tk.serde.json.Writer.init(&fw.interface, .{ .whitespace = .indent_2 });
        try tk.serde.serialize(&jw, self.agent.messages.items);
    }

    pub fn load(self: *Clown, filename: []const u8) !void {
        self.stop();
        const file = try std.fs.cwd().openFile(filename, .{});
        defer file.close();

        const contents = try file.readToEndAlloc(self.agent.arena, 1024 * 1024);
        self.agent.messages.items = try std.json.parseFromSliceLeaky([]tk.ai.chat.Message, self.agent.arena, contents, .{});
    }

    pub fn @"continue"(self: *Clown) !void {
        const filename = tk.util.trim(try tools.runCommand(self.agent.arena, .{ .command = "ls -1 session-*.json 2>/dev/null | head -1" }));
        if (filename.len == 0) return;
        try self.load(filename);
    }

    pub fn busy(self: *Clown) bool {
        return self.worker != null;
    }

    fn start(self: *Clown) !void {
        self.stop();
        self.agent.result = null;

        const pipe = try std.posix.pipe();
        errdefer for (pipe) |fd| std.posix.close(fd);

        const pid = try std.posix.fork();
        if (pid == 0) {
            std.posix.close(pipe[0]); // close read
            self.runWorker(.{ .handle = pipe[1] }) catch std.process.exit(1);
            std.process.exit(0);
        } else {
            std.posix.close(pipe[1]); // close write & set non-blocking
            const flags = try std.posix.fcntl(pipe[0], std.posix.F.GETFL, 0);
            _ = try std.posix.fcntl(pipe[0], std.posix.F.SETFL, flags | @as(usize, 1 << @bitOffsetOf(std.posix.O, "NONBLOCK")));

            self.worker = .{
                .pid = pid,
                .pipe_read = .{ .handle = pipe[0] },
                .pipe_res = try .initCapacity(self.agent.arena, 1024),
                .started_at_ms = std.time.milliTimestamp(),
            };
        }
    }

    fn stop(self: *Clown) void {
        const worker = self.worker orelse return;
        worker.pipe_read.close();
        std.posix.kill(worker.pid, std.posix.SIG.KILL) catch {};
        self.worker = null;
    }

    pub fn tick(self: *Clown) !TickRes {
        const worker = if (self.worker) |*w| w else return .idle;

        // Check worker status
        const changed = std.posix.waitpid(worker.pid, std.posix.W.NOHANG);

        // Read all we can (before we branch)
        while (true) {
            var buf: [BUF_SIZE]u8 = undefined;
            const n = worker.pipe_read.read(&buf) catch |e| switch (e) {
                error.WouldBlock => 0,
                else => return e,
            };
            if (n == 0) break;
            try worker.pipe_res.appendSlice(self.agent.arena, buf[0..n]);
        }

        if (changed.pid == 0) {
            // Try again later
            return .busy;
        } else {
            // Worker finished
            const snapshot = try std.json.parseFromSliceLeaky(WorkerSnapshot, self.agent.arena, worker.pipe_res.items, .{});
            self.agent.messages = .fromOwnedSlice(snapshot.messages);
            self.todos.items = snapshot.todos;

            self.worker = null;
            // worker.pipe_read.close(); // TODO: I think we should close this but I'm getting .BADF

            return .finished;
        }
    }

    fn runWorker(self: *Clown, out: std.fs.File) !void {
        while (try self.agent.next()) |tcs| {
            try self.agent.acceptAll(tcs);
        }

        const snap: WorkerSnapshot = .{
            .messages = self.agent.messages.items,
            .todos = self.todos.items,
        };

        var buf: [BUF_SIZE]u8 = undefined;
        var bw = out.writer(&buf);
        var jw = tk.serde.json.Writer.init(&bw.interface, .{});
        try tk.serde.serialize(&jw, snap);
        try bw.interface.flush();
        out.close();
    }
};
