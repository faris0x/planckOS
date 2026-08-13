[org 0x7C00]
[bits 16]

start:
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00
    mov [drive], dl

    ; Initialise serial port (115200 baud, 8N1)
    call ser_init

    ; Print boot banner
    mov si, msg_banner
    call ser_str

    ; Load 32 sectors from CHS(0,0,2) to 0:0x1000
    mov si, msg_loading
    call ser_str
    mov ah, 0x02
    mov al, 32
    mov ch, 0
    mov cl, 2
    mov dh, 0
    mov dl, [drive]
    mov bx, 0x1000
    int 0x13
    jc load_fail

    mov si, msg_done
    call ser_str

    ; Jump to stage 2
    mov si, msg_stage2
    call ser_str
    mov dl, [drive]
    jmp 0x0000:0x1000

load_fail:
    mov si, msg_fail
    call ser_str
    jmp halt

halt:
    hlt
    jmp halt

; ── Serial routines ──────────────────────────────────────────────

ser_init:
    push ax
    push dx
    ; Set baud rate divisor (115200)
    mov dx, 0x3FB
    mov al, 0x80
    out dx, al
    mov dx, 0x3F8
    mov al, 1
    out dx, al
    mov dx, 0x3F9
    mov al, 0
    out dx, al
    ; 8N1
    mov dx, 0x3FB
    mov al, 3
    out dx, al
    ; FIFO + RTS/DSR
    mov dx, 0x3FC
    mov al, 0x0B
    out dx, al
    pop dx
    pop ax
    ret

ser_str:
    push ax
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
    pop ax
    ret

; ── Messages ─────────────────────────────────────────────────────

msg_banner: db "planckOS v0.1 — x86-64 bootloader", 13, 10, 0
msg_loading: db "  [+] Loading stage 2 from disk...", 13, 10, 0
msg_done: db "      done", 13, 10, 0
msg_stage2: db "  [+] Handing off to stage 2...", 13, 10, 0
msg_fail: db "  [!] Disk read failed", 13, 10, 0

drive: db 0

times 510-($-$$) db 0
dw 0xAA55
