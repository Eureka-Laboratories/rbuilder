use std::ops::Range;

use alloy_primitives::{keccak256, B256};
use alloy_rlp::EMPTY_STRING_CODE;
use arrayvec::ArrayVec;
use reth_trie::Nibbles;

use crate::utils::{encode_branch_node, encode_extension, encode_leaf};

#[derive(Debug, Default)]
pub struct DODiffTrie {
    hashed_nodes: Vec<bool>,
    rlp_ptrs: Vec<ArrayVec<u8, 33>>,
    nodes: Vec<DiffTrieNode>,

    values: Vec<u8>,
    keys: Vec<u8>,
    branch_node_children: Vec<[usize; 16]>, // 0 means child is empty

    // scratchpad
    rlp: Vec<u8>,
    tmp_nibbles: Nibbles,
}

#[derive(Debug, Clone)]
enum DiffTrieNode {
    Leaf {
        key: Range<usize>,
        value: Range<usize>,
    },
    Extension {
        key: Range<usize>,
        next_node: usize,
    },
    Branch {
        children: usize,
    },
    Null,
}

impl DODiffTrie {
    pub fn new_empty() -> Self {
        let mut def = Self::default();
        def.clear_empty();
        def
    }

    pub fn reserve(&mut self, len: usize) {
        self.hashed_nodes.reserve(len);
        self.rlp_ptrs.reserve(len);
        self.nodes.reserve(len);
        self.keys.reserve(len);
        self.branch_node_children.reserve(len);
    }

    pub fn clear_empty(&mut self) {
        self.clear();
        self.push_node(DiffTrieNode::Null);
    }

    fn push_node(&mut self, node: DiffTrieNode) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(node);
        self.hashed_nodes.push(false);
        self.rlp_ptrs.push(Default::default());
        idx
    }

    fn copy_value(&mut self, value: &[u8]) -> Range<usize> {
        let range = self.values.len()..self.values.len() + value.len();
        self.values.extend_from_slice(value);
        range
    }

    fn insert_key(&mut self, key: &[u8]) -> Range<usize> {
        let range = self.keys.len()..self.keys.len() + key.len();
        self.keys.extend_from_slice(key);
        range
    }

    fn copy_or_overwrite_value(&mut self, old_value: Range<usize>, value: &[u8]) -> Range<usize> {
        if old_value.len() >= value.len() {
            let new_range = old_value.start..old_value.start + value.len();
            self.values[old_value].copy_from_slice(value);
            new_range
        } else {
            self.copy_value(value)
        }
    }

    fn create_branch_children(&mut self) -> usize {
        let idx = self.branch_node_children.len();
        self.branch_node_children.push(Default::default());
        idx
    }

    pub fn clear(&mut self) {
        self.hashed_nodes.clear();
        self.rlp_ptrs.clear();
        self.values.clear();
        self.keys.clear();
        self.branch_node_children.clear();
        self.nodes.clear();
    }

    // return prefix (as part of path_left, stripped nibble of suffix 1, suffix 1 as part of path_left, stripped nibbble from suffix2, suffix2 as part of key stored)
    fn extract_prefix_and_suffix<'a>(
        &self,
        path_left: &'a [u8],
        key: Range<usize>,
    ) -> (&'a [u8], u8, &'a [u8], u8, Range<usize>) {
        let p = mismatch(path_left, &self.keys[key.clone()]);
        let prefix = &path_left[..p];
        let n1 = path_left[p];
        let suff1 = &path_left[p + 1..];
        let n2 = self.keys[key.start + p];
        let suff2 = key.start + p + 1..key.end;
        (prefix, n1, suff1, n2, suff2)
    }

    pub fn insert(&mut self, key: &[u8], insert_value: &[u8]) {
        let n = Nibbles::unpack(key);
        let ins_key = n.as_slice();
        let mut current_node = 0;

        let mut path_walked = 0;

        loop {
            self.hashed_nodes[current_node] = false;
            let node = self.nodes.get(current_node).expect("node not found");
            match node {
                DiffTrieNode::Branch { children } => {
                    let children = *children;

                    let n = ins_key[path_walked] as usize;
                    path_walked += 1;
                    if self.branch_node_children[children][n] != 0 {
                        current_node = self.branch_node_children[children][n];
                        continue;
                    } else {
                        let new_leaf_key = self.insert_key(&ins_key[path_walked..]);
                        let leaf_value = self.copy_value(insert_value);
                        let leaf_ptr = self.push_node(DiffTrieNode::Leaf {
                            key: new_leaf_key,
                            value: leaf_value,
                        });
                        self.branch_node_children[children][n] = leaf_ptr;
                    }
                }
                DiffTrieNode::Extension { key, next_node } => {
                    let key = key.clone();
                    let next_node = *next_node;

                    if ins_key[path_walked..].starts_with(&self.keys[key.clone()]) {
                        path_walked += key.len();
                        current_node = next_node;
                        continue;
                    }

                    let (prefix, n1, suff1, n2, suff2) =
                        self.extract_prefix_and_suffix(&ins_key[path_walked..], key);

                    let has_extension_node = !prefix.is_empty();
                    if has_extension_node {
                        let new_ext_key = self.insert_key(prefix);
                        // next node will branch node that we will push below
                        let ext_next_node = self.nodes.len();
                        self.nodes[current_node] = DiffTrieNode::Extension {
                            key: new_ext_key,
                            next_node: ext_next_node,
                        };
                    };
                    let branch_children = self.create_branch_children();
                    let branch_node = DiffTrieNode::Branch {
                        children: branch_children,
                    };
                    if has_extension_node {
                        self.push_node(branch_node);
                    } else {
                        self.nodes[current_node] = branch_node;
                    }

                    let new_leaf_key = self.insert_key(suff1);
                    let new_leaf_value = self.copy_value(insert_value);

                    let new_leaf_ptr = self.push_node(DiffTrieNode::Leaf {
                        key: new_leaf_key,
                        value: new_leaf_value,
                    });

                    let branch_child = if !suff2.is_empty() {
                        self.push_node(DiffTrieNode::Extension {
                            key: suff2,
                            next_node,
                        })
                    } else {
                        next_node
                    };

                    self.branch_node_children[branch_children][n1 as usize] = new_leaf_ptr;
                    self.branch_node_children[branch_children][n2 as usize] = branch_child;
                }
                DiffTrieNode::Leaf { key, value } => {
                    let key = key.clone();
                    let value = value.clone();

                    if self.keys[key.clone()] == ins_key[path_walked..] {
                        // update leaf in place
                        let new_value = self.copy_or_overwrite_value(value, insert_value);
                        self.nodes[current_node] = DiffTrieNode::Leaf {
                            key: key.clone(),
                            value: new_value,
                        };
                        break;
                    }

                    let (prefix, n1, suff1, n2, suff2) =
                        self.extract_prefix_and_suffix(&ins_key[path_walked..], key);

                    let has_extension_node = !prefix.is_empty();
                    if has_extension_node {
                        let new_ext_key = self.insert_key(prefix);
                        // next node will branch node that we will push below
                        let ext_next_node = self.nodes.len();
                        self.nodes[current_node] = DiffTrieNode::Extension {
                            key: new_ext_key,
                            next_node: ext_next_node,
                        };
                    };
                    let branch_children = self.create_branch_children();
                    let branch_node = DiffTrieNode::Branch {
                        children: branch_children,
                    };
                    if has_extension_node {
                        self.push_node(branch_node);
                    } else {
                        self.nodes[current_node] = branch_node;
                    }

                    let first_leaf_key = self.insert_key(suff1);
                    let first_leaf_value = self.copy_value(insert_value);
                    let first_leaf_ptr = self.push_node(DiffTrieNode::Leaf {
                        key: first_leaf_key,
                        value: first_leaf_value,
                    });

                    let second_leaf_key = suff2;
                    let second_leaf_value = value;
                    let second_leaf_ptr = self.push_node(DiffTrieNode::Leaf {
                        key: second_leaf_key,
                        value: second_leaf_value,
                    });

                    self.branch_node_children[branch_children][n1 as usize] = first_leaf_ptr;
                    self.branch_node_children[branch_children][n2 as usize] = second_leaf_ptr;
                }
                DiffTrieNode::Null => {
                    let new_leaf_key = self.insert_key(&ins_key[path_walked..]);
                    let new_leaf_value = self.copy_value(insert_value);
                    self.nodes[current_node] = DiffTrieNode::Leaf {
                        key: new_leaf_key,
                        value: new_leaf_value,
                    };
                }
            }
            break;
        }
    }

    fn rlp_encode_node(&mut self, node_idx: usize) {
        self.rlp.clear();
        let node = self.nodes.get(node_idx).expect("node not found").clone();
        match node {
            DiffTrieNode::Branch { children } => {
                let mut child_rlp_pointers: [Option<&[u8]>; 16] = [None; 16];
                for (idx, child_node_ptr) in self.branch_node_children[children].iter().enumerate()
                {
                    if *child_node_ptr != 0 {
                        debug_assert!(self.hashed_nodes[*child_node_ptr]);
                        child_rlp_pointers[idx] = Some(self.rlp_ptrs[*child_node_ptr].as_slice());
                    }
                }
                encode_branch_node(&child_rlp_pointers, &mut self.rlp);
            }
            DiffTrieNode::Extension { key, next_node } => {
                debug_assert!(self.hashed_nodes[next_node]);
                let vec = self.tmp_nibbles.as_mut_vec_unchecked();
                vec.clear();
                vec.extend_from_slice(&self.keys[key]);
                encode_extension(&self.tmp_nibbles, &self.rlp_ptrs[next_node], &mut self.rlp);
            }
            DiffTrieNode::Leaf { key, value } => {
                let vec = self.tmp_nibbles.as_mut_vec_unchecked();
                vec.clear();
                vec.extend_from_slice(&self.keys[key]);
                encode_leaf(&self.tmp_nibbles, &self.values[value], &mut self.rlp);
            }
            DiffTrieNode::Null => {
                self.rlp.push(EMPTY_STRING_CODE);
            }
        }
    }

    // children must be hashed
    fn calculate_rlp_pointer_node(&mut self, node_idx: usize) {
        self.rlp_encode_node(node_idx);
        if self.rlp.len() < 32 {
            self.rlp_ptrs[node_idx].clear();
            self.rlp_ptrs[node_idx]
                .try_extend_from_slice(&self.rlp)
                .unwrap();
        } else {
            let hash = keccak256(&self.rlp);
            let result = &mut self.rlp_ptrs[node_idx];
            result.clear();
            result.push(EMPTY_STRING_CODE + 32);
            result.try_extend_from_slice(hash.as_slice()).unwrap();
        }
        self.hashed_nodes[node_idx] = true;
    }

    pub fn root_hash(&mut self) -> B256 {
        self.root_hash_node(0);
        self.rlp_encode_node(0);
        keccak256(&self.rlp)
    }

    fn root_hash_node(&mut self, node_idx: usize) {
        if self.hashed_nodes[node_idx] {
            return;
        }
        let node = self.nodes.get(node_idx).expect("node not found");
        match node {
            DiffTrieNode::Branch { children } => {
                for child in self.branch_node_children[*children].into_iter() {
                    if child != 0 {
                        self.root_hash_node(child);
                    }
                }
                self.calculate_rlp_pointer_node(node_idx);
            }
            DiffTrieNode::Extension { next_node, .. } => {
                self.root_hash_node(*next_node);
                self.calculate_rlp_pointer_node(node_idx);
            }
            DiffTrieNode::Null | DiffTrieNode::Leaf { .. } => {
                self.calculate_rlp_pointer_node(node_idx);
            }
        }
    }

    pub fn print_node(&self, node_idx: usize) {
        let node = self.nodes.get(node_idx).expect("node not found").clone();
        let h = alloy_primitives::hex::encode;
        match node {
            DiffTrieNode::Branch { children } => {
                println!("{} Branch", node_idx);
                println!("{}", h(self.rlp_ptrs[node_idx].as_slice()));
                for (idx, child) in self.branch_node_children[children].into_iter().enumerate() {
                    if child != 0 {
                        println!("  {} -> {}", idx, child);
                    }
                }
                for child in self.branch_node_children[children].into_iter() {
                    if child != 0 {
                        self.print_node(child);
                    }
                }
            }
            DiffTrieNode::Extension { next_node, key } => {
                println!(
                    "{} Extension {:?} -> {}",
                    node_idx,
                    h(&self.keys[key]),
                    next_node
                );
                println!("{}", h(self.rlp_ptrs[node_idx].as_slice()));
                self.print_node(next_node);
            }
            DiffTrieNode::Leaf { key, value } => {
                println!(
                    "{} Leaf {:?} : {:?}",
                    node_idx,
                    h(&self.keys[key]),
                    h(&self.values[value])
                );
                println!("{}", h(self.rlp_ptrs[node_idx].as_slice()));
            }
            DiffTrieNode::Null => {
                println!("{} Null", node_idx);
                println!("{}", h(self.rlp_ptrs[node_idx].as_slice()));
            }
        }
    }
}

pub fn mismatch(xs: &[u8], ys: &[u8]) -> usize {
    mismatch_chunks::<8>(xs, ys)
}

fn mismatch_chunks<const N: usize>(xs: &[u8], ys: &[u8]) -> usize {
    let off = std::iter::zip(xs.chunks_exact(N), ys.chunks_exact(N))
        .take_while(|(x, y)| x == y)
        .count()
        * N;
    off + std::iter::zip(&xs[off..], &ys[off..])
        .take_while(|(x, y)| x == y)
        .count()
}
