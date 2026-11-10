//! A line of `pila msg`'s protocol (`ipc::parse_request`): JSON, then the
//! request and its fields (an action's text, events, a window id).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|line: &[u8]| {
    let _ = pila::ipc::parse_request(line);
});
