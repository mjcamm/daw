//! Flushing denormal floats to zero while audio renders.
//!
//! As a sound decays toward silence, filters and envelopes produce numbers
//! so small the CPU handles them in a slow path ("denormals"), which can cost
//! many times the normal work and cause dropouts. Audio code runs with the
//! CPU set to treat them as zero instead: on x86-64, MXCSR's flush-to-zero
//! and denormals-are-zero bits; on ARM64, FPCR's flush-to-zero bit. Other
//! targets run unchanged.

/// Runs `f` with denormals flushed to zero, then restores the previous mode.
#[inline]
pub fn without_denormals<R>(f: impl FnOnce() -> R) -> R {
    let previous = imp::get();
    imp::set(previous | imp::FLUSH);
    let result = f();
    imp::set(previous);
    result
}

#[cfg(target_arch = "x86_64")]
mod imp {
    /// MXCSR flush-to-zero (bit 15) and denormals-are-zero (bit 6).
    pub const FLUSH: u32 = 0x8040;

    #[inline]
    pub fn get() -> u32 {
        let mut value: u32 = 0;
        // SAFETY: STMXCSR only stores the SSE control register to `value`,
        // a valid, writable local.
        unsafe {
            core::arch::asm!(
                "stmxcsr [{}]",
                in(reg) &mut value,
                options(nostack, preserves_flags)
            );
        }
        value
    }

    #[inline]
    pub fn set(value: u32) {
        // SAFETY: LDMXCSR loads a control word read from the same register
        // with only mode bits changed; it touches no memory but `value`.
        unsafe {
            core::arch::asm!(
                "ldmxcsr [{}]",
                in(reg) &value,
                options(nostack, readonly, preserves_flags)
            );
        }
    }
}

#[cfg(target_arch = "aarch64")]
mod imp {
    /// FPCR flush-to-zero (bit 24).
    pub const FLUSH: u64 = 1 << 24;

    #[inline]
    pub fn get() -> u64 {
        let value: u64;
        // SAFETY: reading FPCR has no side effects.
        unsafe {
            core::arch::asm!("mrs {}, fpcr", out(reg) value, options(nomem, nostack, preserves_flags));
        }
        value
    }

    #[inline]
    pub fn set(value: u64) {
        // SAFETY: writes a value read from FPCR with only the FZ mode bit
        // changed.
        unsafe {
            core::arch::asm!("msr fpcr, {}", in(reg) value, options(nomem, nostack, preserves_flags));
        }
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
mod imp {
    pub const FLUSH: u32 = 0;

    pub fn get() -> u32 {
        0
    }

    pub fn set(_: u32) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hint::black_box;

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    #[test]
    fn denormals_become_zero_inside_and_return_after() {
        let tiny = || black_box(f32::MIN_POSITIVE) * black_box(0.25);
        assert!(tiny() > 0.0, "a denormal outside");
        assert_eq!(without_denormals(tiny), 0.0, "flushed inside");
        assert!(tiny() > 0.0, "the mode is restored");
    }
}
