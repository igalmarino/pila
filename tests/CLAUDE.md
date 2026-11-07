# Tests

Unit tests cover pure logic (e.g. title bar/border hit math in `decoration.rs`, config
parsing, including `example_config_is_valid` for `config.example.toml`).

Integration tests (`tests/*.rs`, harness in `tests/common/`) run a headless pila per test
(`--backend headless`, see "Backends" in `docs/ARCHITECTURE.md`), in parallel:

- `TestPila` starts it with a config (animations and shadows off unless the test sets
  `[animations]` or `[shadow]`),
  injects input and reads the state through its control socket, and waits for conditions
  (`wait`, `wait_window`, `stays`; never sleeps).
- `Client` (`tests/common/client.rs`) is a synthetic Wayland client on its own thread that
  acks configures, draws its color at the configured size and records configures and input
  events.
- A failing test prints the state and pila's log; `PILA_TEST_LOG` sets its `RUST_LOG`,
  `PILA_TEST_KEEP=1` keeps a failed test's directory, `PILA_HEADLESS_DEVICE=software` renders
  on llvmpipe as CI does. A pila that panics fails its test.

`tests/FEATURES.md` maps every feature to its tests (or says why it has none), and
`features_inventory_is_complete` fails if a test it names is gone or an integration test
isn't named there. Known bugs are `#[ignore = "bug: ..."]` tests, listed at its end.

What breaks silently:

- The client's `wait` closures run with its lock held: read the `Shared` given, never call
  the client (that deadlocks).
- A test expecting a protocol error calls `Client::expect_error` before the bad request
  (`wait_error` insists): it stops the client's thread from writing, since a write after pila
  closes the connection fails and wayland-backend then never reads the protocol error waiting
  in the socket.

`scripts/coverage [--html]` reports line coverage per file (instrumented build in
`target/coverage`; needs `llvm-profdata`/`llvm-cov` of rustc's LLVM major version), a guide
to what the tests miss.
