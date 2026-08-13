all: boot_asm loader_asm kernel_bin
    dd if=/dev/zero of=planckos.img bs=512 count=102400 2>/dev/null
    dd if=boot_asm of=planckos.img conv=notrunc 2>/dev/null
    dd if=loader_asm of=planckos.img bs=512 seek=1 conv=notrunc 2>/dev/null
    dd if=kernel_bin of=planckos.img bs=512 seek=65 conv=notrunc 2>/dev/null

boot_asm:
    nasm -O0 -f bin boot.asm -o boot_asm

loader_asm:
    nasm -O0 -Wno-error=label-redef-late -f bin loader.asm -o loader_asm

kernel_bin:
    RUSTFLAGS=`python3 generate_cfg.py` \
    RUSTC=/home/faris/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc \
    cargo build --target x86_64-unknown-none --release
    objcopy -O binary target/x86_64-unknown-none/release/planckos kernel_bin

# Create FAT32 filesystem image (64MB)
fs-img:
    dd if=/dev/zero of=planckos_fs.img bs=512 count=131072 2>/dev/null
    mkfs.fat -F 32 planckos_fs.img
    mmd -i planckos_fs.img ::qasm ::results
    mcopy -i planckos_fs.img kernel_bin ::KERNEL.BIN
    echo "planckOS quantum algorithms" | mcopy -i planckos_fs.img - ::qasm/README.TXT

run-sdl: all fs-img
    qemu-system-x86_64 \
      -drive format=raw,file=planckos.img \
      -drive format=raw,file=planckos_fs.img,index=2 \
      -no-reboot -m 128M

run-serial: all fs-img
    qemu-system-x86_64 \
      -drive format=raw,file=planckos.img \
      -drive format=raw,file=planckos_fs.img,index=2 \
      -no-reboot -m 128M -serial stdio

clean:
    rm -f boot_asm loader_asm kernel_bin planckos.img planckos_fs.img
    cargo clean
