use super::TaggedValue;
use crate::{
    bytecode::{Instruction, OperandFormat, VerifiedFunction},
    compiler::CompileFailure,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueRepresentation {
    Tagged,
    Int32,
    Float64,
    Effect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OptimizedEffect {
    Pure,
    FrameWrite,
    Control,
    Poll,
    Reentrant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OptimizedNodeKind {
    GuardNumeric { guard: u32, mid_loop: bool },
    Bytecode { opcode: Box<str> },
    Reuse { source: u32 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptimizedNode {
    id: u32,
    pc: u32,
    kind: OptimizedNodeKind,
    representation: ValueRepresentation,
    effect: OptimizedEffect,
    eliminated: bool,
    bytes: Box<[u8]>,
    branch_target: Option<u32>,
    pops: u16,
    pushes: u8,
    deopt_guard: Option<u32>,
}

impl OptimizedNode {
    pub(super) fn reuse_value(&mut self, source: u32) {
        self.kind = OptimizedNodeKind::Reuse { source };
    }
    pub(super) fn mark_effect_free_inline(&mut self) {
        self.effect = OptimizedEffect::Control;
    }
    pub const fn id(&self) -> u32 {
        self.id
    }
    pub const fn pc(&self) -> u32 {
        self.pc
    }
    pub fn kind(&self) -> &OptimizedNodeKind {
        &self.kind
    }
    pub const fn representation(&self) -> ValueRepresentation {
        self.representation
    }
    pub const fn effect(&self) -> OptimizedEffect {
        self.effect
    }
    pub const fn eliminated(&self) -> bool {
        self.eliminated
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub const fn branch_target(&self) -> Option<u32> {
        self.branch_target
    }
    pub const fn pops(&self) -> u16 {
        self.pops
    }
    pub const fn pushes(&self) -> u8 {
        self.pushes
    }
    pub const fn deopt_guard(&self) -> Option<u32> {
        self.deopt_guard
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptimizedBlock {
    start_pc: u32,
    stack_depth: u16,
    successors: Box<[u32]>,
    loop_header: bool,
    nodes: Box<[u32]>,
}

impl OptimizedBlock {
    pub const fn start_pc(&self) -> u32 {
        self.start_pc
    }
    pub const fn stack_depth(&self) -> u16 {
        self.stack_depth
    }
    pub fn successors(&self) -> &[u32] {
        &self.successors
    }
    pub const fn is_loop_header(&self) -> bool {
        self.loop_header
    }
    pub fn nodes(&self) -> &[u32] {
        &self.nodes
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OptimizedMetrics {
    pub boxes_elided: u64,
    pub cse_eliminated: u64,
    pub dead_nodes_eliminated: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GuardSite {
    guard: u32,
    shape: OptimizedFrameShape,
    map: DeoptMap,
}

impl GuardSite {
    pub const fn guard(&self) -> u32 {
        self.guard
    }
    pub const fn shape(&self) -> OptimizedFrameShape {
        self.shape
    }
    pub const fn map(&self) -> &DeoptMap {
        &self.map
    }
}

/// QuickJS-specific optimizing IR translated directly from verified bytecode.
/// Frame-backed callees retain tagged semantic IR for their recovery boundary;
/// the caller and pure inline regions continue to use the optimizing SSA graph.
#[derive(Clone, Debug)]
pub struct OptimizedIr {
    scalar_graph: super::ScalarGraph,
    blocks: Box<[OptimizedBlock]>,
    nodes: Box<[OptimizedNode]>,
    machine_plan: Box<[OptimizedNode]>,
    guards: Box<[GuardSite]>,
    metrics: OptimizedMetrics,
    feedback_epoch: u64,
    max_stack: u16,
}

impl OptimizedIr {
    pub fn translate(
        function: &VerifiedFunction,
        feedback_epoch: u64,
    ) -> Result<Self, CompileFailure> {
        Self::translate_with_numeric_modes(function, feedback_epoch, &Default::default())
    }

    /// Build arithmetic semantics before value numbering. Modes are guarded
    /// speculation contracts, not permission to omit runtime validation.
    pub fn translate_with_numeric_modes(
        function: &VerifiedFunction,
        feedback_epoch: u64,
        numeric_modes: &std::collections::BTreeMap<u32, super::ScalarNumericMode>,
    ) -> Result<Self, CompileFailure> {
        Self::translate_with_inline_callees(
            function,
            feedback_epoch,
            numeric_modes,
            &Default::default(),
        )
    }

    pub fn translate_with_inline_callees(
        function: &VerifiedFunction,
        feedback_epoch: u64,
        numeric_modes: &std::collections::BTreeMap<u32, super::ScalarNumericMode>,
        inline_callees: &std::collections::BTreeMap<u32, super::InlineCallee>,
    ) -> Result<Self, CompileFailure> {
        Self::translate_with_frame_inline_callees(
            function,
            feedback_epoch,
            numeric_modes,
            inline_callees,
            &Default::default(),
        )
    }

    pub fn translate_with_frame_inline_callees(
        function: &VerifiedFunction,
        feedback_epoch: u64,
        numeric_modes: &std::collections::BTreeMap<u32, super::ScalarNumericMode>,
        inline_callees: &std::collections::BTreeMap<u32, super::InlineCallee>,
        frame_callees: &std::collections::BTreeMap<u32, super::FrameInlineCallee>,
    ) -> Result<Self, CompileFailure> {
        match Self::translate_with_inline_callees_impl(
            function,
            feedback_epoch,
            numeric_modes,
            inline_callees,
            frame_callees,
        ) {
            Err(CompileFailure::ResourceLimit)
                if !inline_callees.is_empty() || !frame_callees.is_empty() =>
            {
                // Expansion may exhaust a graph quota needed by later caller
                // instructions. The ordinary caller remains a valid candidate.
                Self::translate_with_inline_callees_impl(
                    function,
                    feedback_epoch,
                    numeric_modes,
                    &Default::default(),
                    &Default::default(),
                )
            }
            result => result,
        }
    }

    fn translate_with_inline_callees_impl(
        function: &VerifiedFunction,
        feedback_epoch: u64,
        numeric_modes: &std::collections::BTreeMap<u32, super::ScalarNumericMode>,
        inline_callees: &std::collections::BTreeMap<u32, super::InlineCallee>,
        frame_callees: &std::collections::BTreeMap<u32, super::FrameInlineCallee>,
    ) -> Result<Self, CompileFailure> {
        if feedback_epoch == 0 {
            return Err(CompileFailure::InvalidArtifact);
        }
        let snapshot = function.snapshot();
        // Entry and loop-header guards are instruction boundaries with an
        // empty operand stack. Only live arguments/locals participate in the
        // deopt transaction; capacity-only stack slots are not roots.
        let shape = OptimizedFrameShape::new(snapshot.arg_count(), snapshot.local_count(), 0);
        let mut nodes = Vec::new();
        let mut guards = Vec::new();
        let mut next_guard = 0u32;
        let block_depths = optimized_block_depths(function)?;
        if function.control_flow_graph().blocks().iter().any(|block| {
            function
                .control_flow_graph()
                .is_loop_header(block.start_pc())
                && block_depths.get(&block.start_pc()).copied().unwrap_or(1) != 0
        }) {
            return Err(CompileFailure::UnsupportedOpcode);
        }
        // Tier 2 lowers lexical checks as plain frame accesses, so it may only
        // admit them where the binding is initialized on every path.
        prove_lexical_checks(function)?;
        let make_guard = |pc: u32,
                          mid_loop: bool,
                          next_guard: &mut u32,
                          nodes: &mut Vec<OptimizedNode>,
                          guards: &mut Vec<GuardSite>|
         -> Result<u32, CompileFailure> {
            let guard = *next_guard;
            *next_guard = next_guard
                .checked_add(1)
                .ok_or(CompileFailure::ResourceLimit)?;
            let mut recipes = Vec::with_capacity(shape.slot_count());
            let mut flat = 0u16;
            for index in 0..snapshot.arg_count() {
                recipes.push(Materialization::argument(
                    index,
                    MaterializedValue::TaggedSlot(flat),
                ));
                flat = flat.checked_add(1).ok_or(CompileFailure::ResourceLimit)?;
            }
            for index in 0..snapshot.local_count() {
                recipes.push(Materialization::local(
                    index,
                    MaterializedValue::TaggedSlot(flat),
                ));
                flat = flat.checked_add(1).ok_or(CompileFailure::ResourceLimit)?;
            }
            for index in 0..shape.stack() {
                recipes.push(Materialization::stack(
                    index,
                    MaterializedValue::TaggedSlot(flat),
                ));
                flat = flat.checked_add(1).ok_or(CompileFailure::ResourceLimit)?;
            }
            let map = DeoptMap::new(guard, pc, DeoptPhase::BeforeEffect(0), recipes);
            map.validate(shape)
                .map_err(|_| CompileFailure::InvalidArtifact)?;
            let id = u32::try_from(nodes.len()).map_err(|_| CompileFailure::ResourceLimit)?;
            nodes.push(OptimizedNode {
                id,
                pc,
                kind: OptimizedNodeKind::GuardNumeric { guard, mid_loop },
                representation: ValueRepresentation::Effect,
                effect: OptimizedEffect::Control,
                eliminated: false,
                bytes: Box::new([]),
                branch_target: None,
                pops: 0,
                pushes: 0,
                deopt_guard: Some(guard),
            });
            guards.push(GuardSite { guard, shape, map });
            Ok(id)
        };
        make_guard(0, false, &mut next_guard, &mut nodes, &mut guards)?;
        let mut blocks = Vec::with_capacity(function.control_flow_graph().blocks().len());
        let mut boxes_elided = 0u64;
        let mut next_effect = 1u64;
        // The verifier proves every reachable depth, so the operand stack the
        // machine lowering has to model is the deepest point actually reached
        // rather than the declared (possibly unbounded) capacity.
        let mut max_stack = 0u16;
        for block in function.control_flow_graph().blocks() {
            let mut block_nodes = Vec::new();
            let mut stack_depth = *block_depths
                .get(&block.start_pc())
                .ok_or(CompileFailure::InvalidArtifact)?;
            if function
                .control_flow_graph()
                .is_loop_header(block.start_pc())
            {
                block_nodes.push(make_guard(
                    block.start_pc(),
                    true,
                    &mut next_guard,
                    &mut nodes,
                    &mut guards,
                )?);
            }
            for instruction in &function.instructions()[block.instruction_range()] {
                let instruction_pops = u16::try_from(effective_pop(instruction))
                    .map_err(|_| CompileFailure::ResourceLimit)?;
                // A tail call is exactly a call whose result is returned from
                // the same pc. Expanding it here lets the audited call
                // lowering and the runtime's call-site feedback (recorded at
                // this pc) apply unchanged; `tail_call` itself pushes nothing,
                // so the synthesized return consumes the call result.
                let expansions: [Option<(&str, u16, u8)>; 2] = match instruction.opcode().name() {
                    "tail_call" => [Some(("call", instruction_pops, 1)), Some(("return", 1, 0))],
                    "tail_call_method" => [
                        Some(("call_method", instruction_pops, 1)),
                        Some(("return", 1, 0)),
                    ],
                    name => [
                        Some((name, instruction_pops, instruction.opcode().n_push())),
                        None,
                    ],
                };
                for (name, pops, pushes) in expansions.into_iter().flatten() {
                    let (representation, effect) = classify_optimized_opcode(name)?;
                    let deopt_guard = if needs_deopt_guard(name) {
                        let guard = next_guard;
                        next_guard = next_guard
                            .checked_add(1)
                            .ok_or(CompileFailure::ResourceLimit)?;
                        let shape = OptimizedFrameShape::new(
                            snapshot.arg_count(),
                            snapshot.local_count(),
                            stack_depth,
                        );
                        let map = identity_deopt_map(
                            guard,
                            instruction.pc(),
                            DeoptPhase::BeforeEffect(next_effect),
                            shape,
                        )?;
                        guards.push(GuardSite { guard, shape, map });
                        Some(guard)
                    } else {
                        None
                    };
                    boxes_elided = boxes_elided.saturating_add(u64::from(matches!(
                        representation,
                        ValueRepresentation::Int32 | ValueRepresentation::Float64
                    )));
                    let id =
                        u32::try_from(nodes.len()).map_err(|_| CompileFailure::ResourceLimit)?;
                    // Stack drops and bytecode nops have no machine operation once
                    // their SSA uses are dead; producers are removed by the
                    // backwards liveness pass in the lowering stage.
                    nodes.push(OptimizedNode {
                        id,
                        pc: instruction.pc(),
                        kind: OptimizedNodeKind::Bytecode {
                            opcode: name.into(),
                        },
                        representation,
                        effect,
                        eliminated: matches!(name, "nop" | "drop"),
                        bytes: instruction.bytes().into(),
                        branch_target: instruction
                            .branch_target()
                            .and_then(|target| u32::try_from(target).ok()),
                        pops,
                        pushes,
                        deopt_guard,
                    });
                    block_nodes.push(id);
                    stack_depth = stack_depth
                        .checked_sub(pops)
                        .and_then(|depth| depth.checked_add(u16::from(pushes)))
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    max_stack = max_stack.max(stack_depth);
                    if effect != OptimizedEffect::Pure {
                        next_effect = next_effect.saturating_add(1);
                    }
                }
            }
            blocks.push(OptimizedBlock {
                start_pc: block.start_pc(),
                stack_depth: *block_depths
                    .get(&block.start_pc())
                    .ok_or(CompileFailure::InvalidArtifact)?,
                successors: block.successors().into(),
                loop_header: function
                    .control_flow_graph()
                    .is_loop_header(block.start_pc()),
                nodes: block_nodes.into(),
            });
        }
        let (cse_eliminated, dead_nodes_eliminated) =
            rewrite_pure_expressions(&mut nodes, &blocks, numeric_modes);
        let (scalar_graph, scalar_cse) = super::ScalarGraph::rewrite(
            &mut nodes,
            &blocks,
            shape,
            numeric_modes,
            inline_callees,
            frame_callees,
            next_guard,
        )?;
        for value in scalar_graph.values() {
            let super::ScalarValue::Call(call) = value else {
                continue;
            };
            let Some(region) = &call.frame_inline else {
                continue;
            };
            let continuation = region.continuation;
            let call_node = nodes
                .get(continuation.call_node as usize)
                .ok_or(CompileFailure::InvalidArtifact)?;
            let pre = guards
                .get(
                    call_node
                        .deopt_guard()
                        .ok_or(CompileFailure::InvalidArtifact)? as usize,
                )
                .ok_or(CompileFailure::InvalidArtifact)?;
            let DeoptPhase::BeforeEffect(epoch) = pre.map.phase() else {
                return Err(CompileFailure::InvalidArtifact);
            };
            let map = identity_deopt_map(
                continuation.guard,
                continuation.resume_pc,
                DeoptPhase::AfterEffect(epoch),
                continuation.shape,
            )?;
            guards.push(GuardSite {
                guard: continuation.guard,
                shape: continuation.shape,
                map,
            });
        }
        let machine_plan = nodes
            .iter()
            .filter(|node| !node.eliminated)
            .cloned()
            .collect::<Vec<_>>();
        Ok(Self {
            scalar_graph,
            blocks: blocks.into(),
            nodes: nodes.into(),
            machine_plan: machine_plan.into(),
            guards: guards.into(),
            metrics: OptimizedMetrics {
                boxes_elided,
                cse_eliminated: cse_eliminated.saturating_add(scalar_cse),
                dead_nodes_eliminated,
            },
            feedback_epoch,
            max_stack: max_stack.min(snapshot.stack_size()),
        })
    }
    pub fn blocks(&self) -> &[OptimizedBlock] {
        &self.blocks
    }
    #[cfg(feature = "test-support")]
    pub fn recompute_numeric_checks_for_test(&mut self, work: usize) -> Result<(), CompileFailure> {
        self.scalar_graph
            .build_numeric_checks_with_budget(&self.nodes, &self.blocks, work)
    }

    pub fn scalar_graph(&self) -> &super::ScalarGraph {
        &self.scalar_graph
    }

    /// Select checked Int32 updates whose loop-carried inputs are proven only
    /// through frame reads that `preserve` authorizes (for example, planned
    /// guarded property leaves). The caller must emit exactly those guarded
    /// lowerings, or fail compilation. Returns the number of selected updates;
    /// zero leaves the graph untouched.
    pub(crate) fn specialize_integer_updates_with_preserved_frame_reads(
        &mut self,
        preserve: impl Fn(u32) -> bool,
        work: usize,
    ) -> usize {
        self.scalar_graph
            .specialize_integer_updates_with_preserved_frame_reads(&self.nodes, preserve, work)
    }
    pub fn nodes(&self) -> &[OptimizedNode] {
        &self.nodes
    }
    pub fn machine_plan(&self) -> &[OptimizedNode] {
        &self.machine_plan
    }
    pub fn guard_maps(&self) -> &[GuardSite] {
        &self.guards
    }
    pub const fn metrics(&self) -> OptimizedMetrics {
        self.metrics
    }
    pub const fn feedback_epoch(&self) -> u64 {
        self.feedback_epoch
    }
    pub const fn max_stack(&self) -> u16 {
        self.max_stack
    }
}

pub(super) fn effective_pop(instruction: &Instruction) -> usize {
    let base = instruction.opcode().n_pop() as usize;
    match instruction.opcode().format() {
        OperandFormat::NPop | OperandFormat::NPopU16 => {
            base.saturating_add(instruction.operand_u16(1) as usize)
        }
        OperandFormat::NPopFixed => instruction
            .opcode()
            .name()
            .as_bytes()
            .last()
            .and_then(|value| value.is_ascii_digit().then_some((value - b'0') as usize))
            .map_or(base, |arguments| base.saturating_add(arguments)),
        _ => base,
    }
}

fn is_guarded_arithmetic(name: &str) -> bool {
    matches!(
        name,
        "add"
            | "sub"
            | "mul"
            | "div"
            | "mod"
            | "plus"
            | "neg"
            | "not"
            | "lnot"
            | "inc"
            | "dec"
            | "post_inc"
            | "post_dec"
            | "add_loc"
            | "inc_loc"
            | "dec_loc"
            | "eq"
            | "neq"
            | "strict_eq"
            | "strict_neq"
            | "lt"
            | "lte"
            | "gt"
            | "gte"
            | "if_false"
            | "if_true"
            | "if_false8"
            | "if_true8"
            | "get_field"
            | "put_field"
    )
}

/// Every node that can leave optimized code through an exact deoptimization
/// exit owns an instruction-local guard site. Lowering arms for these opcodes
/// must resolve `deopt_guard()`; pure loads, stores and stack shuffles never
/// deoptimize and therefore carry none.
/// Stores into interpreter-owned argument/local buffers. They need an exact
/// deoptimization site because a value of unproven representation must be
/// checked for a heap reference before the borrowed alias is spilled.
fn is_frame_slot_store(name: &str) -> bool {
    name != "set_loc_uninitialized"
        && (name.starts_with("put_loc")
            || name.starts_with("set_loc")
            || name.starts_with("put_arg")
            || name.starts_with("set_arg"))
}

fn needs_deopt_guard(name: &str) -> bool {
    is_guarded_arithmetic(name)
        || name.starts_with("call")
        || is_frame_slot_store(name)
        || matches!(
            name,
            "get_array_el"
                | "put_array_el"
                | "get_length"
                | "to_propkey"
                | "or"
                | "and"
                | "xor"
                | "shl"
                | "sar"
                | "shr"
        )
}

fn identity_deopt_map(
    guard: u32,
    resume_pc: u32,
    phase: DeoptPhase,
    shape: OptimizedFrameShape,
) -> Result<DeoptMap, CompileFailure> {
    let mut recipes = Vec::with_capacity(shape.slot_count());
    let mut flat = 0u16;
    for index in 0..shape.arguments() {
        recipes.push(Materialization::argument(
            index,
            MaterializedValue::TaggedSlot(flat),
        ));
        flat = flat.checked_add(1).ok_or(CompileFailure::ResourceLimit)?;
    }
    for index in 0..shape.locals() {
        recipes.push(Materialization::local(
            index,
            MaterializedValue::TaggedSlot(flat),
        ));
        flat = flat.checked_add(1).ok_or(CompileFailure::ResourceLimit)?;
    }
    for index in 0..shape.stack() {
        recipes.push(Materialization::stack(
            index,
            MaterializedValue::TaggedSlot(flat),
        ));
        flat = flat.checked_add(1).ok_or(CompileFailure::ResourceLimit)?;
    }
    let map = DeoptMap::new(guard, resume_pc, phase, recipes);
    map.validate(shape)
        .map_err(|_| CompileFailure::InvalidArtifact)?;
    Ok(map)
}

fn rewrite_pure_expressions(
    nodes: &mut [OptimizedNode],
    blocks: &[OptimizedBlock],
    numeric_modes: &std::collections::BTreeMap<u32, super::ScalarNumericMode>,
) -> (u64, u64) {
    use std::collections::BTreeMap;
    #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
    struct ExpressionKey {
        opcode: Box<str>,
        lhs: (u8, u16, u64),
        rhs: (u8, u16, u64),
        representation: u8,
        numeric_mode: super::ScalarNumericMode,
        effect_epoch: u64,
    }
    let mut cse = 0u64;
    let mut dead = 0u64;
    for block in blocks {
        let ids = block.nodes();
        let mut expressions = BTreeMap::<ExpressionKey, u32>::new();
        let mut slot_versions = BTreeMap::<(u8, u16), u64>::new();
        let mut effect_epoch = 0u64;
        let mut index = 0usize;
        while index < ids.len() {
            if index + 3 < ids.len() {
                let quartet = [ids[index], ids[index + 1], ids[index + 2], ids[index + 3]];
                let names = quartet.map(|id| opcode_name(&nodes[id as usize]));
                if names[3] == Some("drop")
                    && names[0].is_some_and(is_pure_load)
                    && names[1].is_some_and(is_pure_load)
                    && names[2].is_some_and(is_pure_binary)
                {
                    for id in quartet {
                        let node = &mut nodes[id as usize];
                        node.eliminated = true;
                        node.pops = 0;
                        node.pushes = 0;
                        dead = dead.saturating_add(1);
                    }
                    index += 4;
                    continue;
                }
            }
            if index + 2 < ids.len() {
                let triple = [ids[index], ids[index + 1], ids[index + 2]];
                let names = triple.map(|id| opcode_name(&nodes[id as usize]));
                if names[0].is_some_and(is_pure_load)
                    && names[1].is_some_and(is_pure_load)
                    && names[2].is_some_and(is_pure_binary)
                {
                    let lhs = ssa_load_operand(&nodes[triple[0] as usize], &slot_versions);
                    let rhs = ssa_load_operand(&nodes[triple[1] as usize], &slot_versions);
                    let representation = match nodes[triple[2] as usize].representation {
                        ValueRepresentation::Tagged => 0,
                        ValueRepresentation::Int32 => 1,
                        ValueRepresentation::Float64 => 2,
                        ValueRepresentation::Effect => 3,
                    };
                    let Some((lhs, rhs)) = lhs.zip(rhs) else {
                        index += 1;
                        continue;
                    };
                    let key = ExpressionKey {
                        opcode: names[2].unwrap().into(),
                        lhs,
                        rhs,
                        representation,
                        numeric_mode: numeric_modes
                            .get(&nodes[triple[2] as usize].pc())
                            .copied()
                            .unwrap_or_default(),
                        effect_epoch,
                    };
                    if let Some(source) = expressions.get(&key).copied() {
                        for id in &triple[..2] {
                            let node = &mut nodes[*id as usize];
                            node.eliminated = true;
                            node.pops = 0;
                            node.pushes = 0;
                        }
                        let node = &mut nodes[triple[2] as usize];
                        node.kind = OptimizedNodeKind::Reuse { source };
                        node.pops = 0;
                        node.pushes = 1;
                        cse = cse.saturating_add(1);
                    } else {
                        expressions.insert(key, triple[2]);
                    }
                    index += 3;
                    continue;
                }
            }
            let node = &nodes[ids[index] as usize];
            if node.effect != OptimizedEffect::Pure {
                effect_epoch = effect_epoch.saturating_add(1);
                expressions.clear();
                if node.effect == OptimizedEffect::FrameWrite {
                    if let Some(slot) = frame_write_slot(node) {
                        *slot_versions.entry(slot).or_default() += 1;
                    } else {
                        slot_versions.clear();
                    }
                }
            }
            index += 1;
        }
    }
    (cse, dead)
}

fn ssa_load_operand(
    node: &OptimizedNode,
    slot_versions: &std::collections::BTreeMap<(u8, u16), u64>,
) -> Option<(u8, u16, u64)> {
    let name = opcode_name(node)?;
    let index = indexed_node_operand(name, node.bytes())?;
    let kind = if name.starts_with("get_arg") {
        FRAME_SLOT_ARGUMENT
    } else if name.starts_with("get_loc") {
        FRAME_SLOT_LOCAL
    } else {
        return None;
    };
    Some((
        kind,
        index,
        slot_versions.get(&(kind, index)).copied().unwrap_or(0),
    ))
}

const FRAME_SLOT_ARGUMENT: u8 = 0;
const FRAME_SLOT_LOCAL: u8 = 1;

/// The single argument or local slot a frame-writing node redefines, so the
/// value-numbering pass retires only the loads that alias it.
pub(super) fn frame_write_slot(node: &OptimizedNode) -> Option<(u8, u16)> {
    let name = opcode_name(node)?;
    let kind = if name.starts_with("put_loc")
        || name.starts_with("set_loc")
        || matches!(name, "add_loc" | "inc_loc" | "dec_loc")
    {
        FRAME_SLOT_LOCAL
    } else if name.starts_with("put_arg") || name.starts_with("set_arg") {
        FRAME_SLOT_ARGUMENT
    } else {
        return None;
    };
    Some((kind, indexed_node_operand(name, node.bytes())?))
}

pub(super) fn indexed_node_operand(name: &str, bytes: &[u8]) -> Option<u16> {
    for (suffix, index) in [("0", 0), ("1", 1), ("2", 2), ("3", 3)] {
        if name.ends_with(suffix) {
            return Some(index);
        }
    }
    match bytes.len() {
        2 => Some(u16::from(bytes[1])),
        n if n >= 3 => Some(u16::from_le_bytes([bytes[1], bytes[2]])),
        _ => None,
    }
}

fn opcode_name(node: &OptimizedNode) -> Option<&str> {
    match &node.kind {
        OptimizedNodeKind::Bytecode { opcode } => Some(opcode),
        _ => None,
    }
}

pub(super) fn is_pure_load(name: &str) -> bool {
    matches!(
        name,
        "get_arg"
            | "get_arg0"
            | "get_arg1"
            | "get_arg2"
            | "get_arg3"
            | "get_loc"
            | "get_loc8"
            | "get_loc0"
            | "get_loc1"
            | "get_loc2"
            | "get_loc3"
    )
}

fn is_pure_binary(name: &str) -> bool {
    matches!(name, "add" | "sub" | "mul" | "div")
}

fn optimized_block_depths(
    function: &VerifiedFunction,
) -> Result<std::collections::BTreeMap<u32, u16>, CompileFailure> {
    use std::collections::{BTreeMap, VecDeque};
    let mut depths = BTreeMap::from([(0u32, 0u16)]);
    let mut queue = VecDeque::from([0u32]);
    while let Some(pc) = queue.pop_front() {
        let block = function
            .control_flow_graph()
            .block(pc)
            .ok_or(CompileFailure::InvalidArtifact)?;
        let mut depth = *depths.get(&pc).ok_or(CompileFailure::InvalidArtifact)?;
        for instruction in &function.instructions()[block.instruction_range()] {
            let pops = u16::try_from(effective_pop(instruction))
                .map_err(|_| CompileFailure::ResourceLimit)?;
            depth = depth
                .checked_sub(pops)
                .ok_or(CompileFailure::InvalidArtifact)?;
            depth = depth
                .checked_add(u16::from(instruction.opcode().n_push()))
                .ok_or(CompileFailure::ResourceLimit)?;
        }
        for successor in block.successors() {
            match depths.get(successor) {
                Some(existing) if *existing != depth => {
                    return Err(CompileFailure::InvalidArtifact)
                }
                Some(_) => {}
                None => {
                    depths.insert(*successor, depth);
                    queue.push_back(*successor);
                }
            }
        }
    }
    Ok(depths)
}

/// Feedback-independent part of Tier-2 admission. `false` means translation
/// rejects this function with `UnsupportedOpcode` for every feedback snapshot
/// (a reachable opcode lies outside the optimized vocabulary), so its
/// baseline artifact is the terminal native tier.
pub fn optimized_vocabulary_admits(function: &VerifiedFunction) -> bool {
    let instructions = function.instructions();
    function.control_flow_graph().blocks().iter().all(|block| {
        instructions
            .get(block.instruction_range())
            .is_some_and(|block| {
                block.iter().all(|instruction| {
                    let name = match instruction.opcode().name() {
                        "tail_call" => "call",
                        "tail_call_method" => "call_method",
                        name => name,
                    };
                    classify_optimized_opcode(name).is_ok()
                })
            })
    })
}

/// Whether every instruction has an optimized-IR classification. Tier 2
/// translates the whole function, so a single unclassified opcode (exact
/// generic helpers, literal allocation, ...) makes every attempt fail.
pub(crate) fn optimized_opcodes_supported(function: &VerifiedFunction) -> bool {
    function.instructions().iter().all(|instruction| {
        matches!(
            instruction.opcode().name(),
            "tail_call" | "tail_call_method"
        ) || classify_optimized_opcode(instruction.opcode().name()).is_ok()
    })
}

/// Local may hold an initialized JS value on some path reaching this point.
const LEXICAL_MAY_BE_INITIALIZED: u8 = 1;
/// Local may hold `JS_UNINITIALIZED` on some path reaching this point.
const LEXICAL_MAY_BE_UNINITIALIZED: u8 = 2;

/// Proves that every `get_loc_check`/`put_loc_check` reads a local that is
/// initialized on every incoming path, and rejects `put_loc_check_init`.
///
/// The bytecode verifier deliberately joins "uninitialized" with a JS value
/// kind into a plain tagged cell (a lexical binding declared in a loop body or
/// reached by switch fall-through), because Tier 1 keeps the runtime TDZ
/// check. The Tier 2 lowering has no TDZ check, so without this proof the
/// interpreter's ReferenceError would be silently replaced by a load.
///
/// QuickJS initializes every frame local to `undefined` on entry and only
/// `set_loc_uninitialized` stores `JS_UNINITIALIZED`. Nested closures cannot
/// uninitialize a frame binding, so "initialized on every path" is stable
/// across calls; the converse is not (an arrow `super()` initializes `this`),
/// which is why `put_loc_check_init` is never admitted here.
fn prove_lexical_checks(function: &VerifiedFunction) -> Result<(), CompileFailure> {
    use std::collections::{BTreeMap, VecDeque};
    let cfg = function.control_flow_graph();
    let local_count = usize::from(function.snapshot().local_count());
    let instructions = function.instructions();
    let touches_lexical = instructions.iter().any(|instruction| {
        matches!(
            instruction.opcode().name(),
            "get_loc_check" | "put_loc_check" | "put_loc_check_init" | "set_loc_uninitialized"
        )
    });
    if !touches_lexical {
        return Ok(());
    }
    let mut entry: BTreeMap<u32, Vec<u8>> =
        BTreeMap::from([(0u32, vec![LEXICAL_MAY_BE_INITIALIZED; local_count])]);
    let mut queue = VecDeque::from([0u32]);
    // A block is queued once on discovery and once per join that adds a bit,
    // so the fixpoint is bounded by blocks * (2 * locals + 1).
    let mut budget = cfg
        .blocks()
        .len()
        .saturating_mul(local_count.saturating_mul(2).saturating_add(1))
        .saturating_add(1);
    while let Some(pc) = queue.pop_front() {
        budget = budget.checked_sub(1).ok_or(CompileFailure::ResourceLimit)?;
        let block = cfg.block(pc).ok_or(CompileFailure::InvalidArtifact)?;
        let mut state = entry
            .get(&pc)
            .cloned()
            .ok_or(CompileFailure::InvalidArtifact)?;
        for instruction in &instructions[block.instruction_range()] {
            let name = instruction.opcode().name();
            let Some(index) = crate::bytecode::local_index(instruction) else {
                continue;
            };
            let slot = state
                .get_mut(index)
                .ok_or(CompileFailure::InvalidArtifact)?;
            match name {
                "set_loc_uninitialized" => *slot = LEXICAL_MAY_BE_UNINITIALIZED,
                "get_loc_check" | "put_loc_check" => {
                    if *slot & LEXICAL_MAY_BE_UNINITIALIZED != 0 {
                        return Err(CompileFailure::UnsupportedOpcode);
                    }
                    *slot = LEXICAL_MAY_BE_INITIALIZED;
                }
                "put_loc_check_init" => return Err(CompileFailure::UnsupportedOpcode),
                _ if name.starts_with("put_loc")
                    || name.starts_with("set_loc")
                    || matches!(name, "inc_loc" | "dec_loc" | "add_loc") =>
                {
                    *slot = LEXICAL_MAY_BE_INITIALIZED;
                }
                // Plain reads and `close_loc` leave the cell unchanged.
                _ => {}
            }
        }
        for successor in block.successors() {
            match entry.get_mut(successor) {
                Some(existing) => {
                    let mut changed = false;
                    for (joined, incoming) in existing.iter_mut().zip(&state) {
                        let next = *joined | *incoming;
                        changed |= next != *joined;
                        *joined = next;
                    }
                    if changed {
                        queue.push_back(*successor);
                    }
                }
                None => {
                    entry.insert(*successor, state.clone());
                    queue.push_back(*successor);
                }
            }
        }
    }
    Ok(())
}

fn classify_optimized_opcode(
    name: &str,
) -> Result<(ValueRepresentation, OptimizedEffect), CompileFailure> {
    let result = match name {
        "push_i8" | "push_i16" | "push_i32" | "push_minus1" | "push_0" | "push_1" | "push_2"
        | "push_3" | "push_4" | "push_5" | "push_6" | "push_7" | "post_inc" | "post_dec"
        | "inc" | "dec" | "not" | "shl" | "sar" | "shr" | "and" | "or" | "xor" => {
            (ValueRepresentation::Int32, OptimizedEffect::Pure)
        }
        "add" | "sub" | "mul" | "div" | "mod" | "plus" | "neg" => {
            (ValueRepresentation::Float64, OptimizedEffect::Pure)
        }
        "get_arg" | "get_arg0" | "get_arg1" | "get_arg2" | "get_arg3" | "get_loc" | "get_loc8"
        | "get_loc0" | "get_loc1" | "get_loc2" | "get_loc3" | "get_loc_check" | "get_loc0_loc1"
        | "undefined" | "null" | "push_true" | "push_false" | "dup" | "dup1" | "dup2" | "dup3"
        | "swap" | "nip" | "nip1" | "insert2" | "insert3" | "insert4" | "perm3" | "perm4"
        | "perm5" | "swap2" | "rot3l" | "rot3r" | "rot4l" | "rot5l" => {
            (ValueRepresentation::Tagged, OptimizedEffect::Pure)
        }
        "push_const" | "push_const8" | "get_var" | "get_field2" => {
            (ValueRepresentation::Tagged, OptimizedEffect::Reentrant)
        }
        "is_undefined_or_null" | "is_undefined" | "is_null" | "lnot" => {
            (ValueRepresentation::Int32, OptimizedEffect::Pure)
        }
        "to_propkey" => (ValueRepresentation::Tagged, OptimizedEffect::FrameWrite),
        // Local arithmetic redefines a frame slot in place; it is a frame
        // write for value numbering even though it pushes nothing.
        "put_arg"
        | "put_arg0"
        | "put_arg1"
        | "put_arg2"
        | "put_arg3"
        | "put_loc"
        | "put_loc8"
        | "put_loc0"
        | "put_loc1"
        | "put_loc2"
        | "put_loc3"
        | "put_loc_check"
        | "put_loc_check_init"
        | "set_loc_uninitialized"
        | "add_loc"
        | "inc_loc"
        | "dec_loc"
        | "drop" => (ValueRepresentation::Effect, OptimizedEffect::FrameWrite),
        "set_arg" | "set_arg0" | "set_arg1" | "set_arg2" | "set_arg3" | "set_loc" | "set_loc8"
        | "set_loc0" | "set_loc1" | "set_loc2" | "set_loc3" => {
            (ValueRepresentation::Tagged, OptimizedEffect::FrameWrite)
        }
        "call" | "call0" | "call1" | "call2" | "call3" | "call_method" => {
            (ValueRepresentation::Tagged, OptimizedEffect::Reentrant)
        }
        "get_field" => (ValueRepresentation::Tagged, OptimizedEffect::Reentrant),
        "get_length" => (ValueRepresentation::Int32, OptimizedEffect::FrameWrite),
        "get_array_el" => (ValueRepresentation::Tagged, OptimizedEffect::FrameWrite),
        "put_field" => (ValueRepresentation::Effect, OptimizedEffect::FrameWrite),
        "put_array_el" => (ValueRepresentation::Effect, OptimizedEffect::FrameWrite),
        "if_false" | "if_true" | "if_false8" | "if_true8" | "goto" | "goto8" | "goto16"
        | "return" | "return_undef" | "lt" | "lte" | "gt" | "gte" | "eq" | "neq" | "strict_eq"
        | "strict_neq" | "nop" => (ValueRepresentation::Effect, OptimizedEffect::Control),
        _ => return Err(CompileFailure::UnsupportedOpcode),
    };
    Ok(result)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DeoptSlot {
    Argument(u16),
    Local(u16),
    Stack(u16),
}

#[derive(Clone, Copy, Debug)]
pub enum MaterializedValue {
    Poison,
    Undefined,
    Null,
    Bool(bool),
    Int32(i32),
    Float64(f64),
    TaggedSlot(u16),
}

impl PartialEq for MaterializedValue {
    fn eq(&self, other: &Self) -> bool {
        match (*self, *other) {
            (Self::Poison, Self::Poison)
            | (Self::Undefined, Self::Undefined)
            | (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int32(a), Self::Int32(b)) => a == b,
            (Self::Float64(a), Self::Float64(b)) => a.to_bits() == b.to_bits(),
            (Self::TaggedSlot(a), Self::TaggedSlot(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OptimizedFrameShape {
    arguments: u16,
    locals: u16,
    stack: u16,
}

impl OptimizedFrameShape {
    pub const fn new(arguments: u16, locals: u16, stack: u16) -> Self {
        Self {
            arguments,
            locals,
            stack,
        }
    }
    pub const fn slot_count(self) -> usize {
        self.arguments as usize + self.locals as usize + self.stack as usize
    }
    pub const fn arguments(self) -> u16 {
        self.arguments
    }
    pub const fn locals(self) -> u16 {
        self.locals
    }
    pub const fn stack(self) -> u16 {
        self.stack
    }
    fn index(self, slot: DeoptSlot) -> Option<usize> {
        match slot {
            DeoptSlot::Argument(i) if i < self.arguments => Some(i as usize),
            DeoptSlot::Local(i) if i < self.locals => Some(self.arguments as usize + i as usize),
            DeoptSlot::Stack(i) if i < self.stack => {
                Some(self.arguments as usize + self.locals as usize + i as usize)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeoptPhase {
    BeforeEffect(u64),
    AfterEffect(u64),
}

impl DeoptPhase {
    pub const fn side_effect_epoch(self) -> u64 {
        match self {
            Self::BeforeEffect(epoch) => epoch.saturating_sub(1),
            Self::AfterEffect(epoch) => epoch,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Materialization {
    slot: DeoptSlot,
    value: MaterializedValue,
}

impl Materialization {
    pub const fn argument(index: u16, value: MaterializedValue) -> Self {
        Self {
            slot: DeoptSlot::Argument(index),
            value,
        }
    }
    pub const fn local(index: u16, value: MaterializedValue) -> Self {
        Self {
            slot: DeoptSlot::Local(index),
            value,
        }
    }
    pub const fn stack(index: u16, value: MaterializedValue) -> Self {
        Self {
            slot: DeoptSlot::Stack(index),
            value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeoptValidationError {
    SlotCount,
    DuplicateSlot,
    InvalidSlot,
    DestinationSize,
    UnsupportedRecipe,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeoptMap {
    guard: u32,
    resume_pc: u32,
    phase: DeoptPhase,
    slots: Box<[Materialization]>,
}

impl DeoptMap {
    pub fn new(guard: u32, resume_pc: u32, phase: DeoptPhase, slots: Vec<Materialization>) -> Self {
        Self {
            guard,
            resume_pc,
            phase,
            slots: slots.into_boxed_slice(),
        }
    }
    pub const fn guard(&self) -> u32 {
        self.guard
    }
    pub const fn resume_pc(&self) -> u32 {
        self.resume_pc
    }
    pub const fn phase(&self) -> DeoptPhase {
        self.phase
    }
    pub const fn materialization_count(&self) -> usize {
        self.slots.len()
    }
    pub fn validate(&self, shape: OptimizedFrameShape) -> Result<(), DeoptValidationError> {
        if self.slots.len() != shape.slot_count() {
            return Err(DeoptValidationError::SlotCount);
        }
        let mut seen = vec![false; shape.slot_count()];
        for recipe in &self.slots {
            let index = shape
                .index(recipe.slot)
                .ok_or(DeoptValidationError::InvalidSlot)?;
            if seen[index] {
                return Err(DeoptValidationError::DuplicateSlot);
            }
            seen[index] = true;
        }
        if seen.iter().any(|present| !present) {
            return Err(DeoptValidationError::SlotCount);
        }
        Ok(())
    }

    /// Validates the narrow Tier 2 in-place deopt transaction. Every current
    /// production recipe aliases its own already-rooted frame slot, so the
    /// two-phase transaction performs complete validation before committing
    /// the resume state and requires no refcount mutation.
    pub fn validate_identity_materialization(
        &self,
        shape: OptimizedFrameShape,
    ) -> Result<(), DeoptValidationError> {
        self.validate(shape)?;
        for recipe in &self.slots {
            let destination = shape
                .index(recipe.slot)
                .ok_or(DeoptValidationError::InvalidSlot)?;
            if recipe.value != MaterializedValue::TaggedSlot(destination as u16) {
                return Err(DeoptValidationError::UnsupportedRecipe);
            }
        }
        Ok(())
    }
    pub fn materialize(
        &self,
        shape: OptimizedFrameShape,
    ) -> Result<MaterializedFrame, DeoptValidationError> {
        let mut slots = vec![MaterializedValue::Poison; shape.slot_count()];
        self.materialize_into(shape, &mut slots)?;
        Ok(MaterializedFrame {
            resume_pc: self.resume_pc,
            side_effect_epoch: self.phase.side_effect_epoch(),
            slots: slots.into_boxed_slice(),
        })
    }
    pub fn materialize_into(
        &self,
        shape: OptimizedFrameShape,
        destination: &mut [MaterializedValue],
    ) -> Result<(), DeoptValidationError> {
        self.validate(shape)?;
        if destination.len() != shape.slot_count() {
            return Err(DeoptValidationError::DestinationSize);
        }
        let plan = self
            .slots
            .iter()
            .map(|recipe| (shape.index(recipe.slot).expect("validated"), recipe.value))
            .collect::<Vec<_>>();
        for (index, value) in plan {
            destination[index] = value;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaterializedFrame {
    resume_pc: u32,
    side_effect_epoch: u64,
    slots: Box<[MaterializedValue]>,
}
impl MaterializedFrame {
    pub const fn resume_pc(&self) -> u32 {
        self.resume_pc
    }
    pub const fn side_effect_epoch(&self) -> u64 {
        self.side_effect_epoch
    }
    pub fn slots(&self) -> &[MaterializedValue] {
        &self.slots
    }
}

pub trait DeoptOwnership {
    type Error;
    fn duplicate(&mut self, source_slot: u16) -> Result<TaggedValue, Self::Error>;
    fn release(&mut self, value: TaggedValue);
}

#[derive(Debug)]
pub enum OwnedMaterializeError<E> {
    InvalidMap(DeoptValidationError),
    Ownership(E),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OwnedMaterializedValue {
    Scalar(MaterializedValue),
    Tagged(TaggedValue),
}

#[derive(Clone, Debug, PartialEq)]
pub struct OwnedMaterializedFrame {
    resume_pc: u32,
    side_effect_epoch: u64,
    slots: Box<[OwnedMaterializedValue]>,
    owned_count: usize,
}
impl OwnedMaterializedFrame {
    pub const fn resume_pc(&self) -> u32 {
        self.resume_pc
    }
    pub const fn side_effect_epoch(&self) -> u64 {
        self.side_effect_epoch
    }
    pub fn slots(&self) -> &[OwnedMaterializedValue] {
        &self.slots
    }
    pub const fn owned_count(&self) -> usize {
        self.owned_count
    }
}

impl DeoptMap {
    /// Executes the fallible ownership phase into private scratch storage. No
    /// caller-visible frame slot is changed until every duplication succeeds.
    pub fn materialize_owned<O: DeoptOwnership>(
        &self,
        shape: OptimizedFrameShape,
        ownership: &mut O,
    ) -> Result<OwnedMaterializedFrame, OwnedMaterializeError<O::Error>> {
        self.validate(shape)
            .map_err(OwnedMaterializeError::InvalidMap)?;
        let mut planned =
            vec![OwnedMaterializedValue::Scalar(MaterializedValue::Poison); shape.slot_count()];
        let mut owned = Vec::new();
        for recipe in &self.slots {
            let index = shape
                .index(recipe.slot)
                .expect("map validated before ownership");
            planned[index] = match recipe.value {
                MaterializedValue::TaggedSlot(source) => match ownership.duplicate(source) {
                    Ok(value) => {
                        owned.push(value);
                        OwnedMaterializedValue::Tagged(value)
                    }
                    Err(error) => {
                        for value in owned.drain(..).rev() {
                            ownership.release(value);
                        }
                        return Err(OwnedMaterializeError::Ownership(error));
                    }
                },
                scalar => OwnedMaterializedValue::Scalar(scalar),
            };
        }
        Ok(OwnedMaterializedFrame {
            resume_pc: self.resume_pc,
            side_effect_epoch: self.phase.side_effect_epoch(),
            slots: planned.into_boxed_slice(),
            owned_count: owned.len(),
        })
    }
}

impl MaterializedValue {
    pub fn is_negative_zero(self) -> bool {
        matches!(self, Self::Float64(value) if value.to_bits() == (-0.0f64).to_bits())
    }
}

/// Compile-time ownership carried by a tagged optimized-SSA value.
///
/// Frame arguments and locals enter optimized code as borrowed aliases.  A
/// helper which consumes a `JSValue` owner (for example the interpreter CALL
/// stack cleanup) must first turn that alias into `Owned` with the DUP helper.
/// `Moved` is deliberately terminal: it prevents a second cleanup from being
/// emitted for the same owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SsaValueOwnership {
    Borrowed,
    Owned,
    Moved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnershipTransitionError {
    DuplicateMoved,
    ConsumeBorrowed,
    ConsumeMoved,
}

impl SsaValueOwnership {
    /// Models JS_DupValue. Both borrowed and owned sources may be copied, and
    /// the returned SSA value is a distinct owner.
    pub const fn duplicate(self) -> Result<Self, OwnershipTransitionError> {
        match self {
            Self::Borrowed | Self::Owned => Ok(Self::Owned),
            Self::Moved => Err(OwnershipTransitionError::DuplicateMoved),
        }
    }

    /// Transfers this owner's cleanup obligation to a consuming helper.
    pub fn consume(&mut self) -> Result<(), OwnershipTransitionError> {
        match *self {
            Self::Owned => {
                *self = Self::Moved;
                Ok(())
            }
            Self::Borrowed => Err(OwnershipTransitionError::ConsumeBorrowed),
            Self::Moved => Err(OwnershipTransitionError::ConsumeMoved),
        }
    }

    pub const fn needs_cleanup(self) -> bool {
        matches!(self, Self::Owned)
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::{OwnershipTransitionError, SsaValueOwnership};

    #[test]
    fn borrowed_values_cannot_be_consumed_or_duplicated_after_move() {
        let mut borrowed = SsaValueOwnership::Borrowed;
        assert_eq!(
            borrowed.consume(),
            Err(OwnershipTransitionError::ConsumeBorrowed)
        );

        let mut owner = borrowed.duplicate().unwrap();
        assert!(owner.needs_cleanup());
        owner.consume().unwrap();
        assert!(!owner.needs_cleanup());
        assert_eq!(owner.consume(), Err(OwnershipTransitionError::ConsumeMoved));
        assert_eq!(
            owner.duplicate(),
            Err(OwnershipTransitionError::DuplicateMoved)
        );
    }

    #[test]
    fn call_ownership_protocol_balances_refcounts_under_stress() {
        // Model a refcounted caller argument borrowed by optimized SSA. Each
        // CALL materializes exactly one temporary owner per input, CALL borrows
        // those owners, and stack cleanup consumes them once.
        let caller_owners = 3_i64;
        let mut refcount = caller_owners;
        for _ in 0..100_000 {
            let borrowed = [SsaValueOwnership::Borrowed; 3];
            let mut call_owners = borrowed.map(|value| {
                refcount += 1;
                value.duplicate().unwrap()
            });
            assert_eq!(refcount, caller_owners + 3);
            for owner in &mut call_owners {
                owner.consume().unwrap();
                refcount -= 1;
            }
            assert_eq!(refcount, caller_owners);
        }
        assert_eq!(refcount, caller_owners);
    }
}
