use alloy_primitives::{keccak256, Bytes, B256, U256};
use criterion::{criterion_group, criterion_main, Criterion};
use eth_sparse_mpt::{data_oriented::DODiffTrie, sparse_mpt::DiffTrie};
use reth_trie::Nibbles;

fn prepare_key_value_data(n: usize) -> (Vec<Bytes>, Vec<Bytes>) {
    let mut keys = Vec::with_capacity(n);
    let mut values = Vec::with_capacity(n);
    for i in 0u64..(n as u64) {
        let b: B256 = U256::from(i).into();
        let data = keccak256(b).to_vec();
        let value = keccak256(&data).to_vec();
        keys.push(Bytes::copy_from_slice(data.as_slice()));
        values.push(Bytes::copy_from_slice(value.as_slice()));
    }
    (keys, values)
}

fn insert_nodes(c: &mut Criterion) {
    let (keys, values) = prepare_key_value_data(10000);

    let mut trie = DODiffTrie::new_empty();
    trie.reserve(15_000);
    // let nibble_keys = keys.iter().map(|k| Nibbles::unpack(k)).collect::<Vec<_>>();
    c.bench_function("insert_nodes_do_trie", |b| {
        b.iter(|| {
            trie.clear_empty();
            for (key, value) in keys.iter().zip(values.iter()) {
                trie.insert(key, value);
            }
        })
    });

    let mut trie = DiffTrie::new_empty();
    c.bench_function("insert_nodes_basic_trie", |b| {
        b.iter(|| {
            trie.clear_empty();
            for (key, value) in keys.iter().zip(values.iter()) {
                trie.insert(key.clone(), value.clone()).unwrap();
            }
        })
    });
}

criterion_group!(benches, insert_nodes);
criterion_main!(benches);
