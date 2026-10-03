//! The CPU check for x86-64-v3 builds (`.cargo/config.toml` builds x86_64
//! Linux for x86-64-v3: AVX2, FMA, BMI2 and the rest of the level).
//!
//! A CPU without them would die on the first such instruction with SIGILL,
//! somewhere at random. Instead, before any Rust code runs, a constructor in
//! `.init_array` checks the level with CPUID and, if anything is missing,
//! prints why to stderr and exits with status 1. It is written in assembly
//! because the compiler is free to use the new instructions in any Rust
//! function, including the one that would do the check.
//!
//! Compiled only when the build targets those features, so a baseline build
//! carries no check.

#[cfg(all(
    target_os = "linux",
    target_arch = "x86_64",
    target_feature = "avx2",
    target_feature = "fma",
    target_feature = "bmi2"
))]
mod check {
    // CPUID bits of x86-64-v3 (with v2 below it):
    //   leaf 1 ECX: SSE3 0, SSSE3 9, FMA 12, CX16 13, SSE4.1 19, SSE4.2 20,
    //     MOVBE 22, POPCNT 23, OSXSAVE 27, AVX 28, F16C 29 = 0x38D8_3201;
    //   leaf 7 EBX: BMI1 3, AVX2 5, BMI2 8 = 0x128;
    //   leaf 0x8000_0001 ECX: LAHF 0, LZCNT 5 = 0x21;
    //   and XCR0 bits 1 and 2: the OS saves the SSE and AVX registers.
    std::arch::global_asm!(
        ".pushsection .text.tessera_require_x86_64_v3,\"ax\",@progbits",
        ".globl tessera_require_x86_64_v3",
        ".hidden tessera_require_x86_64_v3",
        ".type tessera_require_x86_64_v3,@function",
        "tessera_require_x86_64_v3:",
        "    push rbx",
        "    xor eax, eax",
        "    cpuid",
        "    cmp eax, 7",
        "    jb 2f",
        "    mov eax, 1",
        "    cpuid",
        "    and ecx, 0x38D83201",
        "    cmp ecx, 0x38D83201",
        "    jne 2f",
        "    xor ecx, ecx",
        "    xgetbv",
        "    and eax, 6",
        "    cmp eax, 6",
        "    jne 2f",
        "    mov eax, 7",
        "    xor ecx, ecx",
        "    cpuid",
        "    and ebx, 0x128",
        "    cmp ebx, 0x128",
        "    jne 2f",
        "    mov eax, 0x80000000",
        "    cpuid",
        "    cmp eax, 0x80000001",
        "    jb 2f",
        "    mov eax, 0x80000001",
        "    cpuid",
        "    and ecx, 0x21",
        "    cmp ecx, 0x21",
        "    jne 2f",
        "    pop rbx",
        "    ret",
        // write(2, message, length), then exit_group(1).
        "2:",
        "    mov eax, 1",
        "    mov edi, 2",
        "    lea rsi, [rip + .Ltessera_x86_64_v3_message]",
        "    lea rdx, [rip + .Ltessera_x86_64_v3_message_end]",
        "    sub rdx, rsi",
        "    syscall",
        "    mov eax, 231",
        "    mov edi, 1",
        "    syscall",
        "    ud2",
        ".size tessera_require_x86_64_v3, . - tessera_require_x86_64_v3",
        ".popsection",
        ".pushsection .rodata.tessera_x86_64_v3_message,\"a\",@progbits",
        ".Ltessera_x86_64_v3_message:",
        ".ascii \"tessera: this build needs an x86-64-v3 processor (AVX2, FMA and BMI2: \"",
        ".ascii \"Intel since Haswell, 2013; AMD since Excavator, 2015), \"",
        ".ascii \"and this one lacks part of it.\\n\"",
        ".Ltessera_x86_64_v3_message_end:",
        ".popsection",
        ".pushsection .init_array,\"aw\",@init_array",
        ".balign 8",
        ".quad tessera_require_x86_64_v3",
        ".popsection",
    );

    unsafe extern "C" {
        fn tessera_require_x86_64_v3();
    }

    pub(super) fn require_cpu_level() {
        // SAFETY: the function above takes no arguments, keeps the
        // callee-saved registers (rbx is pushed and popped) and either
        // returns or ends the process.
        unsafe { tessera_require_x86_64_v3() }
    }
}

/// Checks again that the CPU has what this build was compiled for (the
/// constructor already did, before `main`). Binaries call it first thing:
/// the call is what links the constructor into them.
pub fn require_cpu_level() {
    #[cfg(all(
        target_os = "linux",
        target_arch = "x86_64",
        target_feature = "avx2",
        target_feature = "fma",
        target_feature = "bmi2"
    ))]
    check::require_cpu_level();
}
