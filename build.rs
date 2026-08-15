// Copyright (c) 2026 Faris Alfarhan
// SPDX-License-Identifier: GPL-3.0-only

fn main() {
    println!("cargo:rerun-if-changed=src/hal/trampoline.asm");
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let out = out_dir.join("trampoline.bin");
    let status = std::process::Command::new("nasm")
        .args(["-f", "bin", "-o"])
        .arg(&out)
        .arg("src/hal/trampoline.asm")
        .status()
        .expect("failed to run nasm for the AP trampoline");
    assert!(status.success(), "nasm failed to assemble trampoline.asm");
    println!("cargo:rerun-if-changed=src/hal/trampoline.asm");
}
