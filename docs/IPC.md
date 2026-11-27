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
pila msg [--json] screen-shares      List the screen shares remembered for apps
pila msg forget-screen-share <id>|all  Forget one remembered screen share, or all of them
pila msg [--json] capture-permissions  List direct screen capture permissions
pila msg forget-capture-permission <id>|all  Forget one capture permission, or all of them
pila msg [--json] idle-inhibitors    List the apps keeping the screen on now
pila msg [--json] captures           List what's capturing the screen now
pila msg --waybar captures           Follow the captures as a waybar custom module
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
| `subscribe` | `events`: array of `windows`, `workspaces`, `outputs`, `keyboard`, `colors`, `switch`, `captures` (defaults to all) | `protocol`, followed by event lines (see below) |
| `action` | `action`: string, as in `[bindings]` | — |
| `focus` | `id` | — |
| `close` | `id` | — |
| `minimize` | `id` | — (succeeds even if already minimized; `focus` restores it) |
| `screen-shares` | — | `protocol`, `screen_shares` |
| `forget-screen-share` | `id`, or `all`: `true` | with `all`, `forgotten` (how many); fails for an unknown `id`, and before Pila has read the remembered shares (it does once it serves the portal) |
| `capture-permissions` | — | `protocol`, `capture_permissions` |
| `forget-capture-permission` | `id`, or `all`: `true` | with `all`, `forgotten` (how many); fails for an unknown `id` |
| `idle-inhibitors` | — | `protocol`, `idle_inhibitors` |
| `captures` | — | `protocol`, `captures` |

While the session is locked, `action`, `focus`, `close`, and `minimize` are refused (including
`quit`), while queries continue to work, and so do `forget-screen-share` and
`forget-capture-permission`, which only take access away. The only actions allowed while locked are
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

**Screen share (`screen-shares`):**
A share remembered for an app, which it gets again without the picker (see the manual, "Screen
sharing"), sorted by app:
- `id`: the grant's id, for `forget-screen-share` (a secret the app's portal keeps; don't share it)
- `app`: the app as the picker named it; `app_id`: as the portal named it (empty for an app it
  doesn't name)
- `shares`: an object with `kind`: `"output"` (with `output`, a monitor name), `"region"` (with
  `output` and `x`, `y`, `width`, `height` in logical pixels, from the monitor's top-left), or
  `"window"` (with `app_id` and `title`)
- `persistent`: `true` if it's kept until revoked (saved), `false` while the app runs

**Capture permission (`capture-permissions`):**
A direct Wayland capture permission granted through the confirmation prompt, until Pila exits
(see the manual, "Direct screen capture permissions"; `[[permission]]` rules aren't listed),
sorted by executable:
- `id`: the grant's id, for `forget-capture-permission`
- `executable`: the client's executable path (from `/proc/<pid>/exe`)
- `persistent`: always `false` (only the configuration keeps a permission across restarts; up to
  0.9.8, `true` for one remembered in `capture-permissions.json`)

**Idle inhibitor (`idle-inhibitors`):**
Something keeping the screen on now (swayidle neither locks nor turns the monitors off): first
the apps that asked over D-Bus, in the order they asked, then the visible windows and layer
surfaces with a Wayland idle inhibitor, by `app`. Listed while the session is locked too, when
none of them counts.
- `source`: `"dbus"` (`org.freedesktop.ScreenSaver`'s `Inhibit`) or `"wayland"` (idle-inhibit)
- `app`: for `dbus`, the name the app gave; for `wayland`, the window's app id or the layer
  surface's namespace
- `reason`: for `dbus`, the reason the app gave; `null` for `wayland`

**Capture (`captures`):**
A program capturing the screen now, or that stopped less than a second ago (so a one-shot
screenshot shows): portal shares first, then direct captures. The same program capturing two
things is listed twice.
- `app`: for `portal`, the app as the share picker names it; otherwise the program's executable
  name (its process id, as `process 1234`, until Pila has read it)
- `protocol`: `"portal"`, `"ext-image-copy-capture"` or `"wlr-screencopy"`
- `target`: an object with `kind`: `"output"` (with `output`, a monitor name), `"region"` (with
  `output` and `x`, `y`, `width`, `height` in logical pixels, from the monitor's top-left), or
  `"window"` (with `id`, as in `windows`, `app_id`, and `output`: the monitor it's on, or `null`)

`pila msg --waybar captures` subscribes to `captures` and prints a line of JSON for a waybar
custom module (`"return-type": "json"`) now and at each change: `text` (empty while nothing
captures, which hides the module; the app's name, or how many apps, after a `●`), `tooltip` (a
line per capture) and `class` (`"capturing"` or `"idle"`).

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
events carry `layouts`, `colors` events carry `colors`, `switch` events carry `switch` or
`null`, and `captures` events carry `captures`), so subscribers can simply replace their cached
state.

Timing details:
- `switch` events fire immediately on each step.
- `colors` events fire when a reloaded configuration changes colors.
- `captures` events fire as soon as a capture starts or a program's name is read, and a second
  after a capture ends.
- While a window is being dragged or resized with the pointer, events keep its pre-drag `x`, `y`,
  `width`, and `height` and emit a single update when the drag ends (avoiding per-frame chatter).
  Querying `windows` directly still returns the live coordinates.
- Most events are sent once changes settle on screen (matching when taskbars are notified);
  `keyboard` events fire immediately, and monitor power events fire even when all monitors are off.

A subscribed connection can still send regular requests (responses carry `ok`, while events carry
`event`). Calling `subscribe` again replaces the active event filter. Subscribers that let more
than 1 MiB of events pile up unread are disconnected.
