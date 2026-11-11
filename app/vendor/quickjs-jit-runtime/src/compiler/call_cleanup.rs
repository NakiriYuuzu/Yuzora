//! Ownership-preserving dispatch for consumed generic-call operands.

use cranelift_codegen::ir::{condcodes::IntCC, types, Block, InstBuilder, MemFlags, Value};
use cranelift_frontend::FunctionBuilder;
use rquickjs_core::qjs;

/// Select the existing FREE helper or a primitive slot clear. The caller owns
/// result placement, slot clearing, helper stack maps and exception ordering.
pub(super) fn emit_cleanup_dispatch(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    tag: Value,
    flags_offset: i32,
    primitive: Block,
    helper: Block,
) {
    // Match JS_VALUE_HAS_REF_COUNT exactly: QuickJS casts the tag to unsigned
    // int, even though the fingerprinted JSValue stores it in a 64-bit field.
    let tag = builder.ins().ireduce(types::I32, tag);
    let primitive_tag =
        builder
            .ins()
            .icmp_imm(IntCC::UnsignedLessThan, tag, i64::from(qjs::JS_TAG_FIRST));
    // Stress collection belongs to the helper contract, including for values
    // without a reference count. Keep that path observable in stress mode.
    let flags = builder
        .ins()
        .load(types::I32, MemFlags::new(), frame, flags_offset);
    let stress = builder
        .ins()
        .band_imm(flags, i64::from(qjs::JS_JIT_FRAME_STRESS_GC));
    let no_stress = builder.ins().icmp_imm(IntCC::Equal, stress, 0);
    let may_clear = builder.ins().band(primitive_tag, no_stress);
    builder.ins().brif(may_clear, primitive, &[], helper, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use cranelift_codegen::{
        ir::{types, AbiParam, Function, Signature},
        settings,
    };
    use cranelift_frontend::FunctionBuilderContext;
    use rquickjs_core::qjs;

    fn compiled_dispatch() -> super::super::baseline::PublishedBaselineCode {
        let isa = cranelift_native::builder()
            .unwrap()
            .finish(settings::Flags::new(settings::builder()))
            .unwrap();
        let mut signature = Signature::new(isa.default_call_conv());
        signature.params.push(AbiParam::new(isa.pointer_type()));
        signature.params.push(AbiParam::new(types::I64));
        signature.returns.push(AbiParam::new(types::I32));
        let mut function = Function::with_name_signature(Default::default(), signature);
        let mut context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut function, &mut context);
            let entry = builder.create_block();
            let primitive = builder.create_block();
            let helper = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            let parameters = builder.block_params(entry).to_vec();
            emit_cleanup_dispatch(
                &mut builder,
                parameters[0],
                parameters[1],
                0,
                primitive,
                helper,
            );
            builder.switch_to_block(primitive);
            let cleared = builder.ins().iconst(types::I32, 1);
            builder.ins().return_(&[cleared]);
            builder.switch_to_block(helper);
            let requires_helper = builder.ins().iconst(types::I32, 0);
            builder.ins().return_(&[requires_helper]);
            builder.seal_all_blocks();
            builder.finalize();
        }
        super::super::baseline::finalize_optimized_machine(&isa, function, None, false)
            .unwrap()
            .publish()
            .unwrap()
    }

    #[test]
    fn primitive_call_operands_bypass_free_but_heap_owners_do_not() {
        let code = compiled_dispatch();
        // SAFETY: The generated function has exactly this host ABI, reads only
        // the supplied u32 flags, and its publication remains alive throughout.
        let dispatch: unsafe extern "C" fn(*const u32, i64) -> i32 =
            unsafe { core::mem::transmute(code.as_ptr()) };
        let flags = 0_u32;
        for tag in [
            qjs::JS_TAG_INT,
            qjs::JS_TAG_BOOL,
            qjs::JS_TAG_NULL,
            qjs::JS_TAG_UNDEFINED,
            qjs::JS_TAG_FLOAT64,
            qjs::JS_TAG_SHORT_BIG_INT,
        ] {
            assert_eq!(unsafe { dispatch(&flags, i64::from(tag)) }, 1, "{tag}");
        }
        for tag in [
            qjs::JS_TAG_OBJECT,
            qjs::JS_TAG_STRING,
            qjs::JS_TAG_STRING_ROPE,
            qjs::JS_TAG_SYMBOL,
            qjs::JS_TAG_BIG_INT,
            qjs::JS_TAG_MODULE,
            qjs::JS_TAG_FUNCTION_BYTECODE,
        ] {
            assert_eq!(unsafe { dispatch(&flags, i64::from(tag)) }, 0, "{tag}");
        }
        // C casts the tag to unsigned int before comparing. Negative tags
        // below JS_TAG_FIRST are outside the refcount interval, not owners.
        assert_eq!(unsafe { dispatch(&flags, -10) }, 1);
    }

    #[test]
    fn stress_gc_keeps_free_helpers_for_primitive_call_operands() {
        let code = compiled_dispatch();
        // SAFETY: See compiled_dispatch and the ABI explanation above.
        let dispatch: unsafe extern "C" fn(*const u32, i64) -> i32 =
            unsafe { core::mem::transmute(code.as_ptr()) };
        for tag in [qjs::JS_TAG_INT, qjs::JS_TAG_BOOL, qjs::JS_TAG_OBJECT] {
            let flags = qjs::JS_JIT_FRAME_STRESS_GC | qjs::JS_JIT_FUNCTION_STRICT;
            assert_eq!(unsafe { dispatch(&flags, i64::from(tag)) }, 0);
        }
        let flags = qjs::JS_JIT_FUNCTION_STRICT;
        assert_eq!(unsafe { dispatch(&flags, i64::from(qjs::JS_TAG_INT)) }, 1);
    }
}
