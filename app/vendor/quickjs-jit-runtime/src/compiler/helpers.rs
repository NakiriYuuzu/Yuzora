use rquickjs_core::qjs;

use cranelift_codegen::{
    ir::{types, AbiParam, Signature},
    isa::TargetIsa,
};

use super::CompileFailure;

#[derive(Clone, Copy, Debug)]
pub(super) struct FrameLayout {
    pub flags: i32,
    pub ctx: i32,
    pub arg_buf: i32,
    pub var_buf: i32,
    pub stack_base: i32,
    pub stack_top: i32,
    pub bytecode_start: i32,
    pub pc: i32,
    pub result: i32,
    pub runtime_api: i32,
    pub helper_offsets: [i32; qjs::QJSJIT_GENERATED_HELPER_COUNT],
    pub value_tag: i32,
}

impl FrameLayout {
    pub fn validated(pointer_bytes: u8) -> Result<Self, CompileFailure> {
        if pointer_bytes != 8
            || core::mem::size_of::<qjs::JSValue>() != 16
            || core::mem::align_of::<qjs::JSValue>() != 8
            || core::mem::size_of::<qjs::JSJitExit>() != 24
        {
            return Err(CompileFailure::InvalidArtifact);
        }
        let offset =
            |value: usize| i32::try_from(value).map_err(|_| CompileFailure::InvalidArtifact);
        let mut helper_offsets = [0; qjs::QJSJIT_GENERATED_HELPER_COUNT];
        for (destination, generated) in helper_offsets
            .iter_mut()
            .zip(qjs::qjsjit_generated_helper_offsets())
        {
            *destination = offset(generated)?;
        }
        Ok(Self {
            flags: offset(core::mem::offset_of!(qjs::JSJitExecFrame, flags))?,
            ctx: offset(core::mem::offset_of!(qjs::JSJitExecFrame, ctx))?,
            arg_buf: offset(core::mem::offset_of!(qjs::JSJitExecFrame, arg_buf))?,
            var_buf: offset(core::mem::offset_of!(qjs::JSJitExecFrame, var_buf))?,
            stack_base: offset(core::mem::offset_of!(qjs::JSJitExecFrame, stack_base))?,
            stack_top: offset(core::mem::offset_of!(qjs::JSJitExecFrame, stack_top))?,
            bytecode_start: offset(core::mem::offset_of!(qjs::JSJitExecFrame, bytecode_start))?,
            pc: offset(core::mem::offset_of!(qjs::JSJitExecFrame, pc))?,
            result: offset(core::mem::offset_of!(qjs::JSJitExecFrame, result))?,
            runtime_api: offset(core::mem::offset_of!(qjs::JSJitExecFrame, runtime_api))?,
            helper_offsets,
            value_tag: offset(core::mem::offset_of!(qjs::JSValue, tag))?,
        })
    }
}

pub(super) fn generated_signatures(isa: &dyn TargetIsa) -> Result<Vec<Signature>, CompileFailure> {
    let pointer_type = isa.pointer_type();
    qjs::QJSJIT_GENERATED_HELPERS
        .iter()
        .enumerate()
        .map(|(index, helper)| {
            if usize::from(helper.id) != index
                || helper.abi_types.len() < 2
                || helper.abi_types[0] != qjs::JSJitHelperABIType_JS_JIT_HELPER_ABI_STATUS as u8
                || helper.abi_types[1] != qjs::JSJitHelperABIType_JS_JIT_HELPER_ABI_FRAME as u8
            {
                return Err(CompileFailure::InvalidArtifact);
            }
            let mut signature = Signature::new(isa.default_call_conv());
            for abi_type in &helper.abi_types[1..] {
                let ty = match u32::from(*abi_type) {
                    qjs::JSJitHelperABIType_JS_JIT_HELPER_ABI_FRAME => pointer_type,
                    qjs::JSJitHelperABIType_JS_JIT_HELPER_ABI_U32 => types::I32,
                    _ => return Err(CompileFailure::InvalidArtifact),
                };
                signature.params.push(AbiParam::new(ty));
            }
            signature.returns.push(AbiParam::new(types::I32));
            Ok(signature)
        })
        .collect()
}

/// JavaScript truthiness for an IEEE-754 binary64 payload. AArch64 lowering in
/// Cranelift 0.116 does not implement OrderedNotEqual; integer comparisons also
/// handle both signed zeros and every NaN payload without an FP comparison.
pub(super) fn emit_f64_bits_truthy(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    payload: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    use cranelift_codegen::ir::{condcodes::IntCC, InstBuilder};
    let magnitude = builder.ins().band_imm(payload, i64::MAX);
    let nonzero = builder.ins().icmp_imm(IntCC::NotEqual, magnitude, 0);
    let not_nan = builder.ins().icmp_imm(
        IntCC::UnsignedLessThanOrEqual,
        magnitude,
        0x7ff0_0000_0000_0000_i64,
    );
    builder.ins().band(nonzero, not_nan)
}
