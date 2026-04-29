const std = @import("std");
const tk = @import("tokamak");
const Clown = @import("model.zig").Clown;

pub const Tui = struct {
    ctx: *tk.tui.Context,
    clown: *Clown,
    scroll: i32 = 0, // 0 means auto-scroll to bottom
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
                    // TODO: ctrl_c/esc should clown.stop(), double ctrl_c should break (we could save last key + last time and do both)
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
                    .scroll_up => {
                        self.scroll += 1;
                        if (self.scroll < 0) self.scroll = 0;
                    },
                    .scroll_down => {
                        if (self.scroll > 0) {
                            self.scroll -= 1;
                        }
                        // Reaching 0 from manual scroll keeps it at 0 (auto-scroll)
                    },
                    else => self.ctx.pending_key = k,
                },
                .idle => {
                    if (try self.clown.tick() == .updated) {
                        self.ctx.next_tick = .render;
                    }
                },
            }
        }
    }

    fn render(self: *Tui, ui: tk.tui.Builder) void {
        self.header(ui);
        self.messages(ui);
        self.footer(ui);

        if (self.flash) |msg| {
            ui.flash(msg);
            self.flash = null;
        }
    }

    fn header(self: *Tui, ui: tk.tui.Builder) void {
        if (ui.stack(4)) |p| {
            p.frame.z = 10;
            p.frame.fill(ui.ctx.theme.base2);

            if (p.row(&.{ 36, -1 })) |r| {
                self.banner(r);
                self.todos(r);
            }

            p.frame.fg = ui.ctx.theme.base3;
            p.frame.bottom(1).splat("_");
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
        st.frame.rect[1] += 1;
        st.frame.z = 10;
        st.container().layout.spacing = 0;

        if (st.collapsible(ui.ctx.fmt("Todos: {}", .{items.len}), ui.state(bool, false))) {
            st.spacer(1);
            st.frame.at(0, 1).fill(ui.ctx.theme.base2);

            for (items) |it| {
                st.text(it.name);
                st.text(it.status);
            }
        }
    }

    fn messages(self: *Tui, ui: tk.tui.Builder) void {
        if (ui.stack(-5)) |p| {
            p.frame.* = p.frame.pad(.{ 0, 2, 0, 2 });

            if (p.grid(&.{ 10, -1 }, -1)) |g| {
                const prev_height = g.state(i32, g.frame.rect[3]);

                // scroll == 0 means auto-scroll to bottom; scroll > 0 is lines from bottom
                const available = g.frame.rect[3];
                const content_height = prev_height.*;
                if (content_height > available) {
                    const diff = available - content_height; // negative value
                    const offset = diff + @as(i32, self.scroll);
                    g.frame.* = g.frame.offset(0, offset);
                }

                for (self.clown.agent.messages.items) |msg| {
                    if (msg.role == .system) continue;

                    g.label(@tagName(msg.role));
                    // TODO: TextOrContents
                    g.paragraph(msg.content.?.text, if (msg.role == .tool) 10 else -1);

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

                if (self.clown.busy()) {
                    g.spacer(1);
                    g.text("Processing...");
                }

                const layout = g.container().layout;
                const new_height = layout.cursor[1] + layout.line_height;
                if (new_height != prev_height.*) {
                    prev_height.* = new_height;
                    g.ctx.next_tick = .clear;
                }
            }
        }
    }

    fn footer(self: *Tui, ui: tk.tui.Builder) void {
        if (ui.stack(-1)) |p| {
            p.frame.fill(ui.ctx.theme.base2);
            p.frame.* = p.frame.pad(.{ 1, 2, 1, 2 });

            if (p.row(&.{ -20, -10, -1 })) |r| {
                r.label("User:");

                r.frame.fg = ui.ctx.theme.secondary;
                r.text("Tokens:");
                r.num(self.clown.agent.total_tokens);
            }

            if (p.stack(1)) |r| {
                r.frame.fill(ui.ctx.theme.base1);
                r.textInput(&self.buf, &self.msg_len);
            }
        }
    }

    fn handleCommand(self: *Tui, cmd: []const u8, arg: []const u8) !bool {
        if (std.mem.eql(u8, cmd, "exit") or std.mem.eql(u8, cmd, "quit")) return false;
        if (std.mem.eql(u8, cmd, "clear")) self.clown.clear();
        if (std.mem.eql(u8, cmd, "init")) try self.clown.send("Could you /init this project?");
        if (std.mem.eql(u8, cmd, "retry")) try self.clown.retry();
        if (std.mem.eql(u8, cmd, "sudo")) try self.clown.sudo();
        if (std.mem.eql(u8, cmd, "continue")) try self.clown.@"continue"();
        if (std.mem.eql(u8, cmd, "save")) try self.clown.save();
        if (std.mem.eql(u8, cmd, "load")) try self.clown.load(arg);

        if (std.mem.eql(u8, cmd, "help")) {
            self.flash =
                \\Available commands:
                \\ /exit, /quit - Exit the application
                \\ /clear       - Clear the conversation history
                \\ /init        - Initialize project context
                \\ /retry       - Retry the last interaction
                \\ /sudo        - Retry with "sure" prefix
                \\ /save        - Save the conversation
                \\ /load <file> - Load a saved conversation
                \\ /continue    - Continue the last session
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
