use std::env;

use crate::test_utils::reference_trie_hash_vec;

use super::*;

fn compare_impls(data: &[(Vec<u8>, Vec<u8>)]) {
    let mut trie = DODiffTrie::new_empty();
    for (key, value) in data {
        trie.insert(key, value);
    }
    let got_hash = trie.root_hash();
    if env::var("ETH_SPARSE_MPT_TEST_PRINT").is_ok() {
        trie.print_node(0);
    }
    let expected_hash = reference_trie_hash_vec(data);
    assert_eq!(expected_hash, got_hash);
}

fn compare_with_removals(data: &[(Vec<u8>, Vec<u8>)], remove: &[Vec<u8>]) -> eyre::Result<()> {
    let mut trie = DODiffTrie::new_empty();
    for (key, value) in data {
        trie.insert(key, value);
    }

    if env::var("ETH_SPARSE_MPT_TEST_PRINT").is_ok() {
        println!("Trie before deletes");
        trie.print_node(0);
        println!();
    }
    for key in remove {
        trie.delete(key)?;
    }
    let got_hash = trie.root_hash();
    if env::var("ETH_SPARSE_MPT_TEST_PRINT").is_ok() {
        println!("Trie after deletes");
        trie.print_node(0);
        println!();
    }

    let filtered_data: Vec<_> = data
        .iter()
        .filter(|(key, _)| !remove.contains(key))
        .cloned()
        .collect();

    if env::var("ETH_SPARSE_MPT_TEST_PRINT").is_ok() {
        // for reference trie without any removals
        println!("Trie from filtered data");
        let mut trie = DODiffTrie::new_empty();
        for (key, value) in &filtered_data {
            trie.insert(key, value);
        }
        trie.root_hash();
        trie.print_node(0);
        println!();
    }
    let expected_hash = reference_trie_hash_vec(&filtered_data);
    assert_eq!(expected_hash, got_hash);
    Ok(())
}

#[test]
fn do_empty_trie() {
    compare_impls(&[])
}

#[test]
fn do_one_element_trie() {
    let data = [(vec![1, 1], vec![0xa, 0xa])];
    compare_impls(&data)
}

#[test]
fn do_update_leaf_node() {
    let data = &[(vec![1], vec![2]), (vec![1], vec![3])];
    compare_impls(data);
}

#[test]
fn do_insert_into_leaf_node_no_extension() {
    let data = &[(vec![0x11], vec![0x0a]), (vec![0x22], vec![0x0b])];
    compare_impls(data);

    let data = &[(vec![0x22], vec![0x0b]), (vec![0x11], vec![0x0a])];
    compare_impls(data);
}
#[test]
fn do_insert_into_leaf_node_with_extension() {
    let data = &[
        (vec![0x33, 0x22], vec![0x0a]),
        (vec![0x33, 0x11], vec![0x0b]),
    ];
    compare_impls(data);
}

#[test]
fn do_insert_into_extension_node_no_extension_above() {
    let data = &[
        (vec![0x33, 0x22], vec![0x0a]),
        (vec![0x33, 0x11], vec![0x0b]),
        (vec![0x44, 0x33], vec![0x0c]),
    ];
    compare_impls(data);
}

#[test]
fn do_insert_into_extension_node_with_extension_above() {
    let data = &[
        (vec![0x33, 0x33, 0x22], vec![0x0a]),
        (vec![0x33, 0x33, 0x11], vec![0x0b]),
        (vec![0x33, 0x44, 0x33], vec![0x0c]),
    ];
    compare_impls(data);
}

#[test]
fn do_insert_into_extension_node_collapse_extension() {
    let data = &[
        (vec![0x33, 0x22, 0x44], vec![0x0a]),
        (vec![0x33, 0x11, 0x44], vec![0x0b]),
        (vec![0x34, 0x33, 0x44], vec![0x0c]),
    ];
    compare_impls(data);
}

#[test]
fn do_insert_into_extension_node_collapse_extension_no_ext_above() {
    let data = &[
        (vec![0x31, 0x11], vec![0x0a]),
        (vec![0x32, 0x22], vec![0x0b]),
        (vec![0x11, 0x33], vec![0x0c]),
    ];
    compare_impls(data);
}

#[test]
fn do_insert_into_branch_empty_child() {
    let data = &[
        (vec![0x11], vec![0x0a]),
        (vec![0x22], vec![0x0b]),
        (vec![0x33], vec![0x0c]),
    ];
    compare_impls(data);
}

#[test]
fn do_insert_into_branch_leaf_child() {
    let data = &[
        (vec![0x11], vec![0x0a]),
        (vec![0x22], vec![0x0b]),
        (vec![0x33], vec![0x0c]),
        (vec![0x33], vec![0x0d]),
    ];
    compare_impls(data);
}

#[test]
fn do_remove_empty_trie_err() {
    let add = &[];

    let remove = &[vec![0x12]];

    let _ = compare_with_removals(add, remove).unwrap_err();
}

#[test]
fn do_remove_leaf() {
    let add = &[(vec![0x11], vec![0x0a])];

    let remove = &[vec![0x11]];

    compare_with_removals(add, remove).unwrap();
}

#[test]
fn do_remove_leaf_key_error() {
    let add = &[(vec![0x11], vec![0x0a])];

    let remove = &[vec![0x12]];

    let _ = compare_with_removals(add, remove).unwrap_err();
}

#[test]
fn do_remove_extension_node_error() {
    let add = &[(vec![0x11, 0x1], vec![0x0a]), (vec![0x11, 0x2], vec![0x0b])];

    let remove = &[vec![0x12]];

    let _ = compare_with_removals(add, remove).unwrap_err();
}

#[test]
fn do_remove_branch_err() {
    let add = &[
        (vec![0x01, 0x10], vec![0x0a]),
        (vec![0x01, 0x20], vec![0x0b]),
        (vec![0x01, 0x30], vec![0x0c]),
    ];

    let remove = &[vec![0x01]];

    let _ = compare_with_removals(add, remove).unwrap_err();
}

#[test]
fn do_remove_branch_leave_2_children() {
    let add = &[
        (vec![0x01], vec![0x0a]),
        (vec![0x02], vec![0x0b]),
        (vec![0x03], vec![0x0c]),
    ];

    let remove = &[vec![0x01]];

    compare_with_removals(add, remove).unwrap();
}

#[test]
fn do_remove_branch_leave_1_children_leaf_below_branch_above() {
    let add = &[
        (vec![0x11], vec![0x0a]),
        (vec![0x12], vec![0x0b]),
        (vec![0x23], vec![0x0b]),
        (vec![0x33], vec![0x0c]),
    ];

    let remove = &[vec![0x11]];

    compare_with_removals(add, remove).unwrap();
}

#[test]
fn do_remove_branch_leave_1_children_branch_below_branch_above() {
    let add = &[
        (vec![0x11, 0x00], vec![0x0a]),
        (vec![0x12, 0x10], vec![0x0b]),
        (vec![0x12, 0x20], vec![0x0b]),
        (vec![0x23, 0x00], vec![0x0b]),
        (vec![0x33, 0x00], vec![0x0c]),
    ];

    let remove = &[vec![0x11, 0x00]];

    compare_with_removals(add, remove).unwrap();
}

#[test]
fn do_remove_branch_leave_1_children_ext_below_branch_above() {
    let add = &[
        (vec![0x11, 0x00, 0x00], vec![0x0a]),
        (vec![0x12, 0x10, 0x20], vec![0x0b]),
        (vec![0x12, 0x10, 0x30], vec![0x0b]),
        (vec![0x23, 0x00, 0x00], vec![0x0b]),
        (vec![0x33, 0x00, 0x00], vec![0x0c]),
    ];

    let remove = &[vec![0x11, 0x00, 0x00]];

    compare_with_removals(add, remove).unwrap();
}

#[test]
fn do_remove_branch_leave_1_children_leaf_below_ext_above() {
    let add = &[(vec![0x11], vec![0x0a]), (vec![0x12], vec![0x0b])];

    let remove = &[vec![0x11]];

    compare_with_removals(add, remove).unwrap();
}

#[test]
fn do_remove_branch_leave_1_children_branch_below_ext_above() {
    let add = &[
        (vec![0x11, 0x00], vec![0x0a]),
        (vec![0x12, 0x10], vec![0x0b]),
        (vec![0x12, 0x20], vec![0x0b]),
    ];

    let remove = &[vec![0x11, 0x00]];

    compare_with_removals(add, remove).unwrap();
}

#[test]
fn do_remove_branch_leave_1_children_branch_below_null_above() {
    let add = &[
        (vec![0x10], vec![0xa]),
        (vec![0x23], vec![0xb]),
        (vec![0x24], vec![0xc]),
    ];

    let remove = &[vec![0x10]];

    compare_with_removals(add, remove).unwrap();
}

#[test]
fn do_remove_branch_leave_1_children_ext_below_null_above() {
    let add = &[
        (vec![0x10, 0x00], vec![0xa]),
        (vec![0x23, 0x01], vec![0xb]),
        (vec![0x23, 0x02], vec![0xb]),
    ];

    let remove = &[vec![0x10, 0x00]];

    compare_with_removals(add, remove).unwrap();
}

#[test]
fn do_remove_branch_leave_1_children_leaf_below_null_above() {
    let add = &[(vec![0x10, 0x00], vec![0xa]), (vec![0x23, 0x01], vec![0xb])];

    let remove = &[vec![0x10, 0x00]];

    compare_with_removals(add, remove).unwrap();
}

#[test]
fn do_remove_branch_leave_1_children_ext_below_ext_above() {
    let add = &[
        (vec![0x11, 0x00], vec![0x0a]),
        (vec![0x12, 0x11], vec![0x0b]),
        (vec![0x12, 0x12], vec![0x0b]),
    ];

    let remove = &[vec![0x11, 0x00]];

    compare_with_removals(add, remove).unwrap();
}
