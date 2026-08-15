// Copyright (c) 2026 Faris Alfarhan
// SPDX-License-Identifier: GPL-3.0-only

/// CPUID driver — queries processor vendor, model, and features.

use core::arch::asm;

#[derive(Debug, Clone)]
pub struct CpuInfo {
    pub vendor: [u8; 12],
    pub brand: [u8; 48],
    pub max_leaf: u32,
    pub stepping: u8,
    pub model: u8,
    pub family: u8,
    pub ext_model: u8,
    pub ext_family: u8,
    pub features_edx: u32,
    pub features_ecx: u32,
    pub ext_features_ebx: u32,
}

fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let mut ebx: u32 = 0;
    let ecx: u32;
    let edx: u32;
    unsafe {
        asm!(
            "xchg rbx, {tmp}",
            "cpuid",
            "xchg rbx, {tmp}",
            tmp = inout(reg) ebx => _,
            inout("eax") leaf => eax,
            out("ecx") ecx,
            out("edx") edx,
        );
    }
    (eax, ebx, ecx, edx)
}

pub fn query() -> CpuInfo {
    let mut info = CpuInfo {
        vendor: [0; 12],
        brand: [0; 48],
        max_leaf: 0,
        stepping: 0,
        model: 0,
        family: 0,
        ext_model: 0,
        ext_family: 0,
        features_edx: 0,
        features_ecx: 0,
        ext_features_ebx: 0,
    };

    // Leaf 0: vendor string + max leaf
    let (max_leaf, ebx, ecx, edx) = cpuid(0);
    info.max_leaf = max_leaf;

    // Write vendor string from ebx:edx:ecx
    let vptr = &mut info.vendor as *mut [u8; 12] as *mut u8;
    unsafe {
        core::ptr::write_unaligned(vptr as *mut u32, ebx);
        core::ptr::write_unaligned(vptr.add(4) as *mut u32, edx);
        core::ptr::write_unaligned(vptr.add(8) as *mut u32, ecx);
    }

    // Leaf 1: processor info and features
    if info.max_leaf >= 1 {
        let (eax1, _, ecx1, edx1) = cpuid(1);
        info.stepping = (eax1 & 0xF) as u8;
        info.model = ((eax1 >> 4) & 0xF) as u8;
        info.family = ((eax1 >> 8) & 0xF) as u8;
        info.ext_model = ((eax1 >> 16) & 0xF) as u8;
        info.ext_family = ((eax1 >> 20) & 0xFF) as u8;
        info.features_edx = edx1;
        info.features_ecx = ecx1;
    }

    // Leaf 0x80000000: extended max leaf
    let (eax_ext, _, _, _) = cpuid(0x80000000);

    // Leaf 0x80000001: extended features
    if eax_ext >= 0x80000001 {
        let (_, ebx_ext, _, _) = cpuid(0x80000001);
        info.ext_features_ebx = ebx_ext;
    }

    // Leaf 0x80000002–0x80000004: brand string (48 bytes)
    if eax_ext >= 0x80000004 {
            let bptr = &mut info.brand as *mut [u8; 48] as *mut u8;
            for i in 0..3 {
                let (eax_b, ebx_b, ecx_b, edx_b) = cpuid(0x80000002 + i);
                let offset: usize = i as usize * 16;
                unsafe {
                    core::ptr::write_unaligned(bptr.add(offset) as *mut u32, eax_b);
                    core::ptr::write_unaligned(bptr.add(offset + 4) as *mut u32, ebx_b);
                    core::ptr::write_unaligned(bptr.add(offset + 8) as *mut u32, ecx_b);
                    core::ptr::write_unaligned(bptr.add(offset + 12) as *mut u32, edx_b);
                }
        }
    }

    info
}

pub fn has_feature_edx(info: &CpuInfo, bit: u8) -> bool {
    (info.features_edx >> bit) & 1 != 0
}

pub fn has_feature_ecx(info: &CpuInfo, bit: u8) -> bool {
    (info.features_ecx >> bit) & 1 != 0
}

pub fn vendor_str(info: &CpuInfo) -> &str {
    let len = info.vendor.iter().position(|&c| c == 0).unwrap_or(12);
    core::str::from_utf8(&info.vendor[..len]).unwrap_or("unknown")
}

pub fn brand_str(info: &CpuInfo) -> &str {
    let len = info.brand.iter().position(|&c| c == 0).unwrap_or(48);
    if len == 0 {
        return "unknown";
    }
    let trimmed = &info.brand[..len];
    let start = trimmed.iter().position(|&c| c != b' ').unwrap_or(0);
    let end = trimmed.iter().rposition(|&c| c != b' ').unwrap_or(len).saturating_add(1);
    core::str::from_utf8(&trimmed[start..end]).unwrap_or("unknown")
}
