//! Flush-to-zero for the audio processing thread.
//!
//! sonora 0.2.0 (the AEC3 port) slows down 10-50x while the far end is
//! silent: its filter states decay into denormal floats, which x86 handles
//! about 100x slower (upstream issue dignifiedquire/sonora#34). Upstream
//! WebRTC wraps processing in a `DenormalDisabler`; the port does not yet.
//! Setting flush-to-zero (and denormals-are-zero on x86) on the processing
//! thread has the same effect for everything that runs on it.
//!
//! This is the crate's only `unsafe` code, approved for the pre-production
//! phase (Phase 6 plan, decision 7). Remove it once a sonora release with
//! upstream PR #38 ships (a version bump is asked first, AGENTS.md rule 3).

/// Makes the current thread treat denormal floats as zero. Affects only
/// this thread's floating-point control register; call it at the start of
/// the audio processing thread. A no-op on other architectures.
#[allow(unsafe_code)]
pub fn flush_denormals_on_this_thread() {
    #[cfg(target_arch = "x86_64")]
    {
        // MXCSR bit 15 = FTZ (flush results to zero), bit 6 = DAZ (treat
        // denormal inputs as zero). SSE2 (and so MXCSR) is part of x86_64.
        let mut mxcsr: u32 = 0;
        // SAFETY: stmxcsr/ldmxcsr only read and write this thread's MXCSR
        // through a valid pointer to a local; setting FTZ/DAZ changes
        // denormal handling only, which is the purpose here.
        unsafe {
            std::arch::asm!("stmxcsr [{}]", in(reg) &mut mxcsr, options(nostack, preserves_flags));
            mxcsr |= (1 << 15) | (1 << 6);
            std::arch::asm!("ldmxcsr [{}]", in(reg) &mxcsr, options(nostack, readonly, preserves_flags));
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // FPCR bit 24 = FZ (flush denormal inputs and results to zero).
        // SAFETY: reads and writes only this thread's FPCR; setting FZ
        // changes denormal handling only.
        unsafe {
            let mut fpcr: u64;
            std::arch::asm!("mrs {}, fpcr", out(reg) fpcr, options(nomem, nostack, preserves_flags));
            fpcr |= 1 << 24;
            std::arch::asm!("msr fpcr, {}", in(reg) fpcr, options(nomem, nostack, preserves_flags));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    fn denormals_become_zero_on_this_thread_only() {
        std::thread::spawn(|| {
            let tiny = std::hint::black_box(f32::MIN_POSITIVE);
            let before = std::hint::black_box(tiny / 4.0);
            assert!(before > 0.0 && !before.is_normal(), "denormal before");
            flush_denormals_on_this_thread();
            let after = std::hint::black_box(tiny) / std::hint::black_box(4.0_f32);
            assert_eq!(after, 0.0, "flushed after");
        })
        .join()
        .unwrap();
        // Other threads keep IEEE behaviour.
        let tiny = std::hint::black_box(f32::MIN_POSITIVE);
        assert!(std::hint::black_box(tiny / 4.0) > 0.0);
    }
}
