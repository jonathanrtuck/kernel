# 057 — Space byte-addressing: byte inputs, kernel rounds

Date: 2026-04-24

## Starting point

D25 settled that page size is exposed (not hidden), but left the question of
whether Space operations accept byte-count or page-count inputs. D41 says Space
operations "operate at page granularity," but whether this describes the API
unit or the kernel's internal action quantum was ambiguous. The autonomous plan
noted: "A5, D25 (kernel absorbs → byte with rounding)."

## Exploration

### A5 assigns the alignment computation to the kernel

If the interface requires page-aligned inputs, every userspace caller must:
query PAGE_SIZE, round up, pass the rounded value. This is a repeated mechanical
computation at every call site — not essential complexity (it contains no
workload-specific policy). A5 assigns exactly this class of computation to the
kernel.

Under byte-addressed inputs, the kernel absorbs the rounding. Userspace passes
the byte count it actually needs.

### D25 requires observability, not mandatory per-call alignment

D25 settled: "Observers can query the page size and must account for page
granularity in memory operations." This means the granularity must be observable
and predictable — not that every call site must perform alignment arithmetic.
Under byte inputs: PAGE_SIZE is queryable, the returned Space's actual size is
PAGE_SIZE-aligned, and the difference is observable.

### D25's risk scenario does not apply at the split interface

D25 flagged a risk: implicit rounding could re-hide page size in practice. The
dangerous scenario: two separately-held 4KB Spaces on 16K-page hardware mapped
adjacently — sub-page packing with cross-cap security violations.

Under D26 (capability-addressed memory), there is no explicit map() call. The
kernel assigns VA bases independently. The scenario that motivated D25's caution
does not arise at the split interface.

### D41's "page granularity" describes internals, not API

D41: Space operations "operate at page granularity." This describes the kernel's
action quantum, not the caller's request unit. Direct parallel: Zircon's
`zx_vmo_create` "operates at page granularity" internally while accepting byte
sizes from userspace.

### D9's rejection of seL4's model applies here

D9 rejected seL4's untyped/page-addressed model on A5 grounds. The interface
that follows from seL4's design does not follow from this kernel's. The closer
comparators — Zircon (VMOs), Genode (dataspaces), Mach (vm_allocate) — all use
byte-addressed inputs with rounding. These systems share this kernel's D9
approach.

## What this settles

`space_split(cap, size) → new_cap`: `size` is a byte count. The kernel computes
`actual_size = round_up(size, PAGE_SIZE)`. The returned Space has `actual_size`
bytes. The source shrinks by `actual_size + subtree_overhead`.

- `size = 0` is an error.
- `size` exceeding source capacity is an error.
- The Observer queries PAGE_SIZE to predict exact allocation behavior.

## What remains open

1. **Rounded-size communication.** Whether the kernel returns the actual
   (rounded) size in a second register or the Observer queries separately.
2. **Subtree overhead visibility.** Whether the Observer can see the overhead
   charge or only the net Space size.
3. **Merge interaction.** `space_merge` takes no size — unaffected.

## Status

**Settled.** Byte-addressed inputs, kernel rounds internally. The Zircon/Genode/
Mach model, forced by A5 + D25 + D26 + D9.
