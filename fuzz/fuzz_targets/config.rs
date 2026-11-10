//! A config file as Pila reads it (`Config::from_toml`): TOML, every
//! section, the bindings and gestures and the window and layer rules with
//! their validation, and the environment's overrides.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (&str, Option<&str>, Option<&str>, Option<&str>)| {
    let (text, modifier, terminal, launcher) = input;
    let env = |name: &str| {
        match name {
            "PILA_MOD" => modifier,
            "PILA_TERMINAL" => terminal,
            "PILA_LAUNCHER" => launcher,
            _ => None,
        }
        .map(str::to_string)
    };
    let _ = pila::config::Config::from_toml(text, &env);
});
