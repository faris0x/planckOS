[org 0x1000]
[bits 16]

KERNEL_LBA equ 65
; Must match kernel_bin size after objcopy
; Update this when kernel size changes
KERNEL_SIZE equ 185000
KERNEL_SECTORS equ 362
KERNEL_BUF equ 0x10000

start:
    ; Set DS = CS so string operations work
    push cs
    pop ds
    mov [drive], dl

    call ser_init
    mov si, msg_stage2
    call ser_str

    ; Enable A20
    mov si, msg_a20
    call ser_str
    cli
    call a20_w
    mov al, 0xD1
    out 0x64, al
    call a20_w
    mov al, 0xDF
    out 0x60, al
    call a20_w
    sti
    mov si, msg_done
    call ser_str

    ; Read kernel to buffer
    mov si, msg_loading
    call ser_str
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

    mov si, msg_done
    call ser_str

    ; ── VBE probe ───────────────────────────────────────────────
    mov si, msg_vbe
    call ser_str
    call vbe_probe
    ; VBE may have clobbered segment registers — restore them
    xor ax, ax
    mov ds, ax
    mov es, ax
    cmp word [0x7000], 1
    jne .vbe_no
    mov si, msg_vbe_ok
    call ser_str
    jmp .vbe_cont
.vbe_no:
    mov si, msg_vbe_fail
    call ser_str
.vbe_cont:

    ; ── Switch to protected mode ─────────────────────────────────
    mov si, msg_pmode
    call ser_str
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

    ; Quick serial test: write '>'
    mov dx, 0x3FD
.t:
    in al, dx
    and al, 0x20
    jz .t
    mov dx, 0x3F8
    mov al, '>'
    out dx, al

    ; Copy kernel to 0x100000
    cld
    mov esi, KERNEL_BUF
    mov edi, 0x100000
    mov ecx, KERNEL_SIZE
    rep movsb

    ; Enable PAE
    mov eax, cr4
    or eax, 0x20
    mov cr4, eax

    ; Set PML4 base
    mov eax, pt_template
    mov cr3, eax

    ; Enable long mode
    mov ecx, 0xC0000080
    rdmsr
    or eax, 0x100
    wrmsr

    ; Enable paging
    mov eax, cr0
    or eax, 0x80000000
    mov cr0, eax

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

    ; Found QEMU VGA! Read BAR0 from offset 0x10
    push cx
    xor eax, eax
    mov ax, cx
    shl eax, 11
    or eax, 0x80000010  ; enable bit + offset 0x10
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

msg_stage2: db "  [+] Stage 2 loader entered", 13, 10, 0
msg_a20: db "  [+] Enabling A20 line...", 13, 10, 0
msg_loading: db "  [+] Loading kernel from LBA 65...", 13, 10, 0
msg_done: db "      done", 13, 10, 0
msg_pmode: db "  [+] Entering protected mode...", 13, 10, 0
msg_vbe: db "  [+] Probing VBE...", 13, 10, 0
msg_vbe_ok: db "      VBE enabled", 13, 10, 0
msg_vbe_fail: db "      VBE unavailable", 13, 10, 0
msg_fail: db "  [!] Disk read failed", 13, 10, 0

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
       dq (0xC0000000 + i * 0x200000) | 0x83
%assign i i + 1
%endrep
; All 512 PDEs filled — covers 3GB to 4GB
pt_end:
