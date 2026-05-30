# LEET Render Graph Core Design

This file documents the RED-style render graph core for LEET. It starts from
RED's `renderNodeGraph.h` and will grow file by file as each RED graph source is
inspected.

The frame resource allocator is documented separately in `RenderGraphDesign.md`.
The allocator is one subsystem used by graph execution; the graph layer decides
which node runs, under which camera/flow context, and with which CPU/GPU
ordering constraints.

---

## 1. Render Graph Core

The RED graph is not a simple ordered list of passes. It is a node graph with
typed nodes, typed dependencies, command-list ownership metadata, per-node
execution context, and merge/finalization rules.

### RED Header Shape

`renderNodeGraph.h` defines the core graph vocabulary:

- `ERenderNodeType`
- `ERenderNodeDependencyType`
- `RenderNodeCommandListUsage`
- `CRenderNodeBase`
- `RenderNodeSubtype`
- `SRenderNodeParameters`
- `SRenderNodeContext`
- `CRenderNodeGraph`
- graph node and dependency storage records

The Rust design should preserve these concepts directly, while adapting C++
inheritance, raw pointers, intrusive linked lists, and fixed graph arrays into
Rust-owned storage with typed ids.

### Render Node Kinds

RED node kinds:

```cpp
RNT_Unique
RNT_SequenceBegin
RNT_SequenceEnd
RNT_Temporary
RNT_Stage
```

Rust target:

```rust
pub enum RenderNodeKind {
    Unique,
    SequenceBegin,
    SequenceEnd,
    Temporary,
    Stage,
}
```

Meaning:

- `Unique` nodes may appear in multiple source graphs, but only one node of the
  same unique kind/subtype should survive graph merging.
- `SequenceBegin` and `SequenceEnd` are paired helper nodes used to preserve
  ordered regions when graphs are merged.
- `Temporary` nodes are construction/finalization helpers and should not become
  ordinary long-lived render work by accident.
- `Stage` nodes are regular render graph work nodes.

The graph implementation must therefore track node kind and subtype from day
one. A plain `Vec<Node>` executed in insertion order would be the wrong shape.

### Dependency Kinds

RED has two dependency tracks:

```cpp
RNDT_Cpu
RNDT_Gpu
```

Rust target:

```rust
pub enum RenderNodeDependencyKind {
    Cpu,
    Gpu,
}
```

`Cpu` dependencies order node execution and job scheduling. `Gpu` dependencies
order GPU command/list work. These are related but not identical. A node may
need CPU ordering without adding direct GPU ordering, or GPU ordering without
forcing the same CPU dependency shape.

This is a core contract. LEET must not collapse the graph into a single edge
kind and hope to recover the distinction later.

Terminology note: RED's model does not define separate "CPU nodes" and "GPU
nodes" in the core graph data we have inspected. There are render nodes. A
render node is CPU-side code that may do CPU work, record GPU commands, own or
require a command list, submit/synchronize GPU work, or exist as structural graph
state with no implementation.

The CPU/GPU split belongs to dependencies:

```text
CPU dependency -> orders node processing and command recording
GPU dependency -> orders submission/execution of the GPU work produced by nodes
```

This distinction is what allows a parallel-capable design:

```text
record work in parallel where CPU dependencies allow it
submit or execute recorded GPU work in the order required by GPU dependencies
```

For wgpu, GPU dependencies usually map to command encoder/pass/command-buffer
ordering and queue submission order. wgpu handles backend resource transitions,
but LEET still needs graph-level GPU ordering constraints and lifetime extension
information from day one.

### Command List Usage

RED command-list usage:

```cpp
enum class RenderNodeCommandListUsage
{
    None,
    Require,
    Own,
    Sync,
};
```

Rust target:

```rust
pub enum RenderNodeCommandListUsage {
    None,
    Require,
    Own,
    Sync,
}
```

Meaning:

- `None`: the node does not use a command list.
- `Require`: the node expects a command list to already be available in the node
  implementation context.
- `Own`: the node creates its own command list and the process wrapper handles
  epilogue/submission behavior.
- `Sync`: the node does not directly render through a command list, but it is
  responsible for GPU synchronization.

Command-list behavior belongs to node metadata and the graph execution wrapper,
not to ad hoc code inside every node.

### Node Implementation Contract

RED node implementations derive from `CRenderNodeBase`:

```cpp
virtual void Execute(const SRenderNodeImplContext& rctx, job::Builder* builder) const = 0;
virtual CName GetName() const;
virtual GpuApi::CommandListRef CreateCommandList() const;
virtual Bool GetJobBuilderUsage() const;
virtual RenderNodeCommandListUsage GetCommandListUsage() const = 0;

void Process(const SRenderNodeImplContext& rctx, job::Builder* builder) const;
void ProcessEpilogue(const SRenderNodeImplContext& rctx) const;
```

Rust should mirror this as a node implementation trait plus a separate process
wrapper:

```rust
pub trait RenderNodeImpl {
    fn name(&self) -> RenderNodeName;
    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext,
        jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()>;

    fn command_list_usage(&self) -> RenderNodeCommandListUsage;

    fn uses_job_builder(&self) -> bool {
        false
    }
}
```

`execute` is the node's work. The graph/runtime `process_node` wrapper owns the
common behavior around command-list creation, context setup, resource unbinding,
profiling, job dispatch, and epilogue rules. Nodes should not each reinvent that
outer shell.

### Node Parameters And Context

RED separates node identity/implementation from per-node execution context:

```cpp
struct SRenderNodeParameters
{
    ERenderNodeType m_type;
    RenderNodeSubtype m_subType;
    CRenderNodeBase* m_impl;
};

struct SRenderNodeContext
{
    Int32 m_cameraIndex;
    TRenderFlowGroup m_renderFlowGroups[RNDT_MAX];
};
```

Rust target:

```rust
pub struct RenderNodeParameters {
    pub kind: RenderNodeKind,
    pub subtype: RenderNodeSubtype,
    pub impl_id: Option<RenderNodeImplId>,
}

pub struct RenderNodeContext {
    pub camera_index: Option<u32>,
    pub cpu_flow_group: RenderFlowGroup,
    pub gpu_flow_group: RenderFlowGroup,
}
```

`RenderNodeContext` is graph-owned execution metadata. The graph computes the
camera index and CPU/GPU flow groups; `RenderNodeImplContext` is the node-facing
bridge that uses those groups when node code creates tags, records resource
requests, binds runtime state, and retrieves resolved resources.

### Graph Storage Model

RED stores nodes and dependencies separately. Each node stores the first parent
and child dependency for each dependency kind. Each dependency stores parent,
child, and next links for both relation directions.

RED shape:

```cpp
SRenderNodeData
{
    SRenderNodeParameters m_renderNodeParams;
    SRenderNodeContext m_renderNodeContext;
    ItemId m_firstParentDep[RNDT_MAX];
    ItemId m_firstChildDep[RNDT_MAX];
};

SDependencyData
{
    ERenderNodeDependencyType m_type;
    ItemId m_parentNode;
    ItemId m_childNode;
    ItemId m_parentNodeNextDep;
    ItemId m_childNodeNextDep;
};
```

Rust should preserve the behavior with typed ids:

```rust
pub struct RenderGraphNode {
    pub params: RenderNodeParameters,
    pub context: RenderNodeContext,
    pub first_parent_dep: [Option<RenderDependencyId>; 2],
    pub first_child_dep: [Option<RenderDependencyId>; 2],
}

pub struct RenderGraphDependency {
    pub kind: RenderNodeDependencyKind,
    pub parent: RenderNodeId,
    pub child: RenderNodeId,
    pub next_parent_dep: Option<RenderDependencyId>,
    pub next_child_dep: Option<RenderDependencyId>,
}
```

This preserves fast traversal by relation and dependency kind without copying
RED's C++ union layout.

### Graph API Surface

RED's `CRenderNodeGraph` public API includes:

- add node
- add dependency
- find dependency
- test dependency
- remove node
- remove dependency
- remove node or dependency by id
- add another graph
- remove helper nodes
- build render flow groups
- execute
- execute parallel
- debug log
- reset
- copy dependencies
- remove node dependencies
- access node parameters and node context

Rust should expose the same graph capabilities with Rust naming and typed ids.
Unsupported pieces may be implemented conservatively at first, but the public
shape must not prevent graph merging, helper removal, separate CPU/GPU
dependencies, or flow-group computation.

### Initial Rust File Shape

The render graph core should live outside `resources/`.

Recommended starting layout:

```text
src/render_graph/graph/mod.rs
src/render_graph/graph/node.rs
src/render_graph/graph/dependency.rs
src/render_graph/graph/storage.rs
```

Later layers may add:

```text
src/render_graph/graph/factory.rs
src/render_graph/graph/cache.rs
src/render_graph/graph/execution.rs
```

`RenderNodeImplContext` remains the node-facing context, but it should be used
by graph execution rather than treated as part of the resource allocator.

### Locked Render Graph Direction

The render graph V1 must include:

- typed node ids
- typed dependency ids
- explicit CPU and GPU dependency kinds
- explicit node kind and subtype
- per-node camera index
- per-node CPU and GPU render-flow groups
- command-list usage metadata
- graph storage that can traverse dependencies both ways
- graph operations for dependency copy/removal
- graph import/merge with helper-node removal behavior
- command-list group nodes with ordered subnodes
- strict DAG/cycle validation before execution
- render graph cache keyed by graph-shape inputs
- node execution through a process wrapper, not direct calls only
- dependency-counter consume jobs with explicit completion
- immutable/exclusive graph execution view while jobs are active

This is the minimum shape that remains compatible with RED's render graph
architecture.

## 2. `renderNodeGraph.cpp` Inspection

`renderNodeGraph.cpp` is the implementation behind the core graph contract. It
was inspected in focused passes:

1. graph mutation and dependency storage
2. graph import, merge, and helper-node removal
3. render-flow group computation and flattening
4. execution, parallel request walking, debug, and reset behavior

### Pass 1: Graph Mutation And Dependencies

RED treats the graph as mutable only before it is built. Most mutation paths
assert that `m_isBuilt` is false:

```cpp
RED_ASSERT(!m_isBuilt);
```

This applies to node creation, dependency creation, dependency removal, node
removal, dependency copying, graph import, helper removal, and render-flow group
building. `BuildRenderFlowGroups()` is the transition that marks the graph as
built.

Rust should model this explicitly:

```rust
pub enum RenderGraphBuildState {
    Editing,
    Built,
}
```

Mutating a built graph should return a loud error or panic in debug builds. The
graph must not silently accept topology edits after flow groups and flattened
orders have been computed.

#### Node Creation

RED `AddNode` allocates a node record, assigns parameters, and initializes the
node context:

```cpp
ItemId newNodeId = m_nodes.Allocate();
m_nodes[newNodeId].m_renderNodeParams = nodeParams;
m_nodes[newNodeId].m_renderNodeContext.Set(cameraIndex, 0, 0);
```

Rust should initialize every node with:

- node parameters
- camera index
- CPU flow group = 0
- GPU flow group = 0
- no parent dependencies
- no child dependencies

The flow group values are temporary until the build step.

#### RED Graph Array And Id Encoding

RED stores nodes and dependencies in two `TRenderNodeGraphArray` instances:

```cpp
typedef TRenderNodeGraphArray<SRenderNodeData, 1<<29> NodesArray;
typedef TRenderNodeGraphArray<SDependencyData, 1<<30> DependenciesArray;
```

Both arrays return a `Uint32 ItemId`, but each array ORs a different marker bit
into the id:

```cpp
IndexToId(index) = index | IdTypeMask;
IdToIndex(id) = id & ~IdTypeMask;
IsValidId(id) = 0 != (id & IdTypeMask);
```

So a node id and a dependency id may both refer to physical slot index `5`, but
they are different ids because they carry different marker bits:

```text
node slot 5       -> 5 | (1 << 29)
dependency slot 5 -> 5 | (1 << 30)
```

RED then checks whether an id belongs to node storage or dependency storage with:

```cpp
NodesArray::IsValidId(id) != DependenciesArray::IsValidId(id)
```

Rust should not copy the packed-bit layout. It should preserve the same semantic
distinction with typed ids:

```rust
pub struct RenderNodeId(u32);
pub struct RenderDependencyId(u32);
```

These ids are handles into graph storage, not the node/dependency objects
themselves. A function that needs a node id should take `RenderNodeId`; a
function that needs a dependency id should take `RenderDependencyId`. This is
stricter than RED because incorrect id kinds are rejected by Rust's type system
instead of runtime marker-bit asserts.

Rust should still preserve RED's storage behavior:

- ids refer to stable physical slots
- allocated items have a dense usage order
- iteration happens through usage order, not raw slot order
- freeing an item may swap usage entries
- graph import/reindexing can map source usage order to destination usage order

Rust storage sketch:

```rust
struct GraphSlot<T> {
    value: Option<T>,
    usage_index: Option<u32>,
}

struct GraphStorage<T, Id> {
    slots: Vec<GraphSlot<T>>,
    usage: Vec<u32>,
    _id: PhantomData<Id>,
}
```

This keeps the behavior needed by RED's graph import and merge paths without
making node ids and dependency ids interchangeable.

#### Dependency Creation

RED `AddDependency` inserts one dependency record into two adjacency lists:

```cpp
m_dependencies[newDepId].m_parentNode = parentNodeId;
m_dependencies[newDepId].m_childNode = childNodeId;
m_dependencies[newDepId].m_parentNodeNextDep = parentNode.m_firstChildDep[depType];
m_dependencies[newDepId].m_childNodeNextDep = childNode.m_firstParentDep[depType];
m_dependencies[newDepId].m_type = depType;

parentNode.m_firstChildDep[depType] = newDepId;
childNode.m_firstParentDep[depType] = newDepId;
```

This confirms that LEET should store dependencies as graph records with links in
both directions, not only as an edge list. The graph must support traversal from:

- parent node to child dependencies by dependency kind
- child node to parent dependencies by dependency kind

Rust mirror:

```rust
pub struct RenderGraphDependency {
    pub kind: RenderNodeDependencyKind,
    pub parent: RenderNodeId,
    pub child: RenderNodeId,
    pub next_from_parent: Option<RenderDependencyId>,
    pub next_from_child: Option<RenderDependencyId>,
}
```

`next_from_parent` is the next dependency in the parent's child-dependency list.
`next_from_child` is the next dependency in the child's parent-dependency list.
These names are preferred over `next_parent_dep` and `next_child_dep` because
they describe traversal direction rather than implying another parent/child
node.

`AddDependency` also rejects duplicate edges and self-edges. Rust should enforce
the same invariants.

#### Dependency Removal

RED `RemoveDependency` unlinks a dependency from both the parent's child list and
the child's parent list before freeing the dependency record.

Rust must do the same bookkeeping atomically from the graph's point of view. A
dependency id must never remain reachable from one side after removal from the
other.

#### Node Removal Modes

RED `RemoveNode(id, mergeDependencies)` has two behaviors:

- remove the node and all attached dependencies
- bridge every parent to every child, then remove the node

The bridge mode preserves ordering through helper nodes:

```text
A -> Helper -> B
```

becomes:

```text
A -> B
```

Rust should make this explicit:

```rust
pub enum RemoveNodeMode {
    DropDependencies,
    BridgeParentsToChildren,
}
```

This avoids hiding an important graph-finalization behavior behind a boolean.
Bridge mode must operate per dependency kind. CPU parents bridge only to CPU
children, and GPU parents bridge only to GPU children. The graph must not create
cross-kind edges while removing helpers.

#### Dependency Copying

RED has overloads for copying:

- one relation and one dependency kind
- one relation across all dependency kinds
- all relations across all dependency kinds

The implementation skips existing edges to avoid redundant dependencies. LEET
needs this for unique-node merging and sequence merging.

Rust should expose copying as explicit graph operations rather than ad hoc loops
inside merge code. This keeps merge behavior testable.

Prefer explicit direction-specific helpers over RED's hidden `swapParentChild`
parameter:

```rust
copy_parent_dependencies(source, target, kind)
copy_child_dependencies(source, target, kind)
copy_all_dependencies(source, target)
```

For parent copying:

```text
P -> source
```

becomes:

```text
P -> target
```

For child copying:

```text
source -> C
```

becomes:

```text
target -> C
```

Copying must skip duplicate edges, matching RED's `HasDependency` guard.

### Pass 2: Graph Import, Merge, And Helpers

RED `AddGraph` imports another graph by allocating enough node and dependency
records, copying source records, and reindexing all node and dependency ids into
the destination graph.

Important behavior:

- imported node parameters are copied
- imported node contexts are copied
- camera index may be forcibly overridden during import
- parent/child dependency links are reindexed
- dependency node ids and next links are reindexed
- special-node merging may run after import

Rust must support graph import with an id remap table. The exact storage can be
Rust-native, but the operation must not assume ids are the same between graphs.

Graph import and merge are required V1 behavior. `Unique`, `SequenceBegin`, and
`SequenceEnd` must not be placeholder fields waiting for a later rewrite.

Rust should expose one `add_graph` operation with explicit options, mirroring
RED's `AddGraph(..., performMerge)` while keeping the call readable:

```rust
pub struct AddGraphOptions {
    pub force_camera_index: Option<u32>,
    pub merge_special_nodes: bool,
}

pub fn add_graph(
    &mut self,
    source: &RenderGraph,
    options: AddGraphOptions,
) -> RenderGraphResult<GraphImportMap>;
```

The implementation may internally split import, reindexing, and merge into
helpers, but the graph-facing operation should remain one explicit add/import
operation with options.

#### Node Implementation Ownership During Import

RED copies `SRenderNodeParameters` directly during `AddGraph`, including the raw
`CRenderNodeBase*` implementation pointer:

```cpp
targetNode.m_renderNodeParams = sourceNode.m_renderNodeParams;
```

This implies graph topology storage is not the sole owner of node
implementations. The graph stores implementation references; ownership lives in
the graph build/factory side, such as RED's `NodesContainer`.

Rust should use a shared node implementation arena for V1:

```rust
pub struct RenderNodeImplStore {
    nodes: Vec<Box<dyn RenderNodeImpl>>,
}

pub struct RenderNodeParameters {
    pub kind: RenderNodeKind,
    pub subtype: RenderNodeSubtype,
    pub impl_id: Option<RenderNodeImplId>,
}
```

`Option<RenderNodeImplId>` is required because RED permits structural/helper
nodes with a null implementation. V1 graph import uses a shared implementation
store, so copied `impl_id` values remain valid after graph-id remapping. A
different-store import mode would require explicit implementation-id remapping
and is outside this contract.

#### Import Remapping And Usage Order

RED's `AddGraph` relies on `TRenderNodeGraphArray::Allocate(numItems)` appending
newly imported records at the end of dense usage order. That makes this split
valid:

```text
usage indices before nodesOffset -> original graph
usage indices from nodesOffset   -> imported graph
```

LEET does not need to copy RED's offset math, but it must preserve deterministic
usage-order import and build an explicit source-to-destination remap:

```rust
pub struct GraphImportMap {
    pub nodes: Vec<(RenderNodeId, RenderNodeId)>,
    pub dependencies: Vec<(RenderDependencyId, RenderDependencyId)>,
}
```

`add_graph` must reindex all imported references through that map:

- node dependency heads
- dependency parent/child node ids
- dependency `next_from_parent`
- dependency `next_from_child`

The storage allocator may reuse physical slots, but graph import must still
produce a stable imported usage range or an equivalent explicit remap.

#### Camera Index Override

RED names the import option as a forced camera index, but the parameter type is
`TRenderFlowSpace`:

```cpp
AddGraph(graph, enableForceCameraIndex, forcedCameraIndex, performMerge)
```

Then RED writes it into `SRenderNodeContext::m_cameraIndex`.

LEET should keep the option camera-index flavored for this API:

```rust
pub struct AddGraphOptions {
    pub force_camera_index: Option<CameraIndex>,
    pub merge_special_nodes: bool,
}
```

Do not collapse these concepts:

```text
camera index       -> which camera/view the node belongs to
render-flow space  -> resource tag namespace used by the allocator
render-flow group  -> computed execution/order slot
```

They may be derived from each other in some paths, but they should remain
separate concepts in the graph design.

#### Unique Node Merge

RED treats `Unique` nodes with the same subtype as merge candidates when two
graphs are combined.

If both graphs contain the same unique subtype:

```cpp
CopyDependencies(itemId1, itemId0);
RemoveNode(itemId1, false);
```

The surviving node keeps its implementation and receives the duplicate node's
dependencies. The duplicate node is removed without bridging because its
dependencies were copied onto the survivor.

Rust should preserve this as a named merge rule:

```text
same Unique subtype -> copy duplicate deps to survivor -> remove duplicate
```

This is V1 behavior.

#### Sequence Pair Validation

RED requires `SequenceBegin` and `SequenceEnd` helper nodes to appear as pairs.
A begin without an end, or an end without a begin, is fatal.

Rust should validate this during merge/finalization. Broken sequence pairs
should be a hard graph-build error.

RED uses a temporary fixed table with `nodeSubtypeCapacity = 64`. LEET should not
carry that scratch-buffer limit into the public model. Use a map keyed by
`(RenderNodeKind, RenderNodeSubtype)` or equivalent typed data. The semantic rule
is uniqueness per graph side, kind, and subtype; the fixed `64` is an
implementation detail.

#### Sequence Merge

When both graphs contain a sequence pair with the same subtype, RED stitches the
two sequences together rather than keeping two independent sequence regions.

The important semantic result is:

- preserve parents of the imported sequence begin
- preserve children of the original sequence end
- order the imported sequence after the original sequence
- remove obsolete helper nodes while preserving ordering

This relies on `CopyDependencies`, `RemoveNodeDependencies`, and bridged node
removal. Rust should implement this as a dedicated sequence-merge routine, not a
generic graph simplification pass.

This is V1 behavior.

#### Helper Node Removal

RED removes helper node kinds after graph construction:

```cpp
RemoveNode(nodeId, true);
```

That means helper removal always bridges parents to children. Helper nodes are
construction-time structure; they should not remain as ordinary executable graph
nodes. If LEET needs another bridged construction helper, it should be an
explicit helper kind with the same removal rules.

`Temporary` does not participate in the special-node merge table. It is removed
only by helper-node removal. Preserve this RED distinction:

```text
Unique / SequenceBegin / SequenceEnd -> special merge rules
SequenceBegin / SequenceEnd / Temporary -> helper removal with bridging
```

Helper removal must account for storage usage-order mutation. RED decrements the
usage index after removing a helper because freeing can swap another live node
into the removed node's usage slot:

```cpp
RemoveNode(nodeId, true);
--node_usage_i;
```

Rust should avoid this footgun with a search loop:

```rust
while let Some(helper_id) = graph.find_next_helper_node() {
    graph.remove_node(helper_id, RemoveNodeMode::BridgeParentsToChildren)?;
}
```

After helper removal, LEET must validate that no helper nodes remain. RED has an
assert-only loop that appears intended to do this, but the loop condition is
effectively dead:

```cpp
for (Uint32 node_usage_i = 0; node_usage_i; ++node_usage_i)
```

LEET should implement the validation correctly:

```rust
validate_no_helper_nodes()
```

and fail graph build if any `SequenceBegin`, `SequenceEnd`, or `Temporary` node
survives finalization.

Rust build/finalize shape:

```text
Editing graph
  -> import/merge source graphs
  -> merge unique and sequence nodes
  -> remove helper nodes
  -> build render flow groups
  -> Built graph
```

### Pass 3: Render-Flow Groups And Flattening

RED computes render-flow groups for both dependency kinds. The algorithm starts
from roots, assigns dependency-depth levels, counts nodes per level, then
converts levels into unique dense order slots.

Important detail: the final render-flow group is not merely a depth level. RED
turns each level into unique group ids so every node gets a distinct order slot
for that dependency kind.

Rust should compute:

```rust
RenderNodeContext {
    cpu_flow_group,
    gpu_flow_group,
}
```

during graph build. These values should not be manually authored by ordinary
node code.

#### Safer Rust Flow-Group Build

RED temporarily uses `0` for both "unvisited" and "root level" while computing
levels. The recursive update handles this with special cases:

```cpp
if (childLevel <= level)
{
    if (childLevel > 0)
        levelsNodesCount[childLevel]--;

    childLevel = level + 1;
}
```

LEET should preserve the semantics without preserving that ambiguity. Use a
linear topological pass per dependency kind:

```text
1. count incoming edges for every node
2. push all zero-incoming nodes
3. pop nodes in deterministic usage order
4. for each child, update child depth = max(child depth, parent depth + 1)
5. decrement child incoming count
6. if processed count != node count, report a cycle
```

This is Kahn's algorithm with longest-parent depth tracking:

```text
O(nodes + dependencies)
```

Cycle detection is required V1 behavior. Flow groups must not be accepted if
either the CPU dependency graph or GPU dependency graph contains a cycle:

```rust
RenderGraphError::CycleDetected {
    kind: RenderNodeDependencyKind,
}
```

V1 diagnostics must fail the build reliably and report the dependency kind plus
the unresolved nodes. An exact cycle path is useful diagnostic detail, but not a
different algorithm or a reason to accept cyclic graphs.

#### Longest-Parent Depth

RED's recursive update computes:

```text
child level = max(parent level + 1)
```

For example:

```text
A -> C
B -> D -> C
```

produces:

```text
A: 0
B: 0
D: 1
C: 2
```

The Rust topological pass should compute the same longest-parent depth before
packing levels into final flow-group ids.

Flattening uses a dependency kind:

```cpp
BuildFlattenedNodesArray(outNodes, RNDT_Gpu);
```

It places each node at its computed flow group index. This is why duplicate flow
group ids are invalid after build.

#### Dense Unique Flow Groups

RED's final flow group is not the raw dependency depth. After computing levels,
RED accumulates the per-level counts and assigns every node a unique dense order
slot:

```cpp
node.m_renderNodeContext.m_renderFlowGroups[dep_type_i] =
    levelsNodesCount[levelIndex] - 1;
levelsNodesCount[levelIndex]--;
```

So:

```text
Level 0: A, B
Level 1: C, D
```

becomes dense flow groups:

```text
A/B -> 0,1
C/D -> 2,3
```

Exact ordering inside the same level is not semantically important, but the
result must be:

```text
0..node_count-1
```

with no duplicates and no gaps for each dependency kind.

Rust should validate this after building:

```rust
validate_dense_flow_groups(RenderNodeDependencyKind::Cpu)
validate_dense_flow_groups(RenderNodeDependencyKind::Gpu)
```

Flow-group packing must be deterministic. Same-level nodes should be ordered by
stable graph usage order or another explicit stable key so diagnostics and tests
remain reproducible. Nodes must not rely on same-level order for correctness;
dependencies are the only semantic ordering contract.

Every allocated node receives both a CPU flow group and a GPU flow group,
including isolated nodes and nodes whose command-list usage is `None`. A cycle
in either dependency kind invalidates the graph build.

#### Flattening API

RED can flatten by either dependency kind:

```cpp
BuildFlattenedNodesArray(outNodes, RNDT_Cpu);
BuildFlattenedNodesArray(outNodes, RNDT_Gpu);
```

Rust should expose the same concept:

```rust
flatten_nodes(RenderNodeDependencyKind::Cpu)
flatten_nodes(RenderNodeDependencyKind::Gpu)
```

#### CPU/GPU Dependency Validation

RED has a disabled sanity check that would validate the flattened order against
both CPU and GPU dependencies. The comment explains why it is disabled: GPU
dependencies only cover nodes that deal with GPU command lists, so CPU and GPU
dependency graphs are not necessarily identical.

LEET should keep stricter validation, but split it correctly:

- validate the CPU dependency graph as the job/scheduling graph
- validate the GPU dependency graph for nodes participating in GPU command-list
  ordering
- do not require CPU and GPU edges to be identical

Specifically, LEET should not require the GPU-flattened order to satisfy all CPU
dependencies. RED disables that exact check because GPU dependencies only cover
nodes involved in GPU command-list ordering. CPU-only ordering can exist without
a matching GPU edge.

Locked validation rule:

```text
CPU flattening must satisfy CPU dependencies.
GPU flattening must satisfy GPU dependencies.
CPU and GPU dependency graphs are related, but not required to be identical.
```

Execution order and CPU-only nodes will be revisited in the execution pass.

#### Randomization

RED has an optional `ENABLE_CPU_ORDER_RANDOMIZATION` path to stress dependency
assumptions by changing traversal order. LEET does not need this as a runtime
feature in V1, but tests should cover alternate insertion orders so graph build
does not accidentally rely on authoring order where dependencies should decide
the result.

### Pass 4: Execution, Parallel Walk, Debug, And Reset

RED sequential execution flattens by GPU flow group:

```cpp
BuildFlattenedNodesArray(orderedNodes, RNDT_Gpu);
```

Then, for each node:

```cpp
execRenderNodeContext.SetupNodeData(node.m_renderNodeContext, node.m_renderNodeParams);
impl->Process(execRenderNodeContext, &builder);
```

The node implementation is not called directly. It goes through `Process`, which
is responsible for the common command-list/profiler/epilogue shell around
`Execute`.

Rust execution should preserve this shape:

```text
graph execute
  -> flatten by GPU dependency kind
  -> setup RenderNodeImplContext for current node
  -> process_node wrapper
  -> node.execute(...)
```

Preserving this shape does not mean the executor is single-threaded. It means
every execution path, sequential or dependency-counter parallel, must set up the
same node context, call the same `process_node` wrapper, and preserve the same
GPU-dependency command submission order.

Sequential consume/render execution should use GPU flow-group order, matching
RED:

```rust
let ordered = graph.flatten_nodes(RenderNodeDependencyKind::Gpu)?;
for node_id in ordered {
    process_node(node_id, ...)?;
}
```

The node implementation is optional:

```cpp
CRenderNodeBase* impl = node.m_renderNodeParams.m_impl;
if (!impl)
{
    continue;
}
```

Rust should skip nodes whose `impl_id` is `None`. This supports structural,
group entry/exit, and helper-derived graph nodes without inventing fake executable
implementations.

#### Per-Node Context Setup

RED copies the base render-node implementation context once, resets node-local
data, then reconfigures it for each node:

```cpp
SRenderNodeImplContext execRenderNodeContext = rctx;
execRenderNodeContext.ResetNodeData();

execRenderNodeContext.SetupNodeData(
    node.m_renderNodeContext,
    node.m_renderNodeParams
);
```

Rust should preserve this shape:

```rust
let mut node_rctx = base_rctx.clone_for_graph_execution();
node_rctx.reset_node_data();

for node_id in ordered {
    node_rctx.setup_node_data(node.context, node.params);
    process_node(&mut node_rctx, ...)?;
}
```

The context must be reset/configured between nodes so node-local state such as
current camera, flow groups, node kind, unique-node flag, command-list state, and
unbind tracking does not leak between node executions.

#### Process Wrapper Requirement

RED calls:

```cpp
impl->Process(execRenderNodeContext, &builder);
```

not:

```cpp
impl->Execute(execRenderNodeContext, &builder);
```

The process wrapper is required V1 behavior. It owns the shared execution shell:

- command-list creation/require/sync behavior
- profiler scope
- setup and epilogue rules
- resource unbind tracking
- job-builder handling
- eventual command-list submission/close behavior

Rust graph execution must call a `process_node` wrapper around
`RenderNodeImpl::execute`; it must not call node implementations directly as the
only execution path.

#### Parallel Preconsume Request Walk

RED `ExecuteParallel` dispatches batches named:

```cpp
"FlowAllocator_PreConsume_Batch"
```

It distributes nodes in a deterministic quasi-random order to balance request
counts. This is RED's preconsume request-recording path. It does not enforce
ordinary dependency execution between the parallel batches.

LEET must keep this distinct from consume-time dependency-counter jobs. Both are
V1 behavior, but they solve different problems.

Locked rule:

- parallel preconsume request walking records allocator requests
  deterministically
- parallel consume execution runs node jobs according to CPU dependency counters
- GPU dependencies still govern command-list submission order and resource
  correctness
- thread scheduling must never decide allocator request order

Expose the preconsume path according to its actual role, for example:

```rust
parallel_preconsume_request_walk(...)
```

RED distributes work through a deterministic quasi-random placement using a
fixed prime:

```cpp
const Uint32 primeNumber = 1021;
RED_ASSERT(primeNumber > m_nodes.GetNumItems());
```

LEET should not copy this node-count cap into core graph logic. The V1 parallel
request walk should use deterministic chunking/scheduling without a fixed magic
maximum.

RED creates a separate context copy per parallel bucket:

```cpp
SRenderNodeImplContext nodeCtx = rctx;
nodeCtx.ResetNodeData();
```

LEET must do the same for its parallel request walk. Node-local context state
cannot be shared mutably between worker batches.

The parallel path ends with an explicit fence:

```cpp
builder.DispatchFenceExplicitly();
```

So a LEET parallel preconsume request walk must join before resolve begins.

#### Debug And Reset

RED `DebugLog` prints nodes with type, impl pointer, camera index, CPU/GPU flow
groups, node id, and dependency counts. Dependencies are printed with type,
parent, child, and ids.

LEET should provide equivalent diagnostics:

- node id
- node kind/subtype
- implementation name/id
- camera index
- CPU/GPU flow groups
- parent/child dependency counts by dependency kind
- dependency id, kind, parent, child

`Reset` clears built state, nodes, and dependencies under an exclusive update
guard. Rust should make reset explicit and avoid leaving stale flow groups or
dependency links reachable after reset.

Graph diagnostics are V1 behavior. A textual or structured dump should include:

- node usage index
- node id
- node kind and subtype
- implementation id/name, or `None`
- camera index
- CPU flow group
- GPU flow group
- parent CPU dependency count
- parent GPU dependency count
- child CPU dependency count
- child GPU dependency count
- dependency usage index
- dependency id
- dependency kind
- parent node id/index
- child node id/index

Graph reset should:

- require exclusive mutable graph access
- set build state back to `Editing`
- clear node storage
- clear dependency storage
- invalidate any cached flattened arrays or diagnostics
- not clear the shared node implementation arena unless the graph/factory owner
  explicitly requests that separately

## 3. `renderNodeGraphArray.h` Inspection

`renderNodeGraphArray.h` defines the compact storage behavior used by
`CRenderNodeGraph` for both node records and dependency records. This storage
contract is what makes graph import/reindexing and helper removal work.

### Slot Storage And Usage Order

RED stores graph records in two layers:

```cpp
red::DynArray<DataWrapper> m_items;
red::DynArray<Uint32> m_usage;
Uint32 m_numUsed;
```

Each physical slot stores:

```cpp
struct DataWrapper
{
    Int32 m_usageIndex;
    T m_data;
};
```

So every graph record has two relevant indices:

```text
slot index  -> physical slot in m_items
usage index -> dense live order in m_usage[0..m_numUsed)
```

Rust should preserve this model:

```rust
pub struct GraphSlot<T> {
    value: Option<T>,
    usage_index: Option<u32>,
}

pub struct GraphStorage<T, Id> {
    slots: Vec<GraphSlot<T>>,
    usage: Vec<u32>,
    _id: PhantomData<Id>,
}
```

### Typed Ids Instead Of Masked Ids

RED's storage uses a single `Uint32` id with an array-specific marker bit:

```cpp
IndexToId(index) = index | IdTypeMask;
IdToIndex(id) = id & ~IdTypeMask;
IsValidId(id) = 0 != (id & IdTypeMask);
```

`CRenderNodeGraph` instantiates the storage with different marker bits:

```cpp
typedef TRenderNodeGraphArray<SRenderNodeData, 1<<29> NodesArray;
typedef TRenderNodeGraphArray<SDependencyData, 1<<30> DependenciesArray;
```

Rust should use typed ids instead:

```rust
pub struct RenderNodeId(u32);
pub struct RenderDependencyId(u32);
```

The inner `u32` stores a slot index, not a usage index. The Rust type system
replaces RED's marker-bit checks and prevents passing a dependency id to a node
API.

### Allocation Appends To Usage Order

RED explicitly documents that allocation appends the new live item to the end of
usage order:

```cpp
// Allocated item will be placed at the end of used items.
// Some algorithms in renderNodeGraph may depend on this behavior
// since it allows for easy reindexing of nodes added from other graph.
```

This is required behavior for LEET:

```text
allocate(value) -> id
  id points to a physical slot
  the slot is appended to dense usage order
```

Even if the allocator reuses a previously freed physical slot, that slot must be
placed at the end of the live usage range.

### Free Uses Swap-Remove In Usage Order

RED frees an item by swapping its usage entry with the last live usage entry,
marking the freed slot unused, and decrementing `m_numUsed`:

```cpp
Swap(m_usage[itemToFree.m_usageIndex], m_usage[itemAtBack.m_usageIndex]);
Swap(itemToFree.m_usageIndex, itemAtBack.m_usageIndex);
itemToFree.m_usageIndex = -1;
--m_numUsed;
```

Consequences:

- live ids remain slot-stable
- the freed id becomes invalid
- dense usage order may change
- mutation loops must account for swapped usage entries

This is why helper removal should use a safe search loop rather than naive
iteration while deleting records.

### Import Reindexing

RED's `GetReindexedItemId` depends on imported graph records being appended to
the destination usage order:

```cpp
usageOffset = m_numUsed - importGraph.m_numUsed;
thisUsageIndex = usageOffset + importUsageIndex;
thisItemIndex = m_usage[thisUsageIndex];
return IndexToId(thisItemIndex);
```

LEET should build an explicit source-to-destination remap instead of relying on
offset math, but it must preserve the same semantic guarantee:

```text
source usage order maps deterministically to newly allocated destination ids
```

This is the storage-level reason `add_graph` can reindex node ids, dependency
ids, dependency heads, and dependency next links reliably.

### Required Storage API

Rust graph storage should expose:

```rust
allocate(value) -> Id
free(id)
get(id) -> Option<&T>
get_mut(id) -> Option<&mut T>
is_allocated(id) -> bool
len() -> usize
id_by_usage_index(index) -> Id
usage_index(id) -> Option<usize>
ids_in_usage_order() -> impl Iterator<Item = Id>
clear()
```

Locked behavior:

- ids store slot indices
- usage stores live slot indices in dense order
- allocation appends to usage order
- free swap-removes from usage order
- old ids are invalid after free or reset
- reset clears storage fully
- exact RED capacity start/grow values are performance policy, not public API

---

## 4. `renderNodeGraphFactory.h/.cpp` Inspection

`renderNodeGraphFactory.h/.cpp` defines the graph authoring layer. It owns the
relationship between node implementation storage, graph node creation, group
membership, command-list grouping, and helper dependency creation. It was
inspected in focused passes:

1. factory public API and node implementation ownership
2. groups and entry/exit dependency nodes
3. command-list groups and subnodes
4. automatic linking helpers and authoring macros

### Pass 1: Factory API And Node Ownership

RED's `NodesContainer` owns node implementations:

```cpp
struct NodesContainer
{
    red::DynArray<red::UniquePtr<CRenderNodeBase>> m_nodes;
};
```

The graph stores raw implementation pointers in `SRenderNodeParameters`, but the
owning storage is outside `CRenderNodeGraph`. This confirms the Rust graph
should not own boxed node implementations directly.

Rust target:

```rust
pub struct RenderNodeImplStore {
    nodes: Vec<Box<dyn RenderNodeImpl>>,
}

pub struct RenderNodeParameters {
    pub kind: RenderNodeKind,
    pub subtype: RenderNodeSubtype,
    pub impl_id: Option<RenderNodeImplId>,
}
```

`impl_id` is optional because RED graph execution skips nodes with a null
implementation pointer. Structural, imported, or helper-derived nodes may
exist without executable node code.

#### Factory Reset Semantics

RED's factory constructor resets both the target graph and the node container:

```cpp
if (graphPtr)
{
    graphPtr->Reset();
}

if (nodes)
{
    nodes->Reset();
}
```

LEET should avoid surprising constructor side effects. Prefer an explicit entry
point such as:

```rust
NodeGraphFactory::begin_rebuild(&mut graph, &mut impl_store)
```

or another clearly named constructor that documents it resets graph topology and
node implementation storage.

RED's factory destructor can also mutate graph dependencies in debug command-list
mode:

```cpp
if (ENABLE_DEBUG_COMMANDLISTS_EXECUTION)
{
    LinkCPU();
}
```

LEET should not hide graph mutation in `Drop`. Use an explicit `finish()` or
`finalize_authoring()` step.

#### Normal Node Creation

RED `Create<Node>` constructs a node implementation and then registers it:

```cpp
CRenderNodeBase* nodeImpl = RED_NEW(Node)(std::forward<Args>(args)...);
return Register(groupId, SRenderNodeParameters().Set(type, subType, nodeImpl));
```

`Register` consumes the raw implementation pointer into `NodesContainer`:

```cpp
red::UniquePtr<CRenderNodeBase> nodeImpl =
    red::MakeUniquePtr(nodeParameters.m_impl);

m_nodes->m_nodes.PushBack(std::move(nodeImpl));

const auto nodeId = m_graphPtr->AddNode(nodeParameters, 0);
```

Rust should settle ownership before constructing graph parameters:

```rust
pub fn create_node<N: RenderNodeImpl>(
    &mut self,
    group: NodeGroupId,
    kind: RenderNodeKind,
    subtype: RenderNodeSubtype,
    node: N,
) -> RenderGraphResult<RenderNodeId> {
    let impl_id = self.impl_store.insert(Box::new(node));
    self.register_existing_impl(group, kind, subtype, impl_id)
}
```

Graph nodes receive `impl_id: Some(impl_id)`. The implementation store owns the
boxed node.

`CreateCustom` in RED only calls `Register`, so LEET can use the same underlying
path for boxed/custom node implementations.

#### Command-List Group Subnodes

RED changes `Register` behavior when a command-list group is open:

```cpp
if (m_currentGroup != nullptr)
{
    m_currentGroup->AddSubnode(std::move(nodeImpl));
    return CRenderNodeGraph::INVALID_ITEM_ID;
}
```

That means a created node implementation may become a subnode inside the current
command-list group instead of becoming a graph-visible node. RED represents this
with an invalid graph id.

LEET should avoid returning invalid ids for this case. Use explicit APIs:

```rust
pub fn create_node<N: RenderNodeImpl>(...) -> RenderGraphResult<RenderNodeId>;
pub fn create_subnode<N: RenderNodeImpl>(...) -> RenderGraphResult<()>;
```

Rules:

- `create_node` creates a graph-visible node and returns a `RenderNodeId`
- `create_subnode` creates an implementation inside the currently open
  command-list group and returns no graph id
- subnodes cannot be linked directly by graph dependencies
- calling `create_subnode` without an open command-list group is an error
- calling `create_node` while a command-list group is open is an error

This preserves RED's behavior while making the distinction explicit and
type-safe.

#### Group Membership At Registration

Outside command-list grouping, RED records every graph-visible node into a group
tag list:

```cpp
m_groupTags.Back().m_nodeId = nodeId;
m_groupTags.Back().m_nextIndex = groupData.m_firstGroupTagIndex;
groupData.m_firstGroupTagIndex = m_groupTags.Size() - 1;
```

So graph-visible node creation must accept a `NodeGroupId` from day one. Group
membership is used by later linking helpers and group entry/exit dependency
nodes.

RED also keeps `m_nodeIds` parallel to `NodesContainer::m_nodes`:

```cpp
m_nodeIds.PushBack(nodeId);
RED_ASSERT(m_nodes->m_nodes.Size() == m_nodeIds.Size());
```

Rust can store the implementation id directly in node parameters, but the
factory may still keep an authoring-order `created_node_ids` list for helper
linking such as `link_gpu`, `link_cpu`, and CPU-to-GPU bridge helpers.

### Pass 2: Groups And Entry/Exit Dependency Nodes

RED `NodeGroupID` is an authoring-time group:

```cpp
enum class NodeGroupID : Uint8 { None = 0 };
```

It is not a `RenderFlowGroup`. Keep these concepts separate:

```text
NodeGroupId      -> graph authoring/linking group
RenderFlowGroup  -> computed execution/order slot
RenderFlowSpace  -> allocator resource-name namespace
```

Rust target:

```rust
pub struct NodeGroupId(u8);
```

RED has `MAX_GROUPS_COUNT = 16`. LEET should not bake that in as a core
architecture limit. Use growable group storage or a typed/static group registry
with explicit validation.

#### Group Membership

RED stores group membership as linked `SGroupTag` records:

```cpp
m_groupTags.Back().m_nodeId = nodeId;
m_groupTags.Back().m_nextIndex = groupData.m_firstGroupTagIndex;
groupData.m_firstGroupTagIndex = m_groupTags.Size() - 1;
```

Rust can use a clearer representation:

```rust
pub struct NodeGroupData {
    pub members: Vec<RenderNodeId>,
    pub entry: Option<RenderNodeId>,
    pub exit: Option<RenderNodeId>,
}
```

Every graph-visible node created by the factory should be registered into a
`NodeGroupId`, including `NodeGroupId::None` if LEET keeps RED's default group
concept.

#### Direct Links

RED direct node-to-node links accept either CPU or GPU dependency kind:

```cpp
Link(parent_node, child_node, depType);
```

Rust target:

```rust
pub fn link_nodes(
    &mut self,
    parent: RenderNodeId,
    child: RenderNodeId,
    kind: RenderNodeDependencyKind,
) -> RenderGraphResult<()>;
```

Factory links should be idempotent. RED's `LinkImpl` checks for an existing edge
before adding:

```cpp
if (!m_graphPtr->HasDependency(parent, child, depType))
{
    m_graphPtr->AddDependency(parent, child, depType);
}
```

#### Group Links Are CPU-Only

RED rejects group links for GPU dependencies:

```cpp
RED_FATAL_ASSERT(depType == RNDT_Cpu,
    "Only CPU dependencies are supported when involving groups");
```

This applies to:

```cpp
Link(node, group, depType)
Link(group, node, depType)
Link(group, group, depType)
```

Rust should expose CPU-only group-link APIs instead of accepting a generic
dependency kind:

```rust
pub fn link_node_to_group(
    &mut self,
    parent: RenderNodeId,
    child_group: NodeGroupId,
) -> RenderGraphResult<()>;

pub fn link_group_to_node(
    &mut self,
    parent_group: NodeGroupId,
    child: RenderNodeId,
) -> RenderGraphResult<()>;

pub fn link_group_to_group(
    &mut self,
    parent_group: NodeGroupId,
    child_group: NodeGroupId,
) -> RenderGraphResult<()>;
```

These methods add CPU dependencies only.

#### Group Entry Node

For `Link(parent_node, child_group, RNDT_Cpu)`, RED creates or reuses the child
group's dummy input node. LEET calls this a group entry node:

```cpp
parent_node -> group_entry
group_entry -> each group member
```

Meaning:

```text
parent_node must run before every node in child_group
```

The group entry node is created lazily when first needed.

#### Group Exit Node

For `Link(parent_group, child_node, RNDT_Cpu)`, RED creates or reuses the parent
group's dummy output node. LEET calls this a group exit node:

```cpp
each group member -> group_exit
group_exit -> child_node
```

Meaning:

```text
child_node must run after every node in parent_group
```

The group exit node is created lazily when first needed.

For `Link(parent_group, child_group, RNDT_Cpu)`, RED links:

```text
parent_group_exit -> child_group_entry
```

Meaning:

```text
all nodes in parent_group finish before any node in child_group starts
```

#### Empty Group Handling

RED connects dummy input and dummy output when both exist. In LEET terms:

```cpp
group_entry -> group_exit
```

This preserves group dependencies even when a group has no member nodes.

Rust should preserve this behavior. Empty groups are valid authoring scopes.

#### Group Entry/Exit Node Representation

RED creates group dummy input/output nodes with default
`SRenderNodeParameters()` and a null implementation entry:

```cpp
groupData.m_dummyInputNode = m_graphPtr->AddNode(SRenderNodeParameters(), 0);
m_nodes->m_nodes.PushBack(nullptr);
```

`SRenderNodeParameters()` defaults to `RNT_Stage`, subtype 0, null impl. These
entry/exit nodes are not `Temporary` helper nodes and are not removed by
`RemoveHelperNodes`.

LEET should represent group entry/exit nodes explicitly so they are not confused
with removable helpers:

```rust
pub enum RenderNodeRole {
    Normal,
    GroupEntry(NodeGroupId),
    GroupExit(NodeGroupId),
}
```

or an equivalent internal marker. They should have:

```text
kind: Stage
impl_id: None
role: GroupEntry/GroupExit
```

Execution skips them because `impl_id` is `None`; graph build keeps them because
they preserve group dependency structure.

Do not require a parallel `impl_store.len() == created_node_ids.len()` invariant
in Rust. RED pushes null implementation entries to keep its parallel arrays
aligned, but LEET can store `impl_id: None` directly on graph nodes.

Internal group helpers:

```rust
ensure_group_entry(group) -> RenderGraphResult<RenderNodeId>
ensure_group_exit(group) -> RenderGraphResult<RenderNodeId>
```

### Pass 3: Command-List Groups And Subnodes

RED has a special graph-visible wrapper node for command-list groups:

```cpp
class CRenderNodeCommandListGroup final : public CRenderNodeBase
```

The wrapper node owns a list of internal subnodes:

```cpp
red::DynArray<red::SharedPtr<CRenderNodeBase>> m_subnodes;
```

So the graph topology sees one node:

```text
CommandListGroupNode
```

but that node executes several internal render node implementations:

```text
subnode A
subnode B
subnode C
```

Subnodes are not graph-visible nodes. They cannot be linked directly by graph
dependencies. The command-list group node is the dependency target/source.

#### Group Creation

RED begins a command-list group by creating a `CRenderNodeCommandListGroup` and
registering it as a normal graph node:

```cpp
m_currentGroup = new CRenderNodeCommandListGroup(name, commandListType);
const auto nodeId = Register(
    groupId,
    SRenderNodeParameters(type, subType, m_currentGroup));
```

RED then redirects node creation while `m_currentGroup` is non-null:

```cpp
if (m_currentGroup != nullptr)
{
    m_currentGroup->AddSubnode(std::move(nodeImpl));
    return CRenderNodeGraph::INVALID_ITEM_ID;
}
```

LEET should preserve the behavior but make the authoring API explicit:

```rust
pub fn begin_command_list_group(
    &mut self,
    group_id: NodeGroupId,
    kind: RenderNodeKind,
    subtype: RenderNodeSubtype,
    name: impl Into<String>,
    queue_kind: RenderQueueKind,
) -> RenderGraphResult<RenderNodeId>;

pub fn create_subnode<N: RenderNodeImpl>(
    &mut self,
    node: N,
) -> RenderGraphResult<()>;

pub fn end_command_list_group(&mut self) -> RenderGraphResult<()>;
```

Rules:

- command-list groups are graph-visible nodes
- command-list groups are not nestable
- `create_subnode` is valid only while a command-list group is open
- `create_subnode` returns no `RenderNodeId`
- `create_node` while a command-list group is open is an error in V1
- subnodes are owned by the command-list group, not by graph topology

This avoids RED's invalid-id return path and keeps node creation explicit.

#### Queue Kind

RED command-list groups can be graphics or compute:

```cpp
GpuApi::CommandListType::Default -> RQT_Graphic
GpuApi::CommandListType::Compute -> RQT_Compute
```

LEET should use an explicit queue enum:

```rust
pub enum RenderQueueKind {
    Graphics,
    Compute,
}
```

The command-list group stores this queue kind:

```rust
struct CommandListGroupNode {
    name: String,
    queue_kind: RenderQueueKind,
    subnodes: Vec<RenderNodeImplId>,
}
```

#### Allocator Queue Scope

Before processing subnodes, RED tells the frame resource allocator that a queue
scope has begun:

```cpp
rctx.GetResourceAllocator()->RequestBeginQueue(
    renderQueueType,
    rctx.GetRenderFlowGroup());
```

After the subnodes are processed, RED ends the queue scope:

```cpp
rctx.GetResourceAllocator()->RequestEndQueue(rctx.GetRenderFlowGroup());
```

LEET must preserve this request behavior:

```text
request_begin_queue(queue_kind, flow_group)
  process subnode A
  process subnode B
  process subnode C
request_end_queue(flow_group)
```

This matters even though wgpu does not expose RED-style command lists. The queue
scope is allocator data. It affects lifetime analysis, queue ordering, and the
graphics-vs-compute shape of the request stream.

#### Preconsume And Consume

RED processes command-list subnodes in both phases. During non-consume phases,
it walks subnodes directly:

```cpp
for (const red::SharedPtr<CRenderNodeBase>& subnode : m_subnodes)
{
    subnode->Process(rctx, builder);
}
```

During consume, RED may dispatch child jobs for subnodes that need a job
builder. LEET should keep the same semantic boundary:

- preconsume walks every subnode and records allocator requests
- consume walks every subnode in the same order
- begin/end queue requests must appear in both request streams
- subnode request streams participate in the parent command-list group's
  render-flow group

The exact Rust job API can differ, but command-list groups must remain a
parallel-capable execution unit from V1.

#### Command-List Ownership

RED reports:

```cpp
RenderNodeCommandListUsage::Own
```

for `CRenderNodeCommandListGroup`. LEET should preserve this concept: a
command-list group owns the command encoder/list context used by its subnodes.
Normal subnodes should not each create graph-level command-list ownership.

RED also has a TODO saying command-list buckets should belong here rather than
general `CRenderNodeBase`. LEET should follow that direction and keep
command-list group behavior concentrated in the command-list group node rather
than spreading bucket ownership across every render node implementation.

### Pass 4: Factory Linking Helpers

RED routes all direct dependency creation through `LinkImpl`:

```cpp
void NodeGraphFactory::LinkImpl(
    CRenderNodeGraph::ItemId parent,
    CRenderNodeGraph::ItemId child,
    ERenderNodeDependencyType depType)
{
    RED_FATAL_ASSERT(parent != CRenderNodeGraph::INVALID_ITEM_ID);
    RED_FATAL_ASSERT(child != CRenderNodeGraph::INVALID_ITEM_ID);

    if (!m_graphPtr->HasDependency(parent, child, depType))
    {
        m_graphPtr->AddDependency(parent, child, depType);
    }
}
```

LEET should mirror this as the private dependency insertion path:

```rust
fn link_impl(
    &mut self,
    parent: RenderNodeId,
    child: RenderNodeId,
    kind: RenderNodeDependencyKind,
) -> RenderGraphResult<()>;
```

The public direct-node link remains explicit:

```rust
pub fn link_nodes(
    &mut self,
    parent: RenderNodeId,
    child: RenderNodeId,
    kind: RenderNodeDependencyKind,
) -> RenderGraphResult<()>;
```

`link_impl` must validate ids and avoid duplicate dependencies. Duplicate
requests should be harmless, but the graph storage should contain a single edge
for each `(parent, child, kind)` tuple.

#### Creation-Order Chain Helpers

RED has helper methods that add dependencies based on factory creation order:

```cpp
void LinkGPU();
void LinkCPU();
```

Both walk `m_nodeIds` and connect each created graph-visible node to the next:

```cpp
m_graphPtr->AddDependency(
    m_nodeIds[i - 1],
    m_nodeIds[i],
    ERenderNodeDependencyType::RNDT_Gpu);
```

RED comments call these temporary helpers. LEET should not make this behavior
implicit. If preserved, expose it as explicit authoring utilities with names
that say exactly what they do:

```rust
pub fn link_created_order_gpu_chain(&mut self) -> RenderGraphResult<()>;
pub fn link_created_order_cpu_chain(&mut self) -> RenderGraphResult<()>;
```

These helpers operate only on graph-visible nodes in factory creation order.
Command-list subnodes are not graph-visible and are not directly chained by
these helpers.

#### CPU To Later GPU Work

RED can add CPU dependencies from one node to all later nodes that produce or
use command-list work:

```cpp
void LinkCPUToNextGPU(CRenderNodeGraph::ItemId parent);
```

The check for "GPU-related" is command-list usage:

```cpp
nextNode != nullptr &&
nextNode->GetCommandListUsage() != RenderNodeCommandListUsage::None
```

Meaning:

```text
parent_cpu_node must process before later GPU-work nodes process/record/submit
```

LEET name:

```rust
pub fn link_cpu_to_later_gpu_work(
    &mut self,
    parent: RenderNodeId,
) -> RenderGraphResult<()>;
```

This helper adds CPU dependencies only. It should not add GPU dependencies.

#### CPU From Earlier GPU Work

RED also adds CPU dependencies from all earlier GPU-work nodes to one child:

```cpp
void LinkCPUToPreviousGPU(CRenderNodeGraph::ItemId child);
```

This is used by RED's `SYNC_SUBMIT` macro:

```cpp
auto ecl = ADD_NODE(group, name "_Flush", CRenderNode_Synchronize, ...);
factory.LinkCPUToPreviousGPU(ecl);
```

Meaning:

```text
all earlier GPU-work nodes must process before this sync/submit CPU node
```

LEET name:

```rust
pub fn link_cpu_from_earlier_gpu_work(
    &mut self,
    child: RenderNodeId,
) -> RenderGraphResult<()>;
```

Like RED, the "GPU-work" filter should be:

```text
node.command_list_usage != RenderNodeCommandListUsage::None
```

This preserves the distinction between render nodes and dependency kinds. These
helpers do not create a formal "GPU node" category; they find nodes that own,
require, or synchronize command-list work.

#### Predicate Linking

RED exposes `LinkIf`:

```cpp
template <ERenderNodeDependencyType depType, typename Func>
void LinkIf(CRenderNodeGraph::ItemId parent, const Func& func)
{
    for (Uint32 i = 0, end = m_graphPtr->GetNumNodes(); i < end; ++i)
    {
        const CRenderNodeGraph::ItemId nodeId = m_graphPtr->GetNode(i);
        if (func(m_graphPtr->GetNodeParameters(nodeId).m_impl))
        {
            Link(parent, nodeId, depType);
        }
    }
}
```

Rust should expose the same capability without leaking mutable implementation
pointers:

```rust
pub fn link_if(
    &mut self,
    parent: RenderNodeId,
    kind: RenderNodeDependencyKind,
    predicate: impl FnMut(RenderNodeView<'_>) -> bool,
) -> RenderGraphResult<()>;
```

`RenderNodeView` should provide read-only metadata: id, kind, subtype, role,
command-list usage, group membership, and optional implementation type/debug
name. It should not permit mutation while dependency insertion is in progress.

---

## 5. Inspection Coverage

This document currently covers the RED render graph files that shape LEET's
graph-core contract:

- `renderNodeGraph.h`
- `renderNodeGraph.cpp`
- `renderNodeGraphArray.h`
- `renderNodeGraphFactory.h`
- `renderNodeGraphFactory.cpp`
- `renderGraphNodes.h/.cpp`
- `renderNodeJob.h/.cpp`
- `renderNodeImplContext.h/.cpp`
- `renderGraphCache.h/.cpp`
- `renderRenderFrame.cpp`

The file is intentionally focused on graph-core behavior: topology, dependency
semantics, node implementation contracts, render-flow grouping, command-list
recording, graph caching, and execution orchestration. Frame resource allocator
details remain in `RenderGraphDesign.md`; frame renderer and Bevy extraction
bridge notes remain in `FrameRenderer.md`.

---

## 6. `renderGraphNodes.h` Inspection

`renderGraphNodes.h` is mostly a catalog of concrete RED renderer nodes. It
contains many feature-specific nodes for scene rendering, post-process, shadows,
UI, debug rendering, ray tracing, particles, and other renderer systems.

LEET should not treat every concrete RED node in this file as graph-core design
input. The useful core signal is:

- reusable node implementation helpers
- node metadata exposed to the process wrapper
- lifecycle/system nodes
- synchronization nodes
- declaration-only/resource-related nodes
- common node-authoring patterns

### Pass 1: Node Helpers And Metadata

RED uses `DECLARE_RENDER_NODE` to give every concrete node a stable name and,
when profiling is enabled, an instrumentation object:

```cpp
#define DECLARE_RENDER_NODE(NodeClassName) \
    virtual CName GetName() const override { ... } \
    DEFINE_INSTURMENTATION_GETTER
```

LEET should not mirror this as a macro. Node naming should remain an explicit
part of the Rust node implementation trait:

```rust
pub trait RenderNodeImpl {
    fn name(&self) -> RenderNodeName;
    fn command_list_usage(&self) -> RenderNodeCommandListUsage;
    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext,
        jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()>;
}
```

Profiling, diagnostics, debug dumps, and command encoder labels should all be
able to use the same stable node name.

#### Builder Helper

RED defines a convenience base:

```cpp
template <typename Type = SimpleNode>
class Builder : public CRenderNodeBase
{
public:
    template <typename ... Args>
    Builder(Args&& ... args) : CRenderNodeBase(args...) {}

#ifdef USE_PROFILER
    virtual Bool AllowGpuScope() const override { return Type::gpuScope; }
#endif
};
```

This is an implementation convenience for RED's concrete C++ nodes. It is not a
graph-topology concept. LEET should express the same behavior as trait metadata
or a small behavior struct, not as a required inheritance-style builder layer.

#### GPU Scope Metadata

RED node tags include:

```cpp
struct SimpleNode {
    static constexpr Bool gpuScope = true;
};

struct SimpleNoGPUScope {
    static constexpr Bool gpuScope = false;
};
```

`allow_gpu_scope` is profiling/debug metadata. It tells the process wrapper
whether it may open a named GPU profiler/debug scope around the node's GPU work.
It must not affect graph scheduling, resource lifetime analysis, or command
correctness.

Reasons a node may disable wrapper-created GPU scopes:

- it is CPU-only work
- it is a sync/submit/control node
- it already creates its own inner GPU scopes
- its command-list behavior makes a wrapper-level scope noisy or unsafe

LEET should preserve this as a defaulted node metadata method:

```rust
fn allow_gpu_scope(&self) -> bool {
    true
}
```

CPU-only/control nodes can override it to `false`.

#### Base Node Modifiers

RED `CRenderNodeBase` stores metadata configured by constructor tags:

```cpp
Uint64 m_globalBindingMod;
Bool m_rtBinder;
```

The tags are:

```cpp
rend::node::GlobalBindingsMod(...)
rend::node::RenderTargetBinder()
```

`GlobalBindingsMod` records which global binding slots the node modifies:

```cpp
m_slots |= RED_FLAG64(slot);
```

`RenderTargetBinder` marks a node as one that binds render targets.

These are process-wrapper hints, not dependency edges. LEET should represent
them explicitly:

```rust
pub struct RenderNodeBehavior {
    pub global_binding_mod: GlobalBindingMask,
    pub binds_render_targets: bool,
    pub allow_gpu_scope: bool,
}
```

or as trait methods:

```rust
fn global_binding_mod(&self) -> GlobalBindingMask {
    GlobalBindingMask::empty()
}

fn binds_render_targets(&self) -> bool {
    false
}
```

The important design point is that a render node is not only an `execute`
function. It also exposes metadata that the process wrapper uses to set up and
clean up around execution.

#### Required Node Trait Shape

RED `CRenderNodeBase` confirms the graph must call a process wrapper, not node
`Execute` directly:

```cpp
virtual void Execute(...) const = 0;
virtual GpuApi::CommandListRef CreateCommandList() const;
virtual Bool GetJobBuilderUsage() const;
virtual RenderNodeCommandListUsage GetCommandListUsage() const = 0;

void Process(...) const;
void ProcessEpilogue(...) const;
```

LEET V1 node metadata should therefore include:

```rust
pub trait RenderNodeImpl {
    fn name(&self) -> RenderNodeName;
    fn command_list_usage(&self) -> RenderNodeCommandListUsage;
    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext,
        jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()>;

    fn uses_job_builder(&self) -> bool {
        false
    }

    fn allow_gpu_scope(&self) -> bool {
        true
    }

    fn binds_render_targets(&self) -> bool {
        false
    }

    fn global_binding_mod(&self) -> GlobalBindingMask {
        GlobalBindingMask::empty()
    }
}
```

This mirrors RED's base-node contract without inheriting its C++ macro and
constructor-tag style.

### Pass 2: Core Lifecycle And System Nodes

`renderGraphNodes.h` declares several lifecycle/control nodes near the top of
the file:

```cpp
class CRenderNode_StartRender;
class CRenderNode_EndRender;
class CRenderNode_Present;
class CRenderNode_FlushTextureGrabs;
class CRenderNode_FlushBufferGrabs;
```

Later in the file it also declares:

```cpp
class CRenderNode_EndFrame;
class CRenderNode_CleanupBatchDataAllocator;
```

These are real render node implementations. They inherit from
`CRenderNodeBase` directly or through `rend::node::Builder<T>`, and they are
scheduled through the same graph as visual rendering nodes.

LEET should mirror this with ordinary `RenderNodeImpl` structs, not with a
separate enum-dispatched core-node path:

```rust
pub struct StartRenderNode;
pub struct EndRenderNode;
pub struct PresentNode;
pub struct FlushTextureGrabsNode;
pub struct FlushBufferGrabsNode;
pub struct EndFrameNode;
pub struct CleanupBatchDataAllocatorNode;
```

Each one implements the same trait used by feature renderer nodes:

```rust
impl RenderNodeImpl for PresentNode {
    fn name(&self) -> RenderNodeName {
        RenderNodeName::new("Present")
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::None
    }

    fn allow_gpu_scope(&self) -> bool {
        false
    }

    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext,
        jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        Ok(())
    }
}
```

This preserves the RED split:

```text
graph node record
  -> RenderNodeParameters / RenderNodeId

executable node implementation
  -> CRenderNodeBase in RED
  -> RenderNodeImpl trait object in LEET
```

`CRenderNodeBase::Process` and `ProcessEpilogue` remain runtime wrapper logic,
not per-node trait methods.

#### Header Metadata

`CRenderNode_StartRender`:

```cpp
GetCommandListUsage() -> Require
GetJobBuilderUsage() -> true
Builder<SimpleNoGPUScope>
```

LEET metadata:

```text
command_list_usage = Require
uses_job_builder = true
allow_gpu_scope = false
```

`CRenderNode_EndRender`:

```cpp
GetCommandListUsage() -> Require
Builder<SimpleNoGPUScope>
```

LEET metadata:

```text
command_list_usage = Require
allow_gpu_scope = false
```

`CRenderNode_Present`:

```cpp
GetCommandListUsage() -> None
Builder<SimpleNoGPUScope>
```

LEET metadata:

```text
command_list_usage = None
allow_gpu_scope = false
```

`CRenderNode_FlushTextureGrabs` and `CRenderNode_FlushBufferGrabs`:

```cpp
GetCommandListUsage() -> None
Builder<SimpleNoGPUScope>
```

LEET metadata:

```text
command_list_usage = None
allow_gpu_scope = false
```

`CRenderNode_EndFrame`:

```cpp
GetCommandListUsage() -> None
AllowGpuScope() -> false
```

`CRenderNode_CleanupBatchDataAllocator`:

```cpp
GetCommandListUsage() -> None
AllowGpuScope() -> false
```

#### Core Finding

RED uses render graph nodes for lifecycle and control operations, not only for
visual rendering passes. LEET V1 must support graph nodes that:

- have `RenderNodeCommandListUsage::None`
- disable wrapper GPU scopes
- still participate in graph dependencies and scheduling
- may use the job builder even when they are not ordinary drawing passes

Do not special-case these nodes in graph storage or execution. They are normal
`RenderNodeImpl` implementations with specific metadata.

### Pass 3: Resource Declaration And Allocator-Adjacent Nodes

`renderGraphNodes.h` declares a clear family of declaration-only resource nodes:

```cpp
class CRenderNode_DeclareCommonResourceAllocs;
class CRenderNode_DeclareCommonResourceAllocs_FinalOnly;
class CRenderNode_DeclareCommonResourceAllocs_SafeMode;
class CRenderNode_DeclareCommonResourceAllocs_HitProxy;
class CRenderNode_DeclareCommonResourceAllocs_GBufferOnly;
```

All of these advertise:

```cpp
virtual RenderNodeCommandListUsage GetCommandListUsage() const override
{
    return RenderNodeCommandListUsage::None;
}
```

These are real graph nodes, but they do not require a GPU command list. Their
purpose is allocator-adjacent graph work: record common resource allocation/use
intent as part of the graph request stream.

LEET should preserve this pattern:

```text
declaration-only node
  -> normal RenderNodeImpl implementation
  -> command_list_usage = None
  -> no command list required
  -> participates in graph dependencies and scheduling
  -> records allocator requests through RenderNodeImplContext
```

Example LEET shape:

```rust
pub struct DeclareCommonResourceAllocsNode;

impl RenderNodeImpl for DeclareCommonResourceAllocsNode {
    fn name(&self) -> RenderNodeName {
        RenderNodeName::new("DeclareCommonResourceAllocs")
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::None
    }

    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext,
        jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        // declare_resource/use_begin/use_end calls live here
        Ok(())
    }
}
```

Do not introduce a special graph storage path for these. They are ordinary node
implementations that happen to have no command-list usage.

#### Declaration Variants

RED has multiple declaration variants:

```text
common
final-only
safe-mode
hit-proxy
g-buffer-only
```

The core lesson is not that LEET must port each RED variant immediately. The
lesson is that different graph configurations may insert different declaration
nodes. Graph construction and graph merging must handle declaration nodes like
any other node.

#### Preconsume And Consume

Declaration-only nodes are important because they naturally fit the allocator
two-phase model:

```text
PreConsume
  declaration node records resource declarations and use ranges

Resolve
  allocator resolves physical resources

Consume
  declaration node replays the same declaration/use request stream
```

Declaration nodes should not retrieve resolved textures or buffers during
preconsume. They may declare resources and record uses. Retrieval remains valid
only in consume after resolve.

#### Prepare Nodes Are Not Automatically Core

`renderGraphNodes.h` contains many nodes named `Prepare*`, such as scene,
hit-proxy, distant-shadow, rain-map, and other renderer-feature preparation
nodes. These are useful examples of node authoring, but they are not automatically
graph-core just because their names contain "Prepare".

LEET should inspect individual `Prepare*` nodes only when their implementation
touches graph execution, allocator request semantics, synchronization, or
command-list behavior.

#### Camera Resource Dependency Scope

The header also declares:

```cpp
class CRenderNode_CameraResourceDependencyScope : public CRenderNodeBase
{
    Bool m_isScopeOpen;

public:
    CRenderNode_CameraResourceDependencyScope(Bool isScopeOpen);
    virtual RenderNodeCommandListUsage GetCommandListUsage() const override
    {
        return RenderNodeCommandListUsage::Require;
    }
};
```

Its name suggests allocator or lifetime relevance, but it requires a command
list and the header does not reveal its semantics. Do not design a LEET mirror
from the declaration alone. Inspect its `.cpp` implementation before locking any
equivalent concept.

### Pass 4: Sync, Queue, And Command-List System Nodes

`renderGraphNodes.h` exposes two different sync families. LEET should keep them
conceptually separate.

#### Synchronize Node

RED declares:

```cpp
class CRenderNode_Synchronize : public CRenderNodeBase
{
public:
    CRenderNode_Synchronize(
        GpuApi::CommandListSyncType sync,
        const char* name = nullptr);

    virtual RenderNodeCommandListUsage GetCommandListUsage() const override
    {
        return RenderNodeCommandListUsage::Sync;
    }

    virtual void Execute(
        const SRenderNodeImplContext& rctx,
        job::Builder* builder) const final override;

private:
    GpuApi::CommandListSyncType m_sync;
    String m_name;
};
```

The implementation records an allocator queue-sync request every time it runs:

```cpp
rctx.GetResourceAllocator()->RequestQueueSync(
    rctx.GetRenderFlowGroup(),
    m_sync);
```

During consume it also submits/synchronizes frame command lists:

```cpp
if (rctx.IsConsumePhase())
{
    GetRenderer()->GetFrameCommandLists().Submit(
        m_name.AsChar(),
        rctx.GetRenderFlowGroup(),
        m_sync,
        *builder);
}
```

So `CRenderNode_Synchronize` is both:

```text
allocator queue-sync marker
real consume-phase command-list submit/sync node
```

LEET should mirror this as an ordinary node implementation:

```rust
pub struct SynchronizeNode {
    pub sync: CommandListSyncType,
    pub name: RenderNodeName,
}

impl RenderNodeImpl for SynchronizeNode {
    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Sync
    }

    fn uses_job_builder(&self) -> bool {
        true
    }

    fn allow_gpu_scope(&self) -> bool {
        false
    }

    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext,
        jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        rctx.queue_sync(self.sync)?;

        if rctx.is_consume_phase() {
            // frame_command_lists.submit(...) lives in the runtime layer.
        }

        Ok(())
    }
}
```

The exact `frame_command_lists.submit(...)` Rust API belongs to the command-list
runtime layer, but the semantic contract is locked: the sync node records
allocator `queue_sync` in both phases and, during consume, submits or
synchronizes frame command lists in dependency order.

#### Extended Inter-Command-List Sync

Under `GPUAPI_EXTENDED_SYNC`, RED also declares:

```cpp
class CRenderNode_InterCommandListSyncStartFrame;
class CRenderNode_SignalIntermediateSyncPoint;
class CRenderNode_WaitIntermediateSyncPoint;
```

These nodes advertise:

```cpp
GetCommandListUsage() -> Require
```

Their implementations are backend consume-phase operations:

```cpp
if (!rctx.IsConsumePhase())
    return;

GpuApi::InterCommandListSyncNewFrame();
```

```cpp
if (!rctx.IsConsumePhase())
    return;

GpuApi::SignalInterCommandListSyncPoint(m_syncPoint);
```

```cpp
if (rctx.IsConsumePhase())
{
    GpuApi::WaitOnInterCommandListSyncPoint(m_syncPoint);
}

m_epilogueFunc(rctx);
```

These are GPU-to-GPU or command-list-to-command-list synchronization operations.
They are not the same thing as allocator `queue_sync`.

#### Keep The Sync Layers Separate

Allocator queue sync is planning data:

```text
PreConsume
  record queue_sync in the allocator request stream

Consume
  replay queue_sync in the allocator request stream
```

The allocator can use it to reason about resource lifetimes, reuse boundaries,
queue/fork/join behavior, and request-stream validation.

Backend inter-command-list sync is execution data:

```text
Consume
  signal/wait GPU backend sync point
```

It affects real GPU command execution, but it is not automatically an allocator
lifetime event unless the node explicitly records an allocator request.

`CRenderNode_Synchronize` does both. The extended inter-command-list sync nodes
shown here only show backend sync behavior.

#### wgpu Interpretation

In safe wgpu, the normal sync model is ordered queue submission, not explicit
RED-style signal/wait sync points.

wgpu/HAL guarantees for a single queue:

- command buffers submitted to one queue execute in submit order
- results from earlier command buffers are visible to later command buffers
- ordered calls to `submit` on the same queue complete in order

wgpu also inserts resource state transitions/barriers as needed between command
buffers. It may insert generated command buffers containing barrier commands
between user command buffers to transition resources to the correct state.

So LEET is responsible for semantic ordering:

```text
build GPU dependency graph
detect cycles
flatten GPU dependencies into deterministic order
record command buffers for graph nodes
submit command buffers in graph GPU order
keep resources alive until submitted work is done
avoid reusing pooled resources while submitted work still needs them
```

wgpu is responsible for backend mechanics for that order:

```text
execute same-queue submissions in order
make earlier command-buffer results visible to later command buffers
insert backend resource transitions/barriers
```

Example:

```rust
let a = encoder_a.finish(); // writes scene_color
let b = encoder_b.finish(); // reads scene_color, writes bloom
let c = encoder_c.finish(); // reads bloom, writes final_image

queue.submit([a, b, c]);
```

The graph must produce `[a, b, c]` in the correct GPU dependency order. wgpu will
respect the order given, but it will not infer LEET's render graph semantics if
LEET submits command buffers in the wrong order.

For V1, do not mirror RED's extended inter-command-list sync literally as a
default wgpu path. Keep a backend-sync abstraction available for future backend
or advanced cases, but treat ordered command-buffer submission as the normal
wgpu synchronization primitive.

### Pass 5: Renderer-Feature Node Patterns

`renderGraphNodes.h` contains many concrete renderer-feature nodes. LEET should
not port these wholesale during graph-core work, but their declarations reveal
important node-authoring patterns the core must support.

The header survey shows roughly:

```text
140 nodes with RenderNodeCommandListUsage::Require
19 nodes with RenderNodeCommandListUsage::None
6 nodes with GetJobBuilderUsage() == true
12 uses of RenderTargetBinder
5 uses of GlobalBindingsMod
11 SimpleNoGPUScope nodes
```

These counts are approximate inventory data, but the pattern is clear: most
visual render nodes require an existing command list, while a smaller set are
control/declaration/job/sync-style nodes.

#### Command-List Require Is The Common Visual Path

Most RED feature nodes advertise:

```cpp
virtual RenderNodeCommandListUsage GetCommandListUsage() const override
{
    return RenderNodeCommandListUsage::Require;
}
```

Meaning:

```text
the node expects the graph/runtime wrapper to provide command-list state
```

LEET should treat `RenderNodeCommandListUsage::Require` as the normal path for
visual render nodes. Nodes should not individually invent command-list ownership
or submission behavior when they only need to record work into the active graph
execution context.

#### Render Target Setup Is Graph Work

RED has explicit render-target setup/end nodes:

```cpp
CRenderNode_SetRenderTargetsGBuffer
CRenderNode_EndRenderTargetsGBuffer
CRenderNode_SetRenderTargetsMain
CRenderNode_EndRenderTargetsMain
CRenderNode_SetRenderTargetsDebug
CRenderNode_EndRenderTargetsDebug
```

Several mark themselves with:

```cpp
CRenderNodeBase(rend::node::RenderTargetBinder())
```

This means render target binding/setup is graph-visible work, not hidden outside
the graph. LEET should allow render target setup/end to be ordinary
`RenderNodeImpl` nodes with:

```rust
fn binds_render_targets(&self) -> bool {
    true
}
```

The exact wgpu render-pass API will differ from RED's command-list API, but the
graph concept remains useful: render target/pass boundary setup can be modeled
as explicit graph nodes.

#### Render Target Setup Flags

RED uses `targetMod` flags to configure render-target setup nodes:

```cpp
ClearColor
ClearDepth
WriteDepth_NO
WriteDepth_YES
VelocityBuffer_NO
VelocityBuffer_YES
PostAA_NO
PostAA_YES
```

Constructors validate allowed masks:

```cpp
RED_ASSERT(
    (rtMods & ~targetMod::MASK_Expected_Main) == targetMod::MASK_None,
    "Unexpected targetMod for Main");
```

LEET should mirror the pattern, not necessarily RED's exact flags:

```rust
bitflags::bitflags! {
    pub struct RenderTargetSetupFlags: u32 {
        const CLEAR_COLOR = 1 << 0;
        const CLEAR_DEPTH = 1 << 1;
        const WRITE_DEPTH = 1 << 2;
    }
}
```

Builder/constructor paths should validate incompatible or unsupported flag
combinations early.

#### Global Binding Mutation

Some RED nodes pass:

```cpp
rend::node::GlobalBindingsMod(...)
```

This marks global binding slots the node modifies. It is not a dependency edge,
but it affects process-wrapper cleanup/binding behavior. LEET should keep this
as node metadata:

```rust
fn global_binding_mod(&self) -> GlobalBindingMask {
    GlobalBindingMask::empty()
}
```

#### Job Builder Usage

Only a small set of nodes override:

```cpp
GetJobBuilderUsage() -> true
```

So the default remains:

```rust
fn uses_job_builder(&self) -> bool {
    false
}
```

But V1 must support `true`. RED command-list groups specifically batch and split
subnode execution around nodes that require a job builder.

#### CPU And Control Nodes

Some renderer/system nodes use:

```cpp
RenderNodeCommandListUsage::None
SimpleNoGPUScope
```

These are still normal graph nodes. They may do CPU work, lifecycle work,
readback/flush work, declaration work, or other control behavior. LEET must keep
the execution model broad enough for nodes that do not record GPU commands.

#### Core Capabilities Required By The Pattern Survey

LEET graph core must support these node-authoring patterns from V1:

- visual nodes requiring an existing command list
- declaration/control nodes with no command list
- sync nodes
- render-target/pass-boundary nodes
- global-binding-modifying nodes
- job-builder nodes
- nodes with wrapper GPU profiling scope disabled

The concrete RED feature nodes are examples of the node model. They should not
be treated as required LEET graph-core ports unless their implementation affects
graph execution, allocator request semantics, synchronization, command-list
behavior, or node authoring rules.

---

## 7. `renderGraphNodes.cpp` Inspection

`renderGraphNodes.cpp` contains the implementations for both graph-core/system
nodes and many renderer-feature nodes. LEET should inspect it by behavior slice,
not as a request to port the whole RED renderer.

### Pass 1: Lifecycle And System Node Implementations

The lifecycle/system nodes show an important rule:

```text
lifecycle nodes often run real side effects only in Consume,
but some lifecycle nodes still record allocator requests outside Consume.
```

So LEET must not treat lifecycle nodes as automatically consume-only.

#### StartRender

RED `CRenderNode_StartRender::Execute` starts real frame rendering only during
consume:

```cpp
if (rctx.IsConsumePhase())
{
    RED_ASSERT(builder, "Job builder expected");
    GetRenderer()->StartFrameRendering(rctx, *builder);
}
```

But it also declares frame resources outside that consume-only block:

```cpp
if (nullptr != frameInfo.m_depthResult)
{
    SRenderFlowTargetDesc desc = rctx.GetResolvedFinalTargetDesc();
    desc.m_format = GpuWrapApi::TEXFMT_R32_Float;
    rctx.RTSharedAlloc(RENDER_NAME_TAG("extractedDepthBuffer"), desc);
}
```

and for hit-proxy mode:

```cpp
SRenderFlowTargetDesc desc;
desc.m_type = FRT_Buffer;
desc.m_maxElemCount = desc.m_elemCount = ...;
desc.m_elemStride = sizeof(...);

rctx.RTSharedAlloc(RENDER_NAME_TAG("multiLayerSelectionUAV"), desc);
```

Core lesson:

```text
StartRender is both a lifecycle node and an allocator request-stream node.
```

LEET `StartRenderNode` should therefore execute in both preconsume and consume.
Consume-only side effects must be guarded by `is_consume_phase()`, but
declaration/resource requests must be replayed consistently in both phases.

#### EndRender

RED `CRenderNode_EndRender::Execute` is consume-only:

```cpp
if (rctx.IsConsumePhase())
{
    GetRenderer()->EndFrameRendering(rctx, rctx.GetFrame());
}
```

LEET can model this as a normal `RenderNodeImpl` whose meaningful side effects
happen only in consume.

#### Present

RED `CRenderNode_Present::Execute` is also consume-only:

```cpp
if (rctx.IsConsumePhase())
{
    if (viewport && info.m_present)
    {
        viewport->Present();
    }
}
```

This is a graph-ordered presentation/control node:

```text
command_list_usage = None
allow_gpu_scope = false
consume-only side effects
```

#### Texture And Buffer Grab Flushes

RED flushes texture and buffer grabs only during consume:

```cpp
if (rctx.IsConsumePhase())
{
    GpuApi::FlushTextureGrabs();
    // or GpuApi::TryFinalizeTextureGrabs();
}
```

```cpp
if (rctx.IsConsumePhase())
{
    GpuApi::FlushBufferGrabs();
}
```

These are readback/extraction/control nodes. They do not need command-list
rendering behavior, but their graph position matters.

LEET should support readback/finalize/flush nodes as ordinary `RenderNodeImpl`
implementations:

```text
command_list_usage = None
consume-only side effects
participates in graph ordering
```

#### CleanupBatchDataAllocator

RED cleanup:

```cpp
if (rctx.IsConsumePhase())
{
    GetRenderer()->GetBatchDataAllocator().Cleanup();
}
```

This reinforces the same pattern: subsystem cleanup can be represented as a
graph node if its ordering relative to other nodes matters.

#### EndFrame

RED `CRenderNode_EndFrame::Execute` transitions the resource allocator into
cleanup during consume:

```cpp
if (rctx.IsConsumePhase())
{
    rctx.GetResourceAllocator()->SetPhase(RFP_Cleanup);
    ...
    renderScene->OnEndFrame();
    CRenderNodeJob::SetJobsRenderFrame(nullptr);
}
```

This is graph-core relevant. It shows that allocator cleanup must happen after
the frame's graph work has reached its end-frame point.

LEET should not hide allocator phase ownership inside a normal node. `EndFrame`
remains a graph-visible lifecycle node for end-of-frame renderer work, but the
allocator transition to `Cleanup` is owned by the frame execution shell after
terminal graph-node completion. That makes the phase transition explicit and
ensures cleanup runs exactly once.

Locked V1 shape:

```text
terminal graph nodes complete
  -> frame execution epilogue
  -> resource_allocator.set_phase(Cleanup)
  -> frame/runtime end-of-frame cleanup
```

#### Core Rule From Lifecycle Implementations

LEET node implementations must be phase-aware:

```rust
fn execute(
    &self,
    rctx: &mut RenderNodeImplContext,
    jobs: &mut RenderJobBuilder,
) -> RenderGraphResult<()> {
    // allocator declarations and use requests may run in both phases

    if rctx.is_consume_phase() {
        // real backend/frame side effects
    }

    Ok(())
}
```

Do not infer behavior from command-list usage alone:

- `CommandListUsage::None` does not mean the node is skipped during preconsume
- lifecycle nodes may still emit allocator requests
- consume-only side effects must be explicitly guarded
- request-stream operations must replay consistently between preconsume and
  consume

### Pass 2: Synchronization Implementations

`renderGraphNodes.cpp` confirms that synchronization has several layers. LEET
should not collapse them into one concept.

#### Synchronize Node

RED `CRenderNode_Synchronize::Execute` records allocator queue-sync in every
phase:

```cpp
rctx.GetResourceAllocator()->RequestQueueSync(
    rctx.GetRenderFlowGroup(),
    m_sync);
```

Then, during consume only, it submits/synchronizes frame command lists:

```cpp
if (rctx.IsConsumePhase())
{
    RED_ASSERT(builder, "Job builder expected");
    RED_ASSERT(!m_name.Empty(),
        "Must provide a name when doing RenderSync::Flush");

    GetRenderer()->GetFrameCommandLists().Submit(
        m_name.AsChar(),
        rctx.GetRenderFlowGroup(),
        m_sync,
        *builder);
}
```

So this node combines:

```text
allocator queue_sync request
consume-time frame command-list submit/sync
```

LEET mirror:

```rust
impl RenderNodeImpl for SynchronizeNode {
    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Sync
    }

    fn allow_gpu_scope(&self) -> bool {
        false
    }

    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext,
        jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        rctx.queue_sync(self.sync)?;

        if rctx.is_consume_phase() {
            // Submit/synchronize frame command lists through the runtime.
        }

        Ok(())
    }
}
```

#### Factory Sync Submit

RED's factory macro creates a sync node and links previous GPU-work nodes before
it by CPU dependency:

```cpp
#define SYNC_SUBMIT(group, name, waitType) \
{ \
    auto ecl = ADD_NODE( \
        group, \
        name "_Flush", \
        CRenderNode_Synchronize, \
        waitType, \
        "Submit_" name); \
    factory.LinkCPUToPreviousGPU(ecl); \
}
```

LEET should expose this behavior explicitly:

```rust
let sync = factory.create_node(
    group,
    RenderNodeKind::Stage,
    RenderNodeSubtype::default(),
    SynchronizeNode { sync, name },
)?;

factory.link_cpu_from_earlier_gpu_work(sync)?;
```

This keeps the sync node and its dependency placement visible in authoring code.

#### Job Builder Nuance

`CRenderNode_Synchronize` requires a `job::Builder` during consume, but it does
not override:

```cpp
GetJobBuilderUsage() -> true
```

So RED distinguishes:

```text
node uses job builder for child/deferred job behavior
node needs a builder because sync submit requires one at runtime
```

LEET must not overload `uses_job_builder` to mean every node that ever needs a
runtime job handle. Keep `uses_job_builder` aligned with RED's
`GetJobBuilderUsage`: it describes nodes that affect job dispatch/epilogue
scheduling behavior.

Sync submit's builder requirement belongs to the process/runtime handling for
`RenderNodeCommandListUsage::Sync`.

#### Extended Inter-Command-List Sync

Under `GPUAPI_EXTENDED_SYNC`, RED implements:

```cpp
void CRenderNode_InterCommandListSyncStartFrame::Execute(...)
{
    if (!rctx.IsConsumePhase())
        return;

    GpuApi::InterCommandListSyncNewFrame();
}
```

```cpp
void CRenderNode_SignalIntermediateSyncPoint::Execute(...)
{
    if (!rctx.IsConsumePhase())
        return;

    GpuApi::SignalInterCommandListSyncPoint(m_syncPoint);
}
```

```cpp
void CRenderNode_WaitIntermediateSyncPoint::Execute(...)
{
    if (rctx.IsConsumePhase())
    {
        GpuApi::WaitOnInterCommandListSyncPoint(m_syncPoint);
    }

    m_epilogueFunc(rctx);
}
```

These are backend GPU/command-list synchronization nodes. They do not show
allocator `RequestQueueSync` behavior in the inspected implementation.

Important nuance: `WaitIntermediateSyncPoint` runs its `m_epilogueFunc(rctx)` in
all phases, even though the backend GPU wait happens only during consume. So
backend sync nodes may still have phase-independent request or epilogue behavior.

#### Sync Concepts To Keep Separate

LEET should model sync as separate concerns:

```text
allocator queue_sync request
  -> request-stream operation
  -> affects lifetime/order planning
  -> preconsume and consume

frame command-list submit/sync
  -> runtime command-list behavior
  -> consume only

backend GPU wait/signal
  -> optional backend-specific behavior
  -> consume-side GPU synchronization
```

`CRenderNode_Synchronize` combines allocator queue-sync and frame command-list
submit/sync. Extended inter-command-list sync is mostly backend wait/signal.

For LEET's normal wgpu path, ordered command-buffer submission remains the
primary synchronization primitive. Extended inter-command-list sync should stay
as an optional backend-specific abstraction, not as the default graph-core sync
model.

### Pass 3: Resource Declaration Implementations

The declaration-node implementations are large, so they were inspected in
subpasses.

#### Subpass 3.1: Common Shaded Resource Declarations

`CRenderNode_DeclareCommonResourceAllocs::Execute` is specific to shaded render
mode:

```cpp
const CRenderFrameInfo& info = rctx.GetFrameInfo();
const auto colorDesc = rctx.GetRegularPrecisionColorTargetDesc();

RED_ASSERT(
    info.GetRenderingMode() == RM_Shaded,
    "This node is intended to declare resources for shaded mode");
```

It declares resources conditionally based on frame state and feature flags:

```cpp
if (rctx.HasScene()) { ... }
if (rctx.Test(RFF_VelocityBuffer)) { ... }
if (rctx.Test(RFF_Hair)) { ... }
if (rctx.Test(RFF_ScreenSpaceReflections)) { ... }
if (rctx.Test(RFF_LocalShadows)) { ... }
if (rctx.Test(RFF_HUD)) { ... }
```

This makes the request stream mode-aware and feature-aware. The branches must be
deterministic between preconsume and consume.

Representative declared resources include:

```text
GBuffer:
  gbuffer0
  gbuffer1
  gbuffer2
  normals
  shadowmask
  velocityBuffer

Lighting/color:
  color
  lightBufferDiffuse
  lightBufferSpecular
  bufferReflection
  globalIllumination

Feature resources:
  hair_color
  hair_alpha
  hair_depths
  lightBlockers
  SSR_Result
  SSR_Fade
  localShadowsSharedDepth
  localShadowsSharedVSMIntermediate
  cascades
  cloudsShadow
  topDownCarProxiesDepth
  topDownCarProxiesDepthBackFace
  topDownRainDepth
  transparencyTXAAMask
  temporalInvalidationMask
  distortionMask
  glassBlurRadius
  distortionMaskTiles
  frostedGlassBlurMaskTiles

Frame/depth/composition:
  depthBuffer
  depthBufferWeaponCorrected
  depthBufferHologram
  rtSkinTranslucency
  composition
```

Do not treat this list as a LEET porting checklist. It is evidence of the
resource declaration pattern.

#### Descriptor Construction Pattern

RED uses common descriptor helpers:

```cpp
rctx.GetRegularPrecisionColorTargetDesc();
rctx.GetDepthBufferDesc();
rctx.GetVelocityBufferDesc();
rctx.GetColorTargetDesc(format);
rctx.GetCompositionDesc();
```

Then individual declarations customize descriptor fields:

```text
format
current width/height
max width/height
slice/layer count
samplable flag
unordered/storage capability
extra backend usage flags
optimal clear value
```

This reinforces the LEET descriptor design already locked in the allocator
document:

```text
current size and max size both matter
usage capability must be explicit
clear behavior and optimal clear value matter
texture and buffer resource kinds both exist
```

#### Do Not Declare Unused Resources

RED explicitly avoids declaring resources that cannot be used in the current
frame mode:

```cpp
// Some stuff is only needed if we're doing the full scene render.
// If we aren't, then don't even declare them, to make sure they aren't
// accidentally created.
```

LEET should preserve this policy:

```text
declare only resources reachable in the current graph/frame mode
```

Declaring unused resources is not harmless. It affects allocator request streams,
lifetime analysis, pool pressure, diagnostics, and can hide accidental resource
use.

#### Subsystem Declaration Hooks

The function ends with a subsystem hook:

```cpp
CParticleDPLTexture::DeclareFlowResources(rctx);
```

This shows RED allows subsystems to append resource declarations through the same
node-facing `rctx` request stream.

LEET may support the same pattern, but subsystem hooks must obey the same rules:

- use the node-facing context
- record declarations/use ranges into the allocator request stream
- make deterministic decisions between preconsume and consume
- avoid retrieving resolved GPU resources during preconsume

#### Declaration Design Rules

LEET V1 should preserve these rules:

- declaration nodes are normal `RenderNodeImpl` nodes
- declaration nodes may be large and mode-specific
- declaration nodes can have `RenderNodeCommandListUsage::None`
- descriptor helpers should be used for common target/depth/composition shapes
- feature flags and frame mode can control which resources are declared
- branch decisions that affect declarations must replay identically
- unused resources should not be declared
- subsystem declaration hooks are allowed only if they use the same request
  stream and phase rules
- declaration nodes may declare textures and buffers

#### Subpass 3.2: Mode-Specific Declaration Variants

RED has several declaration nodes for different graph/rendering modes:

```cpp
CRenderNode_DeclareCommonResourceAllocs_SafeMode
CRenderNode_DeclareCommonResourceAllocs_FinalOnly
CRenderNode_DeclareCommonResourceAllocs_HitProxy
CRenderNode_DeclareCommonResourceAllocs_GBufferOnly
```

These are not aliases for the shaded declaration node. Each one declares a
different resource set for a different graph mode.

##### Safe Mode

Safe mode asserts its intended rendering mode:

```cpp
RED_ASSERT(
    info.GetRenderingMode() == RM_SafeMode,
    "This node is intended to declare resources for shaded mode");
```

It declares a reduced subset compared to full shaded mode:

```text
gbuffer0
gbuffer1
gbuffer2
normals
color
topDownCarProxiesDepth
topDownCarProxiesDepthBackFace
topDownRainDepth
transparencyTXAAMask
temporalInvalidationMask
distortionMask
depthBuffer
depthBufferWeaponCorrected
depthBufferHologram
composition
```

It does not declare the full shaded lighting, SSR, hair, local-shadow, and other
feature-heavy resource set.

Design rule:

```text
smaller render mode -> smaller declaration stream
```

##### Final Only

Final-only declaration starts with only the main color target:

```cpp
SRenderFlowTargetDesc desc = rctx.GetRegularPrecisionColorTargetDesc();
rctx.RTAlloc(RENDER_NAME_TAG("color"), desc);
```

Then it declares a handful of common/final resources such as top-down depth maps,
TAA masks, distortion, and optional composition.

RED includes a useful warning:

```cpp
// TODO: Get rid of that here. Allocate it in some less "common" node.
```

LEET should follow the spirit of that TODO. If a resource is only needed by one
feature or subgraph, prefer a local declaration node over bloating a common
declaration node.

##### Hit Proxy

Hit-proxy declaration is intentionally small:

```text
hitProxyId
depthBuffer
optional color
```

The optional color target's format depends on frame purpose/display mode:

```text
FP_CascadesBaking -> R8G8B8A8
DebugHitProxies / Todvis preview -> R16G16B16A16_Float
```

Design rule:

```text
same logical resource name may have mode-specific descriptors
```

This is valid only when the modes are mutually exclusive for the active
frame/flow space. If two active declaration nodes declare the same tag in the
same flow space, descriptor validation must catch conflicts.

##### GBuffer Only

GBuffer-only declaration asserts both mode and scene presence:

```cpp
RED_ASSERT(info.GetRenderingMode() == RM_GBufferOnly);
RED_ASSERT(rctx.HasScene());
```

It declares only:

```text
gbuffer0
gbuffer1
gbuffer2
depthBuffer
```

It also intentionally differs from shaded mode. For example:

```text
GBufferOnly gbuffer0 -> R8G8B8A8_Unorm
Shaded gbuffer0      -> R10G10B10A2_Unorm
```

This reinforces the same mode-specific descriptor rule. Same tag names can have
different descriptors across mutually exclusive graph modes, but active
declarations in one request stream must not conflict.

##### Declaration Variant Rules

LEET V1 should preserve these rules:

- declaration nodes are mode-specific
- graph construction chooses the declaration node appropriate for the active
  rendering mode
- do not create one giant declaration node that declares every possible resource
- feature-specific resources should be declared close to the feature/subgraph
  that uses them when possible
- descriptor conflicts for the same tag and flow space are hard errors
- same tag names may have different descriptors in mutually exclusive graph
  modes
- declaration nodes may encode graph validity assumptions, such as "GBufferOnly
  requires a scene"

#### Subpass 3.3: Declaration API And Validation Rules

The declaration-node implementations require strict Rust API and validation
behavior.

##### Declaration Nodes Produce Allocator Requests

A declaration node is normal executable graph behavior:

```rust
impl RenderNodeImpl for DeclareCommonResourceAllocsNode {
    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::None
    }

    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext,
        jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        let color = rctx.rt_name_tag("color");
        let color_desc = rctx.regular_precision_color_desc();

        rctx.declare_resource(
            color,
            FrameResourceDesc::Texture(color_desc),
        )?;

        Ok(())
    }
}
```

It does not need special graph storage. It records allocator requests through
the node-facing context.

##### Duplicate Declaration Validation

If the same tag is declared twice in the same flow space, validation must be
strict:

```text
same tag
same flow space
same active request stream
  -> descriptor must match exactly unless a specific API says otherwise
```

Default duplicate declaration behavior should use exact descriptor equality.
Mode-specific descriptor differences are allowed only when the modes are
mutually exclusive and therefore do not appear in the same active request
stream.

Example diagnostic shape:

```text
ResourceDeclarationConflict
  tag: "color"
  flow_space: Camera(0)
  first_node: "DeclareCommonResourceAllocs"
  second_node: "DeclareFinalOnlyResourceAllocs"
  first_request_index: 12
  second_request_index: 47
  first_desc: Texture(R16G16B16A16_FLOAT, 1920x1080)
  second_desc: Texture(R8G8B8A8_UNORM, 1920x1080)
```

This must fail loudly. Silent "last declaration wins" behavior would make the
allocator unsafe.

##### Decisions For Request-Affecting Branches

Declaration nodes often branch on frame state:

```rust
if rctx.test(RenderFeature::Hair) {
    declare_hair_resources(rctx)?;
}
```

If a branch changes the allocator request stream, the branch condition should be
recorded as a decision:

```rust
let hair_enabled = rctx.decision(
    "hair_enabled",
    rctx.test(RenderFeature::Hair),
)?;

if hair_enabled {
    declare_hair_resources(rctx)?;
}
```

This protects request-stream matching:

```text
PreConsume: hair true  -> hair declarations exist
Consume:    hair false -> hair declarations missing
```

That divergence is invalid and must be caught.

##### Descriptor Helper APIs

Declaration nodes should use shared descriptor helper APIs instead of
hand-rolling common target descriptions everywhere:

```rust
rctx.regular_precision_color_desc()
rctx.depth_buffer_desc()
rctx.velocity_buffer_desc()
rctx.color_target_desc(format)
rctx.composition_desc()
```

These helpers should construct `FrameResourceDesc::Texture(...)` or return the
typed descriptor that is then wrapped by the caller. The important part is that
common resource shapes are centralized so duplicate declarations do not drift.

##### Common Versus Local Declaration Nodes

Use common declaration nodes only for resources truly shared across a graph mode:

```text
color
depthBuffer
gbuffer0
gbuffer1
gbuffer2
shadowmask
```

Prefer local or subsystem-owned declaration nodes for feature-specific resources:

```text
SSR_Result
hair_depths
distortionMaskTiles
particle DPL resources
```

Subsystem hooks are allowed:

```rust
particle_dpl.declare_flow_resources(rctx)?;
```

but they must use the same `RenderNodeImplContext` and allocator request stream.

##### Texture And Buffer Declarations

Declaration nodes must support both resource kinds from V1:

```rust
FrameResourceDesc::Texture(...)
FrameResourceDesc::Buffer(...)
```

The common declaration paths are texture-heavy, but other lifecycle/declaration
nodes can declare buffers, such as RED's selection/multi-layer buffer path in
`StartRender`.

##### Required Declaration Diagnostics

Declaration validation errors should include:

- logical tag
- flow space
- declaring node names
- request indices
- resource kind
- first and second descriptors
- comparison mode that failed
- current allocator phase

These diagnostics are core infrastructure. Large render graphs are otherwise
too difficult to debug.

##### Locked Rule

```text
Declaration nodes are normal RenderNodeImpl nodes.
Their declarations are strict allocator requests.
Branches that affect requests must replay identically.
Descriptor conflicts fail loudly.
```

### Pass 4: Render Target And Binding Implementations

The render-target node declarations live in `renderGraphNodes.h`, but the heavy
implementations are split into:

```text
renderNode_RenderTargets.cpp
```

This split is useful for LEET too. Core node declarations and authoring helpers
can stay near the graph model, while large target/binding implementations can
live in dedicated node files.

#### Render Target Nodes Are Graph Semantics

RED render-target nodes are not just backend helpers. They define allocator
lifetimes and GPU target binding at the same graph location.

The common pattern is:

```cpp
rctx.RTUseBegin(...);

if (rctx.IsConsumePhase())
{
    auto tex = rctx.RTGet<GpuApi::TextureRef>(...);
    rctx.BindPSO(ColorTarget(...), DepthTarget(...));
}
```

and the matching end node:

```cpp
rctx.RTUseEnd(...);

if (rctx.IsConsumePhase())
{
    GpuApi::BindTextures(..., nullptr, ...);
}
```

So a render-target setup/end pair is dual-purpose:

```text
PreConsume
  records resource use ranges

Consume
  replays the same resource use ranges
  retrieves resolved resources
  performs backend binding/unbinding work
```

LEET should preserve this. Render-target setup/end nodes remain normal
`RenderNodeImpl` nodes. They are not erased into draw calls or hidden inside the
allocator.

#### GBuffer Target Setup

RED `CRenderNode_SetRenderTargetsGBuffer::Execute` writes the GBuffer targets
and either reads or writes depth depending on target flags:

```cpp
auto rtGBuffer0 = rctx.RT<RFUF_Write | RFUF_NoDiscard>(
    RENDER_NAME_TAG("gbuffer0"));
auto rtGBuffer1 = rctx.RT<RFUF_Write | RFUF_NoDiscard>(
    RENDER_NAME_TAG("gbuffer1"));
auto rtGBuffer2 = rctx.RT<RFUF_Write | RFUF_NoDiscard>(
    RENDER_NAME_TAG("gbuffer2"));

auto rtDepth = depthWriteable
    ? rctx.RT<RFUF_Write>(RENDER_NAME_TAG("depthBuffer"))
    : rctx.RT<RFUF_Read>(RENDER_NAME_TAG("depthBuffer"));
```

During consume it clears when requested and binds the actual targets:

```cpp
if (rctx.IsConsumePhase())
{
    if (clearGBuffer)
    {
        ClearGBuffer(rctx, rtGBuffer0, rtGBuffer1, rtGBuffer2,
            GpuApi::TextureRef::Null(), rtDepth);
    }

    rctx.BindPSO(
        ColorTarget(0, isDepthOnly ? GpuApi::TextureRef::Null() : rtGBuffer0),
        ColorTarget(1, isDepthOnly ? GpuApi::TextureRef::Null() : rtGBuffer1),
        ColorTarget(2, isDepthOnly ? GpuApi::TextureRef::Null() : rtGBuffer2),
        DepthTarget(rtDepth, -1, !depthWriteable));
}
```

The matching end node closes the same ranges:

```cpp
rctx.RTUseEnd(RENDER_NAME_TAG("gbuffer0"));
rctx.RTUseEnd(RENDER_NAME_TAG("gbuffer1"));
rctx.RTUseEnd(RENDER_NAME_TAG("gbuffer2"));
rctx.RTUseEnd(RENDER_NAME_TAG("depthBuffer"));
```

Rust mirror:

```rust
impl RenderNodeImpl for SetRenderTargetsGBufferNode {
    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Require
    }

    fn binds_render_targets(&self) -> bool {
        true
    }

    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext,
        jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        let gbuffer0 = rctx.rt_name_tag("gbuffer0");
        let gbuffer1 = rctx.rt_name_tag("gbuffer1");
        let gbuffer2 = rctx.rt_name_tag("gbuffer2");
        let depth = rctx.rt_name_tag("depthBuffer");

        rctx.use_begin(gbuffer0, ResourceUsage::WRITE | ResourceUsage::NO_DISCARD)?;
        rctx.use_begin(gbuffer1, ResourceUsage::WRITE | ResourceUsage::NO_DISCARD)?;
        rctx.use_begin(gbuffer2, ResourceUsage::WRITE | ResourceUsage::NO_DISCARD)?;
        rctx.use_begin(depth, self.depth_usage())?;

        if rctx.is_consume_phase() {
            let gbuffer0 = rctx.get_texture(gbuffer0)?;
            let gbuffer1 = rctx.get_texture(gbuffer1)?;
            let gbuffer2 = rctx.get_texture(gbuffer2)?;
            let depth = rctx.get_texture(depth)?;

            rctx.bind_render_targets(RenderTargetSetup::gbuffer(
                gbuffer0,
                gbuffer1,
                gbuffer2,
                depth,
                self.flags,
            ))?;
        }

        Ok(())
    }
}
```

#### Main Target Setup

RED `CRenderNode_SetRenderTargetsMain` is more conditional. It always uses the
main color and depth resources, then optionally uses feedback or auxiliary
resources:

```text
color                       write
depthBuffer                 write/read depending on flags
depthBufferWeaponCorrected  optional write
colorCopy / colorCopyPostAA optional read
gbuffer2                    optional read
transparencyTXAAMask        optional write
velocityBuffer              optional write
depthBufferHologram         optional read
dplTexture                  optional write/read binding path
upscaledDepthBuffer         optional declare/write for PostAA
```

The end node must mirror these options exactly and call `RTUseEnd` for every
resource whose use was begun.

LEET rule:

```text
target setup flags are part of request-stream behavior
```

If a flag changes which resources are used, that flag must be deterministic
between preconsume and consume. If the flag comes from runtime state that may
diverge, it needs a recorded decision.

#### Binding Feedback Textures

RED target setup nodes sometimes bind a resource as both graph resource and
shader feedback input. Example: read-only depth during GBuffer or main passes:

```cpp
GpuApi::BindTextures(
    rend::Binding::Textures::SCENE_DEPTH,
    1,
    &texDepth,
    GpuWrapApi::PixelShader);

GpuApi::BindTextureStencil(
    rend::Binding::Textures::SCENE_STENCIL,
    texDepth,
    GpuWrapApi::PixelShader);
```

The resource use is still recorded through the allocator:

```cpp
rctx.RTUseBegin(rtDepth, RFUF_Read);
```

LEET should keep these concepts separate:

```text
resource use range
  -> allocator/lifetime correctness

shader binding
  -> consume-side backend binding state
```

The same tag may participate in both.

#### Global Constants Binding

`CRenderNode_BindGlobalConstants` also follows the phase split.

It records read use for top-down depth resources in both phases when relevant:

```cpp
rctx.RTUseBegin<RFUF_Read>(RENDER_NAME_TAG("topDownCarProxiesDepth"));
rctx.RTUseBegin<RFUF_Read>(RENDER_NAME_TAG("topDownCarProxiesDepthBackFace"));
rctx.RTUseBegin<RFUF_Read>(RENDER_NAME_TAG("topDownRainDepth"));
```

During consume it retrieves and binds actual GPU textures:

```cpp
const GpuApi::TextureRef tex =
    rctx.RTGet<GpuApi::TextureRef>(RENDER_NAME_TAG("topDownCarProxiesDepth"));

GpuApi::BindTexture(rend::Binding::Textures::TOP_DOWN_CAR_MAP, tex, shaderTarget);
```

`CRenderNode_UnbindGlobalConstants` closes the same resource uses:

```cpp
rctx.RTUseEnd(RENDER_NAME_TAG("topDownCarProxiesDepth"));
rctx.RTUseEnd(RENDER_NAME_TAG("topDownCarProxiesDepthBackFace"));
rctx.RTUseEnd(RENDER_NAME_TAG("topDownRainDepth"));
```

LEET global binding nodes should therefore be able to record resource reads
without being render-target nodes.

#### Context Binding Helpers

RED `SRenderNodeImplContext::BindPSO` builds a render target setup object from
small typed arguments:

```cpp
rctx.BindPSO(
    ColorTarget(0, texColor),
    DepthTarget(texDepth, -1, readOnly));
```

Internally it lowers to:

```cpp
GpuApi::SetupRenderTargets(pso.renderTargets);
m_psoBound = true;
```

LEET should not copy the old mutable GPU API literally. With wgpu, render pass
attachments are selected when a render pass begins. The equivalent should be a
render-pass/target setup description owned by the command-list/pass layer:

```rust
pub enum RenderTargetAttachment {
    Color { slot: u8, texture: FrameTextureHandle, load: LoadOp },
    Depth { texture: FrameTextureHandle, read_only: bool, load: LoadOp },
}

pub struct RenderTargetSetup {
    pub colors: SmallVec<[RenderTargetAttachment; 4]>,
    pub depth: Option<RenderTargetAttachment>,
}
```

The graph-visible node model remains RED-like, but the backend lowering becomes
wgpu-friendly.

#### Wgpu Pass Boundary Rule

RED can call `GpuApi::SetupRenderTargets` as mutable command-list state. wgpu
does not work that way: attachments belong to a `RenderPass`.

Therefore LEET should preserve RED's graph shape:

```text
SetRenderTargets*
  RenderElements / draw nodes
EndRenderTargets*
```

but lower it as:

```text
setup node
  -> records target setup intent

draw nodes
  -> record commands into the active render pass/scope

end node
  -> closes the target/resource use scope
```

The command-list/pass runtime may batch that region into one wgpu render pass.
This is an implementation adaptation, not a semantic simplification.

#### Process Epilogue And RenderTargetBinder

RED `CRenderNodeBase::ProcessEpilogue` runs after command-list nodes and calls:

```cpp
rctx.Unbind(!m_rtBinder);
```

Nodes constructed with:

```cpp
CRenderNodeBase(rend::node::RenderTargetBinder())
```

set `m_rtBinder`, which changes how aggressively the epilogue unbinds render
targets.

LEET should preserve the metadata idea:

```rust
fn binds_render_targets(&self) -> bool {
    false
}
```

Render-target setup nodes return `true`. The runtime uses that information for
cleanup/restoration policy. The exact cleanup implementation must be adapted to
wgpu, where pass scopes and bind groups replace RED's mutable global GPU state.

#### Draw Calls Stay In Work Nodes

The render-target setup nodes do not issue the main draw calls. In RED's GBuffer
graph authoring, the pattern is:

```cpp
ADD_SUBNODE("BindGlobalConstants", CRenderNode_BindGlobalConstants);
ADD_SUBNODE("SetRenderToGbuff0", CRenderNode_SetRenderTargetsGBuffer, rt_GBuffer_NoClear);
ADD_SUBNODE("RenderElements", CRenderNode_RenderElements, ..., REBatch(...));
ADD_SUBNODE("EndRenderToGbuff0", CRenderNode_EndRenderTargetsGBuffer);
ADD_SUBNODE("UnbindGlobalConstants", CRenderNode_UnbindGlobalConstants);
```

`CRenderNode_RenderElements::Execute` then chooses render-stage batches and
hands work to the geometry batcher:

```cpp
GetRenderer()->GetGeometryBatcher()->RenderGeometry(renderCtx, bucket);
```

So LEET should keep this separation:

```text
target setup node
  -> resource use + render pass attachment intent

draw/work node
  -> actual draw submission/recording

target end node
  -> resource use closure + feedback unbind/cleanup
```

#### Locked Rule

```text
Render-target binding is graph work.
It defines resource lifetimes and GPU pass boundaries.
LEET must keep setup/end nodes visible in the graph, while lowering them through
a wgpu-compatible render-pass model.
```

### Pass 5: Job Builder And Parallel Execution Patterns

RED has two different parallel layers:

```text
parallel allocator request recording
parallel consume-time render node execution
```

LEET must not collapse these into one concept.

#### PreConsume Uses Parallel Request Recording

During frame execution, RED enters preconsume and runs the graph through
`ExecuteParallel`:

```cpp
m_renderFlowAllocator->SetPhase(RFP_PreConsume);
graph->ExecuteParallel(rctx, renderFrameContext.m_builder);
```

`CRenderNodeGraph::ExecuteParallel` batches graph nodes and processes them on
jobs:

```cpp
builder.DispatchParallelForJob<job::Fence::None>(
    "FlowAllocator_PreConsume_Batch",
    { nodeBucketCount },
    [this, rctx, orderedNodes, nodeBucketSize](Uint32 bucket_i,
        const job::RunContext& context)
    {
        SRenderNodeImplContext nodeCtx = rctx;
        nodeCtx.ResetNodeData();

        ...

        job::Builder builder{ context };
        impl->Process(nodeCtx, &builder);
    });
```

This is not GPU rendering. It is parallel construction of allocator request
streams.

LEET should preserve the same high-level flow:

```text
PreConsume
  execute graph nodes in parallel where legal
  record declaration/use/free/swap/decision requests

Resolve
  validate request streams
  compute lifetimes
  assign frame resources

Consume
  execute graph nodes for real work
```

#### Deterministic Request Ordering Is Mandatory

Parallel preconsume must not let thread scheduling decide allocator request
order.

RED solves this through per-flow-group request streams and deterministic
request-time encoding. It also spreads nodes through quasi-random buckets for
load balancing, but the allocator still needs stable logical ordering.

LEET should use deterministic request identity from graph structure, such as:

```text
flow group
node id / graph order
per-node local request index
```

or:

```text
per-node request buffers
deterministic merge by graph order and flow group
```

The locked rule:

```text
parallel request recording is allowed,
but the final request stream must be deterministic.
```

#### Consume Uses A Job Graph

RED builds consume-time render node jobs separately from preconsume. The helper
`RunRenderNodeJobs` creates one runtime job node per graph node:

```cpp
red::DynArray<RenderJobNode> nodes;
nodes.Resize(numNodes);
```

CPU dependencies are linked into job counters:

```cpp
nodes[childIdx].incomingDependenciesCounter +=
    nodes[parentIdx].nodeCounter;
```

Then each node job runs when its incoming counter reaches zero:

```cpp
job::RunJob(jobDecl, waitForZeroCounter, accumulateCounter);
```

So CPU dependencies control CPU job readiness. GPU dependencies still matter for
command-list ordering, queue sync, resource lifetimes, and submission order, but
CPU dependencies are what keep CPU-side node execution from running too early.

LEET consume execution should therefore be dependency-counter based from V1:

```text
build runtime node jobs
link CPU dependencies to readiness counters
run ready nodes in parallel
preserve GPU dependency order for command submission and resource correctness
```

#### Process Is The Runtime Wrapper

RED does not call node `Execute` directly from graph execution. It calls:

```cpp
impl->Process(nodeContext, &builder);
```

`CRenderNodeBase::Process` owns command-list setup, profiler scope, global
binding preparation, per-node bind tracking, node execution, and epilogue.

Relevant shape:

```cpp
const Bool consumePhase = rctx.IsConsumePhase();
const RenderNodeCommandListUsage clUsage = GetCommandListUsage();
const Bool useCommandList = consumePhase && HasCommandList(clUsage);

if (consumePhase && clUsage == RenderNodeCommandListUsage::Own)
{
    GpuApi::CommandListRef commandList = CreateCommandList();
    rctx.SetCommandList(commandList);
}

if (useCommandList)
{
    GpuApi::BindCommandList(rctx.GetCommandList());
    BeginProfilerBlock();
}

rctx.BeginNewNode();
Execute(rctx, builder);

if (useCommandList)
{
    GpuApi::BindCommandList({});
    ProcessEpilogue(rctx);
}
```

LEET should mirror this split:

```rust
fn process_node(
    node: &dyn RenderNodeImpl,
    rctx: &mut RenderNodeImplContext,
    jobs: &mut RenderJobBuilder,
) -> RenderGraphResult<()> {
    // runtime-owned setup
    // command encoder/pass context
    // profiler/debug scope
    // per-node binding tracker reset

    node.execute(rctx, jobs)?;

    // runtime-owned epilogue
    // binding/pass cleanup
    // global binding restoration
    Ok(())
}
```

`execute` is node-authored behavior. `process_node` is graph runtime behavior.

#### ProcessEpilogue

RED `ProcessEpilogue` runs only during consume for command-list nodes:

```cpp
RED_ASSERT(rctx.IsConsumePhase());
RED_ASSERT(clUsage != RenderNodeCommandListUsage::None);
RED_ASSERT(rctx.HasCommandList());

GpuApi::BindCommandList(commandList);
rctx.Unbind(!m_rtBinder);
...
EndProfilerBlock();
GpuApi::BindCommandList({});
```

It also restores any global binding slots declared by `GlobalBindingsMod`.

LEET should preserve the epilogue concept, but implement it with wgpu-shaped
state:

```text
close or validate active pass scope
clear per-node bind tracking
restore global binding metadata
finish profiler/debug scope
release command encoder/pass references as needed
```

The exact backend calls will differ from RED, but the wrapper responsibility is
the same.

#### CommandListUsage

RED command-list usage values:

```cpp
enum class RenderNodeCommandListUsage
{
    None,
    Require,
    Own,
    Sync,
};
```

Meaning for LEET:

```text
None
  node does not record command-list/render-pass work

Require
  node expects a command-list/pass context supplied by a parent/group/runtime

Own
  node creates/owns a command-list group or command encoder scope

Sync
  node does not record ordinary commands, but performs graph/runtime sync
```

`HasCommandList` in RED returns false for `None` and `Sync`:

```cpp
return clUsage != RenderNodeCommandListUsage::None
    && clUsage != RenderNodeCommandListUsage::Sync;
```

LEET should keep that distinction. Sync nodes are graph/runtime synchronization
nodes, not ordinary command-list recording nodes.

#### Command-List Group Node

RED's factory creates a special command-list group node:

```cpp
class CRenderNodeCommandListGroup final : public CRenderNodeBase
{
    RenderNodeCommandListUsage GetCommandListUsage() const override
    {
        return RenderNodeCommandListUsage::Own;
    }

    GpuApi::CommandListRef CreateCommandList() const override
    {
        return GpuApi::CreateCommandList(m_commandListType, m_name.GetHash());
    }
};
```

It owns subnodes:

```cpp
void AddSubnode(red::UniquePtr<CRenderNodeBase>&& subnode);
```

During execution it records queue scope into the allocator:

```cpp
rctx.GetResourceAllocator()->RequestBeginQueue(
    renderQueueType,
    rctx.GetRenderFlowGroup());
```

and eventually:

```cpp
rctx.GetResourceAllocator()->RequestEndQueue(
    rctx.GetRenderFlowGroup());
```

During preconsume, it processes subnodes directly so their allocator requests
are recorded:

```cpp
for (const red::SharedPtr<CRenderNodeBase>& subnode : m_subnodes)
{
    subnode->Process(rctx, builder);
}
```

During consume, it may batch subnodes into child jobs depending on
`GetJobBuilderUsage`.

LEET mirror:

```text
CommandListGroupNode
  command_list_usage = Own
  owns command encoder / command-list scope
  owns ordered subnodes
  records begin_queue/end_queue around subnode work
  preconsume processes subnodes for request recording
  consume records subnodes into backend command/pass scopes
```

#### JobBuilderUsage Has Narrow Meaning

RED's default is:

```cpp
virtual Bool GetJobBuilderUsage() const { return false; }
```

Some nodes override it to return true, including:

```text
StartRender
AccelerationStructureUpdateStatic
AccelerationStructureUpdateDynamic
AccelerationStructureUpdateEpilogue
RenderDistantShadowsCommon
SimulateOffScreenCPUParticles
CommandListGroupNode when any child uses a job builder
```

This flag does not simply mean "the node receives a job builder". All node
`Execute` functions receive:

```cpp
job::Builder* builder
```

Instead, it means the node may dispatch child jobs or needs epilogue scheduling
through the job system.

`CRenderNode_Synchronize` is the important counterexample. It requires a builder
during consume to submit/synchronize command lists, but it does not override
`GetJobBuilderUsage`.

LEET should not use one vague flag like:

```rust
fn needs_jobs(&self) -> bool
```

Use narrower concepts:

```rust
fn uses_child_jobs(&self) -> bool {
    false
}

fn command_list_usage(&self) -> RenderNodeCommandListUsage;
```

Sync-node runtime requirements should be handled by `CommandListUsage::Sync`
and the graph runtime, not by pretending sync nodes are child-job nodes.

#### Child Jobs Inside Nodes

RED feature nodes such as culling dispatch child jobs from their own `Execute`
body:

```cpp
builder->DispatchJob<job::Fence::None>(
    "DoCull_MainScene",
    [rctx](const job::RunContext& context)
    {
        job::Builder depBuilder{ context };
        rctx.GetCollector()->DoCull_MainScene(rctx, depBuilder);
    });
```

They may then explicitly fence those child jobs:

```cpp
if (needExplicitFence)
{
    builder->DispatchFenceExplicitly();
}
```

This means graph-level dependencies are not the only source of parallel work.
Nodes themselves can contain internal job trees.

LEET should allow this from V1 through the node execution API:

```rust
fn execute(
    &self,
    rctx: &mut RenderNodeImplContext,
    jobs: &mut RenderJobBuilder,
) -> RenderGraphResult<()>;
```

But any node that dispatches child jobs must declare it through
`uses_child_jobs`, so the runtime can schedule epilogues and command/pass cleanup
correctly.

#### Locked Rule

```text
Graph execution is parallel from V1.
PreConsume may record requests in parallel, but merges them deterministically.
Consume runs graph nodes through dependency-counter jobs.
Node execute bodies are wrapped by process_node runtime behavior.
uses_child_jobs is not the same thing as command_list_usage or sync behavior.
```

### Pass 6: Feature-Node Survey For Core Requirements

Feature node files should not be treated as a request to port RED's renderer
effects immediately. They are valuable because they show how real graph nodes
use the graph/runtime contract.

Representative files inspected:

```text
renderNode_Lighting.cpp
renderNode_AntyAliasing.cpp
renderNode_BloomAndTonemapping.cpp
renderNode_Composition.cpp
renderNode_Finalize.cpp
renderNode_Shadows.cpp
renderNode_RayTracing.cpp
```

The findings below are graph-core requirements, not feature implementation
requirements.

#### Temporary Resources Are First-Class

`RTTempAlloc` appears throughout post-processing, composition, lighting, ray
tracing, depth of field, motion blur, gameplay FX, SSAO, clouds, wind impulse,
and debug paths.

Typical RED pattern:

```cpp
auto rtColor = rctx.RT<RFUF_Read>(RENDER_NAME_TAG("color"));
auto rtTarget = rctx.RTTempAlloc<RFUF_Write>(
    RENDER_NAME_TAG("FXAA_color"),
    rtColor);
```

Temporary allocation means:

```text
node-local or feature-local scratch
allocator-managed
descriptor-backed
lifetime-tracked
eligible for reuse according to allocator rules
```

It does not mean "fake", "simple", or "free immediately after call".

LEET V1 must support:

```rust
let color = rctx.rt_name_tag("color");
let temp = rctx.temp_resource_tag("FXAA_color");

rctx.declare_like(temp, color)?;
rctx.use_begin(color, ResourceUsage::READ)?;
rctx.use_begin(temp, ResourceUsage::WRITE)?;
```

or the equivalent convenience API, as long as it records the same explicit
allocator requests.

#### Logical Swaps Are Central

Post-process chains frequently write into a temporary target and then swap it
into a public logical output.

RED FXAA:

```cpp
auto rtColor = rctx.RT<RFUF_Read>(RENDER_NAME_TAG("color"));
auto rtTarget = rctx.RTTempAlloc<RFUF_Write>(
    RENDER_NAME_TAG("FXAA_color"),
    rtColor);

...

rctx.RTSwap(rtColor, rtTarget);
```

Similar swap patterns appear in:

```text
TXAA
DLSS/upscale
tonemapping
composition
finalize
motion blur
depth of field
gameplay FX
screen-space rain/underwater
volumetric fog
debug post effects
```

So `swap` is not a rare allocator trick. It is how post-processing moves the
logical meaning of a resource without copying back into the original texture.

LEET V1 requirement:

```rust
rctx.swap_resources(color, fxaa_target)?;
```

The allocator must model:

```text
tag A points to backing X before swap
tag B points to backing Y before swap
tag A points to backing Y after swap
tag B points to backing X after swap
```

Resolved resource getters must therefore be request-time aware. A tag lookup is
not globally constant for the whole frame.

#### Feature-Local Declarations

RED lighting declares temporary classification resources inside the lighting
node, not in one giant common declaration node:

```cpp
auto rtMaterialClass = rctx.RTAlloc<RFUF_Write>(
    RENDER_NAME_TAG("materialClassification"),
    descMaterialClass);

auto bufTileCount = rctx.RTAlloc<RFUF_Write>(
    RENDER_NAME_TAG("materialClassificationTileCount"),
    descTileCount);

auto bufTileList = rctx.RTAlloc<RFUF_Write>(
    RENDER_NAME_TAG("materialClassificationTileList"),
    descTileList);
```

This reinforces the declaration-node rules:

```text
common resources belong in common declaration nodes
feature-local scratch belongs near the feature node/subgraph
```

LEET should not force all resources into a monolithic upfront declaration
system. Nodes and subsystem helpers may declare resources locally as part of the
same request stream.

#### Transient Buffers Are Real Frame Resources

The lighting material-classification path declares transient buffers:

```cpp
descTileCount.m_type = FRT_Buffer;
descTileCount.m_category = GpuWrapApi::BCC_IndirectUAV;
auto bufTileCount = rctx.RTAlloc<RFUF_Write>(
    RENDER_NAME_TAG("materialClassificationTileCount"),
    descTileCount);
```

```cpp
descTileList.m_type = FRT_Buffer;
descTileList.m_category = GpuWrapApi::BCC_StructuredUAV;
auto bufTileList = rctx.RTAlloc<RFUF_Write>(
    RENDER_NAME_TAG("materialClassificationTileList"),
    descTileList);
```

During consume:

```cpp
rctx.BindBufferUAV<0>(bufTileCount);
rctx.BindBufferUAV<1>(bufTileList);
```

LEET must keep `FrameResourceDesc::Buffer` in V1. Buffers are not only
persistent subsystem-owned objects; temporary graph buffers are part of the
render-flow model.

#### Compute Nodes Are First-Class

Many feature nodes perform compute work:

```cpp
rctx.BindTextureUAV<0>(rtMaterialClass);
rctx.BindBufferUAV<0>(bufTileCount);
shader->Dispatch(...);
GpuApi::BarrierTextureUAV(rtMaterialClass);
GpuApi::BarrierBufferUAV(bufTileCount);
```

Composition and ray tracing also bind mip UAVs, persistent textures, transient
textures, and dispatch many compute kernels.

LEET should support compute from the graph core:

```text
compute command/pass scope
texture SRV/storage bindings
buffer SRV/storage bindings
dispatch calls
resource read/write tracking
```

wgpu does not expose RED-style explicit UAV barriers in the same way. LEET
should not copy manual barrier calls as graph API. Instead, graph resource
read/write ranges and command ordering must be strong enough for the backend
layer to encode legal wgpu passes and transitions.

#### Persistent Resources Mixed With Transients

Ray tracing shows persistent/custom-data resources used beside allocator-managed
frame resources.

Frame resources:

```cpp
auto rtDepth = rctx.RT<RFUF_Read>(
    RENDER_NAME_TAG("depthBufferWeaponCorrected"));

auto rtAmbientOcclusionTemp = rctx.RTTempAlloc<RFUF_Read | RFUF_Write>(
    RENDER_NAME_TAG("rayTracedAmbientOcclusionTemp"),
    rctx.GetColorTargetDesc(...));
```

Persistent/custom-data resources:

```cpp
GpuApi::TextureRef viewDepthTexture =
    customData.m_viewDepthTexture[customData.m_readTextureIndex];

rctx.BindTextureUAV<0>(viewDepthTexture);
```

NRD transient textures are conditionally routed through the flow allocator:

```cpp
customData.m_NrdTransientTexturesNameTag[i] =
    rctx.RTTempAlloc(RENDER_NAME_TAG("NRD_TRANSIENT_TEXTURE"), desc);

rctx.RTUseBegin(
    customData.m_NrdTransientTexturesNameTag[i],
    RFUF_Read | RFUF_Write);
```

and later:

```cpp
rctx.RTUseEnd(customData.m_NrdTransientTexturesNameTag[i]);
```

LEET rule:

```text
persistent owner owns cross-frame resources
frame allocator owns/transiently tracks frame resources
imports/swap-with-external handle explicit external participation
```

Nodes may bind persistent resources directly when lifetime tracking is owned by
another subsystem. If graph lifetime tracking is required, import the resource
into the frame allocator explicitly.

#### Optional And Null Resources

Feature nodes use null/optional resource paths heavily:

```cpp
auto rtDebug = enableDebug
    ? rctx.RT<RFUF_Read>(RENDER_NAME_TAG("exposureDebug"))
    : rctx.RTNull();
```

They also use safe getters:

```cpp
rtDebug.GetSafe<GpuApi::TextureRef>()
rctx.RTGetOrNull<GpuApi::TextureRef>(bloomTex)
```

LEET needs:

```text
invalid/null resource tags
optional getters
safe binding helpers for optional resources
```

This should be a deliberate API, not ad hoc `Option` plumbing scattered through
node implementations.

#### Subresource And Mip Views

Composition binds mip-level UAVs and mip-level texture reads:

```cpp
GpuApi::BindTextureMipLevelUAV(0, rtCompositionBlurred, mip);
GpuApi::BindTextureMipLevel(0, rtComposition, 0, 1, GpuWrapApi::ComputeShader);
```

Ray tracing also binds resources with mip ranges/offsets in helper code.

LEET texture retrieval cannot stop at:

```text
default texture view only
```

The resource handle layer needs a consume-time way to request subresource views:

```rust
rctx.get_texture_view(
    tag,
    TextureViewRequest {
        base_mip,
        mip_count,
        base_layer,
        layer_count,
        usage,
    },
)?;
```

The allocator still owns the texture as one logical frame resource. View
creation is a backend/consume-time operation over the resolved texture.

#### Read-Write Usage Exists

Several compute/ray-tracing paths use read-write resource usage:

```cpp
auto rtShadowmask = rctx.RT<RFUF_MASK_ReadWrite>(
    RENDER_NAME_TAG("shadowmask"));
```

LEET `ResourceUsage` already supports bitflags. Nodes should be able to express:

```rust
ResourceUsage::READ | ResourceUsage::WRITE
```

This is distinct from `NO_DISCARD`.

#### Locked Rule

```text
Feature nodes are normal graph nodes.
They may allocate local scratch resources, swap logical tags, dispatch compute,
bind optional resources, request subresource views, and mix persistent external
resources with transient frame resources.
The core graph/runtime must support these patterns from V1.
```

---

## 8. `renderNodeImplContext.h/.cpp` Inspection

`SRenderNodeImplContext` is RED's node-facing runtime context. It is not only a
resource allocator wrapper. It carries the current frame, allocator, node flow
identity, camera/view data, command-list state, binding helpers, and cleanup
tracking used by `CRenderNodeBase::Process`.

LEET's context should be the full node runtime lens while keeping allocator,
command-list, and backend binding responsibilities separated.

### Pass 1: Context State And Per-Node Setup

Inspected RED ranges:

- `renderNodeImplContext.h` lines ~94-205:
  `SInitData`, `ScopeNameTag` start, setup API
- `renderNodeImplContext.h` lines ~489-510:
  stored context state and per-node mutable binding state
- `renderNodeImplContext.h` lines ~1016-1121:
  frame, allocator, flow, camera, and size accessors
- `renderNodeImplContext.cpp` lines ~297-401:
  `IsConsumePhase`, `GetFrameInfo`, `SetupNodeData`, `ResetNodeData`,
  `BeginNewNode`, `Init`, and constructors

RED has two distinct setup layers.

Frame/thread initialization stores data that lives across many node calls:

```cpp
struct SInitData
{
    TRenderPtr<IRenderFrame>        m_frame;
    CRenderFlowResourceAllocator*   m_resAlloc;
    Int32                           m_cameraIndex;
    Uint32                          m_dispatcherThreadIndex;
    Bool                            m_skipCameraData;
};
```

`Init` copies this long-lived frame context into `SRenderNodeImplContext`,
finds camera storage when available, caches maximum active camera render size,
and then resets per-node state.

Per-node setup is separate:

```cpp
void SRenderNodeImplContext::SetupNodeData(
    const SRenderNodeContext& nodeContext,
    const SRenderNodeParameters& nodeParams)
{
    m_renderFlowSpace = 0xFF;
    m_renderFlowGroup = nodeContext.m_renderFlowGroups[RNDT_Gpu];
    m_isNodeUnique = RNT_Unique == nodeParams.m_type;
    m_isConsumePhase = GetResourceAllocator()->IsConsumePhase();

    if (!m_isNodeUnique && m_cameraStorage && m_cameraStorage->HasAnyCamera())
    {
        const auto* cameraData =
            m_cameraStorage->GetCameraData(nodeContext.m_cameraIndex);
        m_renderFlowSpace = cameraData->m_renderFlowSpace;
        m_visViewId = cameraData->m_renderFlowSpace + 1;
        m_cameraData = cameraData;
    }
    else
    {
        m_cameraData = nullptr;
        m_visViewId = 0;
    }
}
```

The node-facing resource context uses the GPU/render flow group:

```cpp
m_renderFlowGroup = nodeContext.m_renderFlowGroups[RNDT_Gpu];
```

CPU flow groups are scheduling data. The allocator request stream and
consume-time resource lookup use the render/GPU flow group.

Flow space is derived from node kind and camera data. RED later creates resource
tags like this:

```cpp
RenderFlowNameTag(name, m_isNodeUnique ? 0 : m_renderFlowSpace)
```

LEET should preserve the rule but avoid sentinel-driven ambiguity:

- unique/global node -> shared flow space
- camera/view node -> assigned camera flow space
- non-camera non-unique node -> explicit valid flow space or hard setup error

`ResetNodeData` and `BeginNewNode` are separate concepts:

```cpp
void ResetNodeData()
{
    m_renderFlowGroup = max;
    m_renderFlowSpace = 0xff;
    m_isNodeUnique = false;
    m_cameraData = nullptr;
    m_visViewId = 0;
    BeginNewNode();
}

void BeginNewNode() const
{
    m_texturesToUnbind = 0;
    m_srvToUnbind = 0;
    m_uavToUnbind = 0;
    m_usedStages = 0;
    m_psoBound = 0;
}
```

LEET should preserve this conceptual split:

- reset node identity/state when changing which graph node the context points at
- begin-node cleanup state before each node execution

The full RED-shaped context contract requires:

- frame/runtime reference or equivalent
- dispatcher thread or worker identity
- camera data and camera storage access
- visibility view id or LEET equivalent
- max camera render-size helpers
- per-node cleanup state for bindings, PSO, and pass state
- command-list state

Locked rule:

```text
RenderNodeImplContext is the full node runtime context.
It must remain the node-facing bridge for allocator requests, but it also owns
the current node's frame/camera/flow identity and the runtime state needed by
process_node.
```

### Pass 2: Resource Allocator Front Door

Inspected RED ranges:

- `renderNodeImplContext.h` lines ~203-280:
  resource API declarations
- `renderNodeImplContext.h` lines ~513-641:
  inline tag, use, and resolved-resource getter helpers
- `renderNodeImplContext.cpp` lines ~223-295:
  allocator request forwarding implementations

Local LEET cross-check:

- `src/render_graph/render_node_impl_context.rs`
- `src/render_graph/resources/allocator.rs`
- `src/render_graph/resources/request.rs`
- `src/render_graph/resources/tag.rs`

RED's context is the normal node-facing allocator front door. Most operations
forward directly to the allocator with the current render flow group:

```cpp
GetResourceAllocator()->RequestAlloc(
    name.GetName(),
    RTNameTag(name),
    m_renderFlowGroup,
    desc);
```

Nodes should not normally call allocator request methods directly. They should
use `RenderNodeImplContext`.

Name-tag creation follows the flow-space rule locked earlier:

```cpp
RenderFlowNameTag SRenderNodeImplContext::RTNameTag(
    const RenderFlowNameAndHash name) const
{
    return RenderFlowNameTag(
        name,
        m_isNodeUnique ? 0 : m_renderFlowSpace);
}
```

Rust mirror:

```rust
let tag = rctx.rt_name_tag("scene_color");
```

For unique/global nodes this resolves to shared flow space. For camera/view
nodes it resolves to the assigned camera flow space.

RED also exposes explicit shared tags:

```cpp
RenderFlowNameTag SRenderNodeImplContext::RTSharedNameTag(
    const RenderFlowNameAndHash name)
{
    return RenderFlowNameTag(name, 0);
}
```

Rust should keep an explicit shared-tag API:

```rust
let tag = rctx.rt_shared_name_tag("global_lighting_lut");
```

This is not a convenience duplicate. It is the deliberate escape hatch for
resources intended to be global across camera/view flow spaces.

RED allocation helpers:

```cpp
RTAlloc(name, desc);
RTAlloc(name, other_tag);
```

Rust should keep the clearer split:

```rust
let tag = rctx.rt_name_tag("color");
rctx.declare_resource(tag, desc)?;
rctx.declare_resource_like(dst, src)?;
```

Creating the tag and declaring the resource remain separate operations.

RED temp allocation does not use the display name as allocator identity:

```cpp
RenderFlowNameTag SRenderNodeImplContext::RTTempAlloc(
    const RenderFlowNameAndHash name,
    const SRenderFlowTargetDesc& desc) const
{
    return GetResourceAllocator()->RequestAlloc(
        name.GetName(),
        RenderFlowNameTag::NAMELESS_ALLOC_REQUEST,
        m_renderFlowGroup,
        desc);
}
```

So `temp` means "allocator-generated logical identity at this request position",
not "free memory immediately after use".

Rust mirror:

```rust
let tmp = rctx.temp_resource_tag("bloom_ping")?;
rctx.declare_resource(tmp, desc)?;
```

The generated tag must be deterministic across preconsume and consume replay.

RED injection is texture-only in this context surface:

```cpp
RTInject(name, GpuApi::TextureRef& texture);
```

LEET V1 deliberately supports both resource kinds:

```rust
rctx.import_texture(tag, imported_texture)?;
rctx.import_buffer(tag, imported_buffer)?;
```

This preserves the full texture/buffer allocator contract rather than copying a
texture-only limitation from this wrapper.

Use ranges map directly:

```cpp
RTUseBegin(tag, usageFlags);
RTUseEnd(tag);
```

Rust mirror:

```rust
rctx.use_begin(tag, ResourceUsage::READ | ResourceUsage::WRITE)?;
rctx.use_end(tag)?;
```

RED also exposes RAII `ScopeNameTag`; that is a separate pass. The base LEET API
should remain explicit. A guard API may exist, but it must not replace explicit
begin/end as the underlying request-stream operations.

RED allocation query:

```cpp
RTIsAlloc(tag);
```

The RED comment says this is awkward for threaded command-list building, but it
is still part of the request stream. Rust mirrors it as a replay-stabilized
query:

```rust
let declared = rctx.is_declared(tag)?;
```

Swap maps directly:

```cpp
RTSwap(tag_a, tag_b);
RTSwap(tag_a, external_texture);
```

Rust mirror:

```rust
rctx.swap(a, b)?;
rctx.swap_external_texture(tag, texture)?;
rctx.swap_external_buffer(tag, buffer)?;
```

The external buffer swap is a LEET V1 extension required by the texture/buffer
resource contract.

Decision is a first-class request:

```cpp
Bool RTDecision(Bool baseDecision) const
{
    return GetResourceAllocator()->RequestDecision(
        baseDecision,
        m_renderFlowGroup);
}
```

Rust mirror:

```rust
let enabled = rctx.decision(feature_enabled)?;
```

Branches that affect allocation/use/free/swap/import requests must use this so
preconsume and consume replay the same request stream.

RED resolved-resource getters always include the current render flow group:

```cpp
GetResourceAllocator()->GetTexture(tag, GetRenderFlowGroup());
GetResourceAllocator()->GetBuffer(tag, GetRenderFlowGroup());
```

The current LEET allocator stores consume position as:

```rust
RequestTime::new(flow_group, request_index)
```

That is semantically equivalent if resolved-resource getters are only called
after the context has replayed requests for the current render flow group.

Locked rule:

```text
RenderNodeImplContext is the only normal node-facing allocator front door.
All request-stream operations route through the current render/GPU flow group.
Resolved getters are consume-time and request-time aware.
Direct allocator getters must not become shortcuts that ignore current flow
group/request position.
```

### Pass 3: `ScopeNameTag` And RAII Resource Use

Inspected RED ranges:

- `renderNodeImplContext.h` lines ~114-189:
  `ScopeNameTag`
- `renderNodeImplContext.h` lines ~205-250:
  guard-returning `RT...` declarations
- `renderNodeImplContext.h` lines ~525-580:
  inline `RT<usageFlags>`, `RTAlloc<usageFlags>`, and
  `RTTempAlloc<usageFlags>` helpers

Usage spot-checks:

- `renderGraphNodes.cpp`
- `renderNode_Composition.cpp`
- `renderNode_AntyAliasing.cpp`
- `renderNode_BloomAndTonemapping.cpp`
- `clusteredDeferred.cpp`

RED supports both explicit use ranges and RAII use guards.

Explicit style:

```cpp
rctx.RTUseBegin(tag, RFUF_Read);
...
rctx.RTUseEnd(tag);
```

Guard style:

```cpp
auto rtColor = rctx.RT<RFUF_Read>(RENDER_NAME_TAG("color"));
```

The guard style creates a `ScopeNameTag`. Its creation records use-begin, and
its destructor records use-end:

```cpp
template <Uint32 usageFlags>
static ScopeNameTag Create(
    const SRenderNodeImplContext* rctx,
    const RenderFlowNameTag tag)
{
    rctx->RTUseBegin(tag, usageFlags);
    return ScopeNameTag(rctx, tag);
}

~ScopeNameTag()
{
    if (m_rctx)
    {
        m_rctx->RTUseEnd(m_tag);
    }
}
```

So `ScopeNameTag` is not only a tag wrapper. It is a live use-range guard.

It also acts as a typed resolved-resource accessor:

```cpp
operator const GpuApi::TextureRef() const;
operator const GpuApi::BufferRef() const;

template <typename ResourceRef>
ResourceRef Get() const;

template <typename ResourceRef>
ResourceRef GetSafe() const;
```

That lets RED node code write:

```cpp
auto rtDepth = rctx.RT<RFUF_Read>(RENDER_NAME_TAG("depthBuffer"));
rctx.BindTexture<0>(rtDepth);
```

The guard handles:

- request use begin
- typed get during consume
- request use end on scope exit

RED also uses null guards for optional resources:

```cpp
auto rtDebug = enableDebug
    ? rctx.RT<RFUF_Read>(RENDER_NAME_TAG("exposureDebug"))
    : rctx.RTNull();
```

and:

```cpp
RenderScopeNameTag::CreateNull();
```

A null guard does not call `RTUseEnd`. Safe getters return null instead of
asserting.

LEET's primary Rust API should remain explicit:

```rust
rctx.use_begin(tag, ResourceUsage::READ)?;
...
rctx.use_end(tag)?;
```

That is the allocator contract and the clearest authoring path. It also avoids
making Rust node code look like the use range is only a local variable lifetime
detail.

LEET may add a separate ergonomic guard layer, but it must be a thin wrapper over
the same request-stream operations:

```rust
let color_use = rctx.begin_resource_use(color, ResourceUsage::READ)?;
let color = color_use.get_texture()?;
color_use.end()?;
```

Avoid vague helpers such as `RT` or `use_scope`. The name should make clear that
the value represents an active resource use range.

Rust guard design must account for fallible `use_end`. `Drop` cannot return a
`Result`, so a guard should either require explicit `end()?`/`finish()?` or use
`Drop` only for debug assertions after the balanced end has already succeeded.

Locked rule:

```text
Explicit use_begin/use_end remains the primary API.
A resource-use guard may exist as an ergonomic layer, but it must be a thin
wrapper over the same request-stream operations.
The guard must be move-only, must end the use exactly once, must support null
optional tags, and must not replace explicit begin/end in the allocator contract.
```

### Pass 4: Descriptor Helper Methods

Inspected RED ranges:

- `renderNodeImplContext.cpp` lines ~1-176:
  `GetRegularPrecisionColorTargetFormat`, `GetHighPrecisionColorTargetFormat`,
  `GetColorTargetWidth`, `GetColorTargetHeight`, `GetColorTargetDesc`,
  `GetResolvedFinalTargetDesc`, `GetScreenshotTargetDesc`,
  `GetVelocityBufferDesc`, `GetDepthBufferDesc`, and `GetCompositionDesc`
- `renderNodeImplContext.h` lines ~281-361:
  camera/frame accessors, resolution helpers, and descriptor helper declarations
- `renderNodeImplContext.h` lines ~1049-1110:
  `GetWidth`, `GetHeight`, `GetMaxWidth`, `GetMaxHeight`,
  `GetPostAAWidth`, `GetPostAAHeight`, `GetFinalWidth`, and
  `GetFinalHeight`

Usage spot-checks:

- `renderGraphNodes.cpp`
- `clusteredDeferred.cpp`

RED uses `rctx` as the place where common frame-resource descriptors are built.
Nodes do not manually rebuild common color/depth/velocity/composition
descriptors everywhere.

Examples:

```cpp
rctx.GetRegularPrecisionColorTargetDesc();
rctx.GetHighPrecisionColorTargetDesc();
rctx.GetColorTargetDesc(format);
rctx.GetDepthBufferDesc();
rctx.GetVelocityBufferDesc();
rctx.GetCompositionDesc();
```

These helpers are not allocator magic. They are descriptor factories using the
current frame, camera, feature, and composition state.

RED exposes multiple resolution concepts:

```cpp
GetWidth() / GetHeight()
GetMaxWidth() / GetMaxHeight()
GetPostAAWidth() / GetPostAAHeight()
GetPostAAMaxWidth() / GetPostAAMaxHeight()
GetFinalWidth() / GetFinalHeight()
```

These mean different things:

- internal size before AA/upscaling
- maximum internal size for dynamic resolution
- post-AA or temporal-upscaled size
- maximum post-AA size
- final output/UI composition size

LEET should preserve these distinctions. A single `width()` / `height()` helper
would be too vague.

RED color-target descriptors preserve current size and max capacity:

```cpp
SRenderFlowTargetDesc SRenderNodeImplContext::GetColorTargetDesc(
    GpuWrapApi::eTextureFormat format) const
{
    SRenderFlowTargetDesc desc;
    desc.m_format = format;
    desc.m_isSamplable = true;
    desc.m_sizeWidth = GetColorTargetWidth();
    desc.m_sizeHeight = GetColorTargetHeight();

    if (IsCameraDataAvailable())
    {
        desc.m_maxWidth =
            GetCameraData()->GetResolutionState().GetMaxInternalWidth();
        desc.m_maxHeight =
            GetCameraData()->GetResolutionState().GetMaxInternalHeight();
    }
    else
    {
        desc.m_maxWidth = desc.m_sizeWidth;
        desc.m_maxHeight = desc.m_sizeHeight;
    }

    return desc;
}
```

This maps to LEET's existing `FrameTextureDesc` split:

```rust
current_size
max_size
current_mip_level_count
max_mip_level_count
```

Texture-specific descriptor helpers may return `FrameTextureDesc`, but generic
allocator declarations still use `FrameResourceDesc`:

```rust
let color_desc = rctx.regular_precision_color_target_desc()?;
let color = rctx.rt_name_tag("color");

rctx.declare_resource(
    color,
    FrameResourceDesc::Texture(color_desc),
)?;
```

No generic allocation record should store only `FrameTextureDesc`; the allocator
contract remains texture-or-buffer through `FrameResourceDesc`.

Special helper behavior to preserve:

- `GetVelocityBufferDesc()` derives from color-target sizing, chooses the
  velocity format, and denies unordered access in RED. LEET should express that
  through texture usage/capability flags, not through a separate resource kind.
- `GetDepthBufferDesc()` uses color-target sizing with a depth/stencil format.
- `GetResolvedFinalTargetDesc()` uses final viewport/output size, not camera
  internal size.
- `GetScreenshotTargetDesc()` overrides max size to exact viewport size to avoid
  oversized staging/copy costs.
- `GetCompositionDesc()` uses composition surface size and computes mip count.
  This means descriptor helpers need access to frame/composition state, not only
  camera state.

Locked rule:

```text
RenderNodeImplContext should expose descriptor helper APIs for common render
targets, depth, velocity, final output, screenshot, and composition shapes.

These helpers are node-authoring conveniences over FrameTextureDesc /
FrameResourceDesc construction. They must preserve current size vs max capacity,
mip count vs max mip count, format, usage capability, and frame-purpose
decisions.

They must not become separate allocator resource kinds.
```

### Pass 5: Command List Access

Inspected RED ranges:

- `renderNodeImplContext.h` lines ~363-365:
  `SetCommandList`, `GetCommandList`, and `HasCommandList`
- `renderNodeImplContext.cpp` lines ~176-191:
  command-list slot lookup through current render flow group
- `renderGraphNodes.cpp` lines ~107-205:
  `CRenderNodeBase::CreateCommandList`, `CRenderNodeBase::Process`, and
  `ProcessEpilogue`
- `renderGraphNodes.cpp` lines ~212-296:
  epilogue cleanup tail and `CRenderNode_Synchronize::Execute`

RED stores command lists by render flow group plus a reserved offset:

```cpp
void SRenderNodeImplContext::SetCommandList(
    GpuApi::CommandListRef cl) const
{
    GetRenderer()->GetFrameCommandLists().SetCommandList(
        GetRenderFlowGroup() + ReservedCls::CL_COUNT,
        cl);
}
```

Getter and existence check use the same slot:

```cpp
GpuApi::CommandListRef SRenderNodeImplContext::GetCommandList() const
{
    GpuApi::CommandListRef cl =
        GetRenderer()->GetFrameCommandLists().GetCommandList(
            GetRenderFlowGroup() + ReservedCls::CL_COUNT);

    RED_ASSERT(cl, "Don't have a command list");
    return cl;
}

Bool SRenderNodeImplContext::HasCommandList() const
{
    GpuApi::CommandListRef cl =
        GetRenderer()->GetFrameCommandLists().GetCommandList(
            GetRenderFlowGroup() + ReservedCls::CL_COUNT);

    return !cl.isNull();
}
```

So `SRenderNodeImplContext` does not own command lists. It addresses the
frame-level command-list registry through the current render/GPU flow group.

`CRenderNodeBase::Process` owns the common command-list wrapper:

```cpp
const Bool consumePhase = rctx.IsConsumePhase();
const RenderNodeCommandListUsage clUsage = GetCommandListUsage();
const Bool useCommandList = consumePhase && HasCommandList(clUsage);

if (consumePhase && clUsage == RenderNodeCommandListUsage::Own)
{
    GpuApi::CommandListRef commandList = CreateCommandList();
    rctx.SetCommandList(commandList);
}

if (useCommandList)
{
    GpuApi::BindCommandList(rctx.GetCommandList());
    BeginProfilerBlock();
}

rctx.BeginNewNode();
Execute(rctx, builder);

if (useCommandList)
{
    GpuApi::BindCommandList({});
    ProcessEpilogue(rctx);
}
```

Command-list handling is graph/runtime wrapper behavior. It is not ordinary node
logic.

Usage meanings:

- `CommandListUsage::Own` creates and stores a command list for the current
  render flow group
- `CommandListUsage::Require` expects a command list already available for the
  current render flow group
- `CommandListUsage::Sync` is handled by sync node/runtime behavior
- `CommandListUsage::None` does not bind a command list

`CRenderNode_Synchronize::Execute` confirms the split between allocator queue
sync and command-list submission:

```cpp
rctx.GetResourceAllocator()->RequestQueueSync(
    rctx.GetRenderFlowGroup(),
    m_sync);

if (rctx.IsConsumePhase())
{
    GetRenderer()->GetFrameCommandLists().Submit(
        m_name.AsChar(),
        rctx.GetRenderFlowGroup(),
        m_sync,
        *builder);
}
```

The allocator receives a queue-sync request for request-stream/lifetime
semantics. The frame command-list runtime submits encoded work during consume.
They happen in the same sync node, but they are different systems.

LEET should not store final command-list state in the resource allocator.
Command-list state belongs to graph/runtime frame state.

`RenderNodeImplContext` should expose command-list accessors, but they should
delegate to the frame command-list runtime:

```rust
rctx.set_command_list(...)?;
rctx.get_command_list()?;
rctx.has_command_list()?;
```

or wgpu-shaped equivalents. Since wgpu command buffers are finished values and
encoders are mutable recording objects, LEET may need context methods around the
active encoder/recording handle rather than copying RED's `CommandListRef`
literally.

Locked rule:

```text
RenderNodeImplContext addresses command-list state by current render/GPU flow
group.
process_node owns command-list creation, binding/activation, profiler wrapping,
and epilogue cleanup.
Nodes should not manually decide command-list slot ownership.
Sync nodes record allocator queue_sync and submit/synchronize frame command
lists during consume, but allocator queue_sync is not the command-list registry.
```

### Pass 6: Binding Helpers

Inspected RED ranges:

- `renderNodeImplContext.h` lines ~22-32:
  `EBindTarget` stage flags: vertex, pixel, compute, ray tracing
- `renderNodeImplContext.h` lines ~374-445:
  binding helper declarations
- `renderNodeImplContext.h` lines ~768-958:
  inline implementations for texture, buffer, sampler, UAV, and constant helpers
- `renderNodeImplContext.cpp` lines ~196-222:
  `Unbind` cleanup behavior

Usage spot-checks:

- `renderGraphNodes.cpp`
- `renderNode_Composition.cpp`
- `renderNode_AntyAliasing.cpp`
- `renderNode_BloomAndTonemapping.cpp`
- `clusteredDeferred.cpp`

RED exposes many immediate binding helpers through `SRenderNodeImplContext`:

```cpp
BindTexture<SLOT, STAGE>(...);
BindTextureMip<SLOT, STAGE>(...);
BindTextureStencil<SLOT, STAGE>(...);
BindTextures<SLOT, STAGE, N>(...);
BindSampler<SLOT, STAGE>(...);
BindSamplers<SLOT, STAGE, N>(...);
BindBufferSRV<SLOT, STAGE>(...);
BindConstantBuffer<SLOT, STAGE>(...);
BindBufferUAV<SLOT>(...);
BindTextureUAV<SLOT>(...);
BindTextureMipUAV<SLOT>(...);
BindTextureUAVs<SLOT, N>(...);
SetConstant<STAGE>(...);
```

These helpers do three jobs.

First, they resolve graph resources when passed tags or guards:

```cpp
BindTexture<SLOT, STAGE>(RTGet<GpuApi::TextureRef>(tag));
BindTextureUAV<SLOT>(tag.Get<GpuApi::TextureRef>());
```

Second, they bind to explicit shader stages:

```cpp
if (STAGE & eVERTEX)  { ... }
if (STAGE & ePIXEL)   { ... }
if (STAGE & eCOMPUTE) { ... }
```

Third, they track cleanup state:

```cpp
m_texturesToUnbind |= RED_FLAG(SLOT);
m_uavToUnbind |= RED_FLAG(SLOT);
m_usedStages |= STAGE;
```

`Unbind` later clears only the state touched by the node:

```cpp
if (m_texturesToUnbind)
{
    BindTextures(0, numSlots, nullptr, stage);
}

if (m_uavToUnbind)
{
    BindTextureUAVs(0, numSlots, nullptr);
}

if (m_psoBound && clearRenderTarget)
{
    SetupBlankRenderTargets();
}
```

So RED binding helpers are not just convenience wrappers. They feed epilogue
cleanup and make node-local binding state visible to `ProcessEpilogue`.

LEET should not copy RED's immediate mutable slot API literally.

RED model:

```text
global-ish mutable GPU binding slots
BindTexture(slot)
BindBufferSRV(slot)
Unbind(slot range)
```

wgpu model:

```text
create/bind bind groups
begin render/compute pass
set pipeline
set bind group
draw/dispatch
pass ends cleanly
```

The behavior to preserve is:

```text
node can declare what resources/samplers/constants it wants bound
bindings are stage-aware
bindings can target textures, texture subresources, buffers, storage/UAV usage,
samplers, constants/uniform data
binding state is scoped to the node/pass and cleaned up deterministically
```

LEET should use wgpu-shaped binding builders or pass encoder wrappers rather
than exposing old-style mutable global slots as the final API.

Possible shape:

```rust
let mut bindings = rctx.bindings();

bindings.texture(slot, ShaderStageMask::FRAGMENT, color_view)?;
bindings.storage_texture(slot, ShaderStageMask::COMPUTE, target_view)?;
bindings.buffer_srv(slot, ShaderStageMask::COMPUTE, lights)?;
bindings.sampler(slot, ShaderStageMask::FRAGMENT, sampler)?;
bindings.constants(slot, ShaderStageMask::FRAGMENT, &params)?;
```

or pass-specific methods:

```rust
let mut pass = rctx.begin_compute_pass(...)?;
pass.bind_texture(...)?;
pass.bind_storage_texture(...)?;
pass.dispatch(...);
```

Exact names belong to the backend/pass design, but the capabilities are V1
requirements.

This pass also reinforces why `BeginNewNode` and `ProcessEpilogue` matter. They
reset and clean per-node binding/pass state around every node.

Locked rule:

```text
RenderNodeImplContext must support the binding capabilities RED nodes rely on,
but LEET should adapt them to wgpu pass/bind-group semantics.

Do not copy global mutable slot binding literally as the final API.
Do preserve stage masks, texture/buffer/sampler/constant/storage bindings,
subresource/mip binding, optional/null bindings, and deterministic cleanup.
```

### Pass 7: PSO And Render Target Setup

Inspected RED ranges:

- `renderNodeImplContext.h` lines ~34-86:
  `BlankOutput`, `ColorTarget`, `NullColorTarget`, `DepthTarget`,
  `UnorderedAccessView`, and `UnorderedAccessViewBase`
- `renderNodeImplContext.h` lines ~450-484:
  `BindPSO` declarations and recursive `BindPSOImpl` overloads
- `renderNodeImplContext.h` lines ~964-1014:
  `BindPSOImpl` implementation, `GpuApi::SetupRenderTargets`, and
  `m_psoBound = true`

Usage spot-checks:

- `renderNode_RenderTargets.cpp`
- `renderNode_Lighting.cpp`
- `renderNode_Shadows.cpp`
- `renderNode_Hair.cpp`
- `renderNode_AntyAliasing.cpp`
- `renderNode_Composition.cpp`
- `renderRenderFrame.cpp`
- several feature/debug files

RED's `BindPSO` is effectively a render-target setup builder.

Node code writes:

```cpp
rctx.BindPSO(
    ColorTarget(0, rtColor),
    DepthTarget(rtDepth, -1, true),
    UnorderedAccessViewBase(3),
    UnorderedAccessView(0, distortionMaskTiles));
```

The small helper structs are typed arguments:

```cpp
ColorTarget(slot, texture, slice);
DepthTarget(texture, slice, readOnly);
NullColorTarget();
BlankOutput();
UnorderedAccessView(slot, texture);
UnorderedAccessViewBase(baseSlot);
```

`BindPSO` folds those arguments into a `GpuApi::RenderTargetSetup`:

```cpp
pso.renderTargets.SetColorTarget(slot, texture, slice);
pso.renderTargets.SetDepthStencilTarget(texture, slice, readOnly);
pso.renderTargets.SetUnorderedAccessView(slot, texture);
pso.renderTargets.uavStartSlot = baseSlot;
```

Then it applies the setup:

```cpp
GpuApi::SetupRenderTargets(pso.renderTargets);
m_psoBound = true;
```

The RED name is misleading in this context. This is not only "bind pipeline
state"; it sets output attachment / render target state.

wgpu does not expose render targets as mutable global state. Attachments are
chosen when a pass begins:

```rust
encoder.begin_render_pass(&RenderPassDescriptor {
    color_attachments: ...,
    depth_stencil_attachment: ...,
    ...
});
```

So LEET should not copy `rctx.BindPSO(ColorTarget(...))` literally as the final
API.

The behavior to preserve is:

```text
node can describe output attachments
node can choose color targets by slot
node can choose depth/stencil target
node can choose depth read-only vs writable intent
node can target array/cubemap slices or mip views
node can represent no color output / blank output
node can bind storage/UAV-style outputs where the backend supports it
process_node/epilogue knows whether render targets were bound
```

LEET shape should be closer to pass setup:

```rust
let mut pass = rctx.begin_render_pass(RenderPassSetup {
    colors: &[ColorAttachment {
        slot: 0,
        view: color_view,
        load_op,
        store_op,
    }],
    depth: Some(DepthAttachment {
        view: depth_view,
        read_only: true,
    }),
    storage_outputs: ...,
})?;
```

or a builder:

```rust
let mut pass = rctx
    .render_pass()
    .color(0, color)
    .depth_read_only(depth)
    .storage_texture(0, tiles)
    .begin()?;
```

Exact naming belongs to backend/pass design, but the abstraction should be
render/compute pass setup, not a direct RED `BindPSO` carryover.

Special cases to preserve:

- `BlankOutput()` resets render target setup. This may map to compute-only work,
  passless encoder work, or a backend-specific no-output path.
- `NullColorTarget()` is different from simply forgetting color attachments. It
  represents deliberate no-color output with depth or other state active, such
  as depth-only shadow rendering.
- `DepthTarget(..., readOnly = true)` is important for lighting/forward passes.
  LEET must preserve depth read/write intent for correctness and wgpu
  validation.
- `ColorTarget(..., slice)` and `DepthTarget(..., slice)` mean pass setup must
  support texture view requests for slices/layers/mips, not only default views.
- `UnorderedAccessView` outputs may need to lower to storage texture/buffer bind
  groups or compute passes in wgpu.

Locked rule:

```text
Do not name the final LEET API BindPSO unless it truly binds pipeline state.
RED's BindPSO should map to a render/compute pass setup abstraction.

RenderNodeImplContext must support output attachment setup from node code:
color attachments, null/blank color output, depth/stencil attachments,
read-only depth, slices/mips/views, and storage/UAV-style outputs.

The implementation must be wgpu-shaped: attachments are chosen at pass begin,
and storage outputs are represented through bind groups or compute passes where
wgpu requires it.
```

### Pass 8: Frame, Camera, Collector, And Feature Accessors

Inspected RED ranges:

- `renderNodeImplContext.h` lines ~281-361:
  visibility, camera, collector, scene, debug/feature, frame/flow accessors
- `renderNodeImplContext.h` lines ~647-746:
  inline camera, collector, debug, and feature accessors
- `renderNodeImplContext.h` lines ~1016-1131:
  frame, flow, allocator, render camera, resolution, consume, and unique
  accessors
- `renderNodeImplContext.cpp` lines ~297-320:
  `IsConsumePhase`, `GetFrameInfo`, `GetScene`, and start of `SetupNodeData`

RED's `SRenderNodeImplContext` gives node implementations controlled access to
frame and camera state:

```cpp
GetFrame();
GetFrameInfo();
GetScene();
HasScene();
GetRenderFlowGroup();
GetRenderFlowSpace();
GetResourceAllocator();
IsUniqueNode();
IsConsumePhase();
```

These are core context identity/runtime accessors. LEET should have equivalents,
but with Rust ownership boundaries: the context can expose references or handles
to frame runtime state, not arbitrary mutable renderer globals.

Camera access is stricter than it first looks.

For ordinary camera/view nodes RED exposes current-camera helpers:

```cpp
GetCameraData();
GetCameraInfo();
GetRenderCamera();
GetWidth();
GetHeight();
GetMaxWidth();
GetFinalWidth();
```

But RED asserts if a unique/global node tries to use camera-only access:

```cpp
RED_FATAL_ASSERT(!m_isNodeUnique, "Node cannot be unique");
RED_FATAL_ASSERT(m_cameraData, "No camera data");
```

For unique/global nodes that need to inspect all cameras, RED provides separate
APIs:

```cpp
GetNumCameras();
GetCameraStorage();
GetCameraDataByIndex(cameraIndex);
GetAllCameraPositions(...);
GetFirstOnScreeenCamera();
```

`GetCameraDataByIndex` explicitly requires a unique node:

```cpp
RED_FATAL_ASSERT(m_isNodeUnique, "Node must be unique");
```

So there are two camera access modes:

```text
camera/view node -> current camera only
unique/global node -> camera storage / indexed camera queries
```

LEET should model that distinction explicitly. A single `camera()` method that
sometimes works and sometimes panics would be too vague.

Collector access is also phase/state guarded:

```cpp
GetCollector();
GetCollectorUnitialized();
```

RED checks whether collector data is ready:

```cpp
m_cameraData->IsAccesible(CRenderFrameCameraStorage::DAF_Collector)
```

LEET should not expose collector-like data as always available. If LEET has
render collectors, the context API should encode readiness/state, likely as
`Result` or typed access rather than an unchecked reference.

Visibility view:

```cpp
GetVisibilityView();
```

comes from camera flow space:

```cpp
m_visViewId = cameraData->m_renderFlowSpace + 1;
```

LEET does not need to copy the `+1` visibility hack, but it should preserve the
concept: camera/view nodes can have a graph-visible view id for visibility and
culling systems, while non-view nodes may not.

Feature/debug tests are context-sensitive:

```cpp
Test(DebugFilter);
Test(ERenderFeatureFlags);
```

RED behavior:

- camera node with camera data checks current camera rendering mask/features
- unique node or no current camera can test debug flags across camera storage
- feature flags return false without camera data

LEET should keep these as explicit frame/camera feature-query APIs, not raw
global config reads scattered through node implementations.

Locked rule:

```text
RenderNodeImplContext should expose frame/runtime identity, flow identity,
phase, and controlled camera/view access.

Camera/view access must distinguish:
- current-camera access for camera/view nodes
- indexed/all-camera access for unique/global nodes

Collector/visibility/feature/debug access must be state-aware and fail loudly or
return Result/Option when used from the wrong node kind or before data is ready.

Do not let nodes bypass the context by reaching directly into renderer globals
for frame, camera, feature, or collector state.
```

### Pass 9: Viewport And Worker Context State

Inspected RED ranges:

- `renderNodeImplContext.h` lines ~190-205:
  copy constructor with dispatcher thread index and setup declarations
- `renderNodeImplContext.h` lines ~448-462:
  `BindPSO`, `SetViewport`, and `GetDispatcherThreadIndex`
- `renderNodeImplContext.h` lines ~1010-1026:
  inline `SetViewport`
- `renderNodeImplContext.cpp` lines ~382-428:
  constructor, copy-with-dispatcher-thread-index constructor, and destructor

RED exposes viewport setting through the node implementation context:

```cpp
void SetViewport( const GpuApi::ViewportDesc& viewportDesc ) const;
```

The inline implementation is just:

```cpp
GpuApi::SetViewport( viewportDesc );
```

but RED leaves an important note:

```cpp
// TODO: consider some overseeing here, allow/disallow nodes to set viewport.
```

That warning matters more in LEET than it does in RED. In wgpu, viewport state
belongs to an active render pass encoder. It is not a safe global renderer
mutation. LEET should expose viewport control through the node context only when
there is an active render-pass recording object, and the viewport should be
validated against the current pass attachments.

The Rust shape should be closer to:

```rust
rctx.set_viewport(viewport)?;
```

where the implementation routes to the current pass/command recorder, not to a
global GPU state object. Calling it outside a compatible recording scope should
fail loudly.

RED also stores a dispatcher thread index:

```cpp
RED_FORCE_INLINE Uint32 GetDispatcherThreadIndex() const
{
    return m_dispatcherThreadIndex;
}
```

and has a context copy constructor that changes only that worker identity:

```cpp
SRenderNodeImplContext::SRenderNodeImplContext(
    const SRenderNodeImplContext& other,
    Uint32 dispatcherThreadIndex )
    : SRenderNodeImplContext( other )
{
    m_dispatcherThreadIndex = dispatcherThreadIndex;
}
```

This is part of RED's parallel execution model. The render graph can copy the
base node context for worker execution and stamp each copy with the worker that
is processing it. That worker identity is useful for diagnostics, profiling,
per-worker command-list selection, temporary scratch state, and batch allocator
selection.

LEET should preserve the concept, but not expose it as a resource-allocator
feature. It belongs to render graph execution/runtime state:

```rust
pub struct RenderNodeImplContext<'frame> {
    worker_index: RenderWorkerIndex,
    // frame runtime, flow identity, command recording state, allocator access...
}
```

Context copies used for parallel node processing must not accidentally share
mutable per-node recording state. Shared immutable frame/runtime state is fine;
per-worker mutable command recording, scratch, profiling, and cleanup state must
be distinct or synchronized through explicit runtime structures.

The default RED constructor initializes invalid sentinels:

```cpp
m_renderFlowGroup( std::numeric_limits<TRenderFlowGroup>::max() )
m_renderFlowSpace( 0xff )
m_isNodeUnique( false )
m_visViewId( 0 )
m_cameraData( nullptr )
m_cameraStorage( nullptr )
m_maxCameraRenderWidth( 0 )
m_maxCameraRenderHeight( 0 )
```

and then calls:

```cpp
ResetNodeData();
```

Rust should use typed construction where possible, but still keep invalid/debug
sentinel states for diagnostics where they help catch misuse. A context should
not be usable for node execution until frame runtime, flow group, flow space,
phase, and node identity have been initialized.

RED's destructor is empty:

```cpp
SRenderNodeImplContext::~SRenderNodeImplContext()
{
}
```

That is a design signal. Context destruction is not where frame-resource cleanup,
command-list cleanup, or binding cleanup happens. Cleanup is explicit in process
epilogue, unbind behavior, allocator phase transitions, and command-list
lifetime management.

Locked rule:

```text
Viewport/encoder state belongs to pass/command recording and must be validated
against the current pass attachments.

Dispatcher/worker identity is runtime context state required for parallel
execution and diagnostics, not a node-authored resource concept.

RenderNodeImplContext copies for parallel work must get independent per-node
mutable state and a distinct worker identity.

Context destruction must not own frame-resource or command-list cleanup.
```

### Pass 10: Context State Audit

Inspected RED ranges:

- `renderNodeImplContext.h` lines ~480-510:
  private/member state
- `renderNodeImplContext.h` lines ~513-647:
  inline resource, tag, and getter helpers
- `renderNodeImplContext.h` lines ~768-1013:
  binding helpers and cleanup-mask writes
- `renderNodeImplContext.cpp` lines ~196-220:
  `Unbind`
- `renderNodeImplContext.cpp` lines ~346-352:
  `BeginNewNode`

This final pass confirms that `SRenderNodeImplContext` is not only a function
bag. It carries stable node execution identity plus small mutable per-node
recording state.

RED stores frame/resource/flow/camera identity:

```cpp
m_frame
m_resAlloc
m_cameraIndex
m_dispatcherThreadIndex
m_cameraData
m_cameraStorage
m_visViewId
m_maxCameraRenderWidth
m_maxCameraRenderHeight
m_isNodeUnique
m_isConsumePhase
m_renderFlowSpace
m_renderFlowGroup
```

That maps to a Rust context carrying frame runtime state, allocator access, node
camera identity, worker identity, camera/view access state, unique/global-node
state, current allocator phase view, render-flow group, and render-flow space.

RED also tracks mutable cleanup state:

```cpp
m_texturesToUnbind
m_srvToUnbind
m_uavToUnbind
m_usedStages
m_psoBound
```

The binding helpers write these masks:

```cpp
m_texturesToUnbind |= RED_FLAG( SLOT );
m_usedStages |= STAGE;
m_uavToUnbind |= RED_FLAG( SLOT );
m_psoBound = true;
```

`BeginNewNode` resets them before node execution:

```cpp
m_texturesToUnbind = 0;
m_srvToUnbind      = 0;
m_uavToUnbind      = 0;
m_usedStages       = 0;
m_psoBound         = 0;
```

and `Unbind` uses the masks to clear RED's global binding slots and optionally
clear render targets:

```cpp
if( m_texturesToUnbind )
{
    if( m_usedStages & eVERTEX )  GpuApi::BindTextures( 0, numSlots, nullptr, VertexShader );
    if( m_usedStages & ePIXEL )   GpuApi::BindTextures( 0, numSlots, nullptr, PixelShader );
    if( m_usedStages & eCOMPUTE ) GpuApi::BindTextures( 0, numSlots, nullptr, ComputeShader );
}

if( m_uavToUnbind )
{
    GpuApi::BindTextureUAVs( 0, numSlots, nullptr );
}

if( m_psoBound && clearRenderTarget )
{
    GpuApi::SetupBlankRenderTargets();
}
```

`m_srvToUnbind` appears in the state and reset path but is not meaningfully used
in the inspected implementation. LEET should treat that as a RED artifact, not
as a field to copy blindly.

The Rust model should preserve the ownership boundary, not the exact bitmasks:

```rust
struct RenderNodeImplContext<'frame> {
    identity: RenderNodeExecutionIdentity,
    frame: FrameRuntimeHandle<'frame>,
    resources: FrameResourceAllocatorHandle<'frame>,
    command_recording: NodeCommandRecordingState,
    cleanup: NodeRecordingCleanupState,
}
```

Because wgpu uses explicit encoders, passes, bind groups, and attachment setup,
LEET's cleanup state should track backend-relevant facts:

```text
active render or compute pass state
attachments selected for this node
bind groups/resources referenced by this node
debug/profiling markers
whether pass/encoder cleanup is still required
```

It should not become a literal clone of RED's global texture/UAV slot clearing
unless a future backend actually needs that behavior.

Locked rule:

```text
RenderNodeImplContext must carry both stable node identity and per-node mutable
execution state.

Do not mirror RED cleanup masks literally unless the backend needs them.

BeginNewNode/process setup must reset per-node recording and cleanup state before
each node executes.

Unbind-style cleanup maps to explicit wgpu pass/encoder finalization, debug
tracking, and resource-reference lifetime bookkeeping, not global slot clearing.
```

## 9. `renderGraphCache.h/.cpp` Inspection

`renderGraphCache.h/.cpp` defines RED's cache for built render graphs. It is
small, but it answers an important architectural question: RED does not rebuild
the render graph from scratch every frame if the camera setup has not changed.

Inspected RED ranges:

- `renderGraphCache.h`:
  `CRenderGraphCache`, `SCameraHash`, `SCameraSetupData`, and `CacheEntry`
- `renderGraphCache.cpp`:
  `GetGraph`, `PostBuildClear`, and `FindCameraSetup`

### renderGraphCache One-Pass Findings

RED uses a 64-bit camera setup hash:

```cpp
struct SCameraHash
{
    red::THash64 m_hash;

    template <typename Type>
    RED_INLINE void Append( const Type& value )
    {
        m_hash = red::CalculateHash64( &value, sizeof(Type), m_hash );
    }

    RED_INLINE Bool IsValid() const
    {
        return m_hash != RED_FNV_OFFSET_BASIS64;
    }
};
```

This hash identifies the render graph shape implied by the camera setup. LEET
should mirror the concept with a deterministic graph-shape hash:

```rust
pub struct RenderGraphCameraSetupHash(u64);
```

The hash should include inputs that change graph topology, such as camera/view
count, view kind, render feature set, and output target class. It should not
include ordinary transient per-frame values unless they change the graph shape.

Each cache entry stores a final graph and per-camera temporary build data:

```cpp
struct SCameraSetupData
{
    CRenderNodeGraph m_graph;
    rend::NodesContainer m_nodes;
    SCameraHash m_cameraSetupHash;
};

struct CacheEntry
{
    Uint32 m_lastUsedFrame = 0;
    SCameraHash m_cameraHash;
    CRenderNodeGraph m_graph;
    red::DynArray<CameraSetupDataPtr> m_cameraBuildData;
};
```

The temporary camera graphs are cleared after final graph building:

```cpp
void CRenderGraphCache::CacheEntry::PostBuildClear()
{
    for ( const CameraSetupDataPtr& cameraSetupData : m_cameraBuildData )
    {
        cameraSetupData->m_graph.Reset();
    }

    // NOTE: nodes need to be kept due to references from final graph
}
```

That comment is the important part. RED clears the temporary per-camera graph
topology, but keeps the per-camera node storage alive because the final merged
graph still references those node implementations.

LEET implication:

```text
temporary graph builder lifetime != node implementation lifetime
```

Because LEET supports graph import/merge from V1, camera subgraph node storage
must outlive the temporary builder graph. We cannot create camera subgraph nodes in a
short-lived local arena and then drop that arena after merge if the final graph
stores references or ids into it.

RED uses a tiny fixed cache:

```cpp
static const Uint32 MaxCacheEntries = 4;
red::FixedArray< CacheEntry, MaxCacheEntries > m_cacheEntries;
```

`GetGraph` returns an existing entry only when the camera setup hash and camera
setup count match exactly, and `forceClear` is false:

```cpp
if ( cacheEntry.m_cameraHash == cameraHash &&
     cacheEntry.m_cameraBuildData.Size() == numCameraSetups &&
     !forceClear )
{
    outNeedsRebuild = false;
    cacheEntry.m_lastUsedFrame = frameTick;
    return cacheEntry;
}
```

On a miss, RED reuses the oldest cache entry:

```cpp
// no graph entry found, reuse the oldest
CacheEntry& cacheEntry = m_cacheEntries[ oldestCacheEntryIndex ];
cacheEntry.m_graph.Reset();
cacheEntry.m_cameraBuildData.Resize( numCameraSetups );
cacheEntry.m_cameraHash = cameraHash;
cacheEntry.m_lastUsedFrame = frameTick;
outNeedsRebuild = true;
```

So the cache policy is a small LRU-like cache keyed by graph-shape/camera setup,
not by transient GPU resources.

LEET should include a render graph cache from V1:

```rust
pub struct RenderGraphCache {
    entries: Vec<RenderGraphCacheEntry>,
    max_entries: usize,
}

pub struct RenderGraphCacheEntry {
    camera_setup_hash: RenderGraphCameraSetupHash,
    last_used_frame: u64,
    final_graph: RenderNodeGraph,
    camera_build_data: Vec<CameraGraphBuildData>,
}
```

`camera_build_data` should own the per-camera node storage required by the final
merged graph. The exact Rust storage may be a shared arena, stable ids, or an
owned node container, but the lifetime rule is strict.

Locked rule:

```text
Graph caching is separate from frame resource allocation.

The graph cache stores graph topology and node ownership/lifetime, not transient
GPU textures or buffers.

Camera subgraph node storage must remain alive as long as the final merged graph
references those nodes.

Cache hits are allowed only when the camera setup hash and camera setup count
match exactly.

On cache miss, reuse/evict the oldest graph cache entry and rebuild the graph
topology for that entry.
```

## 10. `renderNodeJob.h/.cpp` Inspection

`renderNodeJob.h/.cpp` is small. It does not define graph topology. It provides
runtime/profiling glue used while render node jobs are executing.

Inspected RED ranges:

- `renderNodeJob.h`:
  `CDynamicPerfScope` and `CRenderNodeJob`
- `renderNodeJob.cpp`:
  GPU profiler scope construction/destruction and static render-frame pointer

### renderNodeJob One-Pass Findings

When profiling is enabled, RED defines a scoped GPU profiler helper:

```cpp
class CDynamicPerfScope : public red::NonCopyable
{
public:
    CDynamicPerfScope( CRenderNodeBase* node, Bool cmdListBucketOpen );
    ~CDynamicPerfScope();

private:
    CRenderNodeBase* m_renderNodeImpl;
    Bool             m_cmdListOpen;
    Uint32           m_gpuProfilerEntryIndex;
    Uint32           m_gpuProfilerID;
};
```

The constructor only opens a GPU scope if the command-list bucket is open and
the node allows GPU scopes:

```cpp
m_cmdListOpen = cmdListBucketOpen && m_renderNodeImpl->AllowGpuScope();
```

If enabled, RED starts both a GPU annotation block and a profiler entry:

```cpp
GpuApi::BeginProfilerBlock( name.AsChar() );

m_gpuProfilerEntryIndex = gpuProfiler->AddEntry( name );
m_gpuProfilerID = gpuProfiler->StartEntry( m_gpuProfilerEntryIndex );
```

The destructor closes both:

```cpp
GetRenderer()->GetGpuProfiler()->EndEntry( m_gpuProfilerID );
GpuApi::EndProfilerBlock();
```

LEET should preserve the scoped profiling concept, but adapt it to wgpu. A Rust
shape might be:

```rust
pub struct RenderNodeProfileScope<'a> {
    // closes GPU/debug/profiling scope on drop
}
```

This should map to wgpu debug markers, encoder/pass labels, and whatever
timestamp/profiling infrastructure LEET owns. It should not be a direct clone of
RED's global `GpuApi::BeginProfilerBlock`.

The `AllowGpuScope` distinction matters:

```text
AllowGpuScope controls whether a node may create GPU profiling/debug scopes.
It is not the same thing as whether the node records GPU work.
```

`CRenderNodeJob` stores a static render-frame pointer for jobs:

```cpp
class CRenderNodeJob
{
public:
    static void SetJobsRenderFrame( IRenderFrame *renderFrame )
    {
        ms_jobInitFrame = TRenderPtr<IRenderFrame>::MakePtr( renderFrame );
    }

    static TRenderPtr<IRenderFrame> GetJobsRenderFrame()
    {
        return ms_jobInitFrame;
    }

private:
    static TRenderPtr< IRenderFrame > ms_jobInitFrame;
};
```

and the `.cpp` defines it:

```cpp
TRenderPtr< IRenderFrame > CRenderNodeJob::ms_jobInitFrame;
```

This is RED-era job bootstrap convenience. LEET should not mirror it as global
mutable current-frame state. Rust jobs should receive frame/runtime context
explicitly through their job payload or execution context:

```rust
pub struct RenderGraphExecutionContext<'frame> {
    frame: &'frame RenderFrameRuntime,
    // job/runtime handles
}
```

Locked rule:

```text
Node jobs must receive frame/runtime context explicitly.
Do not introduce a global static current render frame.

GPU profiling/debug scopes belong to graph execution and command recording.
They should respect AllowGpuScope-style node metadata, but map to wgpu debug
markers/profiling infrastructure instead of RED global GPU API calls.
```

## 11. `renderRenderFrame.cpp` Usage Study

`renderRenderFrame.cpp` is not a pure graph-core definition file. It contains
real renderer orchestration and concrete rendering behavior. The useful signal is
how RED uses the graph, context, resource allocator, graph cache, and job
execution together. LEET should not copy this file as the graph core API.

There is no matching `renderRenderFrame.h` in the inspected renderer tree.

### Pass 1: Frame Utilities And `rctx` Usage Helpers

Inspected RED range:

- `renderRenderFrame.cpp` lines ~1-1400:
  config flags, GPU frame timer, texture preview/debug helpers, feature-set
  creation, 2D/final UI helpers, `NewFrame`, `EndFrame`, and `FrameTick`

This pass is not graph topology yet, but it shows what real graph nodes expect
from the context and runtime.

`CreateRenderFeatureSet` builds `RenderFeatureSet` from frame purpose, rendering
mode, camera info, display/debug mode, global rendering settings, platform/backend
capabilities, ray tracing/DLSS/async config, per-camera allowed feature flags,
and dependent-feature rules:

```cpp
set.Set( RFF_GBufferExtraction, ... );
set.Set( RFF_EnableAsyncCompute, true );
set.Set( RFF_RayTracedReflection, ... );
set.Set( RFF_VelocityBuffer, true );
set &= cameraData.GetInfo().m_allowedFeatureFlags;
```

Feature selection is therefore graph-shape input. It must happen before graph
build/cache lookup, because feature flags can decide which nodes and dependency
edges exist.

RED node helpers use the same `rctx` function in preconsume and consume. For
example, fullscreen video records a resource use first, then only emits GPU work
during consume:

```cpp
auto rtFinal = rctx.RT<RFUF_Write>( RENDER_NAME_TAG( "color" ) );

if ( rctx.IsConsumePhase() )
{
    rctx.BindPSO( ColorTarget( 0, rtFinal ) );
    RenderBindManager::SetCamera2D(rctx, width, height );
    m_videoPlayer->RenderFullscreenVideo( rctx );
}
```

This reinforces the existing LEET rule: resource-use operations can run in
preconsume and consume, but real GPU commands must be consume-only.

Some `RT<RFUF_Read>` calls are used specifically to extend resource lifetime:

```cpp
if( rctx.Test( rend::DF_PostprocessExposureLuminance_Debug ) )
{
    auto rtLum = rctx.RT<RFUF_Read>( RENDER_NAME_TAG( "exposureSceneLum" ) );
}
```

RED's nearby comment is:

```cpp
// Put here some render flow render targets that You want to keep until drawing them here
```

So resource-use declarations are not always paired with immediate typed
retrieval in the same local block. LEET must support explicit lifetime/use
marking without requiring an immediate `get_texture` or `get_buffer`.

Texture preview/debug helpers use context access to frame and camera storage:

```cpp
rctx.HasAnyCameraStorage()
rctx.GetCameraStorage()->GetCustomData<RenderTexturePreviewData>()
rctx.GetFrameInfo()
rctx.Test(...)
```

This reinforces that `RenderNodeImplContext` needs controlled access to frame
info, camera storage, debug filters, and custom per-camera/per-scene render data.
That state is not frame-resource allocator storage.

`RenderFinal2D` shows two final-output paths. RED can render directly to the
backbuffer, or use the logical `"color"` graph resource and then present/copy:

```cpp
auto rtFinal = rctx.RT<RFUF_Write>( RENDER_NAME_TAG( "color" ) );
rctx.BindPSO(ColorTarget(0, rtFinal));
rctx.SetViewport(...);

auto rtFinal = rctx.RT<RFUF_Read>( RENDER_NAME_TAG( "color" ) );
GetPostProcess()->PresentCopy(
    rctx,
    rctx.RTGet<GpuApi::TextureRef>( rtFinal ),
    GpuApi::TextureRef(),
    presentSourceRect,
    presentTargetRect );
```

LEET needs the same conceptual split:

```text
graph logical final color
optional direct swapchain/backbuffer output
present/copy step from logical graph resource to actual surface
```

In wgpu this must map to `SurfaceTexture` acquisition and render/copy pass setup,
not to ordinary transient resource allocation.

`FrameTick` creates and submits its own command list:

```cpp
GpuApi::CreateCommandList(...)
GpuApi::BindCommandList(commandList)
...
GpuApi::CloseAndSubmitCommandLists("Submit_FrameTick", ...)
```

It updates persistent renderer subsystems such as skinning, dynamic textures,
hair profiles, gradients, video player, surface cache, glyph cache, uploads, and
other renderer-owned state. This is not the render graph. LEET should not force
all renderer maintenance work into graph topology. Some per-frame renderer
service updates remain outside the graph, even if they record or submit GPU work.

GPU frame timing and profiler scopes are runtime infrastructure. They belong to
execution/profiling, not graph topology.

Locked rule:

```text
Feature selection is graph-build input and must be represented in graph cache
keys.

Render nodes may call resource-use APIs in both preconsume and consume, but real
GPU commands must be consume-only.

Resource use can deliberately extend lifetime even when no immediate typed get
happens at that call site.

RenderNodeImplContext needs controlled frame/camera/debug/custom-data access,
but that data is not frame-resource allocator storage.

FrameTick-style persistent renderer maintenance is outside graph topology, even
if it records or submits GPU work.

Present/backbuffer handling must be explicit and wgpu-shaped, not hidden inside
ordinary transient resource allocation.
```

### Pass 2: Graph Builder Vocabulary In Real Use

Inspected RED range:

- `renderRenderFrame.cpp` lines ~1400-1828:
  unique node ids, sequence node ids, particle simulation helper, blank graph,
  hit-proxy graph, and G-buffer-only graph construction

This pass shows how RED uses `NodeGraphFactory` in real graph construction.

RED defines graph-wide unique node identity slots:

```cpp
enum
{
    UNIQUE_RENDERNODE_StartRender,
    UNIQUE_RENDERNODE_EndRender,
    UNIQUE_RENDERNODE_FlushTextureGrabs,
    UNIQUE_RENDERNODE_FlushBufferGrabs,
    UNIQUE_RENDERNODE_EndFrame,
    UNIQUE_RENDERNODE_FinalFlush,
    UNIQUE_RENDERNODE_Present,
};
```

These are used with `ADD_UNIQUE` and the `RENDER_UNIQUE_*` helpers. LEET should
represent unique/system nodes with stable typed identities, not only string
names. This matters for graph merging, deduplication, and the guarantee that
there is only one `StartRender`, `EndRender`, `Present`, and similar system node
per merged graph where intended.

RED also defines sequence ids:

```cpp
enum
{
    SEQUENCE_RENDERNODE_Camera,
};
```

and uses them to create paired graph anchors:

```cpp
auto cameraStartID = ADD_SEQ_BEGIN(..., SEQUENCE_RENDERNODE_Camera);
auto cameraEndID = ADD_SEQ_END(..., SEQUENCE_RENDERNODE_Camera);
```

This confirms LEET's `GroupEntry` / `GroupExit` naming direction. These are not
dummy nodes. They are stable dependency anchors for a camera sequence.

Command-list groups are a primary graph composition unit:

```cpp
RENDER_COMMAND_LIST( G_Render, "GBufferOnly_Regular" )
{
    ADD_SUBNODE( "BindGlobalConstants", CRenderNode_BindGlobalConstants );
    ADD_SUBNODE( "SetRenderToGbuff_Main", CRenderNode_SetRenderTargetsGBuffer, ... );
    ADD_SUBNODE( "RenderElements_Main", CRenderNode_RenderElements, ... );
    ADD_SUBNODE( "EndRenderToGbuff_Main", CRenderNode_EndRenderTargetsGBuffer );
    ADD_SUBNODE( "UnbindGlobalConstants", CRenderNode_UnbindGlobalConstants );
}
```

A command-list node is a parent execution bucket. Its subnodes are ordered work
inside that bucket. LEET needs this from V1 for parallel command recording
without flattening every operation into one serial list.

RED mixes automatic and manual dependency linking. Simple graphs can use:

```cpp
factory.LinkGPU();
factory.LinkCPU();
```

More controlled graphs use automatic GPU ordering and explicit CPU/group edges:

```cpp
factory.LinkGPU();
factory.LinkCPUToNextGPU( startRenderID );

factory.Link( startRenderID, cameraStartID, RNDT_Cpu );
factory.Link( cameraStartID, G_PreRender, RNDT_Cpu );
factory.Link( G_PreRender, G_Culling, RNDT_Cpu );
factory.Link( G_Culling, G_Render, RNDT_Cpu );
```

So the factory has two layers:

```text
convenience auto-linking by insertion/group order
explicit dependency edges for correctness-critical phase order
```

LEET should support both. Explicit links must be the authority where correctness
matters.

Group ids encode coarse execution stages:

```cpp
const rend::NodeGroupID G_None    = rend::NodeGroupID::None;
const rend::NodeGroupID G_Render  = rend::NodeGroupID(1);
const rend::NodeGroupID G_PreRender = rend::NodeGroupID(2);
const rend::NodeGroupID G_Culling = rend::NodeGroupID(3);
```

and groups can be linked directly:

```cpp
factory.Link( G_PreRender, G_Culling, RNDT_Cpu );
factory.Link( G_Culling, G_Render, RNDT_Cpu );
```

Groups are therefore dependency and scheduling units, not merely visual labels.

Conditional graph construction is normal:

```cpp
if( isPrewarm )
{
    ADD_SUBNODE( "RenderSkyScattering", CRenderNode_RenderSkyScattering );
    ADD_SUBNODE( "ReflectionProbes", CRenderNode_ReflectionProbes );
}
```

Hit-proxy and G-buffer-only graphs also branch on debug, multilayer, extraction,
and mode-specific behavior. Inputs that affect graph-build branches must be in
the graph cache key.

Some conditions are runtime predicates inside nodes:

```cpp
ADD_SUBNODE(
    "RenderElements",
    CRenderNode_RenderElements,
    []( const SRenderNodeImplContext &rctx ) {
        return rctx.Test( rend::DF_RenderMask_GBufferLate );
    },
    ...
);
```

LEET should distinguish:

```text
graph-build branch -> affects topology/cache key
node predicate -> affects execution behavior but graph shape stays the same
```

Runtime predicates are allowed, but they must not cause preconsume and consume
request streams to diverge.

The G-buffer-only graph also shows explicit resource dependency scope nodes:

```cpp
ADD_SUBNODE( "CamResDepScopeOpen", CRenderNode_CameraResourceDependencyScope, true );
...
ADD_SUBNODE( "CamResDepScopeClose", CRenderNode_CameraResourceDependencyScope, false );
```

LEET should support explicit scope nodes rather than hiding all lifetime and sync
scope behavior in ad hoc allocator calls.

Locked rule:

```text
Unique/system node ids must be stable typed identities, not only strings.

GroupEntry/GroupExit sequence nodes are real dependency anchors.

Command-list parent nodes with ordered subnodes are part of the V1 graph model.

Graph groups are dependency/scheduling units, not just labels.

Graph-build branches must be represented in graph cache keys.

Runtime node predicates are allowed, but they must not silently change the
resource request stream between preconsume and consume.

Explicit resource/sync scope nodes are valid graph nodes and should remain
visible in the graph model.
```

### Pass 3.1: Main Camera Graph Setup And Async Decisions

Inspected RED range:

- `renderRenderFrame.cpp` lines ~1829-2418:
  start of `BuildRenderGraphCamera`, target mode constants, graph groups,
  feature/async decisions, render-prep nodes, culling groups, depth/G-buffer
  entry, and early async compute branches

This is the beginning of RED's normal camera graph.

At the top, RED defines target-mode combinations:

```cpp
constexpr targetMod rt_GBuffer_NoClear = targetMod::WriteDepth_YES;

constexpr targetMod rt_Unlit =
    targetMod::SamplableNormalsFeedback |
    targetMod::WeaponDepthCorrected_NO |
    targetMod::PostAA_NO |
    targetMod::TransparencyTXAAMask_NO |
    targetMod::VelocityBuffer_NO;
```

These are not allocator descriptors. They are higher-level render-target setup
policies consumed by nodes such as:

```cpp
CRenderNode_SetRenderTargetsGBuffer
CRenderNode_EndRenderTargetsGBuffer
CRenderNode_SetRenderTargetsGBufferWithVelocityBuffer
```

LEET should keep this layer above the frame resource allocator. The allocator
owns descriptors, lifetimes, resolved textures, and resolved buffers. The
render-target/pass setup layer decides which resolved resources become
attachments, storage views, sampled feedback inputs, or read-only depth views.

The main camera graph defines many scheduling groups:

```cpp
const rend::NodeGroupID G_CullingScene         = rend::NodeGroupID( 1 );
const rend::NodeGroupID G_CullingRayTracing    = rend::NodeGroupID( 2 );
const rend::NodeGroupID G_CullingCascades      = rend::NodeGroupID( 3 );
const rend::NodeGroupID G_CullingLocalShadows  = rend::NodeGroupID( 4 );
const rend::NodeGroupID G_PostCullingDebug     = rend::NodeGroupID( 5 );
const rend::NodeGroupID G_RenderPrep           = rend::NodeGroupID( 6 );
const rend::NodeGroupID G_Render               = rend::NodeGroupID( 7 );
const rend::NodeGroupID G_ParticlesOnScreenSim = rend::NodeGroupID( 8 );
const rend::NodeGroupID G_ParticlesOffScreenSim = rend::NodeGroupID( 9 );
const rend::NodeGroupID G_PostRender           = rend::NodeGroupID( 10 );
```

The graph is not a flat list and not simply `pre / render / post`. Groups are
scheduling partitions that expose parallelism and provide explicit dependency
targets.

RED decides async compute behavior before adding many nodes:

```cpp
const Bool doAsyncCompute = Config::cvAsyncComputeEnable.Get()
    && cameraFeatures.Get( RFF_EnableAsyncCompute );

const Bool doAsyncHairClear = Config::cvAsyncHairClears.Get() && doAsyncCompute;
const Bool doAsyncSSAO = cameraFeatures.Get( RFF_SSAO )
    && Config::cvAsyncSSAO.Get()
    && doAsyncCompute;
const Bool doAsyncBuildDepthChain = Config::cvAsyncBuildDepthChain.Get()
    && doAsyncCompute;
```

Those booleans change graph topology:

```cpp
if ( doAsyncHairClear )
{
    SYNC_SUBMIT( G_None, "RenderGraphCamera_GBuffer", GpuApi::CommandListSyncType::ForkAsyncCompute );

    COMPUTE_COMMAND_LIST( G_None, "Hair_RT_Clears" )
    {
        ADD_SUBNODE( "Hair_ClearDepths_CopySceneDepth", CRenderNode_HairFullscreenPass, ... );
    }
}
```

Therefore async decisions are graph-build inputs. LEET must include them in
graph cache invalidation when they change.

RED also treats backend sync capability as graph-shape input:

```cpp
#ifdef GPUAPI_EXTENDED_SYNC
constexpr GpuApi::InterSyncPoint sync_DepthChain = GpuApi::InterSyncPoint(1);
#endif
```

and async SSR depends on backend clear support:

```cpp
#if defined(GPUAPI_CLEAR_CAN_RUN_ON_ASYNC_COMPUTE) && defined(RED_PLATFORM_DURANGO)
const Bool doAsyncSSR = true;
#else
const Bool doAsyncSSR = false;
#endif
```

wgpu does not expose RED's exact multi-queue sync model, but backend scheduling
capability can still affect graph construction. LEET should keep explicit
scheduling/sync abstractions even if the first wgpu backend maps them
conservatively to ordered command-buffer submission.

Render prep opens the camera resource dependency scope and prepares global state:

```cpp
auto renderPrepID = RENDER_COMMAND_LIST( G_RenderPrep, "RenderPrep" )
{
    ADD_SUBNODE( "CamResDepScopeOpen", CRenderNode_CameraResourceDependencyScope, true );
    ADD_SUBNODE( "PrepSceneRendering", CRenderNode_PrepareSceneRendering );
    ADD_SUBNODE( "SkinningCompute", CRenderNode_ComputeSkinningAndTangentUpdates );
    ADD_SUBNODE( "UpdateGlobalBindings", CRenderNode_UpdateGlobalBindings );
}
```

This reinforces that resource and synchronization scopes are first-class graph
nodes, not hidden context side effects.

Culling is split across multiple groups:

```cpp
ADD_NODE( G_CullingScene, "DoCulling", ... );
ADD_NODE( G_CullingRayTracing, "DoCullingRayTracing", ... );
ADD_NODE( G_CullingCascades, "DoCullingCascades", ... );
ADD_NODE( G_CullingLocalShadows, "DoCullingLocalShadows", ... );
```

LEET should not collapse all culling into one opaque phase. Different culling
outputs feed different graph branches.

Feature flags drive topology. For example, velocity buffer support adds extra
G-buffer command lists:

```cpp
if ( cameraFeatures.Get( RFF_VelocityBuffer ) )
{
    RENDER_COMMAND_LIST( G_Render, "GBufferEarly_Velocity_Weapon" )
    {
        ...
    }
}
```

and debug coloring adds extra G-buffer debug passes in non-final builds:

```cpp
if ( cameraFeatures.Get( RFF_DebugColoring ) )
{
    RENDER_COMMAND_LIST( G_Render, "GBuffer_DebugColor_Solid" ) { ... }
}
```

Feature flags that create or remove nodes must be graph cache inputs.

Locked rule:

```text
Target modes belong to render-target/pass setup, not directly to allocator
identity.

Async compute decisions are graph-build decisions and must invalidate graph
cache when they change.

Backend scheduling capability can affect graph shape; keep explicit
sync/schedule abstractions even if wgpu maps them conservatively.

Main camera graph requires many scheduling groups from V1, not a flat pass list.

Resource dependency scopes are first-class graph nodes.

Feature flags that create or remove nodes are graph cache inputs.
```

### Pass 3.2: Mid Main-Camera Graph, Async Work, Lighting, Forward, And Post

Inspected RED range:

- `renderRenderFrame.cpp` lines ~2418-3088:
  async compute during shadowmaps, shadow command lists, lighting, async SSR,
  debug display branches, transparent/forward passes, TXAA/post effects, and
  finalization start

RED explicitly forks and joins async compute around graphics work:

```cpp
SYNC_SUBMIT(... ForkAsyncCompute);

COMPUTE_COMMAND_LIST( G_Render, "AsyncComputeDuringShadowmaps" )
{
    if ( doAsyncFlattenNormals ) { ... }
    if ( doAsyncBuildDepthChain ) { ... }
    if ( doAsyncSSAO ) { ... }
    if ( doAsyncLutGeneration ) { ... }
    if ( doAsyncDynamicTextureGeneration ) { ... }
}
```

and later:

```cpp
SYNC_SUBMIT(... JoinAsyncCompute);
```

LEET should keep graph-level concepts for work that may run in a separate
scheduling lane and for later work that depends on it. The wgpu backend may map
this conservatively to ordered command-buffer submission, but the graph topology
should not erase the dependency.

With extended sync, RED also uses intermediate sync points:

```cpp
ADD_SUBNODE( "SignalDepthChainComplete", CRenderNode_SignalIntermediateSyncPoint, sync_DepthChain );
```

and later waits before lighting:

```cpp
ADD_SUBNODE( "WaitDepthChainComplete", CRenderNode_WaitIntermediateSyncPoint, sync_DepthChain,
    []( const SRenderNodeImplContext& rctx )
    {
        auto rtHiDepth = rctx.RT< RFUF_Read >( "hierarchicalDepthBuffer" );
        auto rtMaxDepth = rctx.RT< RFUF_Read >( "maxDepthBuffer" );
        auto rtNormals  = rctx.RT< RFUF_Read >( "normals" );
        if ( rctx.IsConsumePhase() )
        {
            GpuApi::TellTextureUAV( rtHiDepth );
            GpuApi::TellTextureUAV( rtMaxDepth );
            GpuApi::TellTextureUAV( rtNormals );
        }
    } );
```

That callback shows that sync nodes can carry resource-state knowledge, not only
execution ordering. LEET will not call `TellTextureUAV` in wgpu, but it still
needs to know when compute-written resources become readable by later passes.

Shadow maps are created as many independent command-list nodes:

```cpp
for ( Uint32 cascadeIndex = 0; cascadeIndex < NumShadowCascades; cascadeIndex++ )
{
    RENDER_COMMAND_LIST( G_Render, "RenderCascade%u" )
    {
        ADD_SUBNODE( "BindGlobalConstants", CRenderNode_BindGlobalConstants );
        ADD_SUBNODE( "RenderCascade0", CRenderNode_RenderShadowCascade, cascadeIndex );
        ADD_SUBNODE( "UnbindGlobalConstants", CRenderNode_UnbindGlobalConstants );
    }
}

for ( Uint32 lightIndex = 0; lightIndex < LocalShadowsProcessedPerFrame; lightIndex++ )
{
    RENDER_COMMAND_LIST( G_Render, "RenderLocalShadows%u" )
    {
        ADD_SUBNODE( "BindGlobalConstants", CRenderNode_BindGlobalConstants );
        ADD_SUBNODE( "RenderLocalShadows", CRenderNode_RenderLocalShadowMaps, lightIndex );
        ADD_SUBNODE( "UnbindGlobalConstants", CRenderNode_UnbindGlobalConstants );
    }
}
```

This is a concrete parallel-recording pressure case. LEET must support dynamic
repeated command-list nodes from V1.

Lighting is split differently when async SSR is enabled. The normal path runs
SSR and part of light integration in one lighting node:

```cpp
ADD_SUBNODE( "ScreenSpaceReflections", CRenderNode_ScreenSpaceReflections );
ADD_SUBNODE( "IntegrateLightsRender", CRenderNode_RenderLightsIntegrate, RenderLightsIntegratePass::CalcGI );
```

The async path forks compute SSR, runs a limited graphics lighting pass, joins,
then runs late lighting:

```cpp
SYNC_SUBMIT( G_None, "RenderGraphCamera_ForkSSR", GpuApi::CommandListSyncType::ForkAsyncCompute );

COMPUTE_COMMAND_LIST( G_Render, "AsyncSSRDuringGI" )
{
    ADD_SUBNODE( "ScreenSpaceReflections", CRenderNode_ScreenSpaceReflections );
}

RENDER_COMMAND_LIST( G_Render, "LightingMid" )
{
    ADD_SUBNODE( "IntegrateLightsRender", CRenderNode_RenderLightsIntegrate, RenderLightsIntegratePass::CalcGI_LimitedOccupancy );
}

SYNC_SUBMIT( G_None, "RenderGraphCamera_SSR", GpuApi::CommandListSyncType::JoinAsyncCompute );

RENDER_COMMAND_LIST( G_Render, "LightingLate" )
{
    ADD_SUBNODE( "IntegrateLightsRender", CRenderNode_RenderLightsIntegrate, RenderLightsIntegratePass::Integrate );
}
```

Feature/backend choices can therefore split one conceptual pass into multiple
graph nodes with different scheduling.

Binding and unbinding nodes are repeated structural nodes:

```cpp
ADD_SUBNODE( "BindGlobalConstants", CRenderNode_BindGlobalConstants );
ADD_SUBNODE( "BindLightingGlobalConstants", CRenderNode_BindLightingGlobalConstants );
...
ADD_SUBNODE( "UnbindLightingGlobalConstants", CRenderNode_UnbindLightingGlobalConstants );
ADD_SUBNODE( "UnbindGlobalConstants", CRenderNode_UnbindGlobalConstants );
```

LEET's wgpu layer should make this cleaner through bind groups and pass setup,
but the graph must still express setup/teardown ordering around command-list
bodies.

Feedback buffers are explicit graph operations:

```cpp
ADD_SUBNODE( "PrepareFeedbackNormalBuffer", CRenderNode_PrepareFeedbackNormalBuffer );
ADD_SUBNODE( "PrepareFeedbackColorBuffer", CRenderNode_PrepareFeedbackColorBuffer, true );
ADD_SUBNODE( "PrepareFeedbackSSR", CRenderNode_PrepareFeedbackSSRBuffer );
```

These are resource-flow nodes. They should remain explicit nodes rather than
hidden helper calls.

Debug display modes can replace or add whole render sections:

```cpp
if ( displayMode == EMM_DecalOverdraw ) { ... }
else if ( displayMode == EMM_ParticleOverdraw ) { ... }
else if ( displayMode == EMM_ParticleNumLights ) { ... }
```

These branches affect graph topology and must be graph cache inputs.

The same graph vocabulary covers:

```text
shadows
lighting
compute
transparent/forward
TXAA/post
debug
final/HUD
```

LEET should not introduce a separate post-process graph architecture unless a
real need appears. The node/group/command-list model is general enough.

Locked rule:

```text
Fork/join async compute is part of graph topology, even if the wgpu backend
initially serializes it.

Intermediate sync nodes may carry resource-state meaning, not only scheduling
meaning.

Repeated/dynamic command-list node creation must be supported from V1.

Feature/backend choices may split conceptual passes into multiple graph nodes.

Feedback/copy/preparation operations should remain explicit graph nodes.

Debug display modes that change topology must be graph cache inputs.

The same graph model should cover opaque, shadows, lighting, transparents,
debug, and post-processing.
```

### Pass 3.3: Main Camera Graph Tail, Finalization, And End Links

Inspected RED range:

- `renderRenderFrame.cpp` lines ~3088-3296:
  final/HUD work, color/depth extraction, camera-scope close, unique tail nodes,
  and explicit dependency linking for the normal camera graph

The camera resource dependency scope closes in post-render/final 2D work:

```cpp
RENDER_COMMAND_LIST( G_PostRender, "RenderFinal2D" )
{
    ADD_SUBNODE( "RenderFinal2D", CRenderNode_RenderFinal2D, FEATFLAG_Default );

    if ( cameraFeatures.Get( RFF_ColorExtraction ) )
    {
        ADD_SUBNODE( "ExtractionFinalColor", CRenderNode_ExtractionFinalColor );
    }

    ADD_SUBNODE( "CamResDepScopeClose", CRenderNode_CameraResourceDependencyScope, false );
}
```

That means the camera resource lifetime/scope spans nearly the whole camera
graph, from render prep through final 2D/extraction. LEET should keep this as a
graph-visible lifetime bracket.

Extraction and grab nodes sit after camera rendering or present depending on the
resource:

```cpp
if( cameraFeatures.Get( RFF_DepthExtraction ) )
{
    extractDepthGrabID = RENDER_UNIQUE_SIMPLE_COMMAND_LIST(
        G_None,
        "ExtractionDepthBuffGrab",
        UNIQUE_RENDERNODE_ExtractDepthGrab,
        CRenderNode_ExtractionDepthBufferGrab
    );
}
```

Texture grabs are conditional:

```cpp
if ( hasFlushGrabs )
{
    flushTexGrabsID = ADD_UNIQUE(
        G_None,
        "FlushTextureGrabs",
        UNIQUE_RENDERNODE_FlushTextureGrabs,
        CRenderNode_FlushTextureGrabs
    );
}
```

Extraction, readback, and grab behavior is graph work, but it is not ordinary
transient allocation. LEET should keep it as explicit graph nodes and not hide it
inside `get_texture` or `get_buffer`.

The normal camera graph ends with stable unique lifecycle nodes:

```text
EndRender
FlushGpu
Present
FlushTextureGrabs
CleanupBatchDataAllocator
EndFrame
```

These are not helper calls outside the graph. RED represents them as unique nodes
and links them. LEET should preserve lifecycle nodes for final submission/flush,
present, cleanup, and end-frame accounting.

RED can choose a conservative full CPU ordering path:

```cpp
// This pass requires full CPU synchronization due to injected command flushes
if( cameraFeatures.Get( RFF_GBufferExtraction ) )
{
    factory.LinkCPU();
}
```

So the graph builder must be able to collapse parallelism for correctness when
injected flushes, readbacks, or extraction behavior complicate ordering.

The bridge helpers are critical:

```cpp
factory.LinkCPUToNextGPU( startRenderID );
...
factory.LinkCPUToPreviousGPU( cameraEndID );
...
factory.LinkCPUToPreviousGPU( finalFlushID );
```

RED explains the `cameraEndID` bridge:

```cpp
// Link end-of-camera to any gpu work coming before it. This is needed in case we have multiple cameras,
// using async compute. Otherwise we don't seem to get the correct sequencing of command lists and syncs
// between the camera subgraphs.
```

LEET needs equivalent graph-builder helpers that can connect CPU nodes to
surrounding GPU/command-list work. This matters when multiple camera subgraphs
are merged; otherwise command-list and async/sync sequencing from one camera can
leak incorrectly into another.

Explicit links encode cross-branch dependencies that auto-linking cannot infer:

```cpp
factory.Link( prepDistantShadowsID, idLighting, RNDT_Cpu );
factory.Link( idDecoupledParticleLighting, idTransparents, RNDT_Cpu );
factory.Link( G_PostRender, cleanupBatchDataAllocatorID, RNDT_Cpu );
factory.Link( renderDistantShadowsID, cleanupBatchDataAllocatorID, RNDT_Cpu );
factory.Link( windUpdateAndRainMapID, cleanupBatchDataAllocatorID, RNDT_Cpu );
```

Debug shadow links also connect dynamically generated command lists:

```cpp
for ( CRenderNodeGraph::ItemId id : idsRenderLocalShadowmaps )
{
    factory.Link( id, idShadowsDebug, RNDT_Cpu );
}
```

Auto-linking is therefore not enough for a production graph. The builder must
support explicit node-to-node, group-to-node, node-to-group, group-to-group, and
dynamic-list links.

Cleanup is dependency-driven, not merely appended:

```cpp
factory.Link( G_PostRender, cleanupBatchDataAllocatorID, RNDT_Cpu );
factory.Link( renderDistantShadowsID, cleanupBatchDataAllocatorID, RNDT_Cpu );
factory.Link( windUpdateAndRainMapID, cleanupBatchDataAllocatorID, RNDT_Cpu );
factory.Link( cleanupBatchDataAllocatorID, endFrameID, RNDT_Cpu );
```

Locked rule:

```text
Camera resource dependency scope spans most of the camera graph and should be a
visible lifetime bracket.

Extraction/readback/grab work must be explicit graph nodes, not hidden in
getters.

Lifecycle tail nodes such as EndRender, FlushGpu, Present, FlushTextureGrabs,
cleanup, and EndFrame should be explicit unique graph nodes.

The graph builder must support conservative full CPU ordering when extraction or
injected flushes require it.

CPU-to-previous-GPU and CPU-to-next-GPU bridge links are required for correct
merged multi-camera ordering.

Explicit links must support node/group combinations and dynamically generated
node lists.

Cleanup/end-frame nodes must be dependency-driven, not merely appended.
```

### Pass 4: Alternate Graph Shapes

Inspected RED range:

- `renderRenderFrame.cpp` lines ~3297-4021:
  `BuildRenderGraphSafeMode`, `BuildRenderGraphNoScene`,
  `BuildRenderGraphTodvis`, and `BuildRenderGraphDebugVisualization`

These are not separate graph systems. They reuse the same factory vocabulary
with smaller or different topology.

Alternate graphs preserve the same lifecycle spine:

```text
StartRender
CameraStart
prep / collector / culling / render groups
CameraEnd
EndRender
FlushGpu
Present
FlushTextureGrabs
CleanupBatchDataAllocator
EndFrame
```

Not every graph uses every optional node, but the shape is consistent. LEET's
graph core should not have separate safe-mode, debug, no-scene, or TODVIS graph
architectures. They are graph builds using the same primitives.

Reduced graphs still use command-list parents and subnodes:

```cpp
RENDER_COMMAND_LIST( G_RenderPrep, "PrepRendering" )
{
    ADD_SUBNODE( "CamResDepScopeOpen", CRenderNode_CameraResourceDependencyScope, true );
    ADD_SUBNODE( "PrepSceneRendering", CRenderNode_PrepareSceneRendering );
    ADD_SUBNODE( "PrepareDummyData", CRenderNode_PrepareDummyLightingData );
}
```

and:

```cpp
RENDER_COMMAND_LIST( G_Render, "RenderGraph_NoScene" )
{
    ADD_SUBNODE( "UpdateGlyphCacheTexture", CRenderNode_UpdateGlyphCacheTexture );
    ADD_SUBNODE( "BindGlobalConstants", CRenderNode_BindGlobalConstants );
    ADD_SUBNODE( "ClearFinalTarget", CRenderNode_ClearFinalTarget );
    ADD_SUBNODE( "RenderFinal2D", CRenderNode_RenderFinal2D, baseFeatureFlag );
}
```

Even tiny graphs should use the same parent/subnode model.

Safe mode includes fallback renderer data:

```cpp
ADD_SUBNODE( "PrepareDummyData", CRenderNode_PrepareDummyLightingData );
```

This is not a meaningless graph placeholder. It prepares fallback renderer data
so later nodes can operate through a simplified/safe rendering path. LEET should
avoid `Dummy` for graph structure names, but fallback-data preparation nodes are
valid graph work when the renderer needs fallback resources.

No-scene graph still has collector/debug/final/present behavior:

```cpp
ADD_NODE( G_PreRender, "DeclCommonResAllocs", CRenderNode_DeclareCommonResourceAllocs_FinalOnly );
auto idCollector = ADD_NODE( G_PreRender, "PrepCollector", CRenderNode_PrepareCollector, baseFeatureFlag );
auto idCollectDebugHighPriority = ADD_NODE( G_PreRender, "DebugCollectHighPriority", CRenderNode_CollectDebug, true );
auto idCollectDebugLowPriority = ADD_NODE( G_PreRender, "DebugCollectLowPriority", CRenderNode_CollectDebug, false );
```

So no-scene means a graph that renders final/debug/UI output without world scene
content. It does not mean no graph.

TODVIS is extraction-oriented:

```cpp
ADD_SUBNODE( "ExtractionSelection_TODVIS", CRenderNode_ExtractionSelection );
ADD_SUBNODE( "ExtractionFinalColor_TODVIS", CRenderNode_ExtractionFinalColor );
```

and only presents in debug view:

```cpp
if ( isDebugView )
{
    presentID = ADD_UNIQUE( G_None, "Present", UNIQUE_RENDERNODE_Present, CRenderNode_Present );
}
```

Present is therefore optional. Extraction/readback graph variants may produce
CPU-visible or external results without presenting to the surface.

Debug visualization changes mode, feature flag, and target mode choices, but
still uses the standard primitives:

```text
ADD_SEQ_BEGIN
RENDER_COMMAND_LIST
ADD_SUBNODE
ADD_SEQ_END
ADD_UNIQUE
factory.LinkGPU()
factory.LinkCPUToNextGPU(...)
factory.Link(...)
```

There is no need for a separate debug-render graph architecture. The graph
builder just needs to be flexible enough.

Alternate graphs still use explicit links:

```cpp
factory.Link( G_RenderPrep, G_Render, RNDT_Cpu );
factory.Link( G_Render, G_PostRender, RNDT_Cpu );
factory.Link( G_PostRender, cameraEndID, RNDT_Cpu );
```

and no-scene uses:

```cpp
factory.Link( G_PreRender, G_Render, RNDT_Cpu );
factory.Link( G_Render, cameraEndID, RNDT_Cpu );
```

Explicit linking remains necessary even in reduced graphs.

Camera resource scope placement can vary. Normal/safe graphs close the scope in
post-render/final 2D, while debug visualization closes it inside its HUD/debug
command list:

```cpp
ADD_SUBNODE( "CamResDepScopeClose", CRenderNode_CameraResourceDependencyScope, false );
```

LEET should not hardcode that a scope closes in a particular node. Scope open and
scope close are graph nodes placed by the graph builder.

Locked rule:

```text
Alternate graph modes must use the same graph primitives as the main camera
graph.

Small/reduced graphs still use command-list parent nodes and ordered subnodes.

Fallback renderer data preparation is valid graph work, but graph structural
anchors should not be named dummy.

Present is optional; extraction/readback graph variants may flush results without
presenting.

Debug/no-scene/safe-mode rendering should not require a separate graph core
architecture.

Camera resource scope open/close placement is graph-builder controlled, not
hardcoded by the graph runtime.
```

### Pass 5: `StartFrameRendering` And `EndFrameRendering`

Inspected RED range:

- `renderRenderFrame.cpp` lines ~4022-4251:
  `StartFrameRendering` and `EndFrameRendering`

These functions are the runtime bodies behind the graph lifecycle nodes
`StartRender` and `EndRender`.

`StartFrameRendering` does real frame/runtime setup:

```cpp
NewFrame( info );
```

It computes resolution-dependent mip bias from the current context:

```cpp
internalRenderHeight = rctx.GetHeight();
finalRenderHeight = rctx.GetFinalHeight();
mipBias = log2(internalRenderHeight / finalRenderHeight);
GpuApi::ResetSamplerStates();
```

and then builds backend render settings:

```cpp
GpuApi::RenderSettings renderSettings;
renderSettings.anisotropy = ...;
renderSettings.mipMapBias = ...;
renderSettings.allowImmediatePSOs = ...;
GpuApi::SetRenderSettings( renderSettings );
```

So `StartRender` is not only a marker or profiling label. LEET should model it
as a real lifecycle node/system step that prepares frame render settings and
runtime state.

`StartFrameRendering` uses the normal node implementation context:

```cpp
rctx.GetFrameInfo()
rctx.IsCameraDataAvailable()
rctx.GetHeight()
rctx.GetFinalHeight()
```

Lifecycle nodes should therefore use `RenderNodeImplContext` too, not a separate
special context.

Start also prepares renderer-owned persistent systems:

```cpp
GetRenderer()->GetBatchDataAllocator().NewAllocFrame();
scene->GetVisibilityScene().PrepareForQueries();
scene->TickProxies(...);
scene->SetTickCollectionEnabled(...);
scene->BeginRendering();
```

These are not transient frame-resource allocator operations. They are
renderer/frame/scene runtime operations that need a lifecycle slot in the graph.

RED opens GPU frame/render bookkeeping:

```cpp
m_GpuFrameTimer.BeginSceneRender(...);
GpuApi::BeginRender();
GpuApi::SetupBlankRenderTargets();
```

In LEET/wgpu this should map to frame/render runtime bookkeeping and command
recording setup where appropriate, not necessarily to a literal global
`BeginRender`. The important concept is that graph execution has a frame-render
begin boundary.

`EndFrameRendering` gathers stats, resets collectors, and closes scene state:

```cpp
for ( Uint32 camera_i = 0; camera_i < rctx.GetNumCameras(); ++camera_i )
{
    CRenderCollector& collector = rctx.GetCameraDataByIndex( camera_i )->GetCollector();
    if ( collector.m_scene )
    {
        collector.m_scene->UpdateSceneStats( collector.m_renderingStats );
    }
}

for ( Uint32 camera_i = 0; camera_i < rctx.GetNumCameras(); ++camera_i )
{
    rctx.GetCameraDataByIndex( camera_i )->GetCollector().Reset();
}
```

It also closes scene and visibility state:

```cpp
scene->TickCollectedProxies( rctx );
scene->EndRendering();
scene->GetVisibilityScene().FinishQueries();
```

and ends frame/profiler/GPU bookkeeping:

```cpp
EndFrame();
m_gpuProfiler->OnFrameEnd();
m_GpuFrameTimer.EndSceneRender(...);
GpuApi::EndRender();
```

`EndRender` is unique/global work. It iterates all cameras, updates stats, and
resets collectors. This reinforces the rule that unique/global nodes need
indexed/all-camera access through the context.

Visibility query lifecycle wraps the graph:

```cpp
visScene.PrepareForQueries();
...
visScene.FinishQueries();
```

LEET should keep scene/visibility lifecycle as explicit frame runtime state, not
bury it only inside culling nodes.

Locked rule:

```text
StartRender and EndRender are real lifecycle nodes with runtime side effects.

Lifecycle nodes use RenderNodeImplContext too, including frame/camera/global
access.

StartRender prepares renderer-owned frame state, render settings, visibility
query state, scene tick/render state, and profiling/timing boundaries.

EndRender closes scene/render state, updates stats, resets collectors, finishes
visibility queries, ends profiling/timing, and closes frame render bookkeeping.

EndRender is unique/global and needs all-camera access.

These lifecycle operations are not frame-resource allocator cleanup and should
not be hidden in Drop/destructors.
```

### Pass 6: Job Dispatch And Dependency Execution

Inspected RED range:

- `renderRenderFrame.cpp` lines ~4252-4481:
  `RunRenderJobParameter`, `RunRenderJob`, `RenderJobNode`,
  `LinkRenderNodeGraphDependencies`, `FinishContinuation`,
  `DebugSharedJobsCounter`, `DispatchRenderJobs`, and `RunRenderNodeJobs`

This is the execution bridge that turns a built `CRenderNodeGraph` into
scheduled render jobs.

Each graph node becomes one render job. RED stores the node context and node
parameters in a job payload:

```cpp
struct RunRenderJobParameter
{
    const SRenderNodeContext nodeContext;
    const SRenderNodeParameters nodeParameters;
};
```

Each job constructs its own implementation context:

```cpp
SRenderNodeImplContext::SInitData nodeImplContextInitData( runContext.dispatcherThreadIndex );
nodeImplContextInitData.m_frame       = CRenderNodeJob::GetJobsRenderFrame();
nodeImplContextInitData.m_resAlloc    = GetRenderer()->GetRenderFlowAllocator();
nodeImplContextInitData.m_cameraIndex = param.nodeContext.m_cameraIndex;

SRenderNodeImplContext nodeImplContext;
nodeImplContext.Init( nodeImplContextInitData );
nodeImplContext.SetupNodeData( param.nodeContext, param.nodeParameters );

renderNode->Process(nodeImplContext, &depBuilder);
```

LEET's render graph execution job should receive or copy:

```text
node context
node parameters / node implementation handle
frame runtime
resource allocator handle
camera index
worker index
job builder / child-job context
```

This confirms that `RenderNodeImplContext` is constructed per node execution. It
is not one shared mutable global context.

RED creates one `RenderJobNode` per graph node:

```cpp
struct RenderJobNode
{
    job::Counter nodeCounter{ job::Priority::RenderPath };
    job::Counter incomingDependenciesCounter{ job::Priority::RenderPath };
    Uint32 numIncomingDependencies{0};
    Uint32 numOutgoingDependencies{0};
};
```

CPU graph dependencies become job counter dependencies:

```cpp
if ( RNDT_Cpu != depData.m_type )
{
    continue;
}

nodes[ childIdx ].incomingDependenciesCounter += nodes[ parentIdx ].nodeCounter;
nodes[ childIdx ].numIncomingDependencies += 1;
nodes[ parentIdx ].numOutgoingDependencies += 1;
```

Only `RNDT_Cpu` edges are consumed by job scheduling here. GPU edges are used by
the graph/command-list/sync model, not directly as job wait counters. LEET must
keep this separation:

```text
CPU dependency -> job scheduling dependency
GPU dependency -> command-list/order/sync dependency
```

Root nodes are gated by an external render-start dependency:

```cpp
if ( node.numIncomingDependencies == 0 )
{
    node.incomingDependenciesCounter += renderJobsExternalDependency;
}
```

This external dependency comes from outside graph execution, such as draw-buffer
readiness or a frame kickoff counter. LEET should expose this explicitly in the
graph execution API.

RED has minimal bad-graph assertions:

```cpp
RED_FATAL_ASSERT( nodes.Empty() || syncedToRenderStart, "Cycle in render graph? All nodes have incoming dependencies" );
```

and:

```cpp
RED_FATAL_ASSERT( !syncedToRenderFinish, "Render graph should have at most one finish job to end the frame" );
```

This confirms execution expects a valid DAG with a clear finish. LEET should do
better with strict DAG/cycle validation before dispatch.

Frame completion is tied to terminal graph nodes:

```cpp
if ( node.numOutgoingDependencies == 0 )
{
    renderFinishedCounter += node.nodeCounter;
}
```

and then:

```cpp
depBuilder.DispatchWait(renderFinishedCounter);
```

LEET should expose a graph execution completion handle, counter, or future tied
to terminal graph node completion.

RED wires dependencies first, then schedules all jobs:

```cpp
LinkRenderNodeGraphDependencies( graph, nodes, renderJobsExternalDependency );
FinishContinuation(nodes, renderFinishedCounter);
DispatchRenderJobs( graph, nodes, debugCounter );
```

Each job is submitted with its dependency counter and completion counter:

```cpp
job::RunJob( jobDecl, waitForZeroCounter, accumulateCounter );
```

This lets the job system release work as dependencies become ready. LEET should
schedule the DAG, not manually walk it serially.

RED creates completion deferrals for dependency-counter lifetime:

```cpp
deferrals.EmplaceBack(
    RenderJobCompletionDeferral{
        it.nodeCounter.CreateDeferral(),
        it.incomingDependenciesCounter.CreateDeferral()
    }
);
```

The exact Rust job-system mechanics may differ, but counter/handle state must
live long enough for all scheduled jobs and wait continuations.

RED also tracks a debug count to catch premature finish:

```cpp
debugCounter.Decrement();
RED_FATAL_ASSERT(
    !debugIsFrameFinishJob || debugCounter.IsZero_Snapshot(),
    "Finished frame before all render jobs completed!"
);
```

LEET should keep similar debug validation: terminal node completion must not
happen before all scheduled render jobs complete.

RED passes frame runtime through a static helper:

```cpp
CRenderNodeJob::GetJobsRenderFrame()
```

LEET should not mirror that global. Frame/runtime context must be passed
explicitly to jobs.

Locked rule:

```text
Each graph node execution becomes a scheduled job with its own
RenderNodeImplContext.

CPU dependency edges become job wait-counter dependencies.

GPU dependency edges remain command-list/order/sync dependencies and should not
be confused with CPU job dependencies.

Graph execution needs an explicit external kickoff dependency.

Graph execution must produce a completion handle tied to terminal graph nodes.

All node jobs can be scheduled up front once dependencies are wired.

Counter/job dependency state must outlive all scheduled jobs.

LEET should implement strict DAG/cycle validation, stronger than RED's minimal
runtime assertions.

Frame/runtime context must be passed explicitly to jobs; do not use a global
current render frame.
```

### Pass 7: Frame Orchestration

RED's `RenderFrame` ties the whole render graph runtime together. It is not the
place to copy actual rendering policy into LEET, but it shows the required order
between graph cache lookup, graph build/import, render-flow group construction,
allocator phases, node job scheduling, command-list storage, and frame
completion.

The graph cache hash includes topology-affecting frame and camera inputs:

```cpp
AppendRenderGraphCameraHash(
    h,
    camera.m_renderingMode,
    displayMode,
    isFullRender,
    camera.m_renderCamera.GetFeatures().GetBitset()
);
```

The global graph hash also includes camera count, display mode, local-shadow
configuration, per-camera hashes, and async/feature switches such as async SSAO,
hair clears, LUT generation, dynamic texture generation, flatten normals, and
depth-chain building.

LEET's graph cache key must therefore include every frame, camera, feature,
display, backend, and async/config choice that can change graph topology. A
missing key input is a correctness bug, not only a cache miss issue.

RED reuses camera setup graphs independently before importing them into the
final merged graph:

```cpp
storage = renderGraphCachEntry.FindCameraSetup(cameraSetupHash);

if ( storage->DoNeedRebuild(cameraSetupHash) )
{
    BuildRenderGraphCamera(storage->m_graph, storage->m_nodes, ...);
}

graph.AddGraph(storage->m_graph, true, cameraIndex, true);
```

LEET should preserve this shape:

```text
per-camera setup graph cache
  -> build/reuse camera graph
  -> import into final frame graph with camera index / flow space
  -> build render-flow groups on final merged graph
```

Render-flow groups are built after all per-camera graph imports:

```cpp
graph.BuildRenderFlowGroups();
renderGraphCachEntry.PostBuildClear();
```

This matters because render-flow grouping belongs to the final merged graph, not
only isolated subgraphs.

RED starts setting up node jobs before the allocator has finished resolving, but
holds the jobs behind a kickoff deferral:

```cpp
nodesKickoffCounter += m_renderCommandHandler->GetDrawBuffersWaitCounter();
nodesKickoffDeferral = nodesKickoffCounter.CreateDeferral();
```

The deferral is released only after the allocator transitions into consume:

```cpp
m_renderFlowAllocator->SetPhase(Render::RFP_Consume, &depBuilder);
nodesKickoffDeferral.FinishDeferral();
```

LEET should keep the same dependency shape:

```text
build/reuse final graph
schedule/wire node jobs
Startup allocator phase
PreConsume graph traversal records requests
Resolve allocator requests
release node-job kickoff gate
Consume node jobs retrieve resolved resources
wait for terminal graph nodes
Cleanup allocator/frame state
```

Preconsume uses a shared/base implementation context:

```cpp
SRenderNodeImplContext rctx;
rctx.Init(...);
m_renderFlowAllocator->SetPhase(Render::RFP_PreConsume);
graph->ExecuteParallel(rctx, builder);
```

Consume jobs later construct per-node contexts. LEET must keep the distinction:

```text
preconsume traversal context
per-node consume job context
```

Camera custom data is prepared before allocator preconsume:

```cpp
storage->PrepareCustomData(rctx);
data->Prepare(rctx, cameraData);
data->MarkAsPrepared();
cameraData.MarkAsUsed();
```

LEET should model a controlled frame/camera data preparation stage before
resource request recording starts.

Command-list storage is prepared from graph size before consume jobs run:

```cpp
GetFrameCommandLists().PrepareForFrame(graph->GetNumNodes() + ReservedCls::CL_COUNT);
```

LEET should allocate or reserve command-recording slots after graph build and
before node execution. This does not mean graph nodes own backend command-list
storage forever; it means the frame runtime prepares enough command recording
capacity for the graph execution.

RED acquires an exclusive graph update flag while jobs are active:

```cpp
graph->AcquireExclusiveUpdateFlag();
...
graph->ReleaseExclusiveUpdateFlag();
```

LEET should expose an immutable/exclusive execution view while jobs are
prepared and running. Mutating graph topology while jobs can read it must be a
loud error.

RED also guards against concurrent `RenderFrame` execution in debug:

```cpp
RED_FATAL_ASSERT( !GDebugRenderFrame, "Recursive or MT RenderFrame usage?" );
```

LEET V1 does not support overlapping `RenderFrame` execution. The frame renderer
must reject recursive or concurrent frame execution loudly. If overlapping
frames are supported later, allocator state, graph execution state, command
recording state, and runtime context must be separated per in-flight frame.

Locked rule:

```text
Graph cache keys must include all topology-affecting frame, camera, feature,
display, backend, and async/config inputs.

Per-camera graph build data should be reusable independently before merging
into the final graph.

Final graph import/merge happens before render-flow group construction.

Render-flow groups are built on the final merged graph.

Preconsume may use a shared/base context; consume uses per-node job contexts.

Node job dependencies can be wired early, but node jobs must be gated until
allocator resolve is complete.

Frame/camera custom data preparation happens before allocator preconsume.

Command recording storage is prepared after graph build and before consume jobs.

Graph execution needs an immutable/exclusive execution view while jobs are
active.

Overlapping RenderFrame execution is not supported in V1 and must fail loudly.

Allocator cleanup is a frame execution epilogue responsibility after terminal
graph-node completion, not hidden inside a normal graph node.
```
