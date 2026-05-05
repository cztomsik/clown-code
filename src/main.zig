const std = @import("std");
const tk = @import("tokamak");
const tools = @import("tools.zig");
const Clown = @import("model.zig").Clown;
const Tui = @import("tui.zig").Tui;

pub const panic = @import("panic.zig").panic;

pub const std_options: std.Options = .{
    .logFn = @import("log.zig").debugLog,
};

const Config = struct {
    ai_client: tk.ai.ClientConfig = .{
        .base_url = "http://127.0.0.1:8080",
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

pub fn main() !void {
    try tk.app.run(Tui.run, &.{ Config, App });
}

test {
    std.testing.refAllDecls(@This());
}
