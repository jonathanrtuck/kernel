# D98 — Destroy cascade and return

**Question:** How does the preemptible destroy cascade execute at runtime, and
how does destruction return resources to the destroyer?

**Rests on:** D8 (cap table — allocate_slot for returned Space cap), D11 (close
— revocation mechanism), D31 (root pool — cascade-freed backing returns here),
D32 (type conversion — destruction returns Space), D33 (preemptible cascade —
bounded latency for RT workloads), A3 (generic — RT workloads require bounded
preemption), A4 (purely reactive — preemption uses timer interrupt).

**Status:** settled.

---

## Settles

### Preemptible cascade (#19): bounded-step cleanup with continuation

D33 settles preemptible cascade as the design choice. This entry settles the
runtime mechanics.

The kernel runs N `cascade_step()` calls per batch, where each step processes up
to CASCADE_STEP_SIZE (16) cap table entries via `Table::close()`. After each
batch, the kernel checks for a pending timer interrupt. If pending, the kernel:

1. Saves continuation state in the CoreState: `(ObjectId, u32 slot_cursor)` for
   the object being destroyed plus the current position in its cap table
   iteration.
2. Handles the interrupt (timer expiry, IRQ delivery).
3. Schedules the next Observer (the destroying Observer is blocked — it cannot
   be scheduled while its destroy is in progress, D39).
4. When the core returns to the cascade (no higher-priority work), resumes from
   the saved cursor.

Other Observers on the same core CAN run between cascade batches. The cascade
does not hold any global lock — each step operates on a single cap table entry,
and the object is already dead (D11: dead handles created at destroy time, no
partially-alive state is externally visible). The only state the cascade holds
is the cursor position and the identity of the object being cleaned up.

**Nested cascades:** closing a cap during cascade may trigger a secondary
destroy if the closed object's refcount reaches zero. The continuation state
forms a stack: each cascade level pushes `(ObjectId, u32 cursor)`. D33
establishes that cascade depth is bounded by exclusively-held Observer chains —
in practice shallow (typically 1-2 levels; deep nesting requires a chain of
Observers each exclusively held by the next).

The continuation state is small. Per cascade level: one ObjectId (u64) + one u32
cursor = 12 bytes. A stack of 4 levels (generous upper bound for typical
systems) is 48 bytes. This fits comfortably in the CoreState without dynamic
allocation.

**Batch size tuning:** CASCADE_STEP_SIZE = 16 is the initial value, chosen to
bound per-step latency at roughly 1-2 us (16 close operations, each ~100 ns
including slot tag bump, freelist threading, and potential badge tracking
check). This keeps worst-case interrupt latency bounded: the timer interrupt
handler runs within 2 us of the timer firing, even during an active cascade. The
value is a tuning parameter — adjustable based on measurement without protocol
changes.

### Destroy return mechanism (#20): structural backing becomes Space cap

D32 establishes destruction as reverse type conversion: the object's structural
backing (the Space that was consumed at creation) is returned as a new Space
cap. D33 narrows this: only the top-level target's structural backing returns to
the destroyer. Cascade-freed objects (destroyed as side effects) return their
backing to the kernel's root Space (D31).

The return sequence:

1. The object is marked dead (D11 generation bump — all outstanding caps become
   stale).
2. The cascade runs (preemptible, batched) until all caps in the object's table
   are closed.
3. The kernel creates a new Space object in the arena backed by the freed
   structural pages: register save area, cap table pages, L1 page table root —
   all revert to being a general-purpose Space.
4. The kernel allocates a slot in the destroyer's cap table via
   `Table::allocate_slot`.
5. The kernel constructs a Space Entry and installs it via `Table::install_at`.
6. The slot index is returned in x0 as the typed operation result.

**Table-full failure:** if the destroyer's cap table has no free slots, the
destroy fails before starting the cascade. The kernel cannot return the Space
cap if there is nowhere to put it. This is checked upfront — before the object
is marked dead, before any caps are closed. The error is CapError::TableFull,
which triggers the cap-table-full fault (D40) to the destroyer's handler,
providing an opportunity to grow the table and retry.

This ordering is critical: the destroy must be atomic with respect to the
return. If the object were destroyed first and the table-full check failed
afterward, the structural backing would be lost (object dead, no cap to return
it through). Checking first, destroying second, ensures conservation.

**Cascade-freed objects:** objects whose refcount reaches zero during the
cascade (because the destroyed Observer held the last reference) are themselves
destroyed. Their structural backing returns to the kernel's root Space (D31),
not to the destroyer. Three reasons (D33):

1. Shared resources break the return model — the last holder is arbitrary, not a
   meaningful owner.
2. Internal reorganization during cascade makes returns unpredictable — the
   destroyer cannot know how many objects will cascade-free or what their sizes
   are.
3. Requiring the destroyer to have enough cap table slots for all transitively
   freed objects is impractical — a single destroy could cascade through dozens
   of objects.

The root pool (D31) is the neutral destination. The freed pages re-enter the
system's resource pool and can be re-acquired through the normal pager chain.

**Time caps in the cascade:** Time caps closed during cascade return compute
capacity to the kernel's per-core pool (D32: Time is asymmetric — no Space
involved). The Observer's `compute_aggregate` is irrelevant because the Observer
is already dead. The per-core scheduler updates when the Time arena slot is
freed.

---

## Rejected alternatives

**Inline (run-to-completion) cascade:** D33 explicitly chose preemptible. Inline
cascading violates bounded latency under A3. An Observer with a large cap table
(e.g., 1024 entries) would block the core for ~100 us during destruction. Under
RT workloads with 10-50 us deadlines, this is unacceptable. seL4 MCS
demonstrates that preemptible revocation is feasible and necessary for
mixed-criticality systems.

**Cascade-freed backing returns to destroyer:** would require the destroyer to
have enough cap table slots for all transitively freed objects — N cascade-freed
objects need N free slots. A single destroy of a supervisor Observer holding
references to 50 child objects would need 50 free slots. The destroyer cannot
predict this count. Failing mid-cascade (some objects returned, some not) would
create partial state that violates conservation. Root pool absorption is clean:
one destination, no slot pressure, no partial failure.

**Destroy fails if cascade is too large:** no mechanism for this — the cascade
size is not known until it runs (refcount-zero discoveries happen during
iteration). Preemptible cascade handles arbitrary sizes by batching. The only
upfront check is table-full for the single return cap.

**Lazy cascade (defer cleanup to a background thread):** violates A4 — the
kernel has no background threads. All work is triggered by syscalls or
interrupts. The preemptible cascade runs within the destroy syscall context,
yielding to the scheduler at batch boundaries.

**Return all freed objects to destroyer (flat list):** a middle ground where
cascade-freed objects are returned as a list of Space caps rather than absorbed
into the root pool. Rejected for the same slot-pressure reason: the list length
is unpredictable, and the destroyer's cap table may not accommodate it. Also
violates the conservation argument: shared objects (held by multiple Observers)
have their last reference closed during cascade — the destroyer has no prior
relationship with these objects and no expectation of receiving them.

---

## Does NOT settle

- Continuation state representation in CoreState (exact struct layout, maximum
  nesting depth)
- Cascade ordering within the cap table (whether slot iteration order matters
  for correctness or performance)
- Cross-core cascade coordination (if the destroyed Observer held caps to
  objects on other cores, how the cascade reaches them)
- Structural backing composition for the returned Space (whether register save
  area + cap table pages + L1 root are merged into a single contiguous Space or
  returned as separate Spaces)
