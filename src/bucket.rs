use crate::filter::{CuckooConfiguration, DerivedConfiguration};

#[derive(Hash, Clone, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    data: Box<[u8]>,
}

impl Fingerprint {
    pub(crate) fn new(hash: u32, bytes: usize, mask: u32) -> Self {
        let mut fingerprint = hash & mask;
        if fingerprint == 0 {
            fingerprint = 1;
        }

        Self {
            data: fingerprint.to_ne_bytes()[0..bytes]
                .to_vec()
                .into_boxed_slice(),
        }
    }

    pub(crate) fn new_empty(length: usize) -> Self {
        Self {
            data: vec![0; length].into_boxed_slice(),
        }
    }

    fn is_empty(&self) -> bool {
        self.data.iter().all(|b| *b == 0)
    }
}

impl From<Box<[u8]>> for Fingerprint {
    fn from(value: Box<[u8]>) -> Self {
        Self { data: value }
    }
}

impl From<&[u8]> for Fingerprint {
    fn from(value: &[u8]) -> Self {
        value.to_vec().into_boxed_slice().into()
    }
}

pub(crate) struct Bucket {
    data: Vec<u8>,
}

impl Bucket {
    pub(crate) fn new(configuration: &CuckooConfiguration, derived: &DerivedConfiguration) -> Self {
        Self {
            data: vec![0; configuration.bucket_size * derived.fingerprint_bytes],
        }
    }

    pub(crate) fn insert(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) -> bool {
        for i in 0..configuration.bucket_size {
            let stored: Fingerprint = self.get_fingerprint(i, derived);

            if stored == *fingerprint {
                return true;
            } else if stored.is_empty() {
                self.data[i * derived.fingerprint_bytes..(i + 1) * derived.fingerprint_bytes]
                    .copy_from_slice(&fingerprint.data);
                return true;
            }
        }
        false
    }

    pub(crate) fn kick_random(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) -> Fingerprint {
        let index = rand::random_range(0..configuration.bucket_size);
        let stored: Fingerprint = self.get_fingerprint(index, derived);
        self.data[index * derived.fingerprint_bytes..(index + 1) * derived.fingerprint_bytes]
            .copy_from_slice(&fingerprint.data);
        stored
    }

    pub(crate) fn contains(
        &self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) -> bool {
        for i in 0..configuration.bucket_size {
            let stored: Fingerprint = self.get_fingerprint(i, derived);

            if stored == *fingerprint {
                return true;
            }
        }
        false
    }

    pub(crate) fn remove(
        &mut self,
        fingerprint: &Fingerprint,
        configuration: &CuckooConfiguration,
        derived: &DerivedConfiguration,
    ) -> bool {
        for i in 0..configuration.bucket_size {
            let stored: Fingerprint = self.get_fingerprint(i, derived);

            if stored == *fingerprint {
                self.data[i * derived.fingerprint_bytes..(i + 1) * derived.fingerprint_bytes]
                    .copy_from_slice(&Fingerprint::new_empty(derived.fingerprint_bytes).data);
                return true;
            }
        }
        false
    }

    fn get_fingerprint(&self, index: usize, derived: &DerivedConfiguration) -> Fingerprint {
        self.data[index * derived.fingerprint_bytes..(index + 1) * derived.fingerprint_bytes].into()
    }
}
