use rquickjs_core::qjs;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperandFormat {
    None,
    NoneInt,
    NoneLocal,
    NoneArgument,
    NoneClosure,
    U8,
    I8,
    Local8,
    Constant8,
    Label8,
    U16,
    I16,
    Label16,
    NPop,
    NPopFixed,
    NPopU16,
    Local,
    Argument,
    Closure,
    U32,
    U32x2,
    I32,
    Constant,
    Label,
    Atom,
    AtomU8,
    AtomU16,
    AtomLabelU8,
    AtomLabelU16,
    LabelU16,
}

impl OperandFormat {
    fn from_name(name: &str) -> Self {
        match name {
            "none" => Self::None,
            "none_int" => Self::NoneInt,
            "none_loc" => Self::NoneLocal,
            "none_arg" => Self::NoneArgument,
            "none_var_ref" => Self::NoneClosure,
            "u8" => Self::U8,
            "i8" => Self::I8,
            "loc8" => Self::Local8,
            "const8" => Self::Constant8,
            "label8" => Self::Label8,
            "u16" => Self::U16,
            "i16" => Self::I16,
            "label16" => Self::Label16,
            "npop" => Self::NPop,
            "npopx" => Self::NPopFixed,
            "npop_u16" => Self::NPopU16,
            "loc" => Self::Local,
            "arg" => Self::Argument,
            "var_ref" => Self::Closure,
            "u32" => Self::U32,
            "u32x2" => Self::U32x2,
            "i32" => Self::I32,
            "const" => Self::Constant,
            "label" => Self::Label,
            "atom" => Self::Atom,
            "atom_u8" => Self::AtomU8,
            "atom_u16" => Self::AtomU16,
            "atom_label_u8" => Self::AtomLabelU8,
            "atom_label_u16" => Self::AtomLabelU16,
            "label_u16" => Self::LabelU16,
            _ => unreachable!("build script emitted unknown QuickJS operand format"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Opcode(&'static qjs::JitGeneratedOpcode);

impl Opcode {
    pub(crate) fn from_byte(value: u8) -> Option<Self> {
        qjs::QJSJIT_GENERATED_OPCODES
            .get(value as usize)
            .filter(|info| info.opcode == value)
            .map(Self)
    }

    pub const fn id(self) -> u8 {
        self.0.opcode
    }

    pub const fn name(self) -> &'static str {
        self.0.name
    }

    pub const fn size(self) -> usize {
        self.0.size as usize
    }

    pub const fn n_pop(self) -> u8 {
        self.0.n_pop
    }

    pub const fn n_push(self) -> u8 {
        self.0.n_push
    }

    pub fn format(self) -> OperandFormat {
        OperandFormat::from_name(self.0.format_name)
    }
}

pub fn linked_opcode_table() -> impl ExactSizeIterator<Item = Opcode> {
    qjs::QJSJIT_GENERATED_OPCODES.iter().map(Opcode)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Instruction {
    pc: u32,
    opcode: Opcode,
    bytes: Vec<u8>,
}

impl Instruction {
    pub const fn pc(&self) -> u32 {
        self.pc
    }

    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    pub fn size(&self) -> usize {
        self.bytes.len()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn operand_u8(&self, offset: usize) -> u8 {
        self.bytes[offset]
    }

    pub(crate) fn operand_u16(&self, offset: usize) -> u16 {
        u16::from_le_bytes([self.bytes[offset], self.bytes[offset + 1]])
    }

    pub(crate) fn operand_i16(&self, offset: usize) -> i16 {
        i16::from_le_bytes([self.bytes[offset], self.bytes[offset + 1]])
    }

    pub(crate) fn operand_u32(&self, offset: usize) -> u32 {
        u32::from_le_bytes([
            self.bytes[offset],
            self.bytes[offset + 1],
            self.bytes[offset + 2],
            self.bytes[offset + 3],
        ])
    }

    pub(crate) fn operand_i32(&self, offset: usize) -> i32 {
        self.operand_u32(offset) as i32
    }

    pub(crate) fn branch_target(&self) -> Option<i64> {
        let operand_pc = i64::from(self.pc) + 1;
        match self.opcode.format() {
            OperandFormat::Label8 => Some(operand_pc + i64::from(self.operand_u8(1) as i8)),
            OperandFormat::Label16 => Some(operand_pc + i64::from(self.operand_i16(1))),
            OperandFormat::Label | OperandFormat::LabelU16 => {
                Some(operand_pc + i64::from(self.operand_i32(1)))
            }
            OperandFormat::AtomLabelU8 | OperandFormat::AtomLabelU16 => {
                Some(operand_pc + 4 + i64::from(self.operand_i32(5)))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    UnknownOpcode { pc: u32, opcode: u8 },
    InvalidOpcode { pc: u32 },
    Truncated { pc: u32, size: usize },
}

pub(crate) enum BoundedDecodeError {
    Decode(DecodeError),
    InstructionLimit { pc: u32 },
}

pub(crate) fn decode_bounded(
    bytes: &[u8],
    max_instructions: usize,
) -> Result<Vec<Instruction>, BoundedDecodeError> {
    let mut result = Vec::new();
    let mut pc = 0usize;
    while pc < bytes.len() {
        if result.len() >= max_instructions {
            return Err(BoundedDecodeError::InstructionLimit { pc: pc as u32 });
        }
        let raw_opcode = bytes[pc];
        let opcode = Opcode::from_byte(raw_opcode).ok_or({
            BoundedDecodeError::Decode(DecodeError::UnknownOpcode {
                pc: pc as u32,
                opcode: raw_opcode,
            })
        })?;
        if opcode.name() == "invalid" {
            return Err(BoundedDecodeError::Decode(DecodeError::InvalidOpcode {
                pc: pc as u32,
            }));
        }
        let size = opcode.size();
        let end = pc.checked_add(size).ok_or({
            BoundedDecodeError::Decode(DecodeError::Truncated {
                pc: pc as u32,
                size,
            })
        })?;
        if end > bytes.len() {
            return Err(BoundedDecodeError::Decode(DecodeError::Truncated {
                pc: pc as u32,
                size,
            }));
        }
        result.push(Instruction {
            pc: pc as u32,
            opcode,
            bytes: bytes[pc..end].to_vec(),
        });
        pc = end;
    }
    Ok(result)
}

pub fn decode_raw(bytes: &[u8]) -> Result<Vec<Instruction>, DecodeError> {
    decode_bounded(bytes, usize::MAX).map_err(|error| match error {
        BoundedDecodeError::Decode(error) => error,
        BoundedDecodeError::InstructionLimit { .. } => unreachable!("unbounded decoder limit"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_metadata_matches_the_linked_c_table() {
        let mut count = 0;
        let mut fingerprint = 0;
        let table = unsafe { qjs::JS_JitGetOpcodeTable(&mut count, &mut fingerprint) };
        assert!(!table.is_null());
        assert_eq!(count as usize, qjs::QJSJIT_GENERATED_OPCODE_COUNT);
        assert_eq!(fingerprint, qjs::QJSJIT_GENERATED_OPCODE_FINGERPRINT);

        let table = unsafe { std::slice::from_raw_parts(table, count as usize) };
        for (generated, linked) in qjs::QJSJIT_GENERATED_OPCODES.iter().zip(table) {
            assert_eq!(linked.opcode, generated.opcode);
            assert_eq!(linked.size, generated.size);
            assert_eq!(linked.n_pop, generated.n_pop);
            assert_eq!(linked.n_push, generated.n_push);
            assert_eq!(linked.format, generated.format);
            let name = unsafe { std::ffi::CStr::from_ptr(linked.name) };
            assert_eq!(name.to_bytes(), generated.name.as_bytes());
        }
    }

    #[test]
    fn arbitrary_byte_sequences_decode_completely_or_return_an_error() {
        for mut state in [0xa341_316c_u32, 0x243f_6a88, 0x9e37_79b9, 0xdead_beef] {
            for len in 0..=512 {
                let mut bytes = vec![0; len];
                for byte in &mut bytes {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    *byte = state as u8;
                }
                let result = std::panic::catch_unwind(|| decode_raw(&bytes));
                assert!(result.is_ok(), "decoder panicked for {bytes:02x?}");
                if let Ok(instructions) = result.unwrap() {
                    assert_eq!(
                        instructions.iter().map(Instruction::size).sum::<usize>(),
                        bytes.len()
                    );
                }
            }
        }

        for first in 0..=u8::MAX {
            for second in 0..=u8::MAX {
                let bytes = [first, second];
                let result = std::panic::catch_unwind(|| decode_raw(&bytes));
                assert!(result.is_ok(), "decoder panicked for {bytes:02x?}");
                if let Ok(instructions) = result.unwrap() {
                    assert_eq!(
                        instructions.iter().map(Instruction::size).sum::<usize>(),
                        bytes.len()
                    );
                }
            }
        }

        for opcode in linked_opcode_table() {
            let mut complete = vec![0; opcode.size()];
            complete[0] = opcode.id();
            assert!(std::panic::catch_unwind(|| decode_raw(&complete)).is_ok());
            for prefix_len in 1..opcode.size() {
                assert!(std::panic::catch_unwind(|| decode_raw(&complete[..prefix_len])).is_ok());
            }
        }
    }
}
