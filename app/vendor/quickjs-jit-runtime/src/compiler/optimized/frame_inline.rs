//! Frame-backed inlining: native effects retain an exact interpreter origin.

use super::*;
use crate::ir::{BinaryOp, IrOp, PollKind, ScalarFrameInlineRegion, StackOp, UnaryOp};
use cranelift_codegen::ir::{
    condcodes::IntCC, types, AbiParam, Block, InstBuilder, MemFlags, Signature, StackSlotData,
    StackSlotKind, Value,
};
use cranelift_frontend::FunctionBuilder;
use rquickjs_core::qjs;

fn checked_binary(
    builder: &mut FunctionBuilder<'_>,
    op: BinaryOp,
    lhs: OptPair,
    rhs: OptPair,
    failed: Block,
) -> Option<OptPair> {
    if !matches!(
        op,
        BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::BitAnd
            | BinaryOp::BitOr
            | BinaryOp::BitXor
            | BinaryOp::LessThan
            | BinaryOp::LessThanOrEqual
            | BinaryOp::GreaterThan
            | BinaryOp::GreaterThanOrEqual
            | BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::StrictEqual
            | BinaryOp::StrictNotEqual
    ) {
        return None;
    }
    let li = builder
        .ins()
        .icmp_imm(IntCC::Equal, lhs.tag, i64::from(qjs::JS_TAG_INT));
    let ri = builder
        .ins()
        .icmp_imm(IntCC::Equal, rhs.tag, i64::from(qjs::JS_TAG_INT));
    let valid = builder.ins().band(li, ri);
    let compute = builder.create_block();
    builder.ins().brif(valid, compute, &[], failed, &[]);
    builder.switch_to_block(compute);
    let l = builder.ins().ireduce(types::I32, lhs.payload);
    let r = builder.ins().ireduce(types::I32, rhs.payload);
    let l = builder.ins().sextend(types::I64, l);
    let r = builder.ins().sextend(types::I64, r);
    let mut tag = qjs::JS_TAG_INT;
    let payload = match op {
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => {
            let wide = match op {
                BinaryOp::Add => builder.ins().iadd(l, r),
                BinaryOp::Sub => builder.ins().isub(l, r),
                _ => builder.ins().imul(l, r),
            };
            let narrow = builder.ins().ireduce(types::I32, wide);
            let extended = builder.ins().sextend(types::I64, narrow);
            let mut fits = builder.ins().icmp(IntCC::Equal, wide, extended);
            if op == BinaryOp::Mul {
                let zero = builder.ins().icmp_imm(IntCC::Equal, wide, 0);
                let signs = builder.ins().bxor(l, r);
                let negative = builder.ins().icmp_imm(IntCC::SignedLessThan, signs, 0);
                let negative_zero = builder.ins().band(zero, negative);
                let nonnegative_zero = builder.ins().icmp_imm(IntCC::Equal, negative_zero, 0);
                fits = builder.ins().band(fits, nonnegative_zero);
            }
            let success = builder.create_block();
            builder.ins().brif(fits, success, &[], failed, &[]);
            builder.switch_to_block(success);
            extended
        }
        BinaryOp::BitAnd => builder.ins().band(l, r),
        BinaryOp::BitOr => builder.ins().bor(l, r),
        BinaryOp::BitXor => builder.ins().bxor(l, r),
        _ => {
            tag = qjs::JS_TAG_BOOL;
            let condition = match op {
                BinaryOp::LessThan => IntCC::SignedLessThan,
                BinaryOp::LessThanOrEqual => IntCC::SignedLessThanOrEqual,
                BinaryOp::GreaterThan => IntCC::SignedGreaterThan,
                BinaryOp::GreaterThanOrEqual => IntCC::SignedGreaterThanOrEqual,
                BinaryOp::Equal | BinaryOp::StrictEqual => IntCC::Equal,
                _ => IntCC::NotEqual,
            };
            let boolean = builder.ins().icmp(condition, l, r);
            builder.ins().uextend(types::I64, boolean)
        }
    };
    Some(OptPair {
        payload,
        tag: builder.ins().iconst(types::I64, i64::from(tag)),
    })
}

/// The stable slots are the recovery representation, not a second speculative
/// stack. This follows V8's inlined DeoptFrame and JSC's CodeOrigin rule: every
/// committed effect is followed only by recovery at its successor origin.
struct InlineFrame<'a, 'b> {
    root: &'a OptEnv<'b>,
    region: &'a ScalarFrameInlineRegion,
    api: &'a qjs::JSJitInlineAPI,
    frame: Value,
    arguments: Value,
    locals: Value,
    stack: Value,
}

fn runtime_call(
    builder: &mut FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    address: usize,
    parameters: &[Value],
) -> Value {
    let mut signature = Signature::new(builder.func.signature.call_conv);
    for value in parameters {
        signature
            .params
            .push(AbiParam::new(builder.func.dfg.value_type(*value)));
    }
    signature.returns.push(AbiParam::new(types::I32));
    let signature = builder.import_signature(signature);
    let pointer = builder.ins().iconst(env.pointer_type, address as i64);
    let call = super::super::emit_external_call(
        builder,
        signature,
        pointer,
        parameters,
        env.pointer_type,
        parameters.first().copied(),
        None,
    );
    builder.inst_results(call)[0]
}

fn set_pc(builder: &mut FunctionBuilder<'_>, env: &OptEnv<'_>, frame: Value, pc: u32) -> Value {
    let start = builder.ins().load(
        env.pointer_type,
        MemFlags::new(),
        frame,
        env.layout.bytecode_start,
    );
    let pointer = builder.ins().iadd_imm(start, i64::from(pc));
    builder
        .ins()
        .store(MemFlags::new(), pointer, frame, env.layout.pc);
    pointer
}

fn undefined(builder: &mut FunctionBuilder<'_>) -> OptPair {
    OptPair {
        payload: builder.ins().iconst(types::I64, 0),
        tag: builder
            .ins()
            .iconst(types::I64, i64::from(qjs::JS_TAG_UNDEFINED)),
    }
}

impl InlineFrame<'_, '_> {
    fn flat(&self, stack: usize) -> Result<u32, CompileFailure> {
        u32::try_from(
            usize::from(self.region.body.argument_count)
                + usize::from(self.region.body.local_count)
                + stack,
        )
        .map_err(|_| CompileFailure::ResourceLimit)
    }
    fn depth(&self, builder: &mut FunctionBuilder<'_>, depth: usize) {
        opt_set_stack_top(
            builder,
            self.frame,
            self.stack,
            depth,
            self.root.pointer_type,
            self.root.layout,
        );
    }
    fn resume(
        &self,
        builder: &mut FunctionBuilder<'_>,
        pc: u32,
        exception: bool,
    ) -> Result<(), CompileFailure> {
        set_pc(builder, self.root, self.frame, pc);
        let pc = builder.ins().iconst(types::I32, i64::from(pc));
        let kind = builder.ins().iconst(
            types::I32,
            i64::from(if exception {
                qjs::JS_JIT_INLINE_RESUME_EXCEPTION
            } else {
                qjs::JS_JIT_INLINE_RESUME_INSTRUCTION
            }),
        );
        let address = self.api.resume.ok_or(CompileFailure::InvalidArtifact)? as usize;
        let status = runtime_call(builder, self.root, address, &[self.frame, pc, kind]);
        let success = builder.create_block();
        let failed = builder.create_block();
        let okay = builder
            .ins()
            .icmp_imm(IntCC::Equal, status, i64::from(qjs::JS_JIT_HELPER_OK));
        builder.ins().brif(okay, success, &[], failed, &[]);
        builder.switch_to_block(failed);
        emit_opt_exit(
            builder,
            self.root.sret,
            qjs::JSJitExitKind_JS_JIT_EXIT_EXCEPTION,
            None,
            self.root.pointer_type,
            0,
        );
        builder.switch_to_block(success);
        // Resume has already materialized and consumed every inline ancestor.
        // Never call emit_opt_deopt here: its stale caller SSA would replay or
        // overwrite effects. The fresh AfterEffect identity map is mandatory.
        let pc = builder.ins().load(
            self.root.pointer_type,
            MemFlags::new(),
            self.root.frame,
            self.root.layout.pc,
        );
        emit_opt_exit(
            builder,
            self.root.sret,
            qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
            Some(pc),
            self.root.pointer_type,
            self.region.continuation.guard,
        );
        Ok(())
    }
    fn helper(
        &self,
        builder: &mut FunctionBuilder<'_>,
        id: u32,
        arguments: &[u32],
        pc: u32,
        live: usize,
        exception_depth: usize,
    ) -> Result<(), CompileFailure> {
        set_pc(builder, self.root, self.frame, pc);
        self.depth(builder, live);
        let id = id as usize;
        let signature = *self
            .root
            .helper_signatures
            .get(id)
            .ok_or(CompileFailure::InvalidArtifact)?;
        let api = builder.ins().load(
            self.root.pointer_type,
            MemFlags::new(),
            self.frame,
            self.root.layout.runtime_api,
        );
        let pointer = builder.ins().load(
            self.root.pointer_type,
            MemFlags::new(),
            api,
            self.root.layout.helper_offsets[id],
        );
        let mut params = vec![self.frame];
        params.extend(
            arguments
                .iter()
                .map(|value| builder.ins().iconst(types::I32, i64::from(*value))),
        );
        let call = super::super::emit_external_call(
            builder,
            signature,
            pointer,
            &params,
            self.root.pointer_type,
            Some(self.frame),
            None,
        );
        let status = builder.inst_results(call)[0];
        let okay = builder
            .ins()
            .icmp_imm(IntCC::Equal, status, i64::from(qjs::JS_JIT_HELPER_OK));
        let success = builder.create_block();
        let failed = builder.create_block();
        builder.ins().brif(okay, success, &[], failed, &[]);
        builder.switch_to_block(failed);
        self.depth(builder, exception_depth);
        self.resume(builder, pc, true)?;
        builder.switch_to_block(success);
        Ok(())
    }
    /// Memory location of a flat helper slot in this shadow frame.
    fn slot_address(&self, slot: u32) -> (Value, usize) {
        let slot = slot as usize;
        let arguments = usize::from(self.region.body.argument_count);
        let locals = usize::from(self.region.body.local_count);
        if slot < arguments {
            (self.arguments, slot)
        } else if slot < arguments + locals {
            (self.locals, slot - arguments)
        } else {
            (self.stack, slot - arguments - locals)
        }
    }
    /// Exact `JS_JitHelperFree`: the helper's pc and depth are published
    /// first, then primitives and shared heap references are released inline
    /// (leaving undefined, as the helper does). The last heap reference and
    /// stress-GC frames still call the helper.
    fn free(
        &self,
        builder: &mut FunctionBuilder<'_>,
        slot: u32,
        pc: u32,
        live: usize,
        exception_depth: usize,
    ) -> Result<(), CompileFailure> {
        set_pc(builder, self.root, self.frame, pc);
        self.depth(builder, live);
        let (base, index) = self.slot_address(slot);
        let value = opt_load(builder, base, index);
        let heap = builder.create_block();
        let primitive = builder.create_block();
        let clear = builder.create_block();
        let helper = builder.create_block();
        let done = builder.create_block();
        builder.set_cold_block(helper);
        let refcounted = super::super::refcount::emit_has_ref_count(builder, value.tag);
        builder.ins().brif(refcounted, heap, &[], primitive, &[]);
        builder.switch_to_block(primitive);
        let no_stress =
            super::super::refcount::emit_no_stress(builder, self.frame, self.root.layout.flags);
        builder.ins().brif(no_stress, clear, &[], helper, &[]);
        builder.switch_to_block(heap);
        super::super::refcount::emit_release_refcounted(
            builder,
            self.frame,
            self.root.layout.flags,
            value.payload,
            clear,
            helper,
        );
        builder.switch_to_block(clear);
        let empty = undefined(builder);
        opt_store(builder, base, index, empty);
        builder.ins().jump(done, &[]);
        builder.switch_to_block(helper);
        self.helper(
            builder,
            qjs::JSJitHelperId_JS_JIT_HELPER_FREE,
            &[0, slot],
            pc,
            live,
            exception_depth,
        )?;
        builder.ins().jump(done, &[]);
        builder.switch_to_block(done);
        Ok(())
    }
    /// Exact `JS_JitHelperDup` into an unoccupied slot: primitives are copied
    /// and heap values gain one reference inline; stress-GC frames keep the
    /// helper for every value.
    fn dup(
        &self,
        builder: &mut FunctionBuilder<'_>,
        output: u32,
        input: u32,
        pc: u32,
        live: usize,
        exception_depth: usize,
    ) -> Result<(), CompileFailure> {
        set_pc(builder, self.root, self.frame, pc);
        self.depth(builder, live);
        let (input_base, input_index) = self.slot_address(input);
        let (output_base, output_index) = self.slot_address(output);
        let value = opt_load(builder, input_base, input_index);
        let heap = builder.create_block();
        let copy = builder.create_block();
        let helper = builder.create_block();
        let done = builder.create_block();
        builder.set_cold_block(helper);
        let primitive = builder.create_block();
        let refcounted = super::super::refcount::emit_has_ref_count(builder, value.tag);
        builder.ins().brif(refcounted, heap, &[], primitive, &[]);
        builder.switch_to_block(primitive);
        let no_stress =
            super::super::refcount::emit_no_stress(builder, self.frame, self.root.layout.flags);
        builder.ins().brif(no_stress, copy, &[], helper, &[]);
        builder.switch_to_block(heap);
        super::super::refcount::emit_dup_refcounted(
            builder,
            self.frame,
            self.root.layout.flags,
            value.payload,
            copy,
            helper,
        );
        builder.switch_to_block(copy);
        opt_store(builder, output_base, output_index, value);
        builder.ins().jump(done, &[]);
        builder.switch_to_block(helper);
        self.helper(
            builder,
            qjs::JSJitHelperId_JS_JIT_HELPER_DUP,
            &[0, output, input],
            pc,
            live,
            exception_depth,
        )?;
        builder.ins().jump(done, &[]);
        builder.switch_to_block(done);
        Ok(())
    }
    fn check(&self, builder: &mut FunctionBuilder<'_>, pc: u32) -> Result<(), CompileFailure> {
        let status = runtime_call(
            builder,
            self.root,
            self.api.check.ok_or(CompileFailure::InvalidArtifact)? as usize,
            &[self.frame],
        );
        let good = builder.create_block();
        let bad = builder.create_block();
        let okay = builder
            .ins()
            .icmp_imm(IntCC::Equal, status, i64::from(qjs::JS_JIT_HELPER_OK));
        builder.ins().brif(okay, good, &[], bad, &[]);
        builder.switch_to_block(bad);
        let retired = builder.create_block();
        let malformed = builder.create_block();
        let miss = builder.ins().icmp_imm(
            IntCC::Equal,
            status,
            i64::from(qjs::JS_JIT_HELPER_GUARD_MISS),
        );
        builder.ins().brif(miss, retired, &[], malformed, &[]);
        builder.switch_to_block(retired);
        self.resume(builder, pc, false)?;
        builder.switch_to_block(malformed);
        self.resume(builder, pc, true)?;
        builder.switch_to_block(good);
        Ok(())
    }
    fn leave(
        &self,
        builder: &mut FunctionBuilder<'_>,
        result: usize,
        next: Block,
    ) -> Result<(), CompileFailure> {
        let result = builder
            .ins()
            .iconst(types::I32, i64::from(self.flat(result)?));
        let status = runtime_call(
            builder,
            self.root,
            self.api.leave.ok_or(CompileFailure::InvalidArtifact)? as usize,
            &[self.frame, result],
        );
        let okay = builder
            .ins()
            .icmp_imm(IntCC::Equal, status, i64::from(qjs::JS_JIT_HELPER_OK));
        let failed = builder.create_block();
        builder.ins().brif(okay, next, &[], failed, &[]);
        builder.switch_to_block(failed);
        // Validation failure retains the view; Resume rejects invariant failure
        // without replay, and the root trampoline owns emergency destruction.
        self.resume(
            builder,
            self.region
                .boundaries
                .last()
                .ok_or(CompileFailure::InvalidArtifact)?
                .pc,
            true,
        )
    }
}

/// Emits an actual native callee body between Enter and Leave. Unsupported
/// instructions and speculation failures resume the callee, never the CALL.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit(
    builder: &mut FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    region: &ScalarFrameInlineRegion,
    node: &crate::ir::OptimizedNode,
    argc: usize,
    provenance: &mut [OptProvenance],
    depth: usize,
    api: &qjs::JSJitInlineAPI,
    guard: u32,
) -> Result<usize, CompileFailure> {
    let has_this = region.call_kind == crate::ir::InlineCallKind::Method;
    let base = depth
        .checked_sub(argc + 1 + usize::from(has_this))
        .ok_or(CompileFailure::InvalidArtifact)?;
    let new_depth = base + 1;
    if region.body.blocks.is_empty()
        || region.body.blocks.len() > 16
        || region.continuation.call_node != node.id()
        || region.continuation.call_pc != node.pc()
        || usize::from(region.continuation.shape.arguments()) != env.arguments.len()
        || usize::from(region.continuation.shape.locals()) != env.locals.len()
        || usize::from(region.continuation.shape.stack()) != new_depth
    {
        return Err(CompileFailure::InvalidArtifact);
    }
    env.property_cache.flush(builder);
    env.property_cache.invalidate(builder);
    let function_index = base + usize::from(has_this);
    let callable = opt_use(builder, env.stack[function_index]);
    let guarded = builder.create_block();
    let rejected = builder.create_block();
    super::super::emit_guarded_direct_callee_identity(
        builder,
        callable.tag,
        callable.payload,
        super::super::DirectCalleeIdentity {
            object: region.object_identity,
            bytecode: region.bytecode_identity,
        },
        env.pointer_type,
        guarded,
        rejected,
    );
    builder.switch_to_block(rejected);
    emit_opt_deopt(builder, env, provenance, depth, node.pc(), guard)?;
    builder.switch_to_block(guarded);
    for (index, vars) in env.arguments.iter().enumerate() {
        let pair = opt_use(builder, *vars);
        opt_store(builder, env.arg_buf, index, pair);
    }
    for (index, vars) in env.locals.iter().enumerate() {
        let pair = opt_use(builder, *vars);
        opt_store(builder, env.var_buf, index, pair);
    }
    for (index, vars) in env.stack.iter().take(depth).enumerate() {
        let pair = opt_use(builder, *vars);
        opt_store(builder, env.stack_base, index, pair);
    }
    set_pc(builder, env, env.frame, node.pc());
    opt_own_stack_for_helper(
        builder,
        env.frame,
        env.sret,
        env.stack_base,
        depth,
        env.arguments.len() + env.locals.len(),
        provenance,
        env.helper_signatures,
        env.pointer_type,
        env.layout,
    )?;
    let output = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        env.pointer_type.bytes(),
        0,
    ));
    let output = builder.ins().stack_addr(env.pointer_type, output, 0);
    let values = [
        0,
        node.pc(),
        region.continuation.resume_pc,
        region.call_kind.abi_kind(),
        u32::try_from(argc).map_err(|_| CompileFailure::ResourceLimit)?,
    ];
    let mut params = vec![env.frame];
    params.extend(
        values
            .into_iter()
            .map(|value| builder.ins().iconst(types::I32, i64::from(value))),
    );
    params.push(output);
    let status = runtime_call(
        builder,
        env,
        api.enter.ok_or(CompileFailure::InvalidArtifact)? as usize,
        &params,
    );
    let entered = builder.create_block();
    let declined = builder.create_block();
    let okay = builder
        .ins()
        .icmp_imm(IntCC::Equal, status, i64::from(qjs::JS_JIT_HELPER_OK));
    builder.ins().brif(okay, entered, &[], declined, &[]);
    builder.switch_to_block(declined);
    let missed = builder.create_block();
    let exception = builder.create_block();
    let miss = builder.ins().icmp_imm(
        IntCC::Equal,
        status,
        i64::from(qjs::JS_JIT_HELPER_GUARD_MISS),
    );
    builder.ins().brif(miss, missed, &[], exception, &[]);
    builder.switch_to_block(exception);
    set_pc(builder, env, env.frame, region.continuation.resume_pc);
    emit_opt_exit(
        builder,
        env.sret,
        qjs::JSJitExitKind_JS_JIT_EXIT_EXCEPTION,
        None,
        env.pointer_type,
        0,
    );
    builder.switch_to_block(missed);
    let pc = set_pc(builder, env, env.frame, node.pc());
    emit_opt_exit(
        builder,
        env.sret,
        qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
        Some(pc),
        env.pointer_type,
        guard,
    );
    builder.switch_to_block(entered);
    let frame = builder
        .ins()
        .load(env.pointer_type, MemFlags::new(), output, 0);
    let state = InlineFrame {
        root: env,
        region,
        api,
        frame,
        arguments: builder
            .ins()
            .load(env.pointer_type, MemFlags::new(), frame, env.layout.arg_buf),
        locals: builder
            .ins()
            .load(env.pointer_type, MemFlags::new(), frame, env.layout.var_buf),
        stack: builder.ins().load(
            env.pointer_type,
            MemFlags::new(),
            frame,
            env.layout.stack_base,
        ),
    };
    let body = builder.create_block();
    let retry = builder.create_block();
    let identity = builder.ins().load(
        types::I64,
        MemFlags::new(),
        frame,
        core::mem::offset_of!(qjs::JSJitExecFrame, function_id) as i32,
    );
    let generation = builder.ins().load(
        types::I64,
        MemFlags::new(),
        frame,
        core::mem::offset_of!(qjs::JSJitExecFrame, generation) as i32,
    );
    let id_ok = builder
        .ins()
        .icmp_imm(IntCC::Equal, identity, region.artifact.function_id as i64);
    let generation_ok =
        builder
            .ins()
            .icmp_imm(IntCC::Equal, generation, region.artifact.generation as i64);
    let matches = builder.ins().band(id_ok, generation_ok);
    let target = builder.create_block();
    builder.ins().brif(matches, target, &[], retry, &[]);
    builder.switch_to_block(target);
    // Enter includes a reentrant interrupt checkpoint; use the owned function
    // in the root frame, not pre-poll SSA, and establish no surviving heap facts.
    let callable = opt_load(builder, env.stack_base, function_index);
    super::super::emit_guarded_direct_callee_identity(
        builder,
        callable.tag,
        callable.payload,
        super::super::DirectCalleeIdentity {
            object: region.object_identity,
            bytecode: region.bytecode_identity,
        },
        env.pointer_type,
        body,
        retry,
    );
    builder.switch_to_block(retry);
    state.resume(builder, 0, false)?;
    builder.switch_to_block(body);
    state.check(builder, 0)?;
    let continuation = builder.create_block();
    emit_body(builder, &state, continuation)?;
    builder.switch_to_block(continuation);
    // Leave consumed the callee pointer. Its owner cleanup can reenter, so the
    // active root must also be checked before using the caller's compiled code.
    let status = runtime_call(
        builder,
        env,
        api.check.ok_or(CompileFailure::InvalidArtifact)? as usize,
        &[env.frame],
    );
    let valid = builder.create_block();
    let invalid = builder.create_block();
    let okay = builder
        .ins()
        .icmp_imm(IntCC::Equal, status, i64::from(qjs::JS_JIT_HELPER_OK));
    builder.ins().brif(okay, valid, &[], invalid, &[]);
    builder.switch_to_block(invalid);
    let retired = builder.create_block();
    let malformed = builder.create_block();
    let miss = builder.ins().icmp_imm(
        IntCC::Equal,
        status,
        i64::from(qjs::JS_JIT_HELPER_GUARD_MISS),
    );
    builder.ins().brif(miss, retired, &[], malformed, &[]);
    builder.switch_to_block(malformed);
    emit_opt_exit(
        builder,
        env.sret,
        qjs::JSJitExitKind_JS_JIT_EXIT_EXCEPTION,
        None,
        env.pointer_type,
        0,
    );
    builder.switch_to_block(retired);
    let pc = builder
        .ins()
        .load(env.pointer_type, MemFlags::new(), env.frame, env.layout.pc);
    emit_opt_exit(
        builder,
        env.sret,
        qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
        Some(pc),
        env.pointer_type,
        region.continuation.guard,
    );
    builder.switch_to_block(valid);
    for (index, vars) in env.arguments.iter().enumerate() {
        let pair = opt_load(builder, env.arg_buf, index);
        opt_define(builder, *vars, pair);
    }
    for (index, vars) in env.locals.iter().enumerate() {
        let pair = opt_load(builder, env.var_buf, index);
        opt_define(builder, *vars, pair);
    }
    for (index, vars) in env.stack.iter().enumerate() {
        let pair = if index < new_depth {
            opt_load(builder, env.stack_base, index)
        } else {
            undefined(builder)
        };
        opt_define(builder, *vars, pair);
    }
    for (index, owner) in provenance.iter_mut().enumerate() {
        *owner = if index < new_depth {
            OptProvenance::OwnedSlot
        } else {
            OptProvenance::ImmediatePrimitive
        };
    }
    Ok(new_depth)
}

fn emit_body(
    builder: &mut FunctionBuilder<'_>,
    state: &InlineFrame<'_, '_>,
    continuation: Block,
) -> Result<(), CompileFailure> {
    let blocks = state
        .region
        .body
        .blocks
        .iter()
        .map(|block| (block.start_pc, builder.create_block()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let entry = *blocks
        .get(
            &state
                .region
                .body
                .blocks
                .first()
                .ok_or(CompileFailure::InvalidArtifact)?
                .start_pc,
        )
        .ok_or(CompileFailure::InvalidArtifact)?;
    builder.ins().jump(entry, &[]);
    for (block_index, block) in state.region.body.blocks.iter().enumerate() {
        let native = *blocks
            .get(&block.start_pc)
            .ok_or(CompileFailure::InvalidArtifact)?;
        builder.switch_to_block(native);
        let mut depth = usize::from(block.stack_depth);
        let mut terminated = false;
        for instruction in &block.instructions {
            if let IrOp::Poll { kind, .. } = instruction.op {
                if kind != PollKind::Entry {
                    state.helper(
                        builder,
                        qjs::JSJitHelperId_JS_JIT_HELPER_POLL,
                        &[],
                        instruction.pc,
                        depth,
                        depth,
                    )?;
                    state.check(builder, instruction.pc)?;
                }
                continue;
            }
            if matches!(instruction.op, IrOp::OsrLabel { .. }) {
                continue;
            }
            let boundary = state
                .region
                .boundaries
                .iter()
                .find(|boundary| boundary.pc == instruction.pc)
                .ok_or(CompileFailure::InvalidArtifact)?;
            if usize::from(boundary.stack_before) != depth {
                return Err(CompileFailure::InvalidArtifact);
            }
            set_pc(builder, state.root, state.frame, instruction.pc);
            let next = boundary.next_pc;
            // Baseline translates tail_call into Call+Return at the same origin.
            // Until the frame backend models that expansion, resume the original
            // opcode before any effect instead of inventing an intermediate origin.
            if let IrOp::Call { argc, has_this } = instruction.op {
                let result_depth = depth
                    .checked_sub(usize::from(argc) + usize::from(has_this))
                    .ok_or(CompileFailure::InvalidArtifact)?;
                if usize::from(boundary.stack_after) != result_depth {
                    state.resume(builder, instruction.pc, false)?;
                    terminated = true;
                    break;
                }
            }
            let failed = builder.create_block();
            let mut has_guard = false;
            match instruction.op {
                IrOp::Nop => {}
                IrOp::Push(value) if (value.tag as u32) < qjs::JS_TAG_FIRST as u32 => {
                    let pair = OptPair {
                        payload: builder.ins().iconst(types::I64, value.payload as i64),
                        tag: builder.ins().iconst(types::I64, value.tag),
                    };
                    opt_store(builder, state.stack, depth, pair);
                }
                IrOp::GetArgument(index) | IrOp::GetLocal(index) | IrOp::GetLocalChecked(index) => {
                    let input = if matches!(instruction.op, IrOp::GetArgument(_)) {
                        u32::from(index)
                    } else {
                        u32::from(state.region.body.argument_count) + u32::from(index)
                    };
                    if matches!(instruction.op, IrOp::GetLocalChecked(_)) {
                        let local = opt_load(builder, state.locals, usize::from(index));
                        let initialized = builder.ins().icmp_imm(
                            IntCC::NotEqual,
                            local.tag,
                            i64::from(qjs::JS_TAG_UNINITIALIZED),
                        );
                        let valid = builder.create_block();
                        let tdz = builder.create_block();
                        builder.ins().brif(initialized, valid, &[], tdz, &[]);
                        builder.switch_to_block(tdz);
                        state.resume(builder, instruction.pc, false)?;
                        builder.switch_to_block(valid);
                    }
                    state.dup(builder, state.flat(depth)?, input, next, depth + 1, depth)?;
                }
                IrOp::PutArgument { index, keep } | IrOp::PutLocal { index, keep } => {
                    let (buffer, slot) = if matches!(instruction.op, IrOp::PutArgument { .. }) {
                        (state.arguments, u32::from(index))
                    } else {
                        (
                            state.locals,
                            u32::from(state.region.body.argument_count) + u32::from(index),
                        )
                    };
                    state.free(builder, slot, next, depth, depth)?;
                    if keep {
                        state.dup(builder, slot, state.flat(depth - 1)?, next, depth, depth)?;
                    } else {
                        let pair = opt_load(builder, state.stack, depth - 1);
                        opt_store(builder, buffer, usize::from(index), pair);
                        let empty = undefined(builder);
                        opt_store(builder, state.stack, depth - 1, empty);
                    }
                }
                IrOp::PutLocalChecked { index, initialize } => {
                    if !initialize {
                        let local = opt_load(builder, state.locals, usize::from(index));
                        let initialized = builder.ins().icmp_imm(
                            IntCC::NotEqual,
                            local.tag,
                            i64::from(qjs::JS_TAG_UNINITIALIZED),
                        );
                        let valid = builder.create_block();
                        let tdz = builder.create_block();
                        builder.ins().brif(initialized, valid, &[], tdz, &[]);
                        builder.switch_to_block(tdz);
                        state.resume(builder, instruction.pc, false)?;
                        builder.switch_to_block(valid);
                    }
                    let slot = u32::from(state.region.body.argument_count) + u32::from(index);
                    state.free(builder, slot, next, depth, depth)?;
                    let pair = opt_load(builder, state.stack, depth - 1);
                    opt_store(builder, state.locals, usize::from(index), pair);
                    let empty = undefined(builder);
                    opt_store(builder, state.stack, depth - 1, empty);
                }
                IrOp::SetLocalUninitialized(index) => {
                    let slot = u32::from(state.region.body.argument_count) + u32::from(index);
                    state.free(builder, slot, next, depth, depth)?;
                    let empty = OptPair {
                        payload: builder.ins().iconst(types::I64, 0),
                        tag: builder
                            .ins()
                            .iconst(types::I64, i64::from(qjs::JS_TAG_UNINITIALIZED)),
                    };
                    opt_store(builder, state.locals, usize::from(index), empty);
                }
                IrOp::Drop => state.free(builder, state.flat(depth - 1)?, next, depth, depth)?,
                IrOp::Stack(StackOp::Dup) => state.dup(
                    builder,
                    state.flat(depth)?,
                    state.flat(depth - 1)?,
                    next,
                    depth + 1,
                    depth,
                )?,
                IrOp::Stack(StackOp::Swap) => {
                    let lhs = opt_load(builder, state.stack, depth - 2);
                    let rhs = opt_load(builder, state.stack, depth - 1);
                    opt_store(builder, state.stack, depth - 2, rhs);
                    opt_store(builder, state.stack, depth - 1, lhs);
                }
                IrOp::Binary(op) => {
                    let lhs = opt_load(builder, state.stack, depth - 2);
                    let rhs = opt_load(builder, state.stack, depth - 1);
                    if let Some(result) = checked_binary(builder, op, lhs, rhs, failed) {
                        has_guard = true;
                        opt_store(builder, state.stack, depth - 2, result);
                        let empty = undefined(builder);
                        opt_store(builder, state.stack, depth - 1, empty);
                    } else {
                        state.resume(builder, instruction.pc, false)?;
                        terminated = true;
                        break;
                    }
                }
                IrOp::Unary(op @ (UnaryOp::Increment | UnaryOp::Decrement))
                | IrOp::PostUnary(op @ (UnaryOp::Increment | UnaryOp::Decrement)) => {
                    let lhs = opt_load(builder, state.stack, depth - 1);
                    let rhs = OptPair {
                        payload: builder.ins().iconst(types::I64, 1),
                        tag: builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
                    };
                    let op = if op == UnaryOp::Increment {
                        BinaryOp::Add
                    } else {
                        BinaryOp::Sub
                    };
                    let result = checked_binary(builder, op, lhs, rhs, failed)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    has_guard = true;
                    // PostUnary leaves the old numeric value as the expression
                    // result and pushes the update for the following store.
                    // Both guards precede any write, so recovery retains the
                    // original input and PC even after earlier frame effects.
                    let output = if matches!(instruction.op, IrOp::PostUnary(_)) {
                        depth
                    } else {
                        depth - 1
                    };
                    opt_store(builder, state.stack, output, result);
                }
                IrOp::GetProperty(atom) | IrOp::GetPropertyKeep(atom) => {
                    state.helper(
                        builder,
                        qjs::JSJitHelperId_JS_JIT_HELPER_GET_PROPERTY,
                        &[0, state.flat(depth)?, state.flat(depth - 1)?, atom],
                        next,
                        depth + 1,
                        depth,
                    )?;
                    if matches!(instruction.op, IrOp::GetProperty(_)) {
                        let receiver = opt_load(builder, state.stack, depth - 1);
                        let result = opt_load(builder, state.stack, depth);
                        opt_store(builder, state.stack, depth - 1, result);
                        opt_store(builder, state.stack, depth, receiver);
                        state.free(builder, state.flat(depth)?, next, depth + 1, depth)?;
                    }
                }
                IrOp::SetProperty(atom) => {
                    state.helper(
                        builder,
                        qjs::JSJitHelperId_JS_JIT_HELPER_SET_PROPERTY,
                        &[0, state.flat(depth - 2)?, atom, state.flat(depth - 1)?],
                        next,
                        depth,
                        depth,
                    )?;
                    state.free(builder, state.flat(depth - 2)?, next, depth, depth)?;
                }
                IrOp::Call { argc, has_this } => {
                    if let Some(child) = state.region.children.get(&instruction.pc) {
                        emit_nested_call(
                            builder,
                            state,
                            child,
                            instruction.pc,
                            next,
                            argc,
                            has_this,
                            depth,
                        )?;
                        // InlineLeave has already applied the interpreter's
                        // complete CALL stack transition in the parent frame.
                    } else {
                        let base = depth
                            .checked_sub(usize::from(argc) + 1 + usize::from(has_this))
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        let function = base + usize::from(has_this);
                        let this = if has_this { base } else { depth };
                        let output = if has_this { depth } else { depth + 1 };
                        let argv = if argc == 0 {
                            u32::MAX
                        } else {
                            state.flat(function + 1)?
                        };
                        state.helper(
                            builder,
                            qjs::JSJitHelperId_JS_JIT_HELPER_CALL,
                            &[
                                0,
                                state.flat(output)?,
                                state.flat(function)?,
                                state.flat(this)?,
                                argv,
                                u32::from(argc),
                            ],
                            next,
                            output + 1,
                            depth,
                        )?;
                        let displaced = opt_load(builder, state.stack, base);
                        let result = opt_load(builder, state.stack, output);
                        opt_store(builder, state.stack, base, result);
                        opt_store(builder, state.stack, output, displaced);
                        state.free(builder, state.flat(output)?, next, output + 1, depth)?;
                        for input in base + 1..depth {
                            state.free(builder, state.flat(input)?, next, depth, depth)?;
                        }
                    }
                }
                IrOp::Jump(target) => {
                    let target = *blocks.get(&target).ok_or(CompileFailure::InvalidArtifact)?;
                    builder.ins().jump(target, &[]);
                    terminated = true;
                }
                IrOp::Branch { target, when_true } => {
                    let condition = depth
                        .checked_sub(1)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let slot = state.flat(condition)?;
                    state.helper(
                        builder,
                        qjs::JSJitHelperId_JS_JIT_HELPER_TO_BOOL,
                        &[0, slot, slot],
                        instruction.pc,
                        depth,
                        depth,
                    )?;
                    let boolean = opt_load(builder, state.stack, condition);
                    let truth = builder.ins().ireduce(types::I8, boolean.payload);
                    // The branch successor is not committed yet. A retirement
                    // after ToBoolean must resume this branch, not always its
                    // fallthrough PC; the converted Boolean is equivalent input.
                    state.check(builder, instruction.pc)?;
                    state.free(builder, slot, next, depth, depth - 1)?;
                    depth -= 1;
                    state.depth(builder, depth);
                    let target = *blocks.get(&target).ok_or(CompileFailure::InvalidArtifact)?;
                    let fallthrough = state
                        .region
                        .body
                        .blocks
                        .get(block_index + 1)
                        .and_then(|block| blocks.get(&block.start_pc))
                        .copied()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let taken = if when_true {
                        truth
                    } else {
                        builder.ins().bxor_imm(truth, 1)
                    };
                    builder.ins().brif(taken, target, &[], fallthrough, &[]);
                    terminated = true;
                }
                IrOp::Return => {
                    state.leave(builder, depth - 1, continuation)?;
                    terminated = true;
                }
                IrOp::ReturnUndefined => {
                    let empty = undefined(builder);
                    opt_store(builder, state.stack, depth, empty);
                    state.depth(builder, depth + 1);
                    state.leave(builder, depth, continuation)?;
                    terminated = true;
                }
                _ => {
                    state.resume(builder, instruction.pc, false)?;
                    terminated = true;
                }
            }
            if terminated {
                break;
            }
            depth = usize::from(boundary.stack_after);
            state.depth(builder, depth);
            if matches!(
                instruction.op,
                IrOp::GetArgument(_)
                    | IrOp::GetLocal(_)
                    | IrOp::GetLocalChecked(_)
                    | IrOp::PutArgument { .. }
                    | IrOp::PutLocal { .. }
                    | IrOp::PutLocalChecked { .. }
                    | IrOp::SetLocalUninitialized(_)
                    | IrOp::Drop
                    | IrOp::Stack(StackOp::Dup)
                    | IrOp::GetProperty(_)
                    | IrOp::GetPropertyKeep(_)
                    | IrOp::SetProperty(_)
                    | IrOp::Call { .. }
            ) {
                // Finish the complete semantic operation before checking retirement:
                // a getter/call has committed before its consumed-owner cleanup.
                // Only cleanup DUP/FREE may finish on a retired trusted shadow.
                state.check(builder, next)?;
            }
            if has_guard {
                let next_block = builder.create_block();
                builder.ins().jump(next_block, &[]);
                builder.switch_to_block(failed);
                // No slot is written until every type/overflow guard succeeds.
                state.resume(builder, instruction.pc, false)?;
                builder.switch_to_block(next_block);
            }
            if terminated {
                break;
            }
        }
        if !terminated {
            let next = state
                .region
                .body
                .blocks
                .get(block_index + 1)
                .and_then(|block| blocks.get(&block.start_pc))
                .copied()
                .ok_or(CompileFailure::InvalidArtifact)?;
            builder.ins().jump(next, &[]);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_nested_call(
    builder: &mut FunctionBuilder<'_>,
    parent: &InlineFrame<'_, '_>,
    region: &ScalarFrameInlineRegion,
    call_pc: u32,
    resume_pc: u32,
    argc: u16,
    has_this: bool,
    depth: usize,
) -> Result<(), CompileFailure> {
    let expected_kind = if has_this {
        crate::ir::InlineCallKind::Method
    } else {
        crate::ir::InlineCallKind::Call
    };
    if region.call_kind != expected_kind
        || region.continuation.call_pc != call_pc
        || region.continuation.resume_pc != resume_pc
        || region.body.blocks.is_empty()
        || region.body.blocks.len() > 16
    {
        return Err(CompileFailure::InvalidArtifact);
    }
    let argc = usize::from(argc);
    let base = depth
        .checked_sub(argc + 1 + usize::from(has_this))
        .ok_or(CompileFailure::InvalidArtifact)?;
    let function = base + usize::from(has_this);
    let callable = opt_load(builder, parent.stack, function);
    let guarded = builder.create_block();
    let rejected = builder.create_block();
    super::super::emit_guarded_direct_callee_identity(
        builder,
        callable.tag,
        callable.payload,
        super::super::DirectCalleeIdentity {
            object: region.object_identity,
            bytecode: region.bytecode_identity,
        },
        parent.root.pointer_type,
        guarded,
        rejected,
    );
    builder.switch_to_block(rejected);
    parent.resume(builder, call_pc, false)?;
    builder.switch_to_block(guarded);
    set_pc(builder, parent.root, parent.frame, call_pc);
    parent.depth(builder, depth);
    let output = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        parent.root.pointer_type.bytes(),
        0,
    ));
    let output = builder
        .ins()
        .stack_addr(parent.root.pointer_type, output, 0);
    let values = [
        0,
        call_pc,
        resume_pc,
        region.call_kind.abi_kind(),
        u32::try_from(argc).map_err(|_| CompileFailure::ResourceLimit)?,
    ];
    let mut params = vec![parent.frame];
    params.extend(
        values
            .into_iter()
            .map(|value| builder.ins().iconst(types::I32, i64::from(value))),
    );
    params.push(output);
    let status = runtime_call(
        builder,
        parent.root,
        parent.api.enter.ok_or(CompileFailure::InvalidArtifact)? as usize,
        &params,
    );
    let entered = builder.create_block();
    let declined = builder.create_block();
    let okay = builder
        .ins()
        .icmp_imm(IntCC::Equal, status, i64::from(qjs::JS_JIT_HELPER_OK));
    builder.ins().brif(okay, entered, &[], declined, &[]);
    builder.switch_to_block(declined);
    let missed = builder.create_block();
    let exception = builder.create_block();
    let miss = builder.ins().icmp_imm(
        IntCC::Equal,
        status,
        i64::from(qjs::JS_JIT_HELPER_GUARD_MISS),
    );
    builder.ins().brif(miss, missed, &[], exception, &[]);
    builder.switch_to_block(missed);
    parent.resume(builder, call_pc, false)?;
    builder.switch_to_block(exception);
    parent.resume(builder, resume_pc, true)?;
    builder.switch_to_block(entered);
    let frame = builder
        .ins()
        .load(parent.root.pointer_type, MemFlags::new(), output, 0);
    let child = InlineFrame {
        root: parent.root,
        region,
        api: parent.api,
        frame,
        arguments: builder.ins().load(
            parent.root.pointer_type,
            MemFlags::new(),
            frame,
            parent.root.layout.arg_buf,
        ),
        locals: builder.ins().load(
            parent.root.pointer_type,
            MemFlags::new(),
            frame,
            parent.root.layout.var_buf,
        ),
        stack: builder.ins().load(
            parent.root.pointer_type,
            MemFlags::new(),
            frame,
            parent.root.layout.stack_base,
        ),
    };
    let retry = builder.create_block();
    let body = builder.create_block();
    let identity = builder.ins().load(
        types::I64,
        MemFlags::new(),
        frame,
        core::mem::offset_of!(qjs::JSJitExecFrame, function_id) as i32,
    );
    let generation = builder.ins().load(
        types::I64,
        MemFlags::new(),
        frame,
        core::mem::offset_of!(qjs::JSJitExecFrame, generation) as i32,
    );
    let id_ok = builder
        .ins()
        .icmp_imm(IntCC::Equal, identity, region.artifact.function_id as i64);
    let generation_ok =
        builder
            .ins()
            .icmp_imm(IntCC::Equal, generation, region.artifact.generation as i64);
    let artifact_ok = builder.ins().band(id_ok, generation_ok);
    builder.ins().brif(artifact_ok, body, &[], retry, &[]);
    builder.switch_to_block(body);
    let callable = opt_load(builder, parent.stack, function);
    let verified = builder.create_block();
    super::super::emit_guarded_direct_callee_identity(
        builder,
        callable.tag,
        callable.payload,
        super::super::DirectCalleeIdentity {
            object: region.object_identity,
            bytecode: region.bytecode_identity,
        },
        parent.root.pointer_type,
        verified,
        retry,
    );
    builder.switch_to_block(retry);
    child.resume(builder, 0, false)?;
    builder.switch_to_block(verified);
    child.check(builder, 0)?;
    let continuation = builder.create_block();
    emit_body(builder, &child, continuation)?;
    builder.switch_to_block(continuation);
    // The child view has been consumed by Leave. Revalidate the now-active
    // parent before continuing at the exact post-CALL origin.
    parent.check(builder, resume_pc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cranelift_codegen::{
        ir::{AbiParam, Function, Signature},
        settings,
    };
    use cranelift_frontend::FunctionBuilderContext;

    fn run(op: BinaryOp, lhs: i64, rhs: i64, tag: i64) -> (i64, i64) {
        let isa = cranelift_native::builder()
            .unwrap()
            .finish(settings::Flags::new(settings::builder()))
            .unwrap();
        let mut signature = Signature::new(isa.default_call_conv());
        for _ in 0..3 {
            signature.params.push(AbiParam::new(types::I64));
        }
        signature.params.push(AbiParam::new(isa.pointer_type()));
        let mut function = Function::with_name_signature(Default::default(), signature);
        let mut context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut function, &mut context);
            let entry = builder.create_block();
            let failed = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            let args = builder.block_params(entry).to_vec();
            let int = builder.ins().iconst(types::I64, 0);
            let output = checked_binary(
                &mut builder,
                op,
                OptPair {
                    payload: args[0],
                    tag: int,
                },
                OptPair {
                    payload: args[1],
                    tag: args[2],
                },
                failed,
            )
            .unwrap();
            opt_store(&mut builder, args[3], 0, output);
            builder.ins().return_(&[]);
            builder.switch_to_block(failed);
            builder.ins().return_(&[]);
            builder.seal_all_blocks();
            builder.finalize();
        }
        let code =
            super::super::super::baseline::finalize_optimized_machine(&isa, function, None, false)
                .unwrap()
                .publish()
                .unwrap();
        let invoke: unsafe extern "C" fn(i64, i64, i64, *mut i64) =
            unsafe { core::mem::transmute(code.as_ptr()) };
        let mut result = [99, 77];
        unsafe { invoke(lhs, rhs, tag, result.as_mut_ptr()) };
        (result[0], result[1])
    }

    #[test]
    fn checked_inline_arithmetic_keeps_pre_instruction_state_on_failure() {
        assert_eq!(run(BinaryOp::Add, 12, 30, 0), (42, 0));
        assert_eq!(run(BinaryOp::Add, i64::from(i32::MAX), 1, 0), (99, 77));
        assert_eq!(
            run(
                BinaryOp::Add,
                12,
                30,
                i64::from(rquickjs_core::qjs::JS_TAG_OBJECT)
            ),
            (99, 77)
        );
        assert_eq!(run(BinaryOp::Mul, 0, -1, 0), (99, 77));
    }

    #[test]
    fn checked_inline_arithmetic_preserves_int32_and_boolean_semantics() {
        for (op, lhs, rhs, want) in [
            (BinaryOp::Sub, 42, 30, (12, 0)),
            (BinaryOp::Sub, i64::from(i32::MIN), 1, (99, 77)),
            (BinaryOp::Mul, -6, 7, (-42, 0)),
            (BinaryOp::Mul, i64::from(i32::MIN), -1, (99, 77)),
            (BinaryOp::Mul, -1, 0, (99, 77)),
            (BinaryOp::BitAnd, -1, 7, (7, 0)),
            (BinaryOp::BitOr, 8, 7, (15, 0)),
            (BinaryOp::BitXor, 15, 7, (8, 0)),
            (BinaryOp::LessThan, -1, 0, (1, i64::from(qjs::JS_TAG_BOOL))),
            (
                BinaryOp::GreaterThanOrEqual,
                -1,
                0,
                (0, i64::from(qjs::JS_TAG_BOOL)),
            ),
            (
                BinaryOp::StrictEqual,
                7,
                7,
                (1, i64::from(qjs::JS_TAG_BOOL)),
            ),
            (
                BinaryOp::StrictNotEqual,
                7,
                7,
                (0, i64::from(qjs::JS_TAG_BOOL)),
            ),
            // The JSValue integer payload is an int32 union member; upper bits
            // are not semantic. Treating it as a full i64 changes this result.
            (BinaryOp::Add, 0x1234_5678_0000_0007, 1, (8, 0)),
        ] {
            assert_eq!(run(op, lhs, rhs, 0), want, "{op:?} {lhs} {rhs}");
        }
    }
}
