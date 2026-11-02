//! Inline QuickJS reference-count fast paths.
//!
//! `JS_JitHelperDup` and `JS_JitHelperFree` are exact `js_dup` and
//! `JS_FreeValue` operations behind a full helper-frame validation. Generated
//! code performs the non-finalizing part of both operations directly:
//!
//! * DUP of a refcounted value increments `ref_count`; it can never allocate,
//!   finalize, throw or reenter.
//! * FREE of a refcounted value whose `ref_count` is greater than one only
//!   decrements it. When the count would reach zero the helper still runs, so
//!   finalizers, GC list maintenance and reentrancy stay in C.
//!
//! Frames with `JS_JIT_FRAME_STRESS_GC` keep the helpers, because collecting at
//! every helper boundary belongs to the stress contract. The header offset is
//! pinned by the ABI value-layout fingerprint (see
//! `sys/patches/0022-inline-refcount.patch`), which is validated before any
//! artifact is compiled or entered.

use cranelift_codegen::ir::{condcodes::IntCC, types, Block, InstBuilder, MemFlags, Value};
use cranelift_frontend::FunctionBuilder;
use rquickjs_core::qjs;

/// Offset of the in-place count, pinned by the ABI value-layout fingerprint.
use crate::abi::REF_COUNT_OFFSET;
const _: () = assert!(crate::abi::REF_COUNT_BYTES == 4);

/// Exact `JS_VALUE_HAS_REF_COUNT`: QuickJS compares the tag as `unsigned`.
pub(super) fn emit_has_ref_count(builder: &mut FunctionBuilder<'_>, tag: Value) -> Value {
    let tag = if builder.func.dfg.value_type(tag) == types::I32 {
        tag
    } else {
        builder.ins().ireduce(types::I32, tag)
    };
    builder.ins().icmp_imm(
        IntCC::UnsignedGreaterThanOrEqual,
        tag,
        i64::from(qjs::JS_TAG_FIRST),
    )
}

/// True unless the frame requests a GC at every helper boundary.
pub(super) fn emit_no_stress(builder: &mut FunctionBuilder<'_>, frame: Value, flags: i32) -> Value {
    let flags = builder
        .ins()
        .load(types::I32, MemFlags::new(), frame, flags);
    let stress = builder
        .ins()
        .band_imm(flags, i64::from(qjs::JS_JIT_FRAME_STRESS_GC));
    builder.ins().icmp_imm(IntCC::Equal, stress, 0)
}

/// Duplicates `payload`, which the caller proved refcounted, unless the frame
/// is in stress mode. Continues in `done` after the increment, or in `helper`
/// with no side effect.
pub(super) fn emit_dup_refcounted(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    flags: i32,
    payload: Value,
    done: Block,
    helper: Block,
) {
    let no_stress = emit_no_stress(builder, frame, flags);
    let increment = builder.create_block();
    builder.ins().brif(no_stress, increment, &[], helper, &[]);
    builder.seal_block(increment);
    builder.switch_to_block(increment);
    emit_increment(builder, payload);
    builder.ins().jump(done, &[]);
}

/// Unconditional `ref_count++` of a payload known to be refcounted.
pub(super) fn emit_increment(builder: &mut FunctionBuilder<'_>, payload: Value) {
    let count = builder
        .ins()
        .load(types::I32, MemFlags::trusted(), payload, REF_COUNT_OFFSET);
    let count = builder.ins().iadd_imm(count, 1);
    builder
        .ins()
        .store(MemFlags::trusted(), count, payload, REF_COUNT_OFFSET);
}

/// Releases `payload`, which the caller proved refcounted, without a helper
/// when the reference is not the last one. Continues in `released` after the
/// decrement, or in `helper` with the count untouched (last reference or
/// stress mode) so the helper performs the exact `JS_FreeValue`.
pub(super) fn emit_release_refcounted(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    flags: i32,
    payload: Value,
    released: Block,
    helper: Block,
) {
    let no_stress = emit_no_stress(builder, frame, flags);
    let check = builder.create_block();
    builder.ins().brif(no_stress, check, &[], helper, &[]);
    builder.seal_block(check);
    builder.switch_to_block(check);
    let count = builder
        .ins()
        .load(types::I32, MemFlags::trusted(), payload, REF_COUNT_OFFSET);
    let shared = builder.ins().icmp_imm(IntCC::SignedGreaterThan, count, 1);
    let decrement = builder.create_block();
    builder.ins().brif(shared, decrement, &[], helper, &[]);
    builder.seal_block(decrement);
    builder.switch_to_block(decrement);
    let count = builder.ins().iadd_imm(count, -1);
    builder
        .ins()
        .store(MemFlags::trusted(), count, payload, REF_COUNT_OFFSET);
    builder.ins().jump(released, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use cranelift_codegen::{
        ir::{AbiParam, Function, Signature},
        settings,
    };
    use cranelift_frontend::FunctionBuilderContext;

    /// fn(flags: *const u32, tag: i64, payload: *mut i32) -> i32
    /// returns 1 for the inline path and 0 for the helper path.
    fn compile(release: bool) -> super::super::baseline::PublishedBaselineCode {
        let isa = cranelift_native::builder()
            .unwrap()
            .finish(settings::Flags::new(settings::builder()))
            .unwrap();
        let mut signature = Signature::new(isa.default_call_conv());
        signature.params.push(AbiParam::new(isa.pointer_type()));
        signature.params.push(AbiParam::new(types::I64));
        signature.params.push(AbiParam::new(isa.pointer_type()));
        signature.returns.push(AbiParam::new(types::I32));
        let mut function = Function::with_name_signature(Default::default(), signature);
        let mut context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut function, &mut context);
            let entry = builder.create_block();
            let refcounted = builder.create_block();
            let inline = builder.create_block();
            let helper = builder.create_block();
            let primitive = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            let parameters = builder.block_params(entry).to_vec();
            let has = emit_has_ref_count(&mut builder, parameters[1]);
            builder.ins().brif(has, refcounted, &[], primitive, &[]);
            builder.switch_to_block(refcounted);
            if release {
                emit_release_refcounted(
                    &mut builder,
                    parameters[0],
                    0,
                    parameters[2],
                    inline,
                    helper,
                );
            } else {
                emit_dup_refcounted(
                    &mut builder,
                    parameters[0],
                    0,
                    parameters[2],
                    inline,
                    helper,
                );
            }
            builder.switch_to_block(inline);
            let one = builder.ins().iconst(types::I32, 1);
            builder.ins().return_(&[one]);
            builder.switch_to_block(helper);
            let zero = builder.ins().iconst(types::I32, 0);
            builder.ins().return_(&[zero]);
            builder.switch_to_block(primitive);
            let two = builder.ins().iconst(types::I32, 2);
            builder.ins().return_(&[two]);
            builder.seal_all_blocks();
            builder.finalize();
        }
        super::super::baseline::finalize_optimized_machine(&isa, function, None, false)
            .unwrap()
            .publish()
            .unwrap()
    }

    type Probe = unsafe extern "C" fn(*const u32, i64, *mut i32) -> i32;

    #[test]
    fn dup_increments_only_refcounted_payloads_outside_stress() {
        let code = compile(false);
        // SAFETY: the generated function has exactly this ABI and only touches
        // the supplied flag word and count.
        let probe: Probe = unsafe { core::mem::transmute(code.as_ptr()) };
        let flags = 0_u32;
        let mut count = 1_i32;
        for tag in [qjs::JS_TAG_OBJECT, qjs::JS_TAG_STRING, qjs::JS_TAG_BIG_INT] {
            assert_eq!(unsafe { probe(&flags, i64::from(tag), &mut count) }, 1);
        }
        assert_eq!(count, 4);
        for tag in [
            qjs::JS_TAG_INT,
            qjs::JS_TAG_UNDEFINED,
            qjs::JS_TAG_FLOAT64,
            -10,
        ] {
            assert_eq!(unsafe { probe(&flags, i64::from(tag), &mut count) }, 2);
        }
        let stress = qjs::JS_JIT_FRAME_STRESS_GC;
        assert_eq!(
            unsafe { probe(&stress, i64::from(qjs::JS_TAG_OBJECT), &mut count) },
            0
        );
        assert_eq!(count, 4);
    }

    #[test]
    fn release_defers_last_reference_and_stress_to_helper() {
        let code = compile(true);
        // SAFETY: see dup_increments_only_refcounted_payloads_outside_stress.
        let probe: Probe = unsafe { core::mem::transmute(code.as_ptr()) };
        let flags = 0_u32;
        let mut count = 3_i32;
        let object = i64::from(qjs::JS_TAG_OBJECT);
        assert_eq!(unsafe { probe(&flags, object, &mut count) }, 1);
        assert_eq!(count, 2);
        assert_eq!(unsafe { probe(&flags, object, &mut count) }, 1);
        assert_eq!(count, 1);
        // The last reference must reach JS_FreeValue untouched.
        assert_eq!(unsafe { probe(&flags, object, &mut count) }, 0);
        assert_eq!(count, 1);
        count = 5;
        let stress = qjs::JS_JIT_FRAME_STRESS_GC;
        assert_eq!(unsafe { probe(&stress, object, &mut count) }, 0);
        assert_eq!(count, 5);
        assert_eq!(
            unsafe { probe(&flags, i64::from(qjs::JS_TAG_INT), &mut count) },
            2
        );
        assert_eq!(count, 5);
    }
}
