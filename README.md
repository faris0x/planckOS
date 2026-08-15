# planckOS

Copyright (c) 2026 Faris Alfarhan

Licensed GNU GPL version 3.

## Current features

### Boot chain & startup
- Custom MBR stage-1 (boot.asm, fits exactly 512 bytes incl. 0xAA55 magic), BIOS real-mode bootstrap, loads stage 2 via INT 13h CHS (40 sectors, the full loader incl. its page tables), no reliance on any bootloader (GRUB/Limine etc.)
- Custom stage-2 loader (loader.asm, 20KB), real-mode: serial init, CPUID vendor string, A20 gate via 8042 keyboard controller, E820-independent VBE framebuffer probe, reads the kernel via INT 13h LBA into a staging buffer, then a full real->protected->long-mode transition: its own GDT, CR0.PE, CR4.PAE, a pre-built 4-level page-table template (0-1GB identity + 3-4GB MMIO window covering the framebuffer and the local APIC / IO-APIC), EFER.LME + paging enable, far-jump into 64-bit
- 64-bit kernel entry (_start in `src/main.rs`), kernel copied to 0x100000, relocated, BSS zeroed, boot-info block consumed from 0x7000 (framebuffer geometry, TSC calibration, boot-start TSC)

### Boot diagnostics & logging
- Unified stage-agnostic logging: `[CPU<n>] [PART  ] [SUB   ] message (N.N ms)` with per-phase TSC timing across MBR->loader->kernel->filesystem self-tests
- TSC calibration in the MBR against PIT channel 0 (~10ms window), forwarded via absolute 0x600/0x608 through the loader's boot-info block (0x7020/0x7028) into the kernel's logger
- ~60 timed log lines per boot covering every init stage

### Memory management
- Boundary-tag heap allocator (`src/hal/heap.rs`), 4 MiB heap at 0x200000-0x600000 with prologue/epilogue fence blocks, forward/backward coalescing, verified leak-free (AFAIK), the heap lock is an interrupt-safe spinlock so the allocator is SMP-safe
- 20-test host-side stress harness (`tests/allocator/`), allocations, frees, overflow handling, bogus-pointer robustness, fragmentation, fence integrity, all passing

### Graphics & display
- Framebuffer backend (`src/hal/framebuffer.rs`), Bochs VBE + PCI scan (QEMU 0x1234:0x1111), 1920x1080x32 BGRX at 0x38000000, PSF2 bitmap font rendering (Spleen 32x64, Copyright (c) 2018-2026, Frederic Cambus), software cursor with prev-position erasure, downscale: 2 -> effective 120x33 text grid, background #17002e
- VGA text-mode fallback backend (`src/hal/display.rs`), runtime-selectable DisplayBackend
- Two bitmap fonts bundled (Spleen 32x64, Sun 16x32), third-party assets, copyright belonging to their respective owners (see below)

### Device drivers (HAL)
- Serial: COM1 115200 8N1, polled I/O, both 16/32-bit asm and Rust drivers, cross-core lock and `[CPU<n>]` log prefix
- PS/2 keyboard (`src/hal/input.rs`), scan-code handling, controller init, delivered to the BSP via the IO-APIC
- IDT + APIC: per-core 256-vector IDTs, local APIC + IO-APIC interrupt delivery, 8259 PICs masked (bypassed), APIC timer calibrated to ~1000 Hz
- RTC (`src/hal/rtc.rs`), date/time readout
- CPUID (`src/hal/cpuid.rs`), vendor string, feature bits
- ACPI (`src/hal/acpi.rs`), RSDP scan of BIOS ROM, RSDP/RSDT checksum validation, FADT/DSDT parsing, \_S5 SLP_TYP extraction for power-off, MADT parsing (local APIC base, IO-APIC address, per-core APIC IDs)
- E820 (`src/hal/e820.rs`), BIOS memory-map enumeration, low-memory regions reserved (loader/page tables, AP trampoline, BSP stack, EBDA, kernel heap)
- ATA PIO (`src/hal/ata.rs`, `src/hal/block.rs`), primary/secondary channel detection, sector read/write

### SMP & multicore
- MADT-driven core detection, secondary cores booted via INIT-SIPI from a 16->32->64-bit trampoline staged at 0x8000 (assembled by build.rs with NASM)
- Per-core IDT / GDT / TSS / stack, fixed-delivery IPIs (BSP<->AP ping verified), per-core exception isolation (a fault halts only the offending core)
- Verified under QEMU with `-smp 2` and `-smp 4`

### Filesystem
- Custom FAT32 driver (`src/hal/fat32.rs`, ~68KB of Rust), full read/write: f_stat, f_open/f_read, tell/eof/size, opendir/readdir, findfirst, mkdir, create/write/sync, gets, putc/puts/printf, lseek, truncate, expand, unlink, rename, chmod, utime, getfree, getlabel/setlabel, chdir/getcwd, wildcard patterns, exercised by 30 automated self-tests (`[FS] [TEST] ... PASS`) at every boot
- 64MB FAT32 drive at PIO ATA secondary master

### Shell & user interface
- REPL shell (`src/shell/`) with command history, builtins: `echo`, `cls`, `banner`, `history`, `ls`, `out`, `mk`, `rm`, `cp`, `shutdown`, `help`
- Applet registry + dispatch (compile-time toggled via applets.toml)
- Active applets: `sysinfo` (CPUID/E820/ACPI hardware report), `datetime` (RTC), `heap` (allocator stats), plus `ls`/`mk`/`cp`/`rm`/`out` as file-manipulation applets

### Build & tooling
- just-driven pipeline: NASM -> objcopy -> dd image assembly, `just <run-sdl/run-serial>` QEMU launchers (boot with `-smp 2`), `just run-smp` (4 cores), `just test-alloc`, `just clean` (removes all artifacts)
- Python `generate_cfg.py` -> RUSTFLAGS --cfg applet_* from `applets.toml`
- no_std Rust, custom linker script, GPLv3

## Fonts

- `src/hal/fonts/spleen-32x64.psfu`, Spleen font, Copyright (c) Frederic Cambus, distributed under the BSD 2-Clause license. Not the property of planckOS; all rights reserved to its owner (see [GitHub repository](https://github.com/fcambus/spleen)).
- `src/hal/fonts/sun-16x32.psfu`, derived from the Sun Microsystems bitmap font family (X11). Not the property of planckOS; all rights reserved to its owners.
