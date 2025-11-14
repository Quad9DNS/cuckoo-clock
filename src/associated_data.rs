use crate::{
    data_block::{DataBlock, ReadOnlyDataBlock},
    filter::DerivedConfiguration,
};

pub struct AssociatedData {
    data: Box<[u8]>,
    derived: DerivedConfiguration,
}

impl AssociatedData {
    pub(crate) fn new(data: DataBlock<'_>, derived: DerivedConfiguration) -> Self {
        Self {
            data: data.inner().into(),
            derived,
        }
    }

    pub fn get_fingerprint(&self) -> u32 {
        ReadOnlyDataBlock::from(&self.data[..])
            .get_fingerprint(&self.derived)
            .data()
    }

    pub fn get_lru_counter(&self) -> u8 {
        ReadOnlyDataBlock::from(&self.data[..]).get_lru_counter(&self.derived)
    }

    pub fn get_counter(&self) -> u32 {
        ReadOnlyDataBlock::from(&self.data[..]).get_counter(&self.derived)
    }

    pub fn get_ttl(&self) -> u32 {
        ReadOnlyDataBlock::from(&self.data[..]).get_ttl(&self.derived)
    }
}
