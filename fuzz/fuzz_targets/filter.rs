#![no_main]

use cuckoo_clock::CuckooFilter;
use libfuzzer_sys::{Corpus, fuzz_target};

use cuckoo_clock_fuzz::{CuckooConf, prep_config};

fuzz_target!(|conf: CuckooConf| -> Corpus {
    let Some(config) = prep_config(&conf) else {
        return Corpus::Reject;
    };

    let filter = CuckooFilter::new_random(config);

    for insert in &conf.inserts {
        filter.insert(insert);
        assert!(filter.contains(insert));
        let data = filter.get_associated_data(insert);
        assert!(data.is_some());
        let data = data.unwrap();
        let _fp = data.get_fingerprint();
        let _ = data.get_stored_ttl_value();
        let _ = data.get_lru_counter();
        let _ = data.get_counter();
    }

    for lookup in &conf.lookups {
        filter.contains(lookup);
    }

    for delete in &conf.deletes {
        filter.remove(delete);
        assert!(!filter.contains(delete));
    }

    Corpus::Keep
});
