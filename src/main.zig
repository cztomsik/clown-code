const std = @import("std");
const tk = @import("tokamak");
const tools = @import("tools.zig");

// TODO: This shouldn't be hard-coded
const tool_names: []const []const u8 = &.{ "read_file", "write_file", "edit_file", "run_command" };

const Config = struct {
    ai_client: tk.ai.ClientConfig = .{
        .base_url = "http://localhost:8080",
    },
};

const App = struct {
    http_client: tk.http.StdClient,
    ai_client: tk.ai.Client,
    toolbox: tk.ai.AgentToolbox,
    runtime: tk.ai.AgentRuntime,
    clown: Clown,
    tui: *tk.tui.Context,
    main: Tui,

    pub fn configure(bundle: *tk.Bundle) void {
        bundle.addInitHook(tools.registerAllTools);
    }
};

const Tui = struct {
    ctx: *tk.tui.Context,
    clown: *Clown,
    buf: [256]u8 = undefined,
    msg_len: usize = 0,
    last_esc: i64 = 0,
    flash: ?[]const u8 = null,

    fn run(self: *Tui) !void {
        loop: while (true) {
            const root = try self.ctx.beginFrame();
            self.render(root);
            try self.ctx.endFrame();

            if (self.clown.busy) {
                try self.clown.tick();
            } else {
                const key = try self.ctx.readKey();
                self.ctx.last_key = key;

                switch (key) {
                    .ctrl_c => break :loop,
                    .escape => {
                        // Double escape within 2s -> clear text buffer
                        const now = std.time.milliTimestamp();
                        if (now - self.last_esc < 2000) self.msg_len = 0;
                        self.last_esc = now;
                    },
                    .tab => self.ctx.focus = @mod(self.ctx.focus + 1, @max(1, self.ctx.n_controls)),
                    .shift_tab => self.ctx.focus = @mod(self.ctx.focus - 1 + self.ctx.n_controls, @max(1, self.ctx.n_controls)),
                    .enter => {
                        const input = self.buf[0..self.msg_len];
                        if (input.len == 0) continue :loop;
                        self.msg_len = 0;

                        // Handle special commands
                        if (std.mem.startsWith(u8, input, "/")) {
                            const handled = try self.handleCommand(input[1..]);
                            if (!handled) break;
                            continue;
                        }

                        try self.clown.send(input);
                    },
                    else => {},
                }
            }
        }
    }

    fn render(self: *Tui, ui: tk.tui.Builder) void {
        if (ui.panel(5)) |p| {
            p.frame.* = p.frame.with("fg", .yellow);
            p.paragraph(
                \\ ╭─────╮
                \\ │ O.O │  Clown Code
                \\ │ ──  │  /help for commands
            , -1);
        }

        if (ui.panel(-4)) |p| {
            if (p.grid(&.{ 10, -1 }, -1)) |g| {
                for (self.clown.agent.messages.items) |msg| {
                    if (msg.role == .system) continue;

                    g.label(@tagName(msg.role));
                    // TODO: TextOrContents
                    g.paragraph(msg.content.?.text, if (msg.role == .tool) 2 else -1);

                    if (msg.tool_calls) |tcs| {
                        for (tcs) |tc| {
                            g.spacer(1); // skip first cell
                            if (g.row(&.{ 12, -1 })) |r| {
                                r.text(tc.function.name);
                                r.text(tc.function.arguments);
                            }
                        }
                    }
                }
            }
        }

        if (ui.row(&.{ 5, -1 })) |r| {
            r.label(" User:");
            r.textInput(&self.buf, &self.msg_len);
        }

        if (self.flash) |msg| {
            ui.flash(msg);
            self.flash = null;
        }
    }

    fn handleCommand(self: *Tui, cmd: []const u8) !bool {
        if (std.mem.eql(u8, cmd, "exit") or std.mem.eql(u8, cmd, "quit")) return false;
        if (std.mem.eql(u8, cmd, "clear")) self.clown.clear();
        if (std.mem.eql(u8, cmd, "retry")) try self.clown.retry();
        if (std.mem.eql(u8, cmd, "continue")) try self.clown.@"continue"();
        if (std.mem.eql(u8, cmd, "save")) try self.clown.save();
        if (std.mem.startsWith(u8, cmd, "load ")) try self.clown.load(cmd[5..]);

        if (std.mem.eql(u8, cmd, "help")) {
            self.flash =
                \\Available commands:
                \\ /exit, /quit - Exit the application
                \\ /clear       - Clear the conversation history
                \\ /retry       - Retry the last interaction
                \\ /models      - List available AI models
                \\ /help        - Show this help message
            ;
        }

        // if (std.mem.eql(u8, cmd, "models")) {
        //     const models = try self.clown.agent.runtime.client.listModels(self.clown.agent.arena);
        //     var buf: [1024]u8 = undefined;
        //     var fws = std.io.fixedBufferStream(&buf);
        //     const fw = fws.writer();
        //     try fw.print("Available models:", .{});
        //     for (models) |m| {
        //         try fw.print("\n- {s}", .{m.id});
        //     }
        //     ui.flash(fws.getWritten());
        // }

        return true;
    }
};

const Clown = struct {
    agent: tk.ai.Agent,
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

pub fn main() !void {
    try tk.app.run(Tui.run, &.{ Config, App });
}
