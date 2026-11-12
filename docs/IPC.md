# IPC

`pila msg` asks a running Pila about its windows, monitors, workspaces and keyboard layouts and
runs actions. Other programs can talk to the same socket directly.

## `pila msg`

```
pila msg [--json] windows            The windows, on all workspaces
pila msg [--json] outputs            The monitors
pila msg [--json] workspaces         The workspaces
pila msg [--json] keyboard           The keyboard layouts, and which is in use
pila msg [--json] focused            The focused window
pila msg [--json] version            Pila's version
pila msg [--json] subscribe [EVENTS] Events as they happen, until Pila quits
pila msg action <action>             Run an action, as in [bindings]
pila msg focus <id>                  Focus a window, on whichever workspace
pila msg close <id>                  Ask a window to close
```

For example: `pila msg action workspace 2`, `pila msg --json windows | jq`,
`pila msg --json subscribe workspaces`.

Without `--json` the output is for people, and may change. Scripts should use `--json`.

Exit status: 0 on success, 1 if Pila refused the request (it says why on stderr), 2 for wrong
arguments, 3 if it can't talk to Pila.

## The socket

A Unix stream socket, for the same user only (mode 0600). Programs Pila starts have its path in
`PILA_SOCKET`, and a login session exports it to the D-Bus session bus (and to systemd's user
manager, where that's the bus) too, for programs started as services; otherwise it's
`$XDG_RUNTIME_DIR/pila-<WAYLAND_DISPLAY>.sock`.

Each request is one line of JSON and gets one line of JSON back, in order. A connection can be
kept open for more than one.

```
→ {"request": "action", "action": "workspace 2"}
← {"ok": true}
→ {"request": "nothing"}
← {"ok": false, "error": "unknown request \"nothing\""}
```

Limits: a request is at most 64 KiB, 32 clients at once, and a client that leaves more than
1 MiB of answers unread is disconnected.

## What's guaranteed

Answers to queries carry `protocol`, now 1. New requests and new fields can appear at any time, so
ignore what you don't know. Removing a field or changing what one means raises `protocol`. The
actions are those of `config.example.toml`.

## Requests

| `request` | Fields | Answer |
|---|---|---|
| `version` | | `protocol`, `version` |
| `windows` | | `protocol`, `windows`: mapped windows back to front, then those minimized or on other workspaces |
| `outputs` | | `protocol`, `outputs` |
| `workspaces` | | `protocol`, `workspaces` |
| `keyboard` | | `protocol`, `layouts` |
| `focused` | | `protocol`, `window`: a window, or null |
| `subscribe` | `events`: a list of `windows`, `workspaces`, `outputs`, `keyboard` (all if left out) | `protocol`; then events (below) |
| `action` | `action`: as in `[bindings]` | nothing more |
| `focus` | `id` | nothing more |
| `close` | `id` | nothing more |

While the session is locked, `action`, `focus` and `close` are refused, `quit` included, and
queries still answer. Only the actions `power-off-monitors`, `power-on-monitors` and
`switch-layout` still run: idling locks first and then turns the monitors off, and a lock screen
may switch the keyboard layout to type the password. An action with `spawn` runs a command as your user; any program of yours can
do that anyway.

**A window:** `id` (string, for as long as the window lives; the same identifier as
ext-foreign-toplevel-list), `app_id`, `title`, `workspace` (from 1), `output` (a name, or null off
screen), `focused`, `x`, `y`, `width`, `height` (logical pixels; where it comes back to if it's off
screen; the title bar isn't included), `minimized`, `maximized`, `fullscreen`, `snapped` (`left`,
`right`, `top-left`, `top-right`, `bottom-left`, `bottom-right` or null), `always_on_top`,
`all_workspaces`, `urgent` (asking for attention), `x11`. Titles and app ids are what programs set: they can hold anything,
terminal escapes included. Text from `pila msg` without `--json` has control characters replaced.

**An output:** `name`, `make`, `model`, `x`, `y`, `width`, `height` (logical pixels), `refresh`
(mHz, or null), `scale`, `transform` (as in `[[output]]`), `focused` (the pointer is on it), `power` (its display is on: off
after `power-off-monitors` or a tool such as `wlopm`). Only enabled monitors are listed (not those
with `enabled = false` or turned off through output management); one whose display is powered off
is listed with `power: false`.

**A workspace:** `index` (from 1), `active`, `windows` (how many, minimized included; a window on
all workspaces counts for the active one).

**A keyboard layout:** `index` (from 1, in the order of `[keyboard]`'s `layout`), `name` (xkb's,
such as `English (US)`), `active` (in use). While a virtual keyboard (`wtype`, an on-screen
keyboard) types with a keymap of its own, these are still the physical keyboard's. The action
`switch-layout <n|next|prev>` changes the layout.

## Events

After `subscribe`, the connection gets an event line for each kind asked for, with how things are
now, and then one whenever that kind changes:

```
→ {"request": "subscribe", "events": ["workspaces"]}
← {"ok": true, "protocol": 1}
← {"event": "workspaces", "workspaces": [{"index": 1, "active": true, "windows": 0}, ...]}
← {"event": "workspaces", "workspaces": [{"index": 1, "active": false, ...}, {"index": 2, "active": true, ...}, ...]}
```

Each event is the whole list, as the request of the same name answers it (`keyboard` events carry
`layouts`): replace what you had.
Windows in events leave out `x`, `y`, `width` and `height`, which change in every frame of a move;
ask `windows` for them. Events come once things settle on screen, as taskbars hear about them
(the keyboard's right away, and the monitors' power even when none is on to draw). A
subscribed connection still answers requests (answers have `ok`, events `event`); subscribing
again replaces the list. A subscriber that leaves more than 1 MiB unread is disconnected.
