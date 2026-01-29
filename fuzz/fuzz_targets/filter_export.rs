#![no_main]

use std::collections::VecDeque;

use cuckoo_clock::CuckooFilter;
use cuckoo_clock_fuzz::{CuckooConf, prep_config};
use libfuzzer_sys::{Corpus, fuzz_target};

fuzz_target!(|conf: CuckooConf| -> Corpus {
    let Some(config) = prep_config(&conf) else {
        return Corpus::Reject;
    };

    let filter = CuckooFilter::new_random_exportable(config);

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

    let buf = filter.exporter().snapshot().unwrap();

    let mut buf = VecDeque::from(buf);
    let imported_filter = CuckooFilter::import_random_exportable(&mut buf).unwrap();

    assert_eq!(
        imported_filter.get_configuration(),
        filter.get_configuration()
    );

    for insert in &conf.inserts {
        assert!(imported_filter.contains(insert));
        let data = imported_filter.get_associated_data(insert);
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
