# Tests

Unit tests cover pure logic (e.g. title bar/border hit math in `decoration.rs`, config
parsing, including `example_config_is_valid` for `config.example.toml`).

Integration tests (`tests/*.rs`, harness in `tests/common/`) run a headless Pila per test
(`--backend headless`, see "Backends" in `docs/ARCHITECTURE.md`), in parallel:

- `TestPila` starts it with a config (animations and shadows off and the US layout unless the
  test sets `[animations]`, `[shadow]` or `[keyboard]`), without the session's display, keymap,
  `PILA_*` or D-Bus variables, in a private runtime, config and state directory; it injects
  input and reads the state through its control socket, and waits for conditions by polling
  (`wait`, `wait_window`, `settle`), never with a fixed sleep for something to happen; a check
  that something doesn't happen uses `stays`.
- `Client` (`tests/common/client.rs`) is a synthetic Wayland client on its own thread that
  acks configures, draws its color at the configured size and records configures and input
  events.
- A failing test prints the state and Pila's log; `PILA_TEST_LOG` sets its `RUST_LOG`,
  `PILA_TEST_KEEP=1` keeps a failed test's directory, `PILA_HEADLESS_DEVICE=software` renders
  on llvmpipe as CI does. A Pila that panics fails its test, unless the test made it crash on
  purpose (the control socket's `panic`, after `TestPila::expect_crash`). The harness starts
  Pila without `RUST_BACKTRACE` (Pila logs a crash's backtrace by itself) and, with
  `Options::soft_file_limit`, with a lower soft limit of open files. It sets
  `PILA_HEADLESS_CHECK_REDRAWS=1`: Pila panics ("changed and no redraw was asked for it") if
  an output shows something new that no redraw request for it explains (`headless.rs`,
  `check_redraws`); the fix is a redraw request where the change is made (`redraw.rs`).

`tests/FEATURES.md` maps every feature to its tests (or says why it has none), and
`features_inventory_is_complete` fails if a test it names is gone or an integration test
isn't named there. Known bugs are `#[ignore = "bug: ..."]` tests, listed at its end.

What breaks silently:

- The client's `wait` closures run with its lock held: read the `Shared` given, never call
  the client (that deadlocks).
- A test expecting a protocol error calls `Client::expect_error` before the bad request
  (`wait_error` insists): it stops the client's thread from writing, since a write after Pila
  closes the connection fails and wayland-backend then never reads the protocol error waiting
  in the socket.

`docs_names::the_docs_name_only_code_that_exists` fails when a doc or a comment names a file,
or a path into Pila's code (`module::item`, `Type::method`), that's gone: rename it there too.

`scripts/coverage [--html]` reports line coverage per file (instrumented build in
`target/coverage`; needs `llvm-profdata`/`llvm-cov` of rustc's LLVM major version), a guide
to what the tests miss.
