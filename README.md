# Génie

A fast application launcher for Wayland, in Rust. It stays resident, opens in a
few milliseconds, and draws with egui on a layer surface.

Type to search apps, then files, then Google, in that order. Everything else is a command:
`/` lists them, and the list narrows as you type.

| Command | What it does |
|---|---|
| `/files name` (`/f`), also `folder/name` and `~/path` | Files, through [répertoire](https://github.com/Sigma-studios/repertoire) |
| `/zed name` (`/zd`) | Zed's recent workspaces |
| `/spotify` (`/sp`, `/play`), `/spotify name` | Spotify's controls, and search |
| `/queue name` | Search songs, and queue one, or play it if nothing is playing |
| `/pause`, `/next`, `/previous` | Control the player |
| `/list` | Every command, by group, and the input searches them |
| `/power` | Shut down, restart, log out, lock |
| `/maps`, `/youtube` (`/yt`), `/github` (`/gh`), `/gpt`, `/google` (`/gg`)… | Web searches |

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
    command = "duckduckgo"
    alias = "dd"
    url = "https://duckduckgo.com/?q=%s"
    # In the regular search, instead of Google.
    regular = true

`/spotify` controls the player as is. Searching and `/queue` need an app from
[Spotify's dashboard](https://developer.spotify.com/dashboard) whose redirect URI is
`http://127.0.0.1:47813/callback`. Adding to the queue also takes Spotify Premium. The first search opens a tab to sign in, with
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
