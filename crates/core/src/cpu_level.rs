//! The CPU check for builds that target x86-64-v3 or v4 (`.cargo/config.toml`
//! builds x86_64 Linux and Windows for x86-64-v3: AVX2, FMA, BMI2 and the rest
//! of the level; a build for v4 adds AVX-512).
//!
//! A CPU without them would die on the first such instruction with SIGILL,
//! somewhere at random. Instead, before any Rust code runs, a constructor
//! checks the level with CPUID and, if anything is missing, says why and exits
//! with status 1: on Linux from `.init_array`, printing to stderr; on Windows
//! from the C runtime's initializer table (`.CRT$XCT`), in a message box,
//! since the app has no console there. It is written in assembly because the
//! compiler is free to use the new instructions in any Rust function,
//! including the one that would do the check.
//!
//! The check covers the level the build targets: v4's AVX-512 bits only in a
//! build that targets them. Compiled only when the build targets at least v3,
//! so a baseline build carries no check.

/// CPUID and XCR0 bits each level needs (v2 is inside v3).
#[cfg(any(
    test,
    all(
        target_arch = "x86_64",
        any(target_os = "linux", target_os = "windows"),
        target_feature = "avx2",
        target_feature = "fma",
        target_feature = "bmi2"
    )
))]
mod bits {
    /// Leaf 1 ECX: SSE3 0, SSSE3 9, FMA 12, CX16 13, SSE4.1 19, SSE4.2 20,
    /// MOVBE 22, POPCNT 23, OSXSAVE 27, AVX 28, F16C 29.
    pub(super) const LEAF1_ECX: u32 = 0x38D8_3201;
    /// Leaf 0x8000_0001 ECX: LAHF 0, LZCNT 5.
    pub(super) const EXT1_ECX: u32 = 0x21;
    /// Leaf 7 EBX for v3: BMI1 3, AVX2 5, BMI2 8.
    pub(super) const V3_LEAF7_EBX: u32 = 0x128;
    /// Leaf 7 EBX for v4: v3's, plus AVX512F 16, AVX512DQ 17, AVX512CD 28,
    /// AVX512BW 30, AVX512VL 31.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) const V4_LEAF7_EBX: u32 = V3_LEAF7_EBX | 0xD003_0000;
    /// XCR0 for v3: the OS saves the SSE (1) and AVX (2) registers.
    pub(super) const V3_XCR0: u32 = 0x6;
    /// XCR0 for v4: also the opmask (5), ZMM_Hi256 (6) and Hi16_ZMM (7)
    /// registers.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) const V4_XCR0: u32 = V3_XCR0 | 0xE0;
}

#[cfg(all(
    target_arch = "x86_64",
    any(target_os = "linux", target_os = "windows"),
    target_feature = "avx2",
    target_feature = "fma",
    target_feature = "bmi2"
))]
mod check {
    use super::bits;

    // The level this build targets: v4 when it targets all of AVX-512 F, BW,
    // CD, DQ and VL, else v3.
    #[cfg(not(all(
        target_feature = "avx512f",
        target_feature = "avx512bw",
        target_feature = "avx512cd",
        target_feature = "avx512dq",
        target_feature = "avx512vl"
    )))]
    mod level {
        pub(super) const LEAF7_EBX: u32 = super::bits::V3_LEAF7_EBX;
        pub(super) const XCR0: u32 = super::bits::V3_XCR0;
        macro_rules! needs {
            () => {
                "an x86-64-v3 processor (AVX2, FMA and BMI2: Intel since Haswell, 2013; AMD since Excavator, 2015)"
            };
        }
        pub(super) use needs;
    }

    #[cfg(all(
        target_feature = "avx512f",
        target_feature = "avx512bw",
        target_feature = "avx512cd",
        target_feature = "avx512dq",
        target_feature = "avx512vl"
    ))]
    mod level {
        pub(super) const LEAF7_EBX: u32 = super::bits::V4_LEAF7_EBX;
        pub(super) const XCR0: u32 = super::bits::V4_XCR0;
        macro_rules! needs {
            () => {
                "an x86-64-v4 processor (AVX-512 F, BW, CD, DQ and VL: AMD since Zen 4, 2022; Intel Xeon since Skylake-SP, 2017)"
            };
        }
        pub(super) use needs;
    }

    /// What a refused CPU is told, with the version so a bug report can name it.
    macro_rules! message {
        () => {
            concat!(
                "twicecat ",
                env!("CARGO_PKG_VERSION"),
                ": this build needs ",
                level::needs!(),
                ", and this processor lacks part of it."
            )
        };
    }

    /// The checks, the same on both systems: each jumps to the local label
    /// `2` when something is missing; all present, it pops rbx and returns.
    /// XCR0 is read only after leaf 1 confirmed OSXSAVE.
    macro_rules! cpuid_checks {
        () => {
            concat!(
                "    push rbx\n",
                "    xor eax, eax\n",
                "    cpuid\n",
                "    cmp eax, 7\n",
                "    jb 2f\n",
                "    mov eax, 1\n",
                "    cpuid\n",
                "    and ecx, {leaf1}\n",
                "    cmp ecx, {leaf1}\n",
                "    jne 2f\n",
                "    xor ecx, ecx\n",
                "    xgetbv\n",
                "    and eax, {xcr0}\n",
                "    cmp eax, {xcr0}\n",
                "    jne 2f\n",
                "    mov eax, 7\n",
                "    xor ecx, ecx\n",
                "    cpuid\n",
                "    and ebx, {leaf7}\n",
                "    cmp ebx, {leaf7}\n",
                "    jne 2f\n",
                "    mov eax, 0x80000000\n",
                "    cpuid\n",
                "    cmp eax, 0x80000001\n",
                "    jb 2f\n",
                "    mov eax, 0x80000001\n",
                "    cpuid\n",
                "    and ecx, {ext1}\n",
                "    cmp ecx, {ext1}\n",
                "    jne 2f\n",
                "    pop rbx\n",
                "    ret\n",
            )
        };
    }

    #[cfg(target_os = "linux")]
    std::arch::global_asm!(
        ".pushsection .text.twicecat_require_cpu_level,\"ax\",@progbits",
        ".globl twicecat_require_cpu_level",
        ".hidden twicecat_require_cpu_level",
        ".type twicecat_require_cpu_level,@function",
        "twicecat_require_cpu_level:",
        cpuid_checks!(),
        // write(2, message, length), then exit_group(1).
        "2:",
        "    mov eax, 1",
        "    mov edi, 2",
        "    lea rsi, [rip + .Ltwicecat_cpu_level_message]",
        "    lea rdx, [rip + .Ltwicecat_cpu_level_message_end]",
        "    sub rdx, rsi",
        "    syscall",
        "    mov eax, 231",
        "    mov edi, 1",
        "    syscall",
        "    ud2",
        ".size twicecat_require_cpu_level, . - twicecat_require_cpu_level",
        ".popsection",
        ".pushsection .rodata.twicecat_cpu_level_message,\"a\",@progbits",
        ".Ltwicecat_cpu_level_message:",
        concat!(".ascii \"", message!(), "\\n\""),
        ".Ltwicecat_cpu_level_message_end:",
        ".popsection",
        ".pushsection .init_array,\"aw\",@init_array",
        ".balign 8",
        ".quad twicecat_require_cpu_level",
        ".popsection",
        leaf1 = const bits::LEAF1_ECX,
        ext1 = const bits::EXT1_ECX,
        leaf7 = const level::LEAF7_EBX,
        xcr0 = const level::XCR0,
    );

    // Windows: the C runtime calls every pointer between `.CRT$XCA` and
    // `.CRT$XCZ`, in section name order, before `main`. `XCT` runs before
    // `XCU`, where C++ constructors and Rust crates put theirs. The section is
    // not COMDAT, so the linker keeps it whenever this object is linked, which
    // `require_cpu_level` below makes sure of.
    #[cfg(target_os = "windows")]
    std::arch::global_asm!(
        ".section .text$twicecat_cpu_level,\"xr\"",
        ".globl twicecat_require_cpu_level",
        "twicecat_require_cpu_level:",
        cpuid_checks!(),
        // MessageBoxA(NULL, message, "Twicecat", MB_ICONERROR), then
        // ExitProcess(1). rbx is still pushed, so rsp is 16-byte aligned, and
        // the 32 bytes of shadow space keep it so for the calls.
        "2:",
        "    sub rsp, 32",
        "    xor ecx, ecx",
        "    lea rdx, [rip + .Ltwicecat_cpu_level_message]",
        "    lea r8, [rip + .Ltwicecat_cpu_level_title]",
        "    mov r9d, 0x10",
        "    call qword ptr [rip + __imp_MessageBoxA]",
        "    mov ecx, 1",
        "    call qword ptr [rip + __imp_ExitProcess]",
        "    ud2",
        ".section .rdata$twicecat_cpu_level,\"dr\"",
        ".Ltwicecat_cpu_level_message:",
        concat!(".asciz \"", message!(), "\""),
        ".Ltwicecat_cpu_level_title:",
        ".asciz \"Twicecat\"",
        ".section .CRT$XCT,\"dr\"",
        ".balign 8",
        ".quad twicecat_require_cpu_level",
        ".text",
        leaf1 = const bits::LEAF1_ECX,
        ext1 = const bits::EXT1_ECX,
        leaf7 = const level::LEAF7_EBX,
        xcr0 = const level::XCR0,
    );

    // The import libraries for MessageBoxA and ExitProcess.
    #[cfg(target_os = "windows")]
    #[link(name = "user32")]
    unsafe extern "system" {}
    #[cfg(target_os = "windows")]
    #[link(name = "kernel32")]
    unsafe extern "system" {}

    unsafe extern "C" {
        fn twicecat_require_cpu_level();
    }

    pub(super) fn require_cpu_level() {
        // SAFETY: the function above takes no arguments, keeps the
        // callee-saved registers (rbx is pushed and popped) and either
        // returns or ends the process.
        unsafe { twicecat_require_cpu_level() }
    }
}

/// Checks again that the CPU has what this build was compiled for (the
/// constructor already did, before `main`). Binaries call it first thing:
/// the call is what links the constructor into them.
pub fn require_cpu_level() {
    #[cfg(all(
        target_arch = "x86_64",
        any(target_os = "linux", target_os = "windows"),
        target_feature = "avx2",
        target_feature = "fma",
        target_feature = "bmi2"
    ))]
    check::require_cpu_level();
}

#[cfg(test)]
mod tests {
    use super::bits::*;

    fn mask(bits: &[u32]) -> u32 {
        bits.iter().fold(0, |mask, bit| mask | 1 << bit)
    }

    #[test]
    fn the_masks_are_the_documented_bits() {
        assert_eq!(LEAF1_ECX, mask(&[0, 9, 12, 13, 19, 20, 22, 23, 27, 28, 29]));
        assert_eq!(EXT1_ECX, mask(&[0, 5]));
        assert_eq!(V3_LEAF7_EBX, mask(&[3, 5, 8]));
        assert_eq!(V4_LEAF7_EBX, mask(&[3, 5, 8, 16, 17, 28, 30, 31]));
        assert_eq!(V3_XCR0, mask(&[1, 2]));
        assert_eq!(V4_XCR0, mask(&[1, 2, 5, 6, 7]));
    }

    /// This machine runs the test binary, which this build's constructor let
    /// through, so it has every bit the build needs.
    #[cfg(all(
        target_arch = "x86_64",
        target_feature = "avx2",
        target_feature = "fma",
        target_feature = "bmi2"
    ))]
    #[test]
    fn this_machine_has_the_v3_bits() {
        use std::arch::x86_64::{__cpuid, __cpuid_count, _xgetbv};
        // SAFETY: CPUID and XGETBV exist on every x86-64 CPU that runs a v3
        // build (XGETBV needs OSXSAVE, which leaf 1 reports and v3 requires).
        let (leaf1, leaf7, ext1, xcr0) = unsafe {
            (
                __cpuid(1).ecx,
                __cpuid_count(7, 0).ebx,
                __cpuid(0x8000_0001).ecx,
                _xgetbv(0) as u32,
            )
        };
        assert_eq!(leaf1 & LEAF1_ECX, LEAF1_ECX);
        assert_eq!(leaf7 & V3_LEAF7_EBX, V3_LEAF7_EBX);
        assert_eq!(ext1 & EXT1_ECX, EXT1_ECX);
        assert_eq!(xcr0 & V3_XCR0, V3_XCR0);
    }
}
