#![no_main]
use ic_management_canister_types_private::InstallCodeArgs;
use ic_management_canister_types_private::Payload;
use libfuzzer_sys::fuzz_target;

// This fuzz test feeds binary data to Candid's `Decode!` macro for InstallCodeArgs with the goal of exposing panics
// e.g. caused by stack overflows during decoding.

fuzz_target!(|data: &[u8]| {
    let _ = InstallCodeArgs::decode(data);
});
