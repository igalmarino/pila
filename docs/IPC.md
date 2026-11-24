# IPC

`pila msg` queries a running Pila instance for windows, monitors, workspaces, keyboard layouts, and
colors, and runs compositor actions. External programs can also talk directly to the underlying
Unix socket.

## `pila msg`

```
pila msg [--json] windows            List all windows across all workspaces
pila msg [--json] outputs            List enabled monitors
pila msg [--json] workspaces         List workspaces
pila msg [--json] keyboard           List keyboard layouts and the active one
pila msg [--json] colors             Show current [colors] values
pila msg [--json] focused            Show the focused window
pila msg [--json] version            Show Pila's version
pila msg [--json] subscribe [EVENTS] Stream events as they happen until Pila exits
pila msg action <action>             Run an action (same syntax as [bindings])
pila msg focus <id>                  Focus a window on any workspace
pila msg close <id>                  Ask a window to close
pila msg minimize <id>               Minimize a window (use focus to restore it)
```

Examples:

```bash
pila msg action workspace 2
pila msg --json windows | jq
pila msg --json subscribe workspaces
```

Without `--json`, output is formatted for human reading and may change between releases. Scripts
should always pass `--json`.

**Exit codes:**
- `0`: success
- `1`: Pila refused the request (reason printed to stderr)
- `2`: invalid command-line arguments
- `3`: could not connect to Pila

## The socket

Pila listens on a Unix stream socket accessible only to your user (mode `0600`). Programs launched
by Pila receive the path in `PILA_SOCKET`. Login sessions also export `PILA_SOCKET` to the D-Bus
session bus (and `systemd --user` when present) so user services can find it. Otherwise, the path
is `$XDG_RUNTIME_DIR/pila-<WAYLAND_DISPLAY>.sock`.

The protocol is newline-delimited JSON: each request is one line of JSON and receives one JSON
response line in order. A single connection can send multiple requests.

```
→ {"request": "action", "action": "workspace 2"}
← {"ok": true}
→ {"request": "nothing"}
← {"ok": false, "error": "unknown request \"nothing\""}
```

**Limits:** Requests are capped at 64 KiB, up to 32 concurrent clients are accepted, and clients
that leave more than 1 MiB of unread responses buffered are disconnected.

## Stability guarantees

Query responses include a `protocol` integer (currently `1`). New requests and fields may be added
at any time, so clients should ignore unknown fields. Removing a field or changing its meaning will
increment `protocol`. Available actions match those documented in `config.example.toml`.

## Requests

| `request` | Parameters | Response fields (besides `ok`) |
|---|---|---|
| `version` | — | `protocol`, `version` |
| `windows` | — | `protocol`, `windows` (mapped windows back to front, followed by minimized and other-workspace windows) |
| `outputs` | — | `protocol`, `outputs` |
| `workspaces` | — | `protocol`, `workspaces` |
| `keyboard` | — | `protocol`, `layouts` |
| `colors` | — | `protocol`, `colors` |
| `switch` | — | `protocol`, `switch` (active window-switch state, or `null`) |
| `focused` | — | `protocol`, `window` (focused window object, or `null`) |
| `subscribe` | `events`: array of `windows`, `workspaces`, `outputs`, `keyboard`, `colors`, `switch` (defaults to all) | `protocol`, followed by event lines (see below) |
| `action` | `action`: string, as in `[bindings]` | — |
| `focus` | `id` | — |
| `close` | `id` | — |
| `minimize` | `id` | — (succeeds even if already minimized; `focus` restores it) |

While the session is locked, `action`, `focus`, `close`, and `minimize` are refused (including
`quit`), while queries continue to work. The only actions allowed while locked are
`power-off-monitors`, `power-on-monitors`, and `switch-layout`, so idle daemons can turn off
monitors after locking and lock screens can switch keyboard layouts. Note that `spawn` actions run
commands as your user, which any process running under your account can already do.

### Object schemas

**Window:**
- `id`: string unique for the window's lifetime (matches `ext-foreign-toplevel-list`)
- `app_id`, `title`: strings set by the application (may contain arbitrary characters, including
  terminal escapes; plain-text `pila msg` output replaces control characters, and `--json`
  escapes DEL and the C1 controls as `\u` escapes, as JSON already does those below U+0020)
- `workspace`: 1-based workspace index
- `output`: monitor name, or `null` if off-screen
- `focused`: boolean
- `x`, `y`, `width`, `height`: logical pixels excluding the title bar (for off-screen windows, the
  geometry they will restore to)
- `minimized`, `maximized`, `fullscreen`, `always_on_top`, `all_workspaces`, `urgent`, `x11`:
  booleans
- `snapped`: `"left"`, `"right"`, `"top"`, `"bottom"`, `"top-left"`, `"top-right"`,
  `"bottom-left"`, `"bottom-right"`, or `null`

**Output:**
- `name`, `make`, `model`: strings
- `x`, `y`, `width`, `height`: logical pixels
- `refresh`: refresh rate in mHz, or `null`
- `scale`: number
- `transform`: same values as `[[output]]`
- `focused`: `true` if the pointer is on this monitor
- `power`: `true` if the display is powered on (`false` after `power-off-monitors` or `wlopm`)

Only enabled monitors are included (monitors with `enabled = false` or disabled via output
management are omitted). Powered-off monitors remain listed with `"power": false`.

**Workspace:**
- `index`: 1-based index
- `active`: boolean
- `windows`: window count, including minimized windows (a window on all workspaces counts toward
  the active workspace)

**Keyboard layout:**
- `index`: 1-based index matching the order in `[keyboard]` `layout`
- `name`: XKB layout name (for example, `"English (US)"`)
- `active`: boolean

Even while a virtual keyboard (`wtype` or an on-screen keyboard) temporarily uses its own keymap,
these entries still reflect the physical keyboard. Use `switch-layout <n|next|prev>` to change the
active layout.

**Colors:**
An object containing every `[colors]` key with underscores instead of hyphens (`bar`,
`bar_inactive`, `bar_urgent`, `icon`, `icon_inactive`, `hover`, `hover_icon`, `close_hover`,
`close_hover_icon`, `background`, `snap_preview`), formatted as `#rrggbb` (or `#rrggbbaa` when
translucent). Values reflect the active config, falling back to defaults for omitted keys. Useful
for bars or launchers that want to match Pila's theme.

**Window switch (`switch`):**
While `Mod+Tab`, `Alt+Tab`, or a window-switching swipe is active, an object with:
- `windows`: array of window IDs in most-recently-used order (including minimized windows)
- `selected`: ID of the currently highlighted window
- `output`: name of the monitor the pointer was on when switching started, or `null`

Returns `null` when no window switch is in progress. Pila handles the switching logic and renders
its own switcher UI; this field exists for external tools that also want to display switcher state.
Sending a `focus` request ends the active switch on that window.

## Events

After a `subscribe` request succeeds, Pila immediately sends one snapshot event for each subscribed
category, followed by a new event whenever that category changes:

```
→ {"request": "subscribe", "events": ["workspaces"]}
← {"ok": true, "protocol": 1}
← {"event": "workspaces", "workspaces": [{"index": 1, "active": true, "windows": 0}, ...]}
← {"event": "workspaces", "workspaces": [{"index": 1, "active": false, ...}, {"index": 2, "active": true, ...}, ...]}
```

Each event carries the full current state in the same shape as the matching query (`keyboard`
events carry `layouts`, `colors` events carry `colors`, and `switch` events carry `switch` or
`null`), so subscribers can simply replace their cached state.

Timing details:
- `switch` events fire immediately on each step.
- `colors` events fire when a reloaded configuration changes colors.
- While a window is being dragged or resized with the pointer, events keep its pre-drag `x`, `y`,
  `width`, and `height` and emit a single update when the drag ends (avoiding per-frame chatter).
  Querying `windows` directly still returns the live coordinates.
- Most events are sent once changes settle on screen (matching when taskbars are notified);
  `keyboard` events fire immediately, and monitor power events fire even when all monitors are off.

A subscribed connection can still send regular requests (responses carry `ok`, while events carry
`event`). Calling `subscribe` again replaces the active event filter. Subscribers that let more
than 1 MiB of events pile up unread are disconnected.
