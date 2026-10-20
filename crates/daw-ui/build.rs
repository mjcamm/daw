//! Exports the values that ask graphics drivers for the faster GPU on
//! computers with two (`src/gpu.rs`). Only Windows with the MSVC linker.

fn main() {
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows && msvc {
        for symbol in [
            "NvOptimusEnablement",
            "AmdPowerXpressRequestHighPerformance",
        ] {
            println!("cargo:rustc-link-arg-bins=/EXPORT:{symbol}");
        }
    }
}
