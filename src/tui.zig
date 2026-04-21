const std = @import("std");
const tk = @import("tokamak");
const Clown = @import("model.zig").Clown;

pub const Tui = struct {
    ctx: *tk.tui.Context,
    clown: *Clown,
    buf: [512]u8 = undefined,
    msg_len: usize = 0,
    last_esc: i64 = 0,
    flash: ?[]const u8 = null,

    pub fn run(self: *Tui) !void {
        self.ctx.focus = 1; // Start with text input focused

        loop: while (true) {
            switch (try self.ctx.tick()) {
                .render => |ui| self.render(ui),
                .key => |k| switch (k) {
                    .ctrl_c => break,
                    .escape => {
                        // Double escape within 2s -> clear text buffer
                        const now = std.time.milliTimestamp();
                        if (now - self.last_esc < 2000) self.msg_len = 0;
                        self.last_esc = now;
                    },
                    .enter => {
                        const input = self.buf[0..self.msg_len];
                        if (input.len == 0) continue :loop;
                        self.msg_len = 0;

                        // Handle special commands
                        if (std.mem.startsWith(u8, input, "/")) {
                            const cmd, const arg = tk.util.split2(input[1..], " ");
                            const handled = try self.handleCommand(cmd, arg);
                            if (!handled) break;
                            continue;
                        }

                        try self.clown.send(input);
                    },
                    else => self.ctx.pending_key = k,
                },
                .idle => {
                    // TODO: This is now called periodically, so all we need to do is to wrap agent in a thread or something
                    if (self.clown.busy) {
                        try self.clown.tick();
                        self.ctx.next_tick = .render;
                    }
                },
            }
        }
    }

    fn render(self: *Tui, ui: tk.tui.Builder) void {
        if (ui.panel(5)) |p| {
            if (p.row(&.{ -60, 60 })) |r| {
                self.banner(r);
                self.todos(r);
            }
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

    fn banner(self: *Tui, ui: tk.tui.Builder) void {
        const f = ui.next(-1, 5) orelse return;
        f.with("fg", self.ctx.theme.accent).text(
            \\ ╭─────╮
            \\ │ >.< │  Clown Code
            \\ │ ──  │  /help for commands
        );
    }

    fn todos(self: *Tui, ui: tk.tui.Builder) void {
        const items = self.clown.todos.items;
        const st = ui.grid(&.{ -7, 7 }, @intCast(items.len + 1)) orelse return;
        st.frame.z = 10;
        st.container().layout.spacing = 0;

        if (st.collapsible("Todos:", ui.state(bool, false))) {
            st.spacer(1);
            st.frame.at(0, 1).fill(ui.ctx.theme.base2);

            for (items) |it| {
                st.text(it.name);
                st.text(it.status);
            }
        } else {
            st.num(items.len);
        }
    }

    fn handleCommand(self: *Tui, cmd: []const u8, arg: []const u8) !bool {
        if (std.mem.eql(u8, cmd, "exit") or std.mem.eql(u8, cmd, "quit")) return false;
        if (std.mem.eql(u8, cmd, "clear")) self.clown.clear();
        if (std.mem.eql(u8, cmd, "retry")) try self.clown.retry();
        if (std.mem.eql(u8, cmd, "continue")) try self.clown.@"continue"();
        if (std.mem.eql(u8, cmd, "save")) try self.clown.save();
        if (std.mem.eql(u8, cmd, "load")) try self.clown.load(arg);

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
