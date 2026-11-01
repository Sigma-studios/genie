# Génie

A fast application launcher for Wayland, in Rust. It stays resident, opens in a
few milliseconds, and draws with egui on a layer surface.

| Type | To get |
|---|---|
| a name | Apps, by name and keywords, and the power entries |
| `f name`, `f folder/name`, `f ~/path` | Files, through [répertoire](https://github.com/Sigma-studios/repertoire) |
| `zd name` | Zed's recent workspaces |
| `sp`, `sp name` | Spotify's controls, and search |
| `gg`, `yt`, `gh`, `dd`, `maps`, `gpt`… then a query | Web searches. Some are offered when nothing else matched |

Enter runs the selected result, Alt lists its other actions (Alt+Up/Down, Alt+Enter),
and Tab completes. Up on the first result walks back through past queries. Escape or
clicking elsewhere hides.

## Running

    genie                    # the daemon
    genie toggle             # what to bind to a shortcut
    genie show [text]        # show, optionally with this query
    genie hide
    genie activate           # run the selected result
    genie quit

The commands start the daemon if it isn't running. `RUST_LOG=genie=debug` shows timings.

## Configuration

`~/.config/genie/config.toml` is optional. Replacing `websearch` or `power` replaces the
whole list; the defaults are in `src/config.rs`.

    terminal = ["wezterm", "start", "--"]

    [spotify]
    client_id = "…"

    [[websearch]]
    id = "ddg"
    name = "DuckDuckGo"
    trigger = "dd"
    url = "https://duckduckgo.com/?q=%s"
    fallback = true

`sp` controls the player as is. Its search needs an app from
[Spotify's dashboard](https://developer.spotify.com/dashboard) whose redirect URI is
`http://127.0.0.1:47813/callback`. The first search then opens a tab to sign in, with
OAuth and PKCE, so there's no secret to keep. The tokens stay in
`~/.local/share/genie/spotify.json`, readable by you alone.

Usage history lives in `~/.local/share/genie/history.db`. Albert's history is imported,
read-only, the first time that file is created.

## How the window works

KWin (and most compositors) treat `wlr-layer-shell` as the way to have a launcher: no
anchors means centred on the active screen, and the `Overlay` layer stays above
everything. Each show creates a surface and a swapchain, while the GPU device, egui's
textures and the plugins stay alive, which is what makes showing fast. The surface is
larger than the panel, for the shadow, with an input region so clicks there pass
through. A program started from Génie gets an XDG activation token, requested while
the surface still has focus, and its own systemd scope.

Plugins are compiled in (`src/plugin.rs`). Each query runs on one thread per plugin
and is dropped at the next keystroke.
