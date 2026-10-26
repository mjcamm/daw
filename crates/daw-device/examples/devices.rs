//! Lists output devices and what they support (for checking drivers).

#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;

use cpal::traits::{DeviceTrait, HostTrait};

fn main() {
    for id in cpal::available_hosts() {
        println!("Driver {}", id.name());
        let Ok(host) = cpal::host_from_id(id) else {
            continue;
        };
        for device in host.output_devices().into_iter().flatten() {
            let name = device
                .description()
                .map(|d| d.name().to_owned())
                .unwrap_or_default();
            println!("  {name}");
            for c in device.supported_output_configs().into_iter().flatten() {
                println!(
                    "    {:?} {}ch {}..{} Hz buffer {:?}",
                    c.sample_format(),
                    c.channels(),
                    c.min_sample_rate(),
                    c.max_sample_rate(),
                    c.buffer_size()
                );
            }
        }
    }
}
