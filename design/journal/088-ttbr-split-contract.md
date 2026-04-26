# D88 — TTBR0/TTBR1 split contract

**Question:** How is the ARM64 virtual address space partitioned between kernel
and userspace?

**Rests on:** A2 (ARM64 TTBR0/TTBR1 hardware mechanism), D1 (hot-path — context
switch cost), D5 (MMU-backed virtual memory), D26 (capability-addressed memory —
kernel assigns VA bases in user range; shared L3 subtrees), D43 (Observer
page_table_root = TTBR0 value), D74 (register save on EL0 entry), ARM64
VMSAv8-64 (D5.2 Translation table walks).

**Status:** settled.

---

## Settles

### Split: TTBR1 kernel, TTBR0 user

ARM64 splits the 64-bit VA space at the hardware level:

- TTBR0_EL1 translates the lower range (bits\[63:N\] all zero, N = 64 − T0SZ)
- TTBR1_EL1 translates the upper range (bits\[63:N\] all ones)
- Addresses between the two ranges generate Translation Faults

The kernel occupies TTBR1 (upper half), per-Observer user mappings occupy TTBR0
(lower half). This is the universal ARM64 pattern (Linux, seL4, Zircon,
FreeBSD).

Three convergent reasons force this assignment:

1. **D1 hot path:** context switch writes only TTBR0. TTBR1 is fixed post-boot.
2. **D26 capability-addressed memory:** the kernel assigns VA bases from the
   user range. All Space mappings live in TTBR0.
3. **Security:** E0PD1 prevents EL0 speculative access to TTBR1 (kernel). The
   hardware separation makes Meltdown mitigation structural.

### Asymmetric VA sizes

ARM64 TCR_EL1 has independent T0SZ and T1SZ fields. The two halves use different
VA sizes optimized for their roles:

|             | User (TTBR0)                                    | Kernel (TTBR1)                                  |
| ----------- | ----------------------------------------------- | ----------------------------------------------- |
| TxSZ        | T0SZ=17                                         | T1SZ=28                                         |
| VA size     | 47-bit (128 TiB)                                | 36-bit (64 GiB)                                 |
| Walk levels | 3 (L1→L2→L3)                                    | 2 (L2→L3)                                       |
| Range       | `0x0000_0000_0000_0000`–`0x0000_7FFF_FFFF_FFFF` | `0xFFFF_FFF0_0000_0000`–`0xFFFF_FFFF_FFFF_FFFF` |

User gets 3 levels to enable D26's shared L3 subtree model — each Space aligns
to an L2 entry boundary (32 MiB), giving ~4 million Space slots. Kernel stays at
2 levels for minimum TLB miss cost on syscall dispatch.

### TTBR1 linear map

The kernel TTBR1 table provides a linear map:

```text
VA = PA + KERNEL_VIRT_OFFSET
PA = VA − KERNEL_VIRT_OFFSET
```

Where `KERNEL_VIRT_OFFSET = 0xFFFF_FFF0_0000_0000` (the TTBR1 base address).

Physical address space maps 1:1 into the upper half. `phys_to_virt` and
`virt_to_phys` are single-instruction operations.

### TCR configuration

Boot TCR: `T0SZ=28, T1SZ=28, EPD1=1` (identity map, TTBR1 disabled).

Runtime TCR: `T0SZ=17, T1SZ=28, EPD1=0, E0PD1=1`.

E0PD1 (FEAT_E0PD, ARMv8.5+, bit 56): when set, EL0 speculative accesses that
would translate through TTBR1 return a fixed value. This prevents Meltdown-style
leakage without full KPTI. All Apple Silicon supports FEAT_E0PD.

### ASID via TTBR0

- `TCR_EL1.A1=0`: ASID sourced from TTBR0 (per-Observer)
- `TCR_EL1.AS`: 16-bit if hardware supports (`ID_AA64MMFR0_EL1.ASIDBits[7:4]` ≥
  2), else 8-bit

TTBR0 format (16 KiB granule):

```text
[63:48]  ASID (when A1=0)
[47:14]  L1 root table physical address
[13:1]   RES0 (16 KiB alignment)
[0]      CnP = 0 (per-Observer)
```

TTBR1: `CnP=1` (all cores share the same kernel tables).

### Context switch

TTBR0 write only. `__restore_observer` takes `page_table_root: u64` — a full
TTBR0 value (ASID + L1 root PA). TTBR1 is written once during boot.

### Boot transition

1. Boot with `T0SZ=28, EPD1=1` (current identity map in TTBR0)
2. Build kernel map for TTBR1 (L2 table at upper-half VAs)
3. Write TTBR1, switch TCR to runtime values (`T0SZ=17, EPD1=0, E0PD1=1`)
4. TLB invalidation, branch to upper-half address
5. TTBR0 now available for per-Observer L1 tables

---

## Rejected alternatives

**TTBR0-only** (kernel + user in same table): every per-Observer page table must
include kernel mappings. More complex, weaker security separation. No surveyed
ARM64 kernel uses this.

**Symmetric T0SZ=T1SZ=28** (2-level user): 64 GiB user VA with 2-level walk.
Cheaper TLB miss, but only 2048 Space slots at 32 MiB alignment. More
critically, D26's shared L3 subtrees don't work with 2-level tables — an L3
table covers a 32 MiB region that may contain multiple Spaces, preventing
sharing between Observers with different Space sets. 3-level user tables resolve
this by providing an additional level of indirection (L2) between the per-
Observer root and the per-Space L3 tables.

**Full KPTI**: unnecessary with E0PD. KPTI is the fallback on pre-ARMv8.5
hardware (porting path in `speculation.rs`).

---

## Does NOT settle

- ASID allocation policy (counter, recycling, bitmap on rollover)
- Per-Observer page table structure details (D89)
- PTE population policy — demand fault vs. eager (D90)
- Cap-to-mapping protocol (D91)
- Page table memory accounting (D92)
