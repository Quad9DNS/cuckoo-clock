# cuckoo-clock

[![Crates.io Version](https://img.shields.io/crates/v/cuckoo-clock)](https://crates.io/crates/cuckoo-clock)
[![docs.rs](https://img.shields.io/docsrs/cuckoo-clock)](https://docs.rs/cuckoo-clock)


Cuckoo probabilistic filter with TTL, LRU, and counter features.

Implementation of cuckoo filter per ["Cuckoo Filter: Better Than Bloom" by Bin Fan, Dave Andersen and Michael Kaminsky](https://www.cs.cmu.edu/~dga/papers/cuckoo-conext2014.pdf), with the addition of TTL, LRU and generic counter associated with stored buckets.

## Example usage

```rust
use cuckoo_clock::{CuckooFilter, config::{CuckooConfiguration, CounterConfig, TtlConfig}};

let filter = CuckooFilter::new_random(
    CuckooConfiguration::builder(10_000_000)
        .fingerprint_bits(18.try_into()?)
        .bucket_size(8.try_into()?)
        .with_counter(CounterConfig {
            counter_bits: 4.try_into()?,
            ..Default::default()
        })
        .with_ttl(TtlConfig {
            ttl: 600.try_into()?,
            ttl_bits: 10.try_into()?
        })
        .build()?
);

// In this case, we use `insert_if_not_present` to ensure no duplicates, because we care about
// the counter
// None returned from insertion means no entry was evicted
assert!(filter.insert_if_not_present("example_data").is_none());
assert!(filter.insert_if_not_present("example_data").is_none());

// Insertion must have been successful
assert!(filter.contains("example_data"));

// Counter should be 4 now.
// We have accessed this item 3 times, but `get_associated_data` also counts as an access.
assert_eq!(filter.get_associated_data("example_data").unwrap().get_counter()?, 4);
```

## LICENSE

[MIT](./LICENSE)
