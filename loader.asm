[org 0x1000]
[bits 16]

KERNEL_LBA equ 65
; Must match kernel_bin size after objcopy
; Update this when kernel size changes
KERNEL_SIZE equ 192576
KERNEL_SECTORS equ 377
KERNEL_BUF equ 0x10000

%macro LOG 1
    mov si, %1
    call log_line
%endmacro

%macro LOG32 1
    mov esi, %1
    call log_line32
%endmacro

start:
    ; Grab the boot sector's TSC calibration (absolute 0x600)
    xor ax, ax
    mov ds, ax
    mov eax, [0x600]
    mov [tsc_per_ms], eax
    mov eax, [0x608]
    mov [boot_start_tsc], eax

    ; Set DS = CS so string operations work
    push cs
    pop ds
    mov [drive], dl

    rdtsc
    mov [last_tsc], eax

    call ser_init
    LOG msg_ent

    ; CPU vendor via CPUID
    mov eax, 0
    cpuid
    mov [vendor_buf], ebx
    mov [vendor_buf+4], edx
    mov [vendor_buf+8], ecx
    mov byte [vendor_buf+12], 0
    mov si, msg_cpu
    call ser_str
    mov si, vendor_buf
    call ser_str
    call tsc_delta
    call print_ms10

    LOG msg_timer

    ; Enable A20
    LOG msg_a20
    cli
    call a20_w
    mov al, 0xD1
    out 0x64, al
    call a20_w
    mov al, 0xDF
    out 0x60, al
    call a20_w
    sti
    LOG msg_a20_ok

    ; Read kernel to buffer
    LOG msg_loading
    mov ax, KERNEL_BUF >> 4
    mov es, ax
    xor bx, bx
    mov word [lba], KERNEL_LBA
    mov cx, KERNEL_SECTORS
.lp:
    push cx
    push es
    push bx
    call read_one
    pop bx
    pop es
    pop cx
    jc disk_fail
    add bx, 512
    jnc .n
    mov ax, es
    add ax, 0x1000
    mov es, ax
.n:
    inc word [lba]
    loop .lp

    LOG msg_loaded

    ; ── VBE probe ───────────────────────────────────────────────
    LOG msg_vbe
    call vbe_probe
    ; VBE may have clobbered segment registers — restore them
    xor ax, ax
    mov ds, ax
    mov es, ax
    cmp word [0x7000], 1
    jne .vbe_no
    LOG msg_vbe_ok
    jmp .vbe_cont
.vbe_no:
    LOG msg_vbe_fail
.vbe_cont:

    ; Pass TSC calibration + boot-start TSC to the kernel (boot info)
    xor ax, ax
    mov ds, ax
    mov eax, [tsc_per_ms]
    mov [0x7020], eax
    mov dword [0x7024], 0
    mov eax, [boot_start_tsc]
    mov [0x7028], eax
    mov dword [0x702C], 0
    push cs
    pop ds

    ; ── Switch to protected mode ─────────────────────────────────
    LOG msg_pmode
    lgdt [gdt_ptr]
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    jmp 0x08:m32

[bits 32]
m32:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov esp, 0x90000

    ; Copy kernel to 0x100000
    LOG32 msg_copy
    cld
    mov esi, KERNEL_BUF
    mov edi, 0x100000
    mov ecx, KERNEL_SIZE
    rep movsb
    LOG32 msg_copied

    ; Page tables + PAE + long mode
    LOG32 msg_paging
    mov eax, cr4
    or eax, 0x20
    mov cr4, eax

    mov eax, pt_template
    mov cr3, eax

    mov ecx, 0xC0000080
    rdmsr
    or eax, 0x100
    wrmsr

    mov eax, cr0
    or eax, 0x80000000
    mov cr0, eax
    LOG32 msg_long

    LOG32 msg_handoff
    jmp 0x18:m64

[bits 64]
m64:
    mov ax, 0x20
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov rsp, 0x90000
    cli
    mov rax, 0x100000
    jmp rax

[bits 16]
read_one:
    push ax
    push cx
    push dx
    mov ax, [lba]
    xor dx, dx
    mov cx, 63
    div cx
    mov cx, dx
    inc cx
    xor dx, dx
    mov di, 16
    div di
    mov dh, dl
    mov ch, al
    mov ah, 0x02
    mov al, 1
    mov dl, [drive]
    int 0x13
    pop dx
    pop cx
    pop ax
    ret

a20_w:
    in al, 0x64
    test al, 2
    jnz a20_w
    ret

disk_fail:
    mov si, msg_fail
    call ser_str
    jmp $

; ── TSC timing (16-bit) ─────────────────────────────────────────

; eax = ticks elapsed since last log line; updates the baseline
tsc_delta:
    rdtsc
    mov ecx, [last_tsc]
    mov [last_tsc], eax
    sub eax, ecx
    ret

; si = message → print message then " (N.N ms)" + newline
log_line:
    call ser_str
    call tsc_delta
    jmp print_ms10

; Print " (N.N ms)" for a tick delta in eax
print_ms10:
    mov si, msg_open
    call ser_str
    mov ecx, 10
    mul ecx
    div dword [tsc_per_ms]   ; eax = ms*10
    mov bl, 10
    div bl                   ; AL = ms, AH = tenths digit
    push ax
    xor ah, ah
    call print_dec16
    mov si, msg_dot
    call ser_str
    pop ax
    movzx ax, ah
    call print_dec16
    mov si, msg_ms
    call ser_str
    ret

; Print unsigned decimal value in ax (0..65535)
print_dec16:
    pusha
    mov bx, 10
    mov si, dec_buf + 11
.l:
    xor dx, dx
    div bx
    add dl, '0'
    dec si
    mov [si], dl
    test ax, ax
    jnz .l
    call ser_str
    popa
    ret

; ── TSC timing (32-bit) ─────────────────────────────────────────

[bits 32]
ser_str32:
    push eax
    push edx
.l:
    lodsb
    or al, al
    jz .d
    mov ah, al
    mov dx, 0x3FD
.w:
    in al, dx
    and al, 0x20
    jz .w
    mov dx, 0x3F8
    xchg al, ah
    out dx, al
    jmp .l
.d:
    pop edx
    pop eax
    ret

tsc_delta32:
    rdtsc
    mov ecx, [last_tsc]
    mov [last_tsc], eax
    sub eax, ecx
    ret

log_line32:
    call ser_str32
    call tsc_delta32
    jmp print_ms10_32

print_ms10_32:              ; eax = tick delta
    mov esi, msg_open
    call ser_str32
    mov ecx, 10
    mul ecx
    div dword [tsc_per_ms]  ; eax = ms*10
    mov bl, 10
    div bl                  ; AL = ms, AH = tenths digit
    push eax
    and eax, 0xFF
    call print_dec16_32
    mov esi, msg_dot
    call ser_str32
    pop eax
    xor eax, eax
    mov al, ah
    call print_dec16_32
    mov esi, msg_ms
    call ser_str32
    ret

print_dec16_32:             ; ax = value
    pushad
    mov ebx, 10
    mov esi, dec_buf + 11
.l:
    xor edx, edx
    div ebx
    add dl, '0'
    dec esi
    mov [esi], dl
    test eax, eax
    jnz .l
    call ser_str32
    popad
    ret

[bits 16]
ser_init:
    push ax
    push dx
    mov dx, 0x3F8+1
    mov al, 0
    out dx, al
    mov dx, 0x3F8+3
    mov al, 0x80
    out dx, al
    mov dx, 0x3F8
    mov al, 1
    out dx, al
    mov dx, 0x3F8+1
    mov al, 0
    out dx, al
    mov dx, 0x3F8+3
    mov al, 3
    out dx, al
    mov dx, 0x3F8+2
    mov al, 0xC7
    out dx, al
    mov dx, 0x3F8+4
    mov al, 0x0B
    out dx, al
    pop dx
    pop ax
    ret

ser_str:
    push ax
    push si
    push dx
.l:
    lodsb
    or al, al
    jz .d
    mov dx, 0x3FD
.w:
    in al, dx
    and al, 0x20
    jz .w
    mov dx, 0x3F8
    mov al, [si-1]
    out dx, al
    jmp .l
.d:
    pop dx
    pop si
    pop ax
    ret

; ── VBE probe ──────────────────────────────────────────────────
vbe_probe:
    pusha
    push es

    xor ax, ax
    mov [0x7000], ax

    call vbe_mode_scan
    cmp word [0x7000], 1
    je .done

    call bochs_vbe_setup

.done:
    xor ax, ax
    mov ds, ax
    pop es
    popa
    ret

; ── Standard VBE mode scan ─────────────────────────────────────
; Walks the VBE mode list looking for a usable mode at 32bpp.
; Tries: 2560x1440, 1920x1080, 1366x768, 1024x768.
; Sets [0x7000] = 1 on success.
vbe_mode_scan:
    pusha
    push es

    xor ax, ax
    mov [0x7000], ax

    ; Get VBE controller info
    mov ax, 0x4F00
    mov di, 0x6000
    int 0x10
    cmp ax, 0x004F
    jne .ms_fail

    ; Verify "VESA" signature
    cmp word [0x6000], 0x4156
    jne .ms_fail
    cmp word [0x6002], 0x4553
    jne .ms_fail

    ; Walk mode list: store pointer, keep it in a local variable
    mov ax, [0x6014]
    mov bx, [0x6016]
    mov [.ms_offset], ax
    mov [.ms_seg], bx

.ms_loop:
    mov bx, [.ms_seg]
    mov es, bx
    mov si, [.ms_offset]
    mov cx, [es:si]
    cmp cx, 0xFFFF
    je .ms_fail

    push cx
    mov ax, 0x4F01
    mov di, 0x6200
    int 0x10
    pop cx
    cmp ax, 0x004F
    jne .ms_next

    test word [0x6200], 0x80       ; LFB?
    jz .ms_next
    cmp byte [0x6210], 32           ; 32 bpp?
    jne .ms_next

    mov ax, [0x620C]                ; width
    mov bx, [0x620E]                ; height
    cmp ax, 1920
    jne .ms_chk2560
    cmp bx, 1080
    je .ms_found
.ms_chk2560:
    cmp ax, 2560
    jne .ms_chk1366
    cmp bx, 1440
    je .ms_found
.ms_chk1366:
    cmp ax, 1366
    jne .ms_chk1024
    cmp bx, 768
    je .ms_found
.ms_chk1024:
    cmp ax, 1024
    jne .ms_next
    cmp bx, 768
    jne .ms_next

.ms_found:
    push cx
    mov ax, 0x4F02
    mov bx, cx
    or bx, 0x4000
    int 0x10
    pop cx
    cmp ax, 0x004F
    jne .ms_fail

    xor ax, ax
    mov ds, ax
    mov word [0x7000], 1
    mov eax, [0x621C]
    mov [0x7008], eax
    mov ax, [0x620C]
    mov [0x7010], ax
    mov ax, [0x620E]
    mov [0x7014], ax
    mov ax, [0x6214]
    mov [0x7018], ax
    mov al, [0x6210]
    shr al, 3
    mov [0x701C], al
    jmp .ms_done

.ms_next:
    add word [.ms_offset], 2
    jmp .ms_loop

.ms_fail:
    xor ax, ax
    mov [0x7000], ax

.ms_done:
    xor ax, ax
    mov ds, ax
    pop es
    popa
    ret

.ms_offset: dw 0
.ms_seg: dw 0

; ── Bochs VBE direct programming ──────────────────────────────
; For QEMU's -vga std which uses the Bochs VBE chipset.
; Programs the LFB directly via IO ports at 0x1CE/0x1CF.
bochs_vbe_setup:
    pusha

    call pci_find_vga
    cmp word [0x7000], 1
    jne .bv_fail

    ; Check Bochs VBE presence by reading ID register
    mov dx, 0x1CE
    mov ax, 0
    out dx, ax
    mov dx, 0x1CF
    in ax, dx
    and ax, 0xFFF0
    cmp ax, 0xB0C0
    jne .bv_fail

    ; Disable while programming
    mov dx, 0x1CE
    mov ax, 4
    out dx, ax
    mov dx, 0x1CF
    xor ax, ax
    out dx, ax

    mov dx, 0x1CE
    mov ax, 1
    out dx, ax
    mov dx, 0x1CF
    mov ax, 1920
    out dx, ax

    mov dx, 0x1CE
    mov ax, 2
    out dx, ax
    mov dx, 0x1CF
    mov ax, 1080
    out dx, ax

    mov dx, 0x1CE
    mov ax, 3
    out dx, ax
    mov dx, 0x1CF
    mov ax, 32
    out dx, ax

    mov dx, 0x1CE
    mov ax, 4
    out dx, ax
    mov dx, 0x1CF
    mov ax, 0x41          ; enable + LFB
    out dx, ax

    ; Store boot info (framebuffer address already set by pci_find_vga)
    xor ax, ax
    mov ds, ax
    mov word [0x7000], 1
    mov dword [0x7010], 1920
    mov dword [0x7014], 1080
    mov dword [0x7018], 1920 * 4
    mov byte [0x701C], 4

.bv_fail:
    xor ax, ax
    mov ds, ax
    popa
    ret

; ── PCI config space scan for QEMU VGA ────────────────────────
; Scans bus 0, devices 0-31 for vendor 0x1234, device 0x1111.
; Reads BAR0 (framebuffer address) and stores at [0x7008].
; Sets [0x7000] = 1 on success, 0 on failure.
pci_find_vga:
    pusha
    push es

    xor ax, ax
    mov es, ax
    mov word [0x7000], 0

    xor cx, cx          ; device counter (0-31)
.pv_loop:
    cmp cx, 32
    jge .pv_done

    ; Build PCI config address: bus=0, device=cx, func=0, reg=0
    push cx
    xor eax, eax
    mov ax, cx
    shl eax, 11         ; device << 11
    or eax, 0x80000000  ; enable bit
    mov dx, 0xCF8
    o32 out dx, eax     ; write to config address port (32-bit)

    ; Read vendor/device ID from offset 0
    mov dx, 0xCFC
    o32 in eax, dx
    cmp eax, 0x11111234 ; vendor=0x1234, device=0x1111
    pop cx
    jne .pv_next

    ; Found QEMU VGA! Program BAR0 to 0x38000000 (within 0-1GB mapped region)
    push cx
    
    ; Build config address for BAR0
    xor eax, eax
    mov ax, cx
    shl eax, 11
    or eax, 0x80000010  ; enable bit + offset 0x10
    mov dx, 0xCF8
    o32 out dx, eax
    
    ; Write desired address to BAR0
    mov eax, 0x38000000
    mov dx, 0xCFC
    o32 out dx, eax
    
    ; Read back to verify
    mov dx, 0xCF8
    o32 out dx, eax  ; Re-send config address (it gets reset)
    ; Rebuild config address
    pop cx
    push cx
    xor eax, eax
    mov ax, cx
    shl eax, 11
    or eax, 0x80000010
    mov dx, 0xCF8
    o32 out dx, eax
    mov dx, 0xCFC
    o32 in eax, dx
    o32 and eax, 0xFFFFFFF0 ; mask off lower 4 bits (flags)
    
    ; Store framebuffer address at [0x7008]
    xor bx, bx
    mov es, bx
    mov [es:0x7008], eax
    mov dword [es:0x700C], 0  ; clear upper 32 bits
    mov word [es:0x7000], 1   ; success

    pop cx
    jmp .pv_done

.pv_next:
    inc cx
    jmp .pv_loop

.pv_done:
    xor ax, ax
    mov es, ax
    pop es
    popa
    ret

drive: db 0
lba: dw 0

msg_ent:    db "[LOADER] [INIT  ] COM1 115200 8N1 ready", 0
msg_cpu:    db "[LOADER] [CPU   ] vendor: ", 0
msg_timer:  db "[LOADER] [TIMER ] TSC calibrated by boot sector", 0
msg_a20:    db "[LOADER] [A20   ] enabling via 8042 controller", 0
msg_a20_ok: db "[LOADER] [A20   ] enabled", 0
msg_loading: db "[LOADER] [DISK  ] kernel: 369 sectors @ LBA 65, 188,464 B", 0
msg_loaded: db "[LOADER] [DISK  ] loaded to 0x10000", 0
msg_vbe:    db "[LOADER] [VBE   ] probing VESA modes", 0
msg_vbe_ok: db "[LOADER] [VBE   ] 1920x1080 @32bpp LFB", 0
msg_vbe_fail: db "[LOADER] [VBE   ] unavailable - VGA text fallback", 0
msg_pmode:  db "[LOADER] [CPU   ] switching to protected mode", 0
msg_copy:   db "[LOADER] [KERNEL] copying 188,464 B to 0x100000", 0
msg_copied: db "[LOADER] [KERNEL] copied", 0
msg_paging: db "[LOADER] [MMU   ] page tables: 0-1GB identity + 3-4GB MMIO", 0
msg_long:   db "[LOADER] [CPU   ] PAE + long mode active", 0
msg_handoff: db "[LOADER] [HANDOF] jumping to kernel @ 0x100000", 0
msg_fail:   db "[LOADER] [DISK  ] read FAIL", 13, 10, 0
msg_open:   db " (", 0
msg_dot:    db ".", 0
msg_ms:     db " ms)", 13, 10, 0

tsc_per_ms: dd 0
last_tsc:   dd 0
boot_start_tsc: dd 0
vendor_buf: times 16 db 0
dec_buf:    times 12 db 0

; GDT
gdt:
    dq 0x0000000000000000
    dq 0x00CF9A000000FFFF
    dq 0x00CF92000000FFFF
    dq 0x00209A0000000000
    dq 0x0000920000000000
gdt_end:
gdt_ptr:
    dw gdt_end - gdt - 1
    dq gdt

; Pre-built page tables (16KB)
align 4096
pt_template:
pml4t: dq 0x0000000000003003
       times 511 dq 0

align 4096
pdpt:  dq 0x0000000000004003
       times 2 dq 0
       dq 0x0000000000005003
       times 508 dq 0

align 4096
pdt:   dq 0x0000000000000083
       dq 0x0000000000200083
%assign i 2
%rep 510
       dq (i * 0x200000) | 0x83
%assign i i + 1
%endrep
; All 512 PDEs filled — covers 0 to 1GB

align 4096
pdt2:
%assign i 0
%rep 512
       dq (0xC0000000 + i * 0x200000) | 0x9B
%assign i i + 1
%endrep
; All 512 PDEs filled — covers 3GB to 4GB (with cache disable for MMIO)
pt_end:
