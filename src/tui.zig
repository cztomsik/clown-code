const std = @import("std");
const tk = @import("tokamak");
const Clown = @import("model.zig").Clown;

pub const Tui = struct {
    ctx: *tk.tui.Context,
    clown: *Clown,
    scroll: i32 = 0, // 0 means auto-scroll to bottom
    buf: [4096]u8 = undefined,
    msg_len: usize = 0,
    last_esc: i64 = 0,
    last_ctrl_c: i64 = 0,
    flash: ?[]const u8 = null,

    pub fn run(self: *Tui) !void {
        self.ctx.focus = 1; // Start with text input focused

        loop: while (true) {
            switch (try self.ctx.tick()) {
                .render => |ui| self.render(ui),
                .key => |k| switch (k) {
                    .ctrl_c => {
                        const now = std.time.milliTimestamp();
                        if (now - self.last_ctrl_c < 500) break;
                        self.clown.stop();
                        self.flash = "Worker stopped. Press ctrl_c again to exit.";
                        self.last_ctrl_c = now;
                    },
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
                    .scroll_up, .scroll_down => self.scroll = @max(0, if (k == .scroll_up) self.scroll + 1 else self.scroll - 1),
                    else => self.ctx.pending_key = k,
                },
                .idle => {
                    try self.clown.tick();
                    self.ctx.next_tick = .clear;
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
        st.frame.z = 20;
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
        if (ui.stack(-8)) |p| {
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
                    g.text(g.ctx.fmt("Processing... {d}s", .{self.clown.elapsed()}));
                }

                if (self.clown.err) |e| {
                    g.spacer(1);
                    g.text(g.ctx.fmt("Error: {s}", .{e}));
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

            if (p.stack(3)) |r| {
                r.frame.fill(ui.ctx.theme.base1);
                r.textArea(&self.buf, &self.msg_len, 3);
            }
        }
    }

    fn handleCommand(self: *Tui, cmd: []const u8, arg: []const u8) !bool {
        if (std.mem.eql(u8, cmd, "exit") or std.mem.eql(u8, cmd, "quit")) return false;
        if (std.mem.eql(u8, cmd, "stop")) self.clown.stop();
        if (std.mem.eql(u8, cmd, "clear")) self.clown.clear();
        if (std.mem.eql(u8, cmd, "clear-tools")) self.clown.clearTools();
        if (std.mem.eql(u8, cmd, "compact")) try self.clown.compact();
        if (std.mem.eql(u8, cmd, "init")) try self.clown.send("Could you /init this project?");
        if (std.mem.eql(u8, cmd, "retry")) try self.clown.retry();
        if (std.mem.eql(u8, cmd, "undo")) self.clown.undo();
        if (std.mem.eql(u8, cmd, "sudo")) try self.clown.sudo();
        if (std.mem.eql(u8, cmd, "save")) try self.clown.save();
        if (std.mem.eql(u8, cmd, "load")) try self.clown.load(arg);
        if (std.mem.eql(u8, cmd, "continue")) try self.clown.@"continue"();

        if (std.mem.eql(u8, cmd, "help")) {
            self.flash =
                \\Available commands:
                \\ /exit, /quit  - Exit the application
                \\ /stop         - Stop the current AI processing
                \\ /clear        - Clear the conversation history
                \\ /clear-tools  - Remove all tool call results
                \\ /compact      - Summarize the conversation to reduce token usage
                \\ /init         - Initialize project context
                \\ /retry        - Retry the last interaction
                \\ /undo         - Remove the last message
                \\ /sudo         - Retry with "sure" prefix
                \\ /save         - Save the conversation
                \\ /load <file>  - Load a saved conversation
                \\ /continue     - Continue the last session
                \\ /help         - Show this help message
            ;
        }

        if (std.mem.eql(u8, cmd, "models")) {
            const models = try self.clown.agent.runtime.client.listModels(self.clown.agent.arena);
            // TODO: maybe the ctx.fmt() should be useful even for multi-frame prints...
            self.flash = std.fmt.allocPrint(self.clown.agent.arena, "Available models:\n{f}", .{std.json.fmt(models, .{})}) catch "OOM";
        }

        return true;
    }
};
