//! Logical frame states attached to polls and exits.

/// Dense identifier into a [`FrameStateTable`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FrameStateId(pub(crate) u32);

impl FrameStateId {
    pub(crate) fn from_index(index: usize) -> Option<Self> {
        let id = u32::try_from(index).ok()?;
        (id != u32::MAX).then_some(Self(id))
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A location whose current tagged value is live at a safe point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameSlot {
    Argument(u16),
    Local(u16),
    Stack(u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameStateKind {
    Poll,
    Helper,
    Marker,
}

/// The bytecode position and complete live logical frame at one safe point.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameState {
    pub pc: u32,
    pub slots: Box<[FrameSlot]>,
    pub(crate) kind: FrameStateKind,
}

/// Interned frame states owned by a baseline IR function.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrameStateTable {
    states: Vec<FrameState>,
}

impl FrameStateTable {
    pub(crate) fn push(&mut self, state: FrameState) -> FrameStateId {
        let id = FrameStateId(self.states.len() as u32);
        self.states.push(state);
        id
    }

    pub fn get(&self, id: FrameStateId) -> &FrameState {
        &self.states[id.index()]
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &FrameState> {
        self.states.iter()
    }
}
