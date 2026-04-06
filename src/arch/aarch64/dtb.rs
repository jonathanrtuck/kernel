//! Minimal Flattened Device Tree (FDT) scanner for early boot.
//!
//! Extracts only what the kernel needs to initialize: RAM region and core
//! count. No allocator required — all state is on the stack.
//!
//! Assumes `#address-cells = 2, #size-cells = 2` at the root level
//! (QEMU virt / Apple Hypervisor standard), meaning each `reg` entry in
//! top-level nodes is 16 bytes (base: u64, size: u64).

const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_END: u32 = 9;
const FDT_MAGIC: u32 = 0xD00D_FEED;
const FDT_NOP: u32 = 4;
const FDT_PROP: u32 = 3;
const HEADER_SIZE: usize = 40;

/// Hardware information discovered from the device tree.
pub struct BootInfo {
    /// Physical RAM base address.
    pub ram_base: usize,
    /// Physical RAM size in bytes.
    pub ram_size: usize,
    /// Number of CPU cores.
    pub core_count: usize,
}

/// Scan the FDT blob at `dtb_ptr` and extract boot-critical hardware info.
///
/// Returns `None` if the pointer is null or the DTB is invalid.
pub fn scan(dtb_ptr: usize) -> Option<BootInfo> {
    if dtb_ptr == 0 {
        return None;
    }

    // SAFETY: The DTB is placed at a known address by the hypervisor/firmware.
    // We're in single-threaded physical-mode boot. Read just the header first
    // to discover totalsize, then create the full slice.
    let header = unsafe { core::slice::from_raw_parts(dtb_ptr as *const u8, HEADER_SIZE) };

    let magic = read_be_u32(header, 0);
    if magic != FDT_MAGIC {
        return None;
    }

    let totalsize = read_be_u32(header, 4) as usize;
    if totalsize < HEADER_SIZE {
        return None;
    }

    // SAFETY: totalsize comes from the validated FDT header. The entire blob
    // is within the RAM region placed by the hypervisor.
    let blob = unsafe { core::slice::from_raw_parts(dtb_ptr as *const u8, totalsize) };

    scan_blob(blob)
}

/// Parse an FDT blob from a byte slice.
///
/// This is the pure-computation core, separated from the raw pointer
/// access in [`scan`] so it can be tested on the host.
pub fn scan_blob(blob: &[u8]) -> Option<BootInfo> {
    if blob.len() < HEADER_SIZE {
        return None;
    }

    let magic = read_be_u32(blob, 0);
    if magic != FDT_MAGIC {
        return None;
    }

    let totalsize = read_be_u32(blob, 4) as usize;
    if totalsize < HEADER_SIZE || totalsize > blob.len() {
        return None;
    }

    let off_struct = read_be_u32(blob, 8) as usize;
    let off_strings = read_be_u32(blob, 12) as usize;

    if off_struct >= totalsize || off_strings >= totalsize {
        return None;
    }

    let structs = blob.get(off_struct..totalsize)?;
    let strings = blob.get(off_strings..totalsize)?;

    let mut info = BootInfo {
        ram_base: 0,
        ram_size: 0,
        core_count: 0,
    };

    // Per-node state (reset at each depth-2 BEGIN_NODE, committed at END_NODE).
    let mut is_memory = false;
    let mut reg_base: u64 = 0;
    let mut reg_size: u64 = 0;
    let mut has_reg = false;

    // /cpus tracking.
    let mut in_cpus = false;
    let mut cpus_depth: usize = 0;

    let mut depth: usize = 0;
    let mut offset: usize = 0;

    loop {
        if offset + 4 > structs.len() {
            break;
        }

        let token = read_be_u32(structs, offset);
        offset += 4;

        match token {
            FDT_BEGIN_NODE => {
                // Read the node name (null-terminated, padded to 4 bytes).
                let name_start = offset;
                while offset < structs.len() && structs[offset] != 0 {
                    offset += 1;
                }
                let name = &structs[name_start..offset];
                if offset < structs.len() {
                    offset += 1;
                }
                offset = align4(offset);

                depth += 1;

                if depth == 2 {
                    // Top-level node — reset accumulator.
                    is_memory = false;
                    has_reg = false;
                    in_cpus = name == b"cpus";
                    if in_cpus {
                        cpus_depth = depth;
                    }
                } else if in_cpus && depth == cpus_depth + 1 && starts_with(name, b"cpu@") {
                    info.core_count += 1;
                }
            }
            FDT_END_NODE => {
                if depth == 2 {
                    if is_memory && has_reg {
                        info.ram_base = reg_base as usize;
                        info.ram_size = reg_size as usize;
                    }
                    if depth == cpus_depth {
                        in_cpus = false;
                    }
                }
                depth = depth.saturating_sub(1);
            }
            FDT_PROP => {
                if offset + 8 > structs.len() {
                    return None;
                }
                let len = read_be_u32(structs, offset) as usize;
                offset += 4;
                let nameoff = read_be_u32(structs, offset) as usize;
                offset += 4;

                if offset + len > structs.len() {
                    return None;
                }

                // Only interpret properties of top-level (depth-2) nodes.
                if depth == 2 {
                    let data = &structs[offset..offset + len];
                    let name = read_cstr(strings, nameoff);

                    if name == "device_type" && data_eq_str(data, "memory") {
                        is_memory = true;
                    } else if name == "reg" && data.len() >= 16 {
                        // #address-cells=2, #size-cells=2: first entry is 16 bytes.
                        reg_base = read_be_u64(data, 0);
                        reg_size = read_be_u64(data, 8);
                        has_reg = true;
                    }
                }

                offset = align4(offset + len);
            }
            FDT_NOP => {}
            FDT_END => break,
            _ => break,
        }
    }

    if info.ram_size == 0 {
        return None;
    }

    Some(info)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn align4(v: usize) -> usize {
    (v + 3) & !3
}

fn read_be_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_be_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}

/// Read a null-terminated string from the strings block.
fn read_cstr(data: &[u8], offset: usize) -> &str {
    if offset >= data.len() {
        return "";
    }
    let mut end = offset;
    while end < data.len() && data[end] != 0 {
        end += 1;
    }
    core::str::from_utf8(&data[offset..end]).unwrap_or("")
}

/// Check if a property value is a null-terminated string equal to `expected`.
fn data_eq_str(data: &[u8], expected: &str) -> bool {
    let bytes = expected.as_bytes();
    // Property data is the string + null terminator.
    data.len() >= bytes.len() && &data[..bytes.len()] == bytes
}

fn starts_with(haystack: &[u8], prefix: &[u8]) -> bool {
    haystack.len() >= prefix.len() && &haystack[..prefix.len()] == prefix
}
