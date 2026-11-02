//! Integer-constant canonicalization before Cranelift optimization.
//!
//! Tier 2 lowering defines tag and flag variables with fresh `iconst`
//! instructions in whichever block happens to write them. A loop-carried
//! representation that is the same constant on every edge therefore reaches
//! the loop header through distinct SSA values, and Cranelift's constant-phi
//! pass (which compares value identity) keeps the block parameter alive. The
//! redundant parameters cost registers, spills and tag tests in hot loops.
//!
//! Replacing every `iconst` with one canonical definition per (type, bits) at
//! the start of the entry block makes those edges pass the identical value, so
//! the existing pass removes the parameters. Constants dominate every use from
//! the entry block, the rewrite only introduces aliases, and Cranelift's
//! e-graph rematerializes constants near their uses, so semantics and
//! instruction selection of immediates are unchanged.

use std::collections::HashMap;

use cranelift_codegen::cursor::{Cursor, FuncCursor};
use cranelift_codegen::ir::{Function, Inst, InstBuilder, InstructionData, Opcode, Type};

/// Canonicalizes at most `limit` distinct constants; returns how many
/// redundant `iconst` instructions were replaced. Exceeding the limit leaves
/// the remaining constants untouched (never a correctness issue).
pub(super) fn canonicalize_integer_constants(func: &mut Function, limit: usize) -> usize {
    let Some(entry) = func.layout.entry_block() else {
        return 0;
    };
    let mut constants = Vec::new();
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            if let InstructionData::UnaryImm {
                opcode: Opcode::Iconst,
                imm,
            } = func.dfg.insts[inst]
            {
                let result = func.dfg.first_result(inst);
                constants.push((inst, func.dfg.value_type(result), imm.bits()));
            }
        }
    }
    let mut canonical = HashMap::<(Type, i64), Inst>::new();
    let mut replaced = 0;
    for (inst, ty, bits) in constants {
        let key = (ty, bits);
        let target = match canonical.get(&key) {
            Some(&target) => target,
            None => {
                if canonical.len() == limit {
                    continue;
                }
                let mut cursor = FuncCursor::new(func).at_first_insertion_point(entry);
                let value = cursor.ins().iconst(ty, bits);
                let target = cursor
                    .func
                    .dfg
                    .value_def(value)
                    .inst()
                    .expect("iconst defines an instruction result");
                canonical.insert(key, target);
                target
            }
        };
        func.dfg.replace_with_aliases(inst, target);
        func.layout.remove_inst(inst);
        replaced += 1;
    }
    replaced
}

#[cfg(test)]
mod tests {
    use super::*;
    use cranelift_codegen::ir::{types, AbiParam, Signature};
    use cranelift_codegen::isa::CallConv;
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

    #[test]
    fn identical_loop_edge_constants_become_one_value() {
        let mut signature = Signature::new(CallConv::SystemV);
        signature.params.push(AbiParam::new(types::I64));
        signature.returns.push(AbiParam::new(types::I64));
        let mut func = Function::with_name_signature(Default::default(), signature);
        let mut context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut func, &mut context);
            let entry = builder.create_block();
            let header = builder.create_block();
            let exit = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.append_block_param(header, types::I64);
            builder.switch_to_block(entry);
            let n = builder.block_params(entry)[0];
            let zero = builder.ins().iconst(types::I64, 0);
            builder.ins().jump(header, &[zero]);
            builder.switch_to_block(header);
            let tag = builder.block_params(header)[0];
            let again = builder.ins().iconst(types::I64, 0);
            let done = builder
                .ins()
                .icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, n, 0);
            builder.ins().brif(done, exit, &[], header, &[again]);
            builder.switch_to_block(exit);
            builder.ins().return_(&[tag]);
            builder.seal_all_blocks();
            builder.finalize();
        }
        assert_eq!(canonicalize_integer_constants(&mut func, 16), 2);
        let jumps: Vec<_> = func
            .layout
            .blocks()
            .flat_map(|block| func.layout.block_insts(block).collect::<Vec<_>>())
            .filter_map(|inst| {
                let destinations = func.dfg.insts[inst].branch_destination(&func.dfg.jump_tables);
                destinations
                    .iter()
                    .flat_map(|call| call.args_slice(&func.dfg.value_lists).to_vec())
                    .next()
            })
            .map(|value| func.dfg.resolve_aliases(value))
            .collect();
        assert_eq!(jumps.len(), 2);
        assert_eq!(
            jumps[0], jumps[1],
            "both edges must pass one canonical value"
        );
        // A limit of zero is a no-op.
        assert_eq!(canonicalize_integer_constants(&mut func, 0), 0);
    }
}
