use std::ops::Range;

use alloy_primitives::{keccak256, B256};
use alloy_rlp::Decodable;
use alloy_rlp::EMPTY_STRING_CODE;
use alloy_trie::nodes::{
    BranchNode as AlloyBranchNode, ExtensionNode as AlloyExtensionNode, LeafNode as AlloyLeafNode,
    TrieNode as AlloyTrieNode,
};
use arrayvec::ArrayVec;
use reth_trie::Nibbles;

#[cfg(test)]
mod tests;

use crate::utils::{encode_branch_node, encode_extension, encode_leaf};

#[derive(Debug, Clone, Copy)]
enum NodePtr {
    Local(usize),
    Remote(usize),
}

impl NodePtr {
    fn need_known(&self) -> Result<usize, NodeNotFound> {
        match self {
            NodePtr::Local(idx) => Ok(*idx),
            NodePtr::Remote(_) => Err(NodeNotFound),
        }
    }
}

#[derive(Debug, Default)]
pub struct DODiffTrie {
    // 3 arrays belowe are of the same length
    hashed_nodes: Vec<bool>,
    rlp_ptrs_local: Vec<ArrayVec<u8, 33>>,
    nodes: Vec<DiffTrieNode>,

    // nodes that we don't know but we know their hash
    rlp_ptrs_remote: Vec<ArrayVec<u8, 33>>,

    values: Vec<u8>,
    keys: Vec<u8>,
    branch_node_children: Vec<[Option<NodePtr>; 16]>, // 0 means child is empty

    // scratchpad
    rlp: Vec<u8>,
    tmp_nibbles: Nibbles,
    walk_path: Vec<(usize, u8)>,
}

#[derive(Debug, Clone)]
enum DiffTrieNode {
    Leaf {
        key: Range<usize>,
        value: Range<usize>,
    },
    Extension {
        key: Range<usize>,
        next_node: NodePtr,
    },
    Branch {
        children: usize,
    },
    Null,
}

#[derive(Debug, thiserror::Error)]
pub enum DeletionError {
    #[error("Deletion error: {0:?}")]
    NodeNotFound(#[from] NodeNotFound),
    #[error("Key node not found in the trie")]
    KeyNotFound,
}

#[derive(Debug, thiserror::Error)]
#[error("Node not found")]
pub struct NodeNotFound;

impl DODiffTrie {
    pub fn new_empty() -> Self {
        let mut def = Self::default();
        def.clear_empty();
        def
    }

    pub fn reserve(&mut self, len: usize) {
        self.hashed_nodes.reserve(len);
        self.rlp_ptrs_local.reserve(len);
        self.nodes.reserve(len);
        self.keys.reserve(len);
        self.branch_node_children.reserve(len);
    }

    pub fn clear_empty(&mut self) {
        self.clear();
        self.push_node(DiffTrieNode::Null);
    }

    fn push_node(&mut self, node: DiffTrieNode) -> NodePtr {
        let idx = self.nodes.len();
        self.nodes.push(node);
        self.hashed_nodes.push(false);
        self.rlp_ptrs_local.push(Default::default());
        NodePtr::Local(idx)
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

    fn insert_remote_node_rlp(&mut self, rlp: &ArrayVec<u8, 33>) -> NodePtr {
        let idx = self.rlp_ptrs_remote.len();
        self.rlp_ptrs_remote.push(rlp.clone());
        NodePtr::Remote(idx)
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
        self.rlp_ptrs_local.clear();
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

    pub fn insert(&mut self, key: &[u8], insert_value: &[u8]) -> Result<(), NodeNotFound> {
        let n = Nibbles::unpack(key);
        let ins_key = n.as_slice();

        let mut current_node = 0;
        let mut path_walked = 0;

        loop {
            self.hashed_nodes[current_node] = false;
            let node = self.nodes.get(current_node).ok_or(NodeNotFound)?;
            match node {
                DiffTrieNode::Branch { children } => {
                    let children = *children;

                    let n = ins_key[path_walked] as usize;
                    path_walked += 1;
                    if let Some(child_ptr) = self.branch_node_children[children][n] {
                        current_node = child_ptr.need_known()?;
                        continue;
                    } else {
                        let new_leaf_key = self.insert_key(&ins_key[path_walked..]);
                        let leaf_value = self.copy_value(insert_value);
                        let leaf_ptr = self.push_node(DiffTrieNode::Leaf {
                            key: new_leaf_key,
                            value: leaf_value,
                        });
                        self.branch_node_children[children][n] = Some(leaf_ptr);
                    }
                }
                DiffTrieNode::Extension { key, next_node } => {
                    let key = key.clone();
                    let next_node = *next_node;

                    if ins_key[path_walked..].starts_with(&self.keys[key.clone()]) {
                        path_walked += key.len();
                        current_node = next_node.need_known()?;
                        continue;
                    }

                    let (prefix, n1, suff1, n2, suff2) =
                        self.extract_prefix_and_suffix(&ins_key[path_walked..], key);

                    let has_extension_node = !prefix.is_empty();
                    if has_extension_node {
                        let new_ext_key = self.insert_key(prefix);
                        // next node will branch node that we will push below
                        let ext_next_node = NodePtr::Local(self.nodes.len());
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

                    self.branch_node_children[branch_children][n1 as usize] = Some(new_leaf_ptr);
                    self.branch_node_children[branch_children][n2 as usize] = Some(branch_child);
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
                        let ext_next_node = NodePtr::Local(self.nodes.len());
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

                    self.branch_node_children[branch_children][n1 as usize] = Some(first_leaf_ptr);
                    self.branch_node_children[branch_children][n2 as usize] = Some(second_leaf_ptr);
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
        Ok(())
    }

    fn merge_keys(&mut self, key1: Range<usize>, nibble: u8, key2: Range<usize>) -> Range<usize> {
        let new_start = self.keys.len();
        let new_len = self.keys.len() + key1.len() + key2.len() + 1;
        self.keys.resize(new_len, 0);
        self.keys.copy_within(key1.clone(), new_start);
        self.keys[new_start + key1.len()] = nibble;
        self.keys.copy_within(key2, new_start + key1.len() + 1);
        new_start..new_len
    }

    // if it returned NodeNotFound, this means that trie is in unsable state
    pub fn delete(&mut self, key: &[u8]) -> Result<(), DeletionError> {
        let n = Nibbles::unpack(key);
        let del_key = n.as_slice();

        let mut current_node = 0;
        let mut path_walked = 0;

        self.walk_path.clear();

        loop {
            self.hashed_nodes[current_node] = false;
            let node = self.nodes.get(current_node).expect("node not found");
            match node {
                DiffTrieNode::Branch { children } => {
                    // deleting from branch, key not found
                    if del_key.len() == path_walked {
                        return Err(DeletionError::KeyNotFound);
                    }

                    let children = *children;

                    let n = del_key[path_walked];
                    path_walked += 1;
                    self.walk_path.push((current_node, n));

                    if let Some(child_ptr) = self.branch_node_children[children][n as usize] {
                        current_node = child_ptr.need_known()?;
                        continue;
                    } else {
                        return Err(DeletionError::KeyNotFound);
                    }
                }
                DiffTrieNode::Extension { key, next_node } => {
                    let key = key.clone();
                    let next_node = *next_node;

                    if del_key[path_walked..].starts_with(&self.keys[key.clone()]) {
                        self.walk_path.push((current_node, 0));
                        path_walked += key.len();
                        current_node = next_node.need_known()?;
                        continue;
                    }
                    return Err(DeletionError::KeyNotFound);
                }
                DiffTrieNode::Leaf { key, .. } => {
                    if self.keys[key.clone()] == del_key[path_walked..] {
                        self.walk_path.push((current_node, 0));
                        break;
                    }
                    return Err(DeletionError::KeyNotFound);
                }
                DiffTrieNode::Null => {
                    return Err(DeletionError::KeyNotFound);
                }
            }
        }

        #[derive(Debug)]
        enum NodeDeletionResult {
            NodeDeleted,
            NodeUpdated,
            BranchBelowRemovedWithOneChild {
                child_nibble: u8,
                child_ptr: NodePtr,
            },
        }

        let mut deletion_result = NodeDeletionResult::NodeDeleted;

        for (current_node, current_node_child) in self.walk_path.iter().rev() {
            let current_node = *current_node;
            let current_node_child = *current_node_child;
            match deletion_result {
                NodeDeletionResult::NodeDeleted => match &self.nodes[current_node] {
                    DiffTrieNode::Leaf { .. } => {
                        deletion_result = NodeDeletionResult::NodeDeleted;
                    }
                    DiffTrieNode::Branch { children } => {
                        let children = &mut self.branch_node_children[*children];
                        let children_count = children.iter().filter(|c| c.is_some()).count();
                        match children_count {
                            3.. => {
                                children[current_node_child as usize] = None;
                                deletion_result = NodeDeletionResult::NodeUpdated;
                            }
                            2 => {
                                children[current_node_child as usize] = None;
                                let (orphan_nibble, orphan_ptr) = children
                                    .iter()
                                    .enumerate()
                                    .find(|(_, c)| c.is_some())
                                    .unwrap();
                                let orphan_ptr = orphan_ptr.unwrap();

                                // we do it so we don't panic below
                                orphan_ptr.need_known()?;

                                deletion_result =
                                    NodeDeletionResult::BranchBelowRemovedWithOneChild {
                                        child_nibble: orphan_nibble as u8,
                                        child_ptr: orphan_ptr,
                                    };
                            }
                            _ => unreachable!(),
                        }
                    }
                    _ => unreachable!(),
                },
                NodeDeletionResult::BranchBelowRemovedWithOneChild {
                    child_nibble: orphan_nibble,
                    child_ptr: orphan_ptr,
                } => {
                    // we need to merge orphaned node and its nibble into the new parent
                    let orphan_ptr_idx =
                        orphan_ptr.need_known().expect("orphan is not in the trie");
                    let new_parent = self.nodes[current_node].clone();
                    let orphaned_node = self.nodes[orphan_ptr_idx].clone();
                    match (new_parent, orphaned_node) {
                        (
                            DiffTrieNode::Extension {
                                key: parent_key, ..
                            },
                            DiffTrieNode::Leaf {
                                key: orphan_key,
                                value,
                            },
                        ) => {
                            // replace extension node by merging its path into leaf with child_nibble
                            let new_leaf_key =
                                self.merge_keys(parent_key, orphan_nibble, orphan_key);
                            self.nodes[current_node] = DiffTrieNode::Leaf {
                                key: new_leaf_key,
                                value,
                            }
                        }
                        (
                            DiffTrieNode::Extension {
                                key: parent_key, ..
                            },
                            DiffTrieNode::Extension {
                                key: orphan_key,
                                next_node: orphan_next,
                            },
                        ) => {
                            // we merge two extensions together
                            let new_ext_key =
                                self.merge_keys(parent_key, orphan_nibble, orphan_key);
                            self.nodes[current_node] = DiffTrieNode::Extension {
                                key: new_ext_key,
                                next_node: orphan_next,
                            }
                        }
                        (
                            DiffTrieNode::Extension {
                                key: parent_key, ..
                            },
                            DiffTrieNode::Branch { .. },
                        ) => {
                            // extension eats the orphan nibble and start to point to orphan branch
                            // branch is not changed
                            let new_ext_key = self.merge_keys(parent_key, orphan_nibble, 0..0);
                            self.nodes[current_node] = DiffTrieNode::Extension {
                                key: new_ext_key,
                                next_node: orphan_ptr,
                            }
                        }
                        (
                            DiffTrieNode::Branch {
                                children: parent_children,
                            },
                            DiffTrieNode::Leaf {
                                key: orphan_key,
                                value,
                            },
                        ) => {
                            // leaf eats nibble and branch starts to point to leaf
                            let new_leaf_key = self.merge_keys(0..0, orphan_nibble, orphan_key);
                            self.nodes[orphan_ptr_idx] = DiffTrieNode::Leaf {
                                key: new_leaf_key,
                                value,
                            };
                            self.branch_node_children[parent_children]
                                [current_node_child as usize] = Some(orphan_ptr);
                        }
                        (
                            DiffTrieNode::Branch {
                                children: parent_children,
                            },
                            DiffTrieNode::Extension {
                                key: orphan_key,
                                next_node,
                            },
                        ) => {
                            // extension eats nibble and branch starts to point to leaf
                            let new_ext_key = self.merge_keys(0..0, orphan_nibble, orphan_key);
                            self.nodes[orphan_ptr_idx] = DiffTrieNode::Extension {
                                key: new_ext_key,
                                next_node,
                            };
                            self.branch_node_children[parent_children]
                                [current_node_child as usize] = Some(orphan_ptr);
                        }
                        (
                            DiffTrieNode::Branch {
                                children: parent_children,
                            },
                            DiffTrieNode::Branch { .. },
                        ) => {
                            // create extension node that eats nibble
                            let new_ext_key = self.insert_key(&[orphan_nibble]);
                            let next_ext_ptr = self.push_node(DiffTrieNode::Extension {
                                key: new_ext_key,
                                next_node: orphan_ptr,
                            });
                            self.branch_node_children[parent_children]
                                [current_node_child as usize] = Some(next_ext_ptr);
                        }
                        _ => unreachable!(),
                    }
                    deletion_result = NodeDeletionResult::NodeUpdated;
                    break;
                }
                NodeDeletionResult::NodeUpdated => break,
            }
        }

        // here we handle the case when deletion reaches the head
        match deletion_result {
            // updates terminated before reaching the top
            NodeDeletionResult::NodeUpdated => {}
            NodeDeletionResult::NodeDeleted => {
                // trie is empty, insert the null node on top
                self.nodes[0] = DiffTrieNode::Null;
            }
            // orphan becomes head
            NodeDeletionResult::BranchBelowRemovedWithOneChild {
                child_nibble: orphan_nibble,
                child_ptr: orphan_ptr,
            } => {
                let orphan_ptr_idx = orphan_ptr.need_known().expect("orphan is not in the trie");
                match &self.nodes[orphan_ptr_idx] {
                    DiffTrieNode::Leaf {
                        key: orphan_key,
                        value,
                    } => {
                        let value = value.clone();
                        // leaf eats nibble
                        let new_leaf_key = self.merge_keys(0..0, orphan_nibble, orphan_key.clone());
                        self.nodes[0] = DiffTrieNode::Leaf {
                            key: new_leaf_key,
                            value,
                        };
                    }
                    DiffTrieNode::Extension {
                        key: orphan_key,
                        next_node,
                    } => {
                        let next_node = *next_node;
                        let new_ext_key = self.merge_keys(0..0, orphan_nibble, orphan_key.clone());
                        self.nodes[0] = DiffTrieNode::Extension {
                            key: new_ext_key,
                            next_node,
                        }
                    }
                    DiffTrieNode::Branch { .. } => {
                        let new_ext_key = self.insert_key(&[orphan_nibble]);
                        self.nodes[0] = DiffTrieNode::Extension {
                            key: new_ext_key,
                            next_node: orphan_ptr,
                        }
                    }
                    DiffTrieNode::Null => unreachable!(),
                }
            }
        }

        Ok(())
    }

    fn rlp_encode_node(&mut self, node_idx: usize) {
        self.rlp.clear();
        let node = self.nodes.get(node_idx).expect("node not found").clone();
        match node {
            DiffTrieNode::Branch { children } => {
                let mut child_rlp_pointers: [Option<&[u8]>; 16] = [None; 16];
                for (idx, child_node_ptr) in self.branch_node_children[children].iter().enumerate()
                {
                    if let Some(child) = child_node_ptr {
                        let child_rlp_ptr = match child {
                            NodePtr::Local(idx) => {
                                debug_assert!(self.hashed_nodes[*idx]);
                                Some(self.rlp_ptrs_local[*idx].as_slice())
                            }
                            NodePtr::Remote(idx) => Some(self.rlp_ptrs_remote[*idx].as_slice()),
                        };
                        child_rlp_pointers[idx] = child_rlp_ptr;
                    }
                }
                encode_branch_node(&child_rlp_pointers, &mut self.rlp);
            }
            DiffTrieNode::Extension { key, next_node } => {
                let vec = self.tmp_nibbles.as_mut_vec_unchecked();
                vec.clear();
                vec.extend_from_slice(&self.keys[key]);
                let child_rlp_ptr = match next_node {
                    NodePtr::Local(idx) => {
                        debug_assert!(self.hashed_nodes[idx]);
                        self.rlp_ptrs_local[idx].as_slice()
                    }
                    NodePtr::Remote(idx) => self.rlp_ptrs_remote[idx].as_slice(),
                };
                encode_extension(&self.tmp_nibbles, child_rlp_ptr, &mut self.rlp);
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
            self.rlp_ptrs_local[node_idx].clear();
            self.rlp_ptrs_local[node_idx]
                .try_extend_from_slice(&self.rlp)
                .unwrap();
        } else {
            let hash = keccak256(&self.rlp);
            let result = &mut self.rlp_ptrs_local[node_idx];
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
                for child in self.branch_node_children[*children].into_iter().flatten() {
                    if let NodePtr::Local(child) = child {
                        self.root_hash_node(child);
                    }
                }
                self.calculate_rlp_pointer_node(node_idx);
            }
            DiffTrieNode::Extension { next_node, .. } => {
                if let NodePtr::Local(child) = next_node {
                    self.root_hash_node(*child);
                }
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
                println!("{}", h(self.rlp_ptrs_local[node_idx].as_slice()));
                for (idx, child) in self.branch_node_children[children].into_iter().enumerate() {
                    if child.is_some() {
                        println!("  {} -> {:?}", idx, child);
                    }
                }
                for child in self.branch_node_children[children].into_iter().flatten() {
                    if let NodePtr::Local(idx) = child {
                        self.print_node(idx);
                    }
                }
            }
            DiffTrieNode::Extension { next_node, key } => {
                println!(
                    "{} Extension {:?} -> {:?}",
                    node_idx,
                    h(&self.keys[key]),
                    next_node
                );
                println!("{}", h(self.rlp_ptrs_local[node_idx].as_slice()));
                if let NodePtr::Local(idx) = next_node {
                    self.print_node(idx);
                }
            }
            DiffTrieNode::Leaf { key, value } => {
                println!(
                    "{} Leaf {:?} : {:?}",
                    node_idx,
                    h(&self.keys[key]),
                    h(&self.values[value])
                );
                println!("{}", h(self.rlp_ptrs_local[node_idx].as_slice()));
            }
            DiffTrieNode::Null => {
                println!("{} Null", node_idx);
                println!("{}", h(self.rlp_ptrs_local[node_idx].as_slice()));
            }
        }
    }

    // node can be adde only if all of its parents are actually in the trie
    pub fn add_node_from_proof(
        &mut self,
        path: &[u8],
        node: &ProofNode,
    ) -> Result<(), NodeNotFound> {
        let mut current_node = 0;
        let mut path_walked = 0;

        let mut parent_ptr = None;
        let mut parent_nibble = 0;
        loop {
            let node = self.nodes.get(current_node).ok_or(NodeNotFound)?;
            match node {
                DiffTrieNode::Branch { children } => {
                    let children = *children;

                    let n = path[path_walked] as usize;
                    path_walked += 1;
                    if path[path_walked..].is_empty() {
                        parent_ptr = self.branch_node_children[children][n];
                        parent_nibble = n;
                        break;
                    }
                    if let Some(child_ptr) = self.branch_node_children[children][n] {
                        current_node = child_ptr.need_known()?;
                        continue;
                    } else {
                        return Err(NodeNotFound);
                    }
                }
                DiffTrieNode::Extension { key, next_node } => {
                    let key = key.clone();
                    let next_node = *next_node;

                    if path[path_walked..].is_empty() {
                        parent_ptr = Some(next_node);
                        parent_nibble = 0;
                        break;
                    }

                    if path[path_walked..].starts_with(&self.keys[key.clone()]) {
                        path_walked += key.len();
                        current_node = next_node.need_known()?;
                        continue;
                    }
                }
                _ => {
                    // no proofs can be added here,
                    return Ok(());
                }
            }
            break;
        }

        match parent_ptr {
            Some(NodePtr::Remote(_)) => {}
            _ => {
                // node is not needed
                return Ok(());
            }
        };

        let new_node = match node {
            ProofNode::Leaf { key, value } => {
                let key = self.insert_key(key);
                let value = self.copy_value(value);
                self.push_node(DiffTrieNode::Leaf { key, value })
            }
            ProofNode::Extension { key, child } => {
                let key = self.insert_key(key);
                let next_node = self.insert_remote_node_rlp(child);
                self.push_node(DiffTrieNode::Extension { key, next_node })
            }
            ProofNode::Branch { children } => {
                let branch_node_children = self.create_branch_children();
                for b in 0..16 {
                    if let Some(child_rlp) = &children[b] {
                        let child_ptr = self.insert_remote_node_rlp(child_rlp);
                        self.branch_node_children[branch_node_children][b] = Some(child_ptr);
                    }
                }
                self.push_node(DiffTrieNode::Branch {
                    children: branch_node_children,
                })
            }
        };

        // give pointer to parent
        match &mut self.nodes[current_node] {
            DiffTrieNode::Branch { children } => {
                self.branch_node_children[*children][parent_nibble] = Some(new_node);
            }
            DiffTrieNode::Extension { next_node, .. } => {
                *next_node = new_node;
            }
            _ => unreachable!(),
        }

        Ok(())
    }
}

#[allow(clippy::large_enum_variant)]
pub enum ProofNode {
    Leaf {
        key: Nibbles,
        value: Vec<u8>,
    },
    Extension {
        key: Nibbles,
        child: ArrayVec<u8, 33>,
    },
    Branch {
        children: [Option<ArrayVec<u8, 33>>; 16],
    },
}

impl ProofNode {
    pub fn try_from_rlp_encoded_node(mut encoded_node: &[u8]) -> Result<Self, alloy_rlp::Error> {
        let alloy_trie_node = AlloyTrieNode::decode(&mut encoded_node)?;
        let result = match alloy_trie_node {
            AlloyTrieNode::Branch(alloy_node) => {
                let mut children: [Option<ArrayVec<u8, 33>>; 16] = Default::default();
                let mut stack_iter = alloy_node.stack.into_iter();
                for index in 0..16 {
                    if alloy_node.state_mask.is_bit_set(index) {
                        let rlp_ptr = stack_iter
                            .next()
                            .expect("stack must be the same size as mask")
                            .as_slice()
                            .try_into()
                            .unwrap();
                        children[index as usize] = Some(rlp_ptr);
                    }
                }
                ProofNode::Branch { children }
            }
            AlloyTrieNode::Extension(node) => ProofNode::Extension {
                key: node.key,
                child: node.child.as_slice().try_into().unwrap(),
            },
            AlloyTrieNode::Leaf(node) => ProofNode::Leaf {
                key: node.key,
                value: node.value,
            },
            AlloyTrieNode::EmptyRoot => todo!("handle empty root from proof"),
        };
        Ok(result)
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
