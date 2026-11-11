//! Exact element-address arithmetic after the caller proves index bounds.

use cranelift_codegen::ir::{InstBuilder, Type, Value};
use cranelift_frontend::FunctionBuilder;

/// `index` is a nonnegative, bounds-checked I32 element index; `base` points
/// into its live checked allocation. This helper does not perform guards or
/// memory accesses, and the caller retains responsibility for those proofs.
pub(super) fn emit(
    builder: &mut FunctionBuilder<'_>,
    base: Value,
    index: Value,
    stride: i64,
    pointer_type: Type,
) -> Value {
    // Scaling in I32 first would silently wrap at 4 GiB before extension.
    let index = builder.ins().uextend(pointer_type, index);
    let offset = builder.ins().imul_imm(index, stride);
    builder.ins().iadd(base, offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cranelift_codegen::{
        ir::{condcodes::IntCC, types, AbiParam, Function, Signature},
        settings,
    };
    use cranelift_frontend::FunctionBuilderContext;

    fn compiled_address(stride: i64) -> super::super::super::baseline::PublishedBaselineCode {
        let isa = cranelift_native::builder()
            .unwrap()
            .finish(settings::Flags::new(settings::builder()))
            .unwrap();
        let pointer_type = isa.pointer_type();
        let mut signature = Signature::new(isa.default_call_conv());
        signature.params.push(AbiParam::new(pointer_type));
        signature.params.push(AbiParam::new(types::I32));
        signature.params.push(AbiParam::new(types::I32));
        signature.returns.push(AbiParam::new(pointer_type));
        let mut function = Function::with_name_signature(Default::default(), signature);
        let mut context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut function, &mut context);
            let entry = builder.create_block();
            let access = builder.create_block();
            let reject = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            let parameters = builder.block_params(entry).to_vec();
            // Reproduce the caller's contract explicitly: even a huge unsigned
            // length must not turn a negative JS integer into a legal index.
            let nonnegative =
                builder
                    .ins()
                    .icmp_imm(IntCC::SignedGreaterThanOrEqual, parameters[1], 0);
            let below_length =
                builder
                    .ins()
                    .icmp(IntCC::UnsignedLessThan, parameters[1], parameters[2]);
            let admitted = builder.ins().band(nonnegative, below_length);
            builder.ins().brif(admitted, access, &[], reject, &[]);
            builder.switch_to_block(access);
            let address = emit(
                &mut builder,
                parameters[0],
                parameters[1],
                stride,
                pointer_type,
            );
            builder.ins().return_(&[address]);
            builder.switch_to_block(reject);
            let invalid = builder.ins().iconst(pointer_type, -1);
            builder.ins().return_(&[invalid]);
            builder.seal_all_blocks();
            builder.finalize();
        }
        super::super::super::baseline::finalize_optimized_machine(&isa, function, None, false)
            .unwrap()
            .publish()
            .unwrap()
    }

    #[test]
    fn element_offsets_do_not_wrap_at_four_gibibytes() {
        for (stride, cases) in [
            (
                4,
                [
                    (0x3fff_ffff, 0x1_0000_0ffc_u64),
                    (0x4000_0000, 0x1_0000_1000),
                    (0x7fff_ffff, 0x2_0000_0ffc),
                ],
            ),
            (
                8,
                [
                    (0x1fff_ffff, 0x1_0000_0ff8),
                    (0x2000_0000, 0x1_0000_1000),
                    (0x7fff_ffff, 0x4_0000_0ff8),
                ],
            ),
            (
                16,
                [
                    (0x0fff_ffff, 0x1_0000_0ff0),
                    (0x1000_0000, 0x1_0000_1000),
                    (0x7fff_ffff, 0x8_0000_0ff0),
                ],
            ),
        ] {
            let code = compiled_address(stride);
            // SAFETY: The signature exactly matches the generated host ABI;
            // the integer address is returned without dereferencing any memory.
            let run: unsafe extern "C" fn(u64, i32, u32) -> u64 =
                unsafe { core::mem::transmute(code.as_ptr()) };
            for (index, expected) in cases {
                assert_eq!(
                    unsafe { run(0x1000, index, u32::MAX) },
                    expected,
                    "stride={stride}, index={index}"
                );
            }
            assert_eq!(unsafe { run(0x1000, 0, 1) }, 0x1000);
            assert_eq!(unsafe { run(0x1000, 1, 2) }, 0x1000 + stride as u64);
        }
    }

    #[test]
    fn caller_bounds_contract_rejects_negative_and_out_of_range_indices() {
        for stride in [4, 8, 16] {
            let code = compiled_address(stride);
            // SAFETY: This is pure integer arithmetic with the exact ABI above.
            let run: unsafe extern "C" fn(u64, i32, u32) -> u64 =
                unsafe { core::mem::transmute(code.as_ptr()) };
            for (index, length) in [
                (0, 0),
                (1, 1),
                (-1, 4),
                (-2, u32::MAX),
                (i32::MIN, u32::MAX),
            ] {
                assert_eq!(
                    unsafe { run(0x1000, index, length) },
                    u64::MAX,
                    "stride={stride}, index={index}, length={length}"
                );
            }
        }
    }
}
