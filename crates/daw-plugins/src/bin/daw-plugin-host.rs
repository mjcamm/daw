//! A plugin host process on its own, for tests. The app runs itself in
//! plugin-host mode instead.

#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;

fn main() -> std::process::ExitCode {
    daw_plugins::host::run()
}
