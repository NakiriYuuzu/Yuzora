//! Baseline operations. Every operation is explicitly tagged with its source PC.

use super::FrameStateId;

/// A raw 16-byte QuickJS value split into its payload and tag words.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaggedValue {
    pub payload: u64,
    pub tag: i64,
}

impl TaggedValue {
    pub const fn new(payload: u64, tag: i64) -> Self {
        Self { payload, tag }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackOp {
    Nip,
    Nip1,
    Dup,
    Dup1,
    Dup2,
    Dup3,
    Insert2,
    Insert3,
    Insert4,
    Perm3,
    Perm4,
    Perm5,
    Swap,
    Swap2,
    Rot3Left,
    Rot3Right,
    Rot4Left,
    Rot5Left,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    IsUndefinedOrNull,
    IsUndefined,
    IsNull,
    Plus,
    Neg,
    Increment,
    Decrement,
    BitNot,
    LogicalNot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
    ShiftRightUnsigned,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Equal,
    NotEqual,
    StrictEqual,
    StrictNotEqual,
}

/// TDZ checking mode for closure-variable stores. Values match the native
/// `JSJitVarRefMode` ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VarRefMode {
    Plain,
    Check,
    CheckInit,
}

/// Opcodes executed with exact interpreter semantics by the `GENERIC_OP`
/// helper (or `BINARY_ARITH_SLOW` for `pow`). Atoms and 8-bit immediates are
/// the verified bytecode operands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenericOp {
    /// `push_this`: pushes the (possibly boxed) receiver.
    PushThis,
    /// `special_object`: pushes the frame object selected by the immediate.
    SpecialObject(u8),
    /// `get_var_undef`: pushes a global or `undefined` when it is missing.
    GetVarUndef(u32),
    /// `delete_var`: pushes the boolean result of deleting a global binding.
    DeleteVar(u32),
    /// `put_var`: pops a value into a global binding.
    PutVar(u32),
    /// `typeof`: replaces the top value with its type string.
    TypeOf,
    /// `typeof_is_undefined`: replaces the top value with a boolean.
    TypeOfIsUndefined,
    /// `typeof_is_function`: replaces the top value with a boolean.
    TypeOfIsFunction,
    /// `to_object`: converts the top value to an object in place.
    ToObject,
    /// `to_propkey2`: checks that `sp[-2]` is object-coercible and converts
    /// the key at `sp[-1]` to a property key in place.
    ToPropertyKey2,
    /// `in`: `key in object`, producing a boolean.
    In,
    /// `instanceof`: `value instanceof constructor`, producing a boolean.
    InstanceOf,
    /// `delete`: `delete object[key]`, producing a boolean.
    Delete,
    /// `pow`: exponentiation through the exact arithmetic slow path.
    Pow,
}

impl GenericOp {
    /// Values consumed from the operand stack and values pushed back.
    pub const fn stack_effect(self) -> (usize, usize) {
        match self {
            Self::PushThis | Self::SpecialObject(_) | Self::GetVarUndef(_) | Self::DeleteVar(_) => {
                (0, 1)
            }
            Self::PutVar(_) => (1, 0),
            Self::TypeOf | Self::TypeOfIsUndefined | Self::TypeOfIsFunction | Self::ToObject => {
                (1, 1)
            }
            Self::ToPropertyKey2 => (2, 2),
            Self::In | Self::InstanceOf | Self::Delete | Self::Pow => (2, 1),
        }
    }

    /// Whether the single result is always a JS boolean.
    pub const fn produces_boolean(self) -> bool {
        matches!(
            self,
            Self::TypeOfIsUndefined
                | Self::TypeOfIsFunction
                | Self::In
                | Self::InstanceOf
                | Self::Delete
                | Self::DeleteVar(_)
        )
    }
}

/// Synchronous iteration opcodes lowered through the exact interpreter
/// stack-effect helper (`ITERATOR_OP`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IteratorOp {
    /// `obj -> iter next catch_offset`
    ForOfStart,
    /// `iter next catch_offset [objs] -> ... value done`; the operand counts
    /// the extra objects between the catch offset and the stack top.
    ForOfNext(u8),
    /// `obj -> enum_obj`
    ForInStart,
    /// `enum_obj -> enum_obj value done`
    ForInNext,
    /// `iter next catch_offset ->`
    Close,
}

impl IteratorOp {
    /// Values removed below the stack top (the interpreter reads, and may
    /// rewrite, this window in place).
    pub const fn window_below(self) -> usize {
        match self {
            Self::ForOfStart | Self::ForInStart | Self::ForInNext => 1,
            Self::ForOfNext(extra) => 3 + extra as usize,
            Self::Close => 3,
        }
    }

    /// Net stack-depth change.
    pub const fn net_push(self) -> isize {
        match self {
            Self::ForOfStart | Self::ForOfNext(_) | Self::ForInNext => 2,
            Self::ForInStart => 0,
            Self::Close => -3,
        }
    }
}

/// Exception regions: `Catch` pushes the catch offset that resumes the
/// handler at its PC, `NipCatch` is the direct `nip_catch` form, `Throw` and
/// `ThrowError` raise, `Gosub` pushes the Int32 return offset and enters a
/// finally block and `Ret` resumes at one of its static return points.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IrOp {
    Poll {
        state: FrameStateId,
        kind: PollKind,
    },
    OsrLabel {
        state: FrameStateId,
    },
    Nop,
    Push(TaggedValue),
    ResolveConstant(u32),
    ResolveAtom(u32),
    GetGlobal(u32),
    NewObject,
    NewArrayFrom(u16),
    GetProperty(u32),
    GetPropertyKeep(u32),
    SetProperty(u32),
    DefineProperty(u32),
    GetElement,
    SetElement,
    DefineElement,
    ToPropertyKey,
    Call {
        argc: u16,
        has_this: bool,
    },
    CallConstructor(u16),
    Regexp,
    /// `fclosure`/`fclosure8`: instantiate the constant-pool bytecode as a
    /// closure over the current frame's arguments, locals and var refs.
    FClosure(u32),
    /// `get_var_ref*`: load a closure variable, optionally with the TDZ check.
    GetVarRef {
        index: u16,
        checked: bool,
    },
    /// `put_var_ref*`/`set_var_ref*`: store a closure variable. `keep` leaves
    /// the stored value on the operand stack (`set_var_ref*`).
    PutVarRef {
        index: u16,
        mode: VarRefMode,
        keep: bool,
    },
    /// `close_loc`: detach the var ref capturing a lexical local.
    CloseLocal(u16),
    /// `set_name`: define the anonymous function/class name of the stack top.
    SetName(u32),
    Generic(GenericOp),
    Iterator(IteratorOp),
    GetArgument(u16),
    GetLocal(u16),
    GetLocalChecked(u16),
    GetLocalPair,
    PutArgument {
        index: u16,
        keep: bool,
    },
    PutLocal {
        index: u16,
        keep: bool,
    },
    PutLocalChecked {
        index: u16,
        initialize: bool,
    },
    SetLocalUninitialized(u16),
    Drop,
    Stack(StackOp),
    Unary(UnaryOp),
    PostUnary(UnaryOp),
    LocalUnary {
        index: u16,
        op: UnaryOp,
    },
    AddLocal(u16),
    Binary(BinaryOp),
    Jump(u32),
    Branch {
        target: u32,
        when_true: bool,
    },
    Return,
    ReturnUndefined,
    Catch(u32),
    NipCatch,
    Throw,
    ThrowError {
        atom: u32,
        kind: u8,
    },
    Gosub {
        target: u32,
        return_pc: u32,
    },
    Ret {
        targets: Box<[u32]>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PollKind {
    Entry,
    Periodic,
    LoopHeader,
    Return,
    Edge,
}
