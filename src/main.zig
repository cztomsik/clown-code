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
        .timeout = 15 * 60,
    },
};

const EnvOverrides = struct {
    pub fn configure(bundle: *tk.Bundle) void {
        // TODO: This is not perfect (ordering), but it works fine for now.
        bundle.addInitHook(applyOverrides);
    }

    fn applyOverrides(config: *Config, init: std.process.Init) void {
        if (init.environ_map.get("CLOWN_API")) |url| config.ai_client.base_url = url;
    }
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

pub fn main(init: std.process.Init) !void {
    try tk.app.run(init, Tui.run, &.{ Config, EnvOverrides, App });
}

test {
    std.testing.refAllDecls(@This());
}
