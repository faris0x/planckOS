; AP startup trampoline. Assembled with NASM to a flat binary and embedded
; into the kernel (see build.rs / smp.rs). At runtime smp.rs copies it to
; physical 0x8000 and sends INIT/SIPI so a secondary core executes the
; 16-bit entry here, transitions through 32-bit to long mode, loads its
; per-CPU GDT/TSS/IDT/stack, and jumps to ap_main64.
;
; Patchable slots are located by the byte markers below; smp.rs searches for
; the marker and writes the following field.
[org 0x8000]
[bits 16]

; Real-mode entry (SIPI vector 0x08 -> CS:IP = 0x0000:0x8000).
start16:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    lgdt [gdt16_ptr]
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    jmp 0x08:tramp32

[bits 32]
tramp32:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov eax, cr4
    or eax, 0x20
    mov cr4, eax
    mov eax, [patch_cr3]
    mov cr3, eax
    mov ecx, 0xC0000080
    rdmsr
    or eax, 0x100
    wrmsr
    mov eax, cr0
    or eax, 0x80000000
    mov cr0, eax
    jmp 0x18:tramp64

[bits 64]
tramp64:
    mov ax, 0x20
    mov ds, ax
    mov es, ax
    mov ss, ax
    lgdt [patch_gdt]
    mov ax, 0x28
    ltr ax
    lidt [patch_idt]
    mov rsp, [patch_stack]
    mov rax, [patch_entry]
    mov rdi, [patch_cpu]
    xor rbp, rbp
    jmp rax

; ── Patchable data (located by markers) ───────────────────────────
align 8
m_cr3:     db 0xDE, 0xAD, 0xBE, 0xEF
patch_cr3: dq 0
m_gdt:     db 0xCA, 0xFE, 0xBA, 0xBE
patch_gdt: dw 0
           dq 0
m_idt:     db 0x01, 0x23, 0x45, 0x67
patch_idt: dw 0
           dq 0
m_stack:   db 0x89, 0xAB, 0xCD, 0xEF
patch_stack: dq 0
m_cpu:     db 0xFE, 0xDC, 0xBA, 0x98
patch_cpu: dq 0
m_entry:   db 0x76, 0x54, 0x32, 0x10
patch_entry: dq 0

; 16-bit GDT for the real -> protected -> long transition.
gdt16_ptr: dw gdt16_end - gdt16 - 1
           dd gdt16
gdt16:
    dq 0x0000000000000000
    dq 0x00CF9A000000FFFF
    dq 0x00CF92000000FFFF
    dq 0x00209A0000000000
    dq 0x0000920000000000
gdt16_end:
