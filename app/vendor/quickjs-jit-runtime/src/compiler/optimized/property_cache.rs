//! Lazy guarded primitive-field values carried in Cranelift SSA variables.

use super::*;
use crate::ir::{PropertyAccessPlan, PropertyCandidate, PropertyPlan};
use cranelift_codegen::ir::{types, InstBuilder, Type};
use cranelift_frontend::{FunctionBuilder, Variable};

struct CachedProperty {
    candidate: PropertyCandidate,
    /// The guarded representation. Every definition that can be observed
    /// while `valid` (or `dirty`) holds carries exactly this tag.
    tag: i32,
    value: OptVars,
    data: Variable,
    valid: Variable,
    dirty: Variable,
}

impl CachedProperty {
    /// Read a valid (or dirty) cached value. Its tag is a compile-time
    /// constant: loop-carried tag phis would otherwise survive Cranelift's
    /// constant-phi removal and keep a redundant tag check in the loop.
    fn checked_value(&self, builder: &mut FunctionBuilder<'_>) -> OptPair {
        let payload = builder.use_var(self.value.payload);
        let tag = builder.ins().iconst(types::I64, i64::from(self.tag));
        OptPair { payload, tag }
    }
}

#[derive(Default)]
pub(super) struct PropertyCache {
    entries: Vec<CachedProperty>,
}

impl PropertyCache {
    pub(super) fn new(
        builder: &mut FunctionBuilder<'_>,
        plan: &PropertyPlan,
        next_var: &mut u32,
        pointer_type: Type,
    ) -> Result<Self, CompileFailure> {
        Self::from_candidates(builder, plan.candidates(), next_var, pointer_type)
    }

    fn from_candidates(
        builder: &mut FunctionBuilder<'_>,
        candidates: &[PropertyCandidate],
        next_var: &mut u32,
        pointer_type: Type,
    ) -> Result<Self, CompileFailure> {
        let mut entries = Vec::with_capacity(candidates.len());
        for &candidate in candidates {
            let first = *next_var;
            *next_var = first.checked_add(5).ok_or(CompileFailure::ResourceLimit)?;
            let entry = CachedProperty {
                candidate,
                tag: primitive_tag(candidate.observation.value())?,
                value: OptVars {
                    payload: Variable::from_u32(first),
                    tag: Variable::from_u32(first + 1),
                },
                data: Variable::from_u32(first + 2),
                valid: Variable::from_u32(first + 3),
                dirty: Variable::from_u32(first + 4),
            };
            builder.declare_var(entry.value.payload, types::I64);
            builder.declare_var(entry.value.tag, types::I64);
            builder.declare_var(entry.data, pointer_type);
            builder.declare_var(entry.valid, types::I8);
            builder.declare_var(entry.dirty, types::I8);
            let zero = builder.ins().iconst(types::I64, 0);
            let tag = builder
                .ins()
                .iconst(types::I64, i64::from(rquickjs_core::qjs::JS_TAG_UNDEFINED));
            opt_define(builder, entry.value, OptPair { payload: zero, tag });
            let null = builder.ins().iconst(pointer_type, 0);
            builder.def_var(entry.data, null);
            let no = builder.ins().iconst(types::I8, 0);
            builder.def_var(entry.valid, no);
            builder.def_var(entry.dirty, no);
            entries.push(entry);
        }
        Ok(Self { entries })
    }

    /// Commit before any callback, ownership helper or exit can observe the
    /// object. Dirty implies both a successfully checked field and a live root.
    pub(super) fn flush(&self, builder: &mut FunctionBuilder<'_>) {
        for entry in &self.entries {
            Self::flush_entry(entry, builder);
        }
    }

    fn flush_entry(entry: &CachedProperty, builder: &mut FunctionBuilder<'_>) {
        let dirty = builder.use_var(entry.dirty);
        let commit = builder.create_block();
        let done = builder.create_block();
        builder.ins().brif(dirty, commit, &[], done, &[]);
        builder.switch_to_block(commit);
        let data = builder.use_var(entry.data);
        let value = entry.checked_value(builder);
        opt_store_at(builder, data, 0, value);
        let no = builder.ins().iconst(types::I8, 0);
        builder.def_var(entry.dirty, no);
        builder.ins().jump(done, &[]);
        builder.switch_to_block(done);
    }

    /// The caller must flush before the reentrant boundary, then invalidate
    /// after it. No cached address is dereferenced by this operation.
    pub(super) fn invalidate(&self, builder: &mut FunctionBuilder<'_>) {
        for entry in &self.entries {
            Self::invalidate_entry(entry, builder);
        }
    }

    /// Re-establish every promoted field after the cold poll has published
    /// and invalidated the cache.  Successful loop-entry and poll edges then
    /// both carry `valid = true`, allowing Cranelift to remove the per-access
    /// validity branch from the hot loop.  Any changed receiver/shape/value
    /// tag deoptimizes from the poll frame state before JavaScript resumes.
    pub(super) fn revalidate_after_poll(
        &self,
        builder: &mut FunctionBuilder<'_>,
        env: &OptEnv<'_>,
        provenance: &[OptProvenance],
        depth: usize,
        pc: u32,
        guard: u32,
    ) -> Result<(), CompileFailure> {
        use cranelift_codegen::ir::{condcodes::IntCC, MemFlags};
        use rquickjs_core::qjs;

        if self.entries.is_empty() {
            return Ok(());
        }
        let layout = crate::abi::AbiInfo::linked()
            .map_err(|_| CompileFailure::InvalidArtifact)?
            .property_layout();
        let deopt = builder.create_block();
        builder.set_cold_block(deopt);
        let continuation = builder.create_block();
        builder.set_cold_block(continuation);
        for entry in &self.entries {
            let property = entry.candidate.observation;
            let object = *env
                .arguments
                .get(usize::from(entry.candidate.argument))
                .ok_or(CompileFailure::InvalidArtifact)?;
            let object = opt_use(builder, object);
            let object_ok =
                builder
                    .ins()
                    .icmp_imm(IntCC::Equal, object.tag, i64::from(qjs::JS_TAG_OBJECT));
            let shape_block = builder.create_block();
            builder.set_cold_block(shape_block);
            builder.ins().brif(object_ok, shape_block, &[], deopt, &[]);
            builder.switch_to_block(shape_block);
            let shape = builder.ins().load(
                env.pointer_type,
                MemFlags::new(),
                object.payload,
                layout.object_shape_offset,
            );
            let same_shape =
                builder
                    .ins()
                    .icmp_imm(IntCC::Equal, shape, property.shape().identity() as i64);
            let generation = builder.ins().load(
                types::I64,
                MemFlags::new(),
                shape,
                layout.shape_generation_offset,
            );
            let same_generation = builder.ins().icmp_imm(
                IntCC::Equal,
                generation,
                property.shape().generation() as i64,
            );
            let shape_ok = builder.ins().band(same_shape, same_generation);
            let field_block = builder.create_block();
            builder.set_cold_block(field_block);
            builder.ins().brif(shape_ok, field_block, &[], deopt, &[]);
            builder.switch_to_block(field_block);
            let offset = property
                .offset()
                .checked_mul(16)
                .and_then(|n| i32::try_from(n).ok())
                .filter(|n| n.checked_add(8).is_some())
                .ok_or(CompileFailure::ResourceLimit)?;
            let props = builder.ins().load(
                env.pointer_type,
                MemFlags::new(),
                object.payload,
                layout.object_properties_offset,
            );
            let data = builder.ins().iadd_imm(props, i64::from(offset));
            let current = OptPair {
                payload: builder.ins().load(types::I64, MemFlags::new(), data, 0),
                tag: builder.ins().load(types::I64, MemFlags::new(), data, 8),
            };
            let expected_tag = primitive_tag(property.value())?;
            let tag_ok = builder
                .ins()
                .icmp_imm(IntCC::Equal, current.tag, i64::from(expected_tag));
            let checked = builder.create_block();
            builder.set_cold_block(checked);
            builder.ins().brif(tag_ok, checked, &[], deopt, &[]);
            builder.switch_to_block(checked);
            // The tag was just checked; carry it as a constant so loop-carried
            // cache values have a provably fixed representation.
            let checked_tag = builder.ins().iconst(types::I64, i64::from(expected_tag));
            opt_define(
                builder,
                entry.value,
                OptPair {
                    payload: current.payload,
                    tag: checked_tag,
                },
            );
            builder.def_var(entry.data, data);
            let yes = builder.ins().iconst(types::I8, 1);
            let no = builder.ins().iconst(types::I8, 0);
            builder.def_var(entry.valid, yes);
            builder.def_var(entry.dirty, no);
        }
        builder.ins().jump(continuation, &[]);
        builder.switch_to_block(deopt);
        emit_opt_deopt(builder, env, provenance, depth, pc, guard)?;
        builder.switch_to_block(continuation);
        Ok(())
    }

    fn invalidate_entry(entry: &CachedProperty, builder: &mut FunctionBuilder<'_>) {
        let no = builder.ins().iconst(types::I8, 0);
        builder.def_var(entry.valid, no);
        builder.def_var(entry.dirty, no);
        let prior = builder.use_var(entry.data);
        let pointer_type = builder.func.dfg.value_type(prior);
        let null = builder.ins().iconst(pointer_type, 0);
        builder.def_var(entry.data, null);
    }

    /// Match the existing guarded leaf path's ownership contract. Otherwise
    /// the caller must flush/invalidate and use the owning helper path.
    pub(super) fn can_emit_access(
        &self,
        access: &PropertyAccessPlan,
        depth: usize,
        provenance: &[OptProvenance],
    ) -> bool {
        let Some(object) = depth.checked_sub(if access.store { 2 } else { 1 }) else {
            return false;
        };
        let Some(live) = provenance.get(..depth) else {
            return false;
        };
        self.entries.get(access.candidate).is_some()
            && matches!(
                live[object],
                OptProvenance::Argument(_) | OptProvenance::Local(_)
            )
            && live.iter().all(|source| {
                matches!(
                    source,
                    OptProvenance::Argument(_)
                        | OptProvenance::Local(_)
                        | OptProvenance::ImmediatePrimitive
                )
            })
    }

    #[allow(clippy::too_many_arguments)] // Lowering inputs mirror the guarded access contract.
    pub(super) fn emit_access(
        &self,
        builder: &mut FunctionBuilder<'_>,
        env: &OptEnv<'_>,
        access: &PropertyAccessPlan,
        node: &crate::ir::OptimizedNode,
        loop_forwarded: bool,
        proven_input: Option<crate::ir::ScalarNumericMode>,
        depth: usize,
        provenance: &mut [OptProvenance],
    ) -> Result<usize, CompileFailure> {
        use cranelift_codegen::ir::{condcodes::IntCC, MemFlags};
        use rquickjs_core::qjs;
        if !self.can_emit_access(access, depth, provenance)
            || env.payload_type != types::I64
            || !matches!(node.kind(), crate::ir::OptimizedNodeKind::Bytecode { opcode }
                if opcode.as_ref() == if access.store { "put_field" } else { "get_field" })
        {
            return Err(CompileFailure::InvalidArtifact);
        }
        let entry = &self.entries[access.candidate];
        let candidate = entry.candidate;
        if usize::from(candidate.argument) >= env.arguments.len() {
            return Err(CompileFailure::InvalidArtifact);
        }
        let object_index = depth - if access.store { 2 } else { 1 };
        if env.stack.len() < depth {
            return Err(CompileFailure::InvalidArtifact);
        }
        let guard = node.deopt_guard().ok_or(CompileFailure::InvalidArtifact)?;
        let expected_tag = primitive_tag(candidate.observation.value())?;
        let property = candidate.observation;
        let offset = property
            .offset()
            .checked_mul(16)
            .and_then(|n| i32::try_from(n).ok())
            .filter(|n| n.checked_add(8).is_some())
            .ok_or(CompileFailure::ResourceLimit)?;
        if property.shape().identity() == 0
            || property.shape().generation() == 0
            || property.prototype().identity() != 0
            || property.prototype().generation() != 0
            || property
                .attributes()
                .contains(crate::runtime::PropertyAttributes::ACCESSOR)
            || !property
                .attributes()
                .contains(crate::runtime::PropertyAttributes::WRITABLE)
        {
            return Err(CompileFailure::InvalidArtifact);
        }
        // Distinct argument identities are allowed to alias at runtime. First
        // publish their writes, then forget their values and guarded addresses.
        for &alias in access.aliases.iter() {
            if alias == access.candidate {
                return Err(CompileFailure::InvalidArtifact);
            }
            let other = self
                .entries
                .get(alias)
                .ok_or(CompileFailure::InvalidArtifact)?;
            Self::flush_entry(other, builder);
            Self::invalidate_entry(other, builder);
        }
        let deopt = builder.create_block();
        builder.set_cold_block(deopt);
        let access_block = builder.create_block();
        if loop_forwarded {
            builder.ins().jump(access_block, &[]);
        } else {
            let miss = builder.create_block();
            let valid = builder.use_var(entry.valid);
            builder.ins().brif(valid, access_block, &[], miss, &[]);
            builder.switch_to_block(miss);
            let object = opt_use(builder, env.stack[object_index]);
            let object_ok =
                builder
                    .ins()
                    .icmp_imm(IntCC::Equal, object.tag, i64::from(qjs::JS_TAG_OBJECT));
            let shape_block = builder.create_block();
            builder.ins().brif(object_ok, shape_block, &[], deopt, &[]);
            builder.switch_to_block(shape_block);
            let layout = crate::abi::AbiInfo::linked()
                .map_err(|_| CompileFailure::InvalidArtifact)?
                .property_layout();
            let shape = builder.ins().load(
                env.pointer_type,
                MemFlags::new(),
                object.payload,
                layout.object_shape_offset,
            );
            let same_shape =
                builder
                    .ins()
                    .icmp_imm(IntCC::Equal, shape, property.shape().identity() as i64);
            let generation = builder.ins().load(
                types::I64,
                MemFlags::new(),
                shape,
                layout.shape_generation_offset,
            );
            let same_generation = builder.ins().icmp_imm(
                IntCC::Equal,
                generation,
                property.shape().generation() as i64,
            );
            let shape_ok = builder.ins().band(same_shape, same_generation);
            let field_block = builder.create_block();
            builder.ins().brif(shape_ok, field_block, &[], deopt, &[]);
            builder.switch_to_block(field_block);
            let props = builder.ins().load(
                env.pointer_type,
                MemFlags::new(),
                object.payload,
                layout.object_properties_offset,
            );
            let data = builder.ins().iadd_imm(props, i64::from(offset));
            let current = OptPair {
                payload: builder.ins().load(types::I64, MemFlags::new(), data, 0),
                tag: builder.ins().load(types::I64, MemFlags::new(), data, 8),
            };
            let tag_ok = builder
                .ins()
                .icmp_imm(IntCC::Equal, current.tag, i64::from(expected_tag));
            let checked = builder.create_block();
            builder.ins().brif(tag_ok, checked, &[], deopt, &[]);
            builder.switch_to_block(checked);
            let checked_tag = builder.ins().iconst(types::I64, i64::from(expected_tag));
            opt_define(
                builder,
                entry.value,
                OptPair {
                    payload: current.payload,
                    tag: checked_tag,
                },
            );
            builder.def_var(entry.data, data);
            let yes = builder.ins().iconst(types::I8, 1);
            builder.def_var(entry.valid, yes);
            builder.ins().jump(access_block, &[]);
        }
        builder.switch_to_block(access_block);
        if access.store {
            let value = opt_use(builder, env.stack[depth - 1]);
            let input_ok = builder
                .ins()
                .icmp_imm(IntCC::Equal, value.tag, i64::from(expected_tag));
            // A statically proven input representation (for example a
            // checked Int32 arithmetic result, whose lowering always carries
            // JS_TAG_INT) needs no branch in the hot path.
            let proven = matches!(
                (proven_input, expected_tag),
                (Some(crate::ir::ScalarNumericMode::Int32), qjs::JS_TAG_INT)
                    | (
                        Some(crate::ir::ScalarNumericMode::Float64),
                        qjs::JS_TAG_FLOAT64
                    )
            );
            if !proven && opt_known_condition(builder.func, input_ok, 8) != Some(true) {
                let commit = builder.create_block();
                builder.ins().brif(input_ok, commit, &[], deopt, &[]);
                builder.switch_to_block(commit);
            }
            // This is the semantic store. Recovery may publish it from here
            // onward, but a failing incoming-value guard must retain the prior
            // completed value instead of writing the failed input.
            let checked_tag = builder.ins().iconst(types::I64, i64::from(expected_tag));
            opt_define(
                builder,
                entry.value,
                OptPair {
                    payload: value.payload,
                    tag: checked_tag,
                },
            );
            let yes = builder.ins().iconst(types::I8, 1);
            builder.def_var(entry.dirty, yes);
        }
        let continuation = builder.create_block();
        builder.ins().jump(continuation, &[]);
        builder.switch_to_block(deopt);
        // Root integration makes this common exit flush every dirty candidate
        // before spilling or invoking an ownership helper.
        emit_opt_deopt(builder, env, provenance, depth, node.pc(), guard)?;
        builder.switch_to_block(continuation);

        // No stack memory is written on this leaf path. Every live operand is
        // a borrowed argument/local or an immediate primitive (see
        // `can_emit_access`), so no owner can be lost, and every observer of
        // interpreter stack memory republishes it from SSA first: deopt
        // (`emit_opt_deopt`), ownership helpers (`opt_own_stack_for_exit`
        // callers) and generic bridges all store the live prefix and set
        // `stack_top` themselves. Leaving memory and `stack_top` untouched
        // preserves the invariant that `[stack_base, stack_top)` holds only
        // values the interpreter may release, which is all an interrupt
        // unwind can observe.
        let new_depth = if access.store {
            provenance[object_index..depth].fill(OptProvenance::Unknown);
            object_index
        } else {
            let value = entry.checked_value(builder);
            opt_define(builder, env.stack[object_index], value);
            provenance[object_index] = OptProvenance::ImmediatePrimitive;
            depth
        };
        Ok(new_depth)
    }
}

fn primitive_tag(value: crate::runtime::ObservedType) -> Result<i32, CompileFailure> {
    use crate::runtime::ObservedType;
    use rquickjs_core::qjs;
    Ok(match value {
        ObservedType::Int32 => qjs::JS_TAG_INT,
        ObservedType::Float64 => qjs::JS_TAG_FLOAT64,
        ObservedType::Bool => qjs::JS_TAG_BOOL,
        ObservedType::Null => qjs::JS_TAG_NULL,
        ObservedType::Undefined => qjs::JS_TAG_UNDEFINED,
        _ => return Err(CompileFailure::InvalidArtifact),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{
        ObservedType, PropertyAttributes, PrototypeDependencyToken, ShapeObservation, ShapeToken,
    };
    use cranelift_codegen::{
        ir::{AbiParam, Function, Signature},
        settings,
    };
    use cranelift_frontend::FunctionBuilderContext;

    fn candidate() -> PropertyCandidate {
        PropertyCandidate {
            argument: 0,
            atom: 1,
            observation: ShapeObservation::new(
                ShapeToken::new(123, 1),
                PrototypeDependencyToken::new(0, 0),
                0,
                PropertyAttributes::WRITABLE,
                ObservedType::Int32,
            ),
        }
    }

    fn compiled_flush(twice: bool) -> super::super::super::baseline::PublishedBaselineCode {
        let isa = cranelift_native::builder()
            .unwrap()
            .finish(settings::Flags::new(settings::builder()))
            .unwrap();
        let mut signature = Signature::new(isa.default_call_conv());
        signature.params.push(AbiParam::new(isa.pointer_type()));
        signature.params.push(AbiParam::new(types::I8));
        signature.returns.push(AbiParam::new(types::I8));
        let mut function = Function::with_name_signature(Default::default(), signature);
        let mut context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut function, &mut context);
            let block = builder.create_block();
            builder.append_block_params_for_function_params(block);
            builder.switch_to_block(block);
            let params = builder.block_params(block).to_vec();
            let cache = PropertyCache::from_candidates(
                &mut builder,
                &[candidate()],
                &mut 0,
                isa.pointer_type(),
            )
            .unwrap();
            let entry = &cache.entries[0];
            builder.def_var(entry.data, params[0]);
            builder.def_var(entry.dirty, params[1]);
            let yes = builder.ins().iconst(types::I8, 1);
            builder.def_var(entry.valid, yes);
            let payload = builder.ins().iconst(types::I64, 42);
            let tag = builder
                .ins()
                .iconst(types::I64, i64::from(rquickjs_core::qjs::JS_TAG_INT));
            opt_define(&mut builder, entry.value, OptPair { payload, tag });
            cache.flush(&mut builder);
            if twice {
                // A second flush must not replay a value already committed.
                let changed = builder.ins().iconst(types::I64, 99);
                builder.ins().store(
                    cranelift_codegen::ir::MemFlags::new(),
                    changed,
                    params[0],
                    0,
                );
                cache.flush(&mut builder);
            }
            let dirty = builder.use_var(entry.dirty);
            builder.ins().return_(&[dirty]);
            builder.seal_all_blocks();
            builder.finalize();
        }
        super::super::super::baseline::finalize_optimized_machine(&isa, function, None, false)
            .unwrap()
            .publish()
            .unwrap()
    }

    #[test]
    fn flush_commits_only_dirty_values_and_clears_dirty_flag() {
        let code = compiled_flush(false);
        // SAFETY: This exactly matches the generated host ABI and the pointer
        // spans both stored i64 words while the published code remains alive.
        let run: unsafe extern "C" fn(*mut i64, u8) -> u8 =
            unsafe { core::mem::transmute(code.as_ptr()) };
        // The untouched/zero-trip state must not access its cached address.
        assert_eq!(unsafe { run(core::ptr::null_mut(), 0) }, 0);
        let mut field = [7, 8];
        assert_eq!(unsafe { run(field.as_mut_ptr(), 0) }, 0);
        assert_eq!(field, [7, 8]);
        assert_eq!(unsafe { run(field.as_mut_ptr(), 1) }, 0);
        assert_eq!(field, [42, i64::from(rquickjs_core::qjs::JS_TAG_INT)]);
    }

    #[test]
    fn repeated_flush_does_not_replay_a_committed_store() {
        let code = compiled_flush(true);
        // SAFETY: See the generated signature and live two-word field above.
        let run: unsafe extern "C" fn(*mut i64, u8) -> u8 =
            unsafe { core::mem::transmute(code.as_ptr()) };
        let mut field = [7, 8];
        assert_eq!(unsafe { run(field.as_mut_ptr(), 1) }, 0);
        assert_eq!(field, [99, i64::from(rquickjs_core::qjs::JS_TAG_INT)]);
    }

    #[test]
    fn cache_access_requires_borrowed_roots_and_rejects_owned_stack_values() {
        let cache = PropertyCache {
            entries: vec![CachedProperty {
                candidate: candidate(),
                tag: rquickjs_core::qjs::JS_TAG_INT,
                value: OptVars {
                    payload: Variable::from_u32(0),
                    tag: Variable::from_u32(1),
                },
                data: Variable::from_u32(2),
                valid: Variable::from_u32(3),
                dirty: Variable::from_u32(4),
            }],
        };
        let load = PropertyAccessPlan {
            candidate: 0,
            store: false,
            aliases: Box::new([]),
        };
        let store = PropertyAccessPlan {
            candidate: 0,
            store: true,
            aliases: Box::new([]),
        };
        assert!(cache.can_emit_access(&load, 1, &[OptProvenance::Argument(0)]));
        assert!(cache.can_emit_access(
            &store,
            2,
            &[OptProvenance::Local(0), OptProvenance::ImmediatePrimitive]
        ));
        for source in [OptProvenance::OwnedSlot, OptProvenance::Unknown] {
            assert!(!cache.can_emit_access(&load, 1, &[source]));
            assert!(!cache.can_emit_access(&load, 2, &[source, OptProvenance::Argument(0)]));
            assert!(!cache.can_emit_access(&store, 2, &[OptProvenance::Argument(0), source]));
        }
        assert!(!cache.can_emit_access(&load, 0, &[]));
        assert!(!cache.can_emit_access(&store, 1, &[OptProvenance::Argument(0)]));
        assert!(!cache.can_emit_access(&store, usize::MAX, &[]));
    }
}
