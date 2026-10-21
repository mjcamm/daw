//! A plugin host process on its own, for tests: `--plugin-scan <bundle>`
//! scans one bundle, anything else serves plugins. The app runs itself in
//! these modes instead.

#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some(daw_plugins::scan::SCAN_FLAG) => match args.get(2) {
            Some(bundle) => daw_plugins::scan::run_scan(std::path::Path::new(bundle)),
            None => std::process::ExitCode::FAILURE,
        },
        _ => daw_plugins::host::run(),
    }
}
