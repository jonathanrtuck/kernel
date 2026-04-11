# Design Philosophy

How to think about this system. If you internalize these principles, you should
be able to predict why any component is structured the way it is — and make
decisions that are consistent with the rest of the design without having to ask.

---

## Two root principles

Everything in this system flows from two ideas:

1. **Understand the true shape of the problem.**
2. **Design boundaries that match that shape.**

The first is about seeing clearly. The second is about acting on what you see.
Every specific design rule in this project is a consequence of one or both.

---

## Understand the true shape

### Fix root causes, never paper over symptoms

If something is broken, find out why. A workaround that makes the symptom go
away without understanding the cause is technical debt that compounds silently.
This applies at every level: a rendering bug is not fixed by adjusting pixel
offsets until it looks right — it's fixed by finding the incorrect assumption in
the pipeline.

Defense-in-depth (assertions, validation, retry loops) is fine as a safety net
if worth the added complexity. But it doesn't close the investigation.

### React to reality, don't poll for it

Event-driven over polling. If you need to know when something changes, set up a
mechanism so it tells you — don't repeatedly ask. Polling is a workaround for
not having a notification path. It's papering over a missing interface.

This is a specific case of fixing root causes: the root problem is "I need to
react to state changes," and polling treats the symptom instead of solving the
problem.

### Validate assumptions at the highest leverage point first

The cost of a wrong assumption is proportional to how many decisions sit on top
of it. Before building, identify which assumption, if wrong, invalidates the
most work — and test that one first. Settle the architecture before investing in
the implementation. Spike before you build.

### When independent paths converge, trust the convergence

If you arrive at the same answer from two completely different directions —
different starting assumptions, different reasoning chains, different prior art
— that's stronger evidence than any single argument. They're all seeing the same
underlying shape.

---

## Design boundaries that match the shape

### The system is a series of data transformations

Every part of the system, at every level of abstraction, has the same structure:
data of one shape goes in, data of another shape goes out, and the translation
logic is fully encapsulated. A component is a black box defined entirely by its
input and output shapes.

This model is fractal. Zoom into any black box and it's itself a pipeline of
smaller transformations. Zoom out and any subsystem collapses to a single
translator. At the highest level: `user intent → [OS] → perceptual feedback`.

### The architecture is the interfaces, not the components

Components come and go. Implementations get rewritten. The interfaces — the data
shapes between components — are what make the system a system. Design the
interfaces first. The components follow. Settle the approach before choosing the
technology.

When adding something new, the question is never "which component should this go
in?" It's "which data transformation is this?"

### Push complexity outward to the leaves

Total _essential_ complexity is conserved — it can only be moved, not
eliminated. Put it in leaf nodes: the outermost components that don't connect to
anything downstream. The font rasterizer, the device driver, the format parser.
Complex inside, simple interface. The complexity is contained — it can't leak.

Keep the connective tissue simple: the interfaces, protocols, and relationships
between components. If the connective tissue is complex, the boundaries are in
the wrong place. A messy interface is never solved by adding more surface area.
It's solved by moving the boundary.

This is the same principle as functional programming's "pure core, side effects
at the boundaries." The pure core is the simple connective tissue. The side
effects are the leaf-node complexity. Both approaches achieve the same thing: a
system you can reason about from the center outward, where surprises are
confined to the edges.

The practical payoff: leaf nodes can be rewritten, optimized, or replaced
without affecting anything else. The rendering backend can switch from CPU
scanline rasterization to GPU compute shaders. A font library can be swapped
out. A driver can be replaced for different hardware. None of these changes
ripple inward, because the interface absorbs the variation. The system is as
maintainable and extensible as its interfaces are stable.

### Isolate uncertain decisions behind interfaces

When you must act before a question is fully settled, put an interface in front
of it. Code against the interface, never the implementation. The cost of the
abstraction is small. The cost of a rewrite when the decision changes is large.

This is "push complexity to the leaves" applied to time: the uncertain decision
IS a leaf node. You'll swap it out when you learn more. The interface ensures
that swap is cheap.

### Find the abstraction that absorbs the edge cases

Real simplicity isn't avoiding hard cases — it's finding the abstraction where
they stop being special. The test: when a new use case appears that you didn't
explicitly design for, does the abstraction handle it naturally? If yes, the
boundary is right. If you need a special case, the abstraction is wrong.

When you genuinely can't absorb an edge case, that's a pressure point. Document
it. Don't warp the abstraction to accommodate it.

---

## How this system is designed

The design process has the same fractal structure as the system it describes. If
the system is interfaces all the way down, the design is too.

### Levels of resolution

The design is a tree. Each node is a black box. At any given level, you see a
set of black boxes and the interfaces between them. The interfaces are the
design — they define what each box must accept and produce. The boxes themselves
are opaque. You don't define a box's internals at the level where it appears.
You define them by zooming in, which reveals the next level of interfaces inside
it.

The highest level is the whole system as one box: inputs in, outputs out. Each
subsequent level increases resolution. At first you see the system's major
seams. Then the structure within each section. Then the modules, the data
structures, the functions, the individual lines. There is no sharp boundary
between "design" and "implementation" — it's the same process at every level,
moving from abstract to concrete.

### Working one level at a time

At each level, the work is: define the interfaces between sibling black boxes.
Siblings push and pull on each other — moving responsibility from A to B changes
the A|B interface and may ripple to B|C. Resist defining next-level internals
until the current-level interfaces are as stable as you can make them. Premature
depth locks in boundaries that may need to move.

This doesn't mean rigidly completing an entire level before going deeper
_anywhere_. The tree provides real isolation. Once the A|B interface is stable,
A's internals are mechanically isolated from B's internals. You can follow one
branch deeper while another remains unexplored.

You may also explore deeper levels _speculatively_ — zooming into a black box to
test whether its parent interface could actually work. "What would the
ramifications of this boundary be downstream?" is a legitimate reason to go
deeper. But two constraints apply:

1. **A decision cannot be more settled than its least-settled ancestor.** You
   can tentatively accept interface A and then declare A1 settled — but if A
   moves, A1 may be thrown away entirely. The confidence of any node is capped
   by the confidence of the path above it.
2. **Settling open questions higher up is almost always more valuable than going
   deeper.** An unresolved question at level N affects everything below it. An
   unresolved question at level N+3 affects only its own subtree. Time spent
   resolving higher-level uncertainty has higher leverage, even when the deeper
   question feels more tractable.

### Informed exploration, not exhaustive search

The design process is not breadth-first (complete all of level N before touching
level N+1) or depth-first (follow one branch to the bottom before starting
another). It's informed exploration.

Like a chess engine choosing its next move: of all available options, prune the
obviously wrong ones immediately. Follow the most promising line a few moves
deeper to test it. Go back and do the same for the second most promising. For
the line that still looks best, follow it further. Backtrack when hitting dead
ends. Try different combinations. Settle on something close to ideal — without
brute-forcing every combination to the end of the game, which is infeasible.

You are discovering the shape of the system by tentatively accepting decisions
and evaluating their consequences. Each exploration narrows the space of
possibilities. Backtracking is not failure — it's the price of finding the right
design rather than merely _a_ design.

### Popping the stack

Sometimes you zoom into a black box and discover it can't work as its parent
interface specifies. The interface above is wrong. You have to pop the stack —
go back up one level and rework the sibling boundaries.

Popping one level is normal. It's the expected cost of exploration. Popping two
levels is painful but survivable. Popping to the foundations means most of the
tree built on top is invalidated. The deeper a mistake is buried, the more
expensive it is to correct — which is why the highest-leverage work is getting
the top-level interfaces right.

This gives a natural measure of design risk for any decision: **how many levels
up would I have to go if this turns out to be wrong?** A decision that shapes a
top-level interface shakes everything below it. A decision deep in a leaf is
cheap to revisit — it's behind interfaces that absorb the change.

### Push complexity down the tree

Each level should be as simple as possible. The top levels — the most
foundational, most abstract interfaces — should be the simplest of all.
Complexity increases as you descend. Leaf nodes are nothing but implementation
detail.

This is the "push complexity to the leaves" principle applied to the design
tree. Complexity at a foundational level permeates every level built on top of
it — every component below must understand it, accommodate it, work around it.
Complexity at a leaf node is contained behind an interface. It affects nothing
above it.

When you find complexity at a high level, actively push it down. Ask: can this
be an implementation detail of a component at the next level? Can it be hidden
behind the interface? If complexity cannot be pushed lower, that's a signal —
either the interfaces are in the wrong place, or the complexity is genuinely
essential at that level and must be accepted. But the default is always: push it
down until something stops you.

### Completeness compounds, incompleteness compounds harder

If each level is only 80% right before building the next, the compound effect is
multiplicative: 80% × 80% × 80% = 51% after three levels. Five levels deep, the
foundation is 33% solid. The end product — what the user actually touches —
inherits every gap from every level below.

This is why it's worth spending more time than feels productive on getting each
level right before proceeding. The time invested in a solid foundation is repaid
at every level built on top of it. The time "saved" by moving on too soon is
borrowed at compound interest.

### Deadends are data

A deadend means you've learned something about the shape of the problem that you
couldn't have learned without following the path. Maybe a foundational
assumption isn't coherent. Maybe two principles that seemed compatible actually
conflict when you push them far enough. These discoveries are the most valuable
output of the design process — they're not recoverable any other way.

There is no external pressure on this project. No deadline. The goal is to
understand the problem deeply and find the design that fits it. Better to invest
one designer's time now than millions of users' time for years afterward.
