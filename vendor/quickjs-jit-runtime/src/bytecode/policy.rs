//! Closed Tier 1 policy for the authoritative, build-generated opcode set.

include!(concat!(env!("OUT_DIR"), "/tier1-opcodes.rs"));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuditedOpcodePolicy {
    pub id: u8,
    pub name: &'static str,
    pub policy: Tier1Policy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HelperId {
    Dup,
    Free,
    ResolveConst,
    AtomValue,
    ToNumeric,
    ToBool,
    AddSlow,
    BinaryArithSlow,
    UnaryArithSlow,
    CompareSlow,
    GetProperty,
    SetProperty,
    GetElement,
    SetElement,
    ToPropertyKey,
    GetGlobal,
    Call,
    NewArray,
    NewObject,
    CallConstructor,
    Regexp,
    FClosure,
    GetVarRef,
    PutVarRef,
    CloseLocal,
    SetName,
    GenericOp,
    IteratorOp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FallbackReason {
    UnsupportedOpcode,
    DirectEval,
    WithScope,
    Generator,
    Async,
    DynamicImport,
    ExplicitResourceManagement,
    ExceptionRegion,
    ClosureFrame,
    ExtendedFrame,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tier1Policy {
    Native,
    Helper(HelperId),
    Reject(FallbackReason),
}

mod audit {
    use super::*;
    include!("policy_audit.rs");
}

pub fn audited_opcode_policy_table() -> &'static [AuditedOpcodePolicy] {
    &audit::AUDITED_POLICIES
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tier1Rejection {
    pc: u32,
    reason: FallbackReason,
}

impl Tier1Rejection {
    pub(crate) const fn new(pc: u32, reason: FallbackReason) -> Self {
        Self { pc, reason }
    }

    pub const fn pc(self) -> u32 {
        self.pc
    }

    pub const fn reason(self) -> FallbackReason {
        self.reason
    }
}

pub fn tier1_policy(id: u8) -> Option<Tier1Policy> {
    audit::AUDITED_POLICIES
        .get(id as usize)
        .and_then(|entry| (entry.id == id).then_some(entry.policy))
}
