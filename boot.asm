; Copyright (c) 2026 Faris Alfarhan
; SPDX-License-Identifier: GPL-3.0-only

[org 0x7C00]
[bits 16]

%macro LOG 1
    mov si, %1
    call log_line
%endmacro

start:
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00
    mov [drive], dl

    ; Snapshot entry TSC for the loader (absolute 0x608 gets it via 0:0)
    rdtsc
    mov [0x608], eax

    ; Initialise serial port (115200 baud, 8N1)
    mov dx, 0x3FB
    mov al, 0x80
    out dx, al
    mov dx, 0x3F8
    mov al, 1
    out dx, al
    mov dx, 0x3F9
    mov al, 0
    out dx, al
    mov dx, 0x3FB
    mov al, 3
    out dx, al

    ; Calibrate TSC against the PIT (10 ms window) — restores BIOS PIT state
    call calibrate
    LOG msg_timer
    LOG msg_banner

    ; Load 40 sectors (full loader, incl. page tables) from CHS(0,0,2)
    LOG msg_loading
    mov ah, 0x02
    mov al, 40
    mov ch, 0
    mov cl, 2
    mov dh, 0
    mov dl, [drive]
    mov bx, 0x1000
    int 0x13
    jc load_fail

    LOG msg_stage2
    mov dl, [drive]
    jmp 0x0000:0x1000

load_fail:
    mov si, msg_fail
    call ser_str
    jmp $

; ── Serial routines ──────────────────────────────────────────────

ser_str:
    push ax
    push dx
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
    pop dx
    pop ax
    ret

; ── TSC timing ───────────────────────────────────────────────────

; Calibrate TSC frequency using PIT channel 0, mode 0, count 11932
; (~10 ms). Reuses `last_tsc` as its window baseline — the next log
; line therefore reports the calibration time. Restores the BIOS PIT
; default (mode 3, 54.9 ms tick) and EOIs the PIC afterwards.
calibrate:
    mov al, 0x34
    out 0x43, al
    mov al, 0x2C
    out 0x40, al
    mov al, 0x2E
    out 0x40, al
    rdtsc
    mov [last_tsc], eax
.await:
    mov al, 0x00
    out 0x43, al
    in al, 0x40
    mov ah, al
    in al, 0x40
    xchg al, ah
    cmp ax, 0x1000
    ja .await
    rdtsc
    sub eax, [last_tsc]
    xor edx, edx
    mov ecx, 10
    div ecx
    test eax, eax
    jnz .ok
    mov eax, 1000000
.ok:
    mov [tsc_per_ms], eax
    mov al, 0x36
    out 0x43, al
    xor al, al
    out 0x40, al
    out 0x40, al
    mov al, 0x20
    out 0x20, al
    ret

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
    mov si, dec_buf + 7
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

; ── Messages ─────────────────────────────────────────────────────

msg_banner:  db "[BOOT  ] [BANNER] planckOS", 0
msg_timer:   db "[BOOT  ] [TIMER ] TSC ready", 0
msg_loading: db "[BOOT  ] [DISK  ] reading stage 2 (40)", 0
msg_stage2:  db "[BOOT  ] [HANDOF] stage 2 loaded", 0
msg_fail:    db "[BOOT  ] [DISK  ] FAIL", 13, 10, 0
msg_open:    db " (", 0
msg_dot:     db ".", 0
msg_ms:    db " ms)", 13, 10, 0

drive:       db 0
tsc_per_ms  equ 0x600
last_tsc:    dd 0
dec_buf:     times 8 db 0

times 510-($-$$) db 0
dw 0xAA55