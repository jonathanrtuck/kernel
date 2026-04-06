//! DTB (Flattened Device Tree) scanner tests.
//!
//! Constructs synthetic FDT blobs and verifies the scanner extracts the
//! correct values. The scanner logic is duplicated from
//! `src/arch/aarch64/dtb.rs` — keep in sync.

// ---------------------------------------------------------------------------
// Scanner (duplicated from kernel for host-side testing)
// ---------------------------------------------------------------------------

const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_END: u32 = 9;
const FDT_MAGIC: u32 = 0xD00D_FEED;
const FDT_NOP: u32 = 4;
const FDT_PROP: u32 = 3;
const HEADER_SIZE: usize = 40;

struct BootInfo {
    ram_base: usize,
    ram_size: usize,
    core_count: usize,
}

fn scan_blob(blob: &[u8]) -> Option<BootInfo> {
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

    let mut is_memory = false;
    let mut reg_base: u64 = 0;
    let mut reg_size: u64 = 0;
    let mut has_reg = false;
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

                if depth == 2 {
                    let data = &structs[offset..offset + len];
                    let name = read_cstr(strings, nameoff);
                    if name == "device_type" && data_eq_str(data, "memory") {
                        is_memory = true;
                    } else if name == "reg" && data.len() >= 16 {
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
fn data_eq_str(data: &[u8], expected: &str) -> bool {
    let bytes = expected.as_bytes();
    data.len() >= bytes.len() && &data[..bytes.len()] == bytes
}
fn starts_with(haystack: &[u8], prefix: &[u8]) -> bool {
    haystack.len() >= prefix.len() && &haystack[..prefix.len()] == prefix
}

// ---------------------------------------------------------------------------
// FDT builder (test helper)
// ---------------------------------------------------------------------------

struct FdtBuilder {
    structs: Vec<u8>,
    strings: Vec<u8>,
    string_offsets: std::collections::HashMap<String, u32>,
}

impl FdtBuilder {
    fn new() -> Self {
        Self {
            structs: Vec::new(),
            strings: Vec::new(),
            string_offsets: std::collections::HashMap::new(),
        }
    }

    fn begin_node(&mut self, name: &str) {
        self.push_u32(FDT_BEGIN_NODE);
        self.structs.extend_from_slice(name.as_bytes());
        self.structs.push(0);
        self.align4();
    }

    fn end_node(&mut self) {
        self.push_u32(FDT_END_NODE);
    }

    fn prop_u32(&mut self, name: &str, val: u32) {
        let nameoff = self.add_string(name);
        self.push_u32(FDT_PROP);
        self.push_u32(4);
        self.push_u32(nameoff);
        self.push_u32(val);
    }

    fn prop_string(&mut self, name: &str, val: &str) {
        let nameoff = self.add_string(name);
        let bytes: Vec<u8> = val.bytes().chain(std::iter::once(0)).collect();
        self.push_u32(FDT_PROP);
        self.push_u32(bytes.len() as u32);
        self.push_u32(nameoff);
        self.structs.extend_from_slice(&bytes);
        self.align4();
    }

    fn prop_reg(&mut self, addr: u64, size: u64) {
        let nameoff = self.add_string("reg");
        self.push_u32(FDT_PROP);
        self.push_u32(16);
        self.push_u32(nameoff);
        self.push_u64(addr);
        self.push_u64(size);
    }

    fn finish(mut self) -> Vec<u8> {
        self.push_u32(FDT_END);

        let header_size: u32 = 40;
        let rsvmap_size: u32 = 16; // one empty entry
        let struct_off = header_size + rsvmap_size;
        let strings_off = struct_off + self.structs.len() as u32;
        let totalsize = strings_off + self.strings.len() as u32;

        let mut blob = Vec::new();
        // Header
        push_be_u32(&mut blob, FDT_MAGIC);
        push_be_u32(&mut blob, totalsize);
        push_be_u32(&mut blob, struct_off);
        push_be_u32(&mut blob, strings_off);
        push_be_u32(&mut blob, header_size); // off_mem_rsvmap
        push_be_u32(&mut blob, 17); // version
        push_be_u32(&mut blob, 16); // last_comp_version
        push_be_u32(&mut blob, 0); // boot_cpuid_phys
        push_be_u32(&mut blob, self.strings.len() as u32);
        push_be_u32(&mut blob, self.structs.len() as u32);
        // Memory reservation map (empty)
        blob.extend_from_slice(&[0u8; 16]);
        // Structure + strings
        blob.extend_from_slice(&self.structs);
        blob.extend_from_slice(&self.strings);

        blob
    }

    fn push_u32(&mut self, val: u32) {
        self.structs.extend_from_slice(&val.to_be_bytes());
    }

    fn push_u64(&mut self, val: u64) {
        self.structs.extend_from_slice(&val.to_be_bytes());
    }

    fn align4(&mut self) {
        while self.structs.len() % 4 != 0 {
            self.structs.push(0);
        }
    }

    fn add_string(&mut self, name: &str) -> u32 {
        if let Some(&off) = self.string_offsets.get(name) {
            return off;
        }
        let off = self.strings.len() as u32;
        self.string_offsets.insert(name.to_string(), off);
        self.strings.extend_from_slice(name.as_bytes());
        self.strings.push(0);
        off
    }
}

fn push_be_u32(v: &mut Vec<u8>, val: u32) {
    v.extend_from_slice(&val.to_be_bytes());
}

/// Build a DTB matching the QEMU virt / Apple Hypervisor layout.
fn build_test_dtb(ram_base: u64, ram_size: u64, cpu_count: usize) -> Vec<u8> {
    let mut b = FdtBuilder::new();

    b.begin_node(""); // root
    b.prop_u32("#address-cells", 2);
    b.prop_u32("#size-cells", 2);

    // /memory
    b.begin_node(&format!("memory@{ram_base:x}"));
    b.prop_string("device_type", "memory");
    b.prop_reg(ram_base, ram_size);
    b.end_node();

    // /cpus
    b.begin_node("cpus");
    b.prop_u32("#address-cells", 1);
    b.prop_u32("#size-cells", 0);
    for i in 0..cpu_count {
        b.begin_node(&format!("cpu@{i}"));
        b.prop_string("device_type", "cpu");
        b.prop_u32("reg", i as u32);
        b.end_node();
    }
    b.end_node();

    b.end_node(); // root

    b.finish()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn parse_qemu_virt_layout() {
    let blob = build_test_dtb(0x4000_0000, 256 * 1024 * 1024, 4);
    let info = scan_blob(&blob).expect("scan should succeed");
    assert_eq!(info.ram_base, 0x4000_0000);
    assert_eq!(info.ram_size, 256 * 1024 * 1024);
    assert_eq!(info.core_count, 4);
}

#[test]
fn parse_different_ram_size() {
    let blob = build_test_dtb(0x4000_0000, 512 * 1024 * 1024, 8);
    let info = scan_blob(&blob).expect("scan should succeed");
    assert_eq!(info.ram_size, 512 * 1024 * 1024);
    assert_eq!(info.core_count, 8);
}

#[test]
fn parse_single_core() {
    let blob = build_test_dtb(0x4000_0000, 128 * 1024 * 1024, 1);
    let info = scan_blob(&blob).expect("scan should succeed");
    assert_eq!(info.core_count, 1);
}

#[test]
fn reject_bad_magic() {
    let mut blob = build_test_dtb(0x4000_0000, 256 * 1024 * 1024, 4);
    blob[0] = 0; // corrupt magic
    assert!(scan_blob(&blob).is_none());
}

#[test]
fn reject_truncated_header() {
    let blob = vec![0xD0, 0x0D, 0xFE, 0xED]; // magic only, no totalsize
    assert!(scan_blob(&blob).is_none());
}

#[test]
fn reject_empty_blob() {
    assert!(scan_blob(&[]).is_none());
}

#[test]
fn reject_no_memory_node() {
    // DTB with only /cpus, no /memory → ram_size stays 0 → returns None.
    let mut b = FdtBuilder::new();
    b.begin_node("");
    b.prop_u32("#address-cells", 2);
    b.prop_u32("#size-cells", 2);
    b.begin_node("cpus");
    b.begin_node("cpu@0");
    b.end_node();
    b.end_node();
    b.end_node();
    let blob = b.finish();
    assert!(scan_blob(&blob).is_none());
}

#[test]
fn nop_tokens_are_skipped() {
    // Insert NOP tokens into the structure — scanner should skip them.
    let mut b = FdtBuilder::new();
    b.begin_node("");
    b.prop_u32("#address-cells", 2);
    b.push_u32(FDT_NOP); // NOP in the structure
    b.prop_u32("#size-cells", 2);
    b.begin_node("memory@40000000");
    b.prop_string("device_type", "memory");
    b.prop_reg(0x4000_0000, 0x1000_0000);
    b.end_node();
    b.end_node();
    let blob = b.finish();
    let info = scan_blob(&blob).expect("scan should succeed despite NOPs");
    assert_eq!(info.ram_base, 0x4000_0000);
}
