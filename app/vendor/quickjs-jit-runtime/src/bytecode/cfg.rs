use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use super::{Instruction, Resource, VerifyError, VerifyErrorKind};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BasicBlock {
    start_pc: u32,
    end_pc: u32,
    instruction_range: Range<usize>,
    successors: Vec<u32>,
}

impl BasicBlock {
    pub const fn start_pc(&self) -> u32 {
        self.start_pc
    }

    pub const fn end_pc(&self) -> u32 {
        self.end_pc
    }

    pub fn instruction_range(&self) -> Range<usize> {
        self.instruction_range.clone()
    }

    pub fn successors(&self) -> &[u32] {
        &self.successors
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlFlowGraph {
    blocks: Vec<BasicBlock>,
    by_pc: BTreeMap<u32, usize>,
    loop_headers: BTreeSet<u32>,
}

impl ControlFlowGraph {
    pub fn blocks(&self) -> &[BasicBlock] {
        &self.blocks
    }

    pub fn block(&self, pc: u32) -> Option<&BasicBlock> {
        self.by_pc.get(&pc).map(|index| &self.blocks[*index])
    }

    pub fn is_loop_header(&self, pc: u32) -> bool {
        self.loop_headers.contains(&pc)
    }
}

fn is_unconditional(instruction: &Instruction) -> bool {
    // `gosub` transfers to its finally block; control returns to the next
    // instruction only through that block's `ret`, never by fallthrough.
    matches!(
        instruction.opcode().name(),
        "goto" | "goto8" | "goto16" | "gosub"
    )
}

/// `ret` has no static target. Its successors are the return points of the
/// `gosub` sites that enter the finally block containing it.
fn is_subroutine_return(instruction: &Instruction) -> bool {
    instruction.opcode().name() == "ret"
}

fn is_terminal(instruction: &Instruction) -> bool {
    matches!(
        instruction.opcode().name(),
        "return"
            | "return_undef"
            | "return_async"
            | "throw"
            | "throw_error"
            | "tail_call"
            | "tail_call_method"
    )
}

pub(crate) fn has_valid_exit(instruction: &Instruction) -> bool {
    is_terminal(instruction)
        || is_subroutine_return(instruction)
        || (instruction.branch_target().is_some() && is_unconditional(instruction))
}

pub(crate) fn build(
    instructions: &[Instruction],
    byte_len: usize,
    max_blocks: usize,
) -> Result<ControlFlowGraph, VerifyError> {
    if instructions.is_empty() {
        return Err(VerifyError::new(0, VerifyErrorKind::EmptyBytecode));
    }
    let instruction_pcs: BTreeSet<u32> = instructions.iter().map(Instruction::pc).collect();
    let mut boundaries = BTreeSet::from([0_u32]);
    let mut loop_headers = BTreeSet::new();

    for instruction in instructions {
        let next_pc = instruction.pc() as usize + instruction.size();
        if let Some(target) = instruction.branch_target() {
            if target < 0 || target as usize >= byte_len {
                return Err(VerifyError::new(
                    instruction.pc(),
                    VerifyErrorKind::BranchTargetOutOfRange { target },
                ));
            }
            let target = target as u32;
            if !instruction_pcs.contains(&target) {
                return Err(VerifyError::new(
                    instruction.pc(),
                    VerifyErrorKind::BranchTargetInsideInstruction { target },
                ));
            }
            boundaries.insert(target);
            if target <= instruction.pc() {
                loop_headers.insert(target);
            }
            if next_pc < byte_len {
                boundaries.insert(next_pc as u32);
            }
        } else if (is_terminal(instruction) || is_subroutine_return(instruction))
            && next_pc < byte_len
        {
            boundaries.insert(next_pc as u32);
        }
    }

    if boundaries.len() > max_blocks {
        return Err(VerifyError::new(
            0,
            VerifyErrorKind::ResourceLimit {
                resource: Resource::BasicBlocks,
            },
        ));
    }

    let starts: Vec<u32> = boundaries.into_iter().collect();
    let index_by_pc: BTreeMap<u32, usize> = instructions
        .iter()
        .enumerate()
        .map(|(index, instruction)| (instruction.pc(), index))
        .collect();
    let mut blocks = Vec::with_capacity(starts.len());
    for (position, start) in starts.iter().copied().enumerate() {
        let end = starts.get(position + 1).copied().unwrap_or(byte_len as u32);
        let first = index_by_pc[&start];
        let last = instructions[first..]
            .iter()
            .position(|instruction| instruction.pc() >= end)
            .map_or(instructions.len(), |offset| first + offset);
        let tail = &instructions[last - 1];
        let next = (tail.pc() as usize + tail.size() < byte_len)
            .then_some((tail.pc() as usize + tail.size()) as u32);
        let mut successors = Vec::new();
        if let Some(target) = tail.branch_target() {
            successors.push(target as u32);
            if !is_unconditional(tail) {
                if let Some(next) = next {
                    successors.push(next);
                }
            }
        } else if !is_terminal(tail) && !is_subroutine_return(tail) {
            if let Some(next) = next {
                successors.push(next);
            }
        }
        successors.sort_unstable();
        successors.dedup();
        blocks.push(BasicBlock {
            start_pc: start,
            end_pc: end,
            instruction_range: first..last,
            successors,
        });
    }
    let by_pc: BTreeMap<u32, usize> = blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.start_pc, index))
        .collect();
    link_subroutine_returns(instructions, &mut blocks, &by_pc)?;
    Ok(ControlFlowGraph {
        blocks,
        by_pc,
        loop_headers,
    })
}

/// Resolves every `ret` to the return points of the finally block that owns
/// it. The owner is found by a walk from each `gosub` target in which nested
/// `gosub` sites step over their finally block to their own return point, so
/// a `ret` is attributed only to the finally block that reaches it directly.
/// A `ret` reachable from two different finally entries has no exact static
/// return set and is rejected.
fn link_subroutine_returns(
    instructions: &[Instruction],
    blocks: &mut [BasicBlock],
    by_pc: &BTreeMap<u32, usize>,
) -> Result<(), VerifyError> {
    let unsupported = |pc| VerifyError::new(pc, VerifyErrorKind::UnsupportedExceptionRegion);
    let return_point = |instruction: &Instruction| {
        (instruction.pc() as usize)
            .checked_add(instruction.size())
            .and_then(|pc| u32::try_from(pc).ok())
            .filter(|pc| by_pc.contains_key(pc))
            .ok_or_else(|| unsupported(instruction.pc()))
    };
    let mut return_points: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for block in blocks.iter() {
        let tail = &instructions[block.instruction_range.end - 1];
        if tail.opcode().name() == "gosub" {
            let target = *block
                .successors
                .first()
                .ok_or_else(|| unsupported(tail.pc()))?;
            return_points
                .entry(target)
                .or_default()
                .insert(return_point(tail)?);
        }
    }
    let mut owners: BTreeMap<usize, u32> = BTreeMap::new();
    for &entry in return_points.keys() {
        let mut visited = BTreeSet::new();
        let mut pending = vec![entry];
        while let Some(pc) = pending.pop() {
            if !visited.insert(pc) {
                continue;
            }
            let index = *by_pc.get(&pc).ok_or_else(|| unsupported(pc))?;
            let block = &blocks[index];
            let tail = &instructions[block.instruction_range.end - 1];
            match tail.opcode().name() {
                "ret" => {
                    if owners
                        .insert(index, entry)
                        .is_some_and(|owner| owner != entry)
                    {
                        return Err(unsupported(tail.pc()));
                    }
                }
                "gosub" => pending.push(return_point(tail)?),
                _ => pending.extend(block.successors.iter().copied()),
            }
        }
    }
    for (index, owner) in owners {
        blocks[index].successors = return_points[&owner].iter().copied().collect();
    }
    Ok(())
}
