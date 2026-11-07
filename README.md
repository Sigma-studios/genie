# Génie

A fast application launcher for Wayland, in Rust. It stays resident, opens in a
few milliseconds, and draws with egui on a layer surface.

Type to search apps, then files, then Ecosia, in that order. A calculation, like `2*(3+4)`, shows its result on top, and a URL, like `github.com` or `localhost:3000`, opens in the browser. `folder/name` and `~/path` list
what's in there, and Tab goes one level down. Everything else is a command:
`/` lists them, and the list narrows as you type.

| Command | What it does |
|---|---|
| `/files name` (`/f`), also `folder/name` and `~/path` | Files, through [répertoire](https://github.com/Sigma-studios/repertoire) |
| `/zed name` (`/zd`) | Zed's recent workspaces |
| `/spotify` (`/sp`, `/play`), `/spotify name` | Spotify's controls, and search |
| `/queue name` | Search songs, and queue one, or play it if nothing is playing |
| `/pause`, `/next`, `/previous`, `/track-restart` | Control the player |
| `/calc 2*(3+4)` (`/=`), or just `2*(3+4)` | Calculates, and Enter copies the result. Tab puts it in the input to go on |
| `/kill name` | Your running programs, most memory first. Tab completes, Enter ends one asking first, Alt has forcing it |
| `/list` | Opens a window with every command, by group, and a search bar |
| `/shutdown`, `/restart`, `/logout`, `/lock` | What they say, asking first except to lock: Enter twice. `/power` lists them |
| `/ecosia` (`/ec`), `/maps`, `/youtube` (`/yt`), `/github` (`/gh`), `/amazon`… | Web searches |

Enter or a click runs a result, and an app that's open comes forward rather than starting
again. Alt+Enter or Alt+click runs its second action: New window, for an app. Alt+Down lists
them all. Tab completes. Up on the first result walks back through past queries. Escape or
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
    # In the regular search, instead of Ecosia.
    regular = true

`~/.config/genie/commands.toml`, also optional, adds your own commands. Génie doesn't
bind `shortcut`: it's shown beside the command, for a key you've bound elsewhere. With
`arguments = true`, what's typed after the name goes on the end of `command`:
`/clip 30` below runs `gsr-replay 30`.

    [[command]]
    name = "record-start"
    title = "Recording: start"
    icon = "media-record"
    command = ["gsr-start"]
    shortcut = "Ctrl+Alt+Home"

    [[command]]
    name = "clip"
    title = "Recording: save replay"
    icon = "document-save"
    command = ["gsr-replay"]
    arguments = true

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
