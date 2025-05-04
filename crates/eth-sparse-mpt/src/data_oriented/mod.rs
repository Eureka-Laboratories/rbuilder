use reth_trie::Nibbles;

use crate::utils::{extract_prefix_and_suffix, strip_first_nibble_mut};

type Ptr = u32;

type Idx = u32;

#[derive(Debug, Default)]
pub struct DODiffTrie {
    hashed_nodes: Vec<bool>,
    rlp_ptrs: Vec<[u8; 32]>,
    nodes: Vec<DiffTrieNode>,

    values: Vec<u8>,
    keys: Vec<Nibbles>,
    branch_node_children: Vec<[Ptr; 16]>, // 0 means child is empty

    // scratchpad
    current_node: Idx,
    current_path: Nibbles,
    path_left: Nibbles,

    prefix: Nibbles,
    suffix1: Nibbles,
    suffix2: Nibbles,
    nibble1: u8,
    nibble2: u8,
}

#[derive(Debug, Clone, Copy)]
enum DiffTrieNode {
    Leaf { key: Idx, value: Idx },
    Extension { key: Idx, next_node: Ptr },
    Branch { children: Idx },
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
        self.hashed_nodes.push(false);
        self.rlp_ptrs.push(Default::default());
        self.nodes.push(DiffTrieNode::Null);
    }

    fn copy_value(&mut self, value: &[u8]) -> Idx {
        let idx = self.values.len();
        let size = value.len();
        self.values.extend_from_slice(&size.to_be_bytes());
        self.values.extend_from_slice(value);
        idx as Idx
    }

    fn insert_key(&mut self, key: Nibbles) -> Idx {
        let idx = self.keys.len() as Idx;
        self.keys.push(key);
        idx
    }

    fn copy_or_overwrite_value(&mut self, _old_value_idx: Idx, value: &[u8]) -> Idx {
        self.copy_value(value)
    }

    fn create_branch_children(&mut self) -> Idx {
        let idx = self.branch_node_children.len() as Idx;
        self.branch_node_children.push(Default::default());
        idx
    }

    fn extract_prefix_and_suffix(&mut self, key: Idx) {
        let (pref, mut suff1, mut suff2) =
            extract_prefix_and_suffix(&self.path_left, &self.keys[key as usize]);
        let n1 = strip_first_nibble_mut(&mut suff1);
        let n2 = strip_first_nibble_mut(&mut suff2);

        self.prefix = pref;
        self.suffix1 = suff1;
        self.suffix2 = suff2;
        self.nibble1 = n1;
        self.nibble2 = n2;
    }

    pub fn clear(&mut self) {
        self.hashed_nodes.clear();
        self.rlp_ptrs.clear();
        self.values.clear();
        self.keys.clear();
        self.branch_node_children.clear();
        self.nodes.clear();
    }

    pub fn insert(&mut self, key: &Nibbles, insert_value: &[u8]) {
        self.current_node = 0;
        self.current_path.clear();
        self.path_left = key.clone();

        loop {
            let node = *self
                .nodes
                .get(self.current_node as usize)
                .expect("node not found");
            self.hashed_nodes[self.current_node as usize] = false;
            match node {
                DiffTrieNode::Branch { children } => {
                    let n = strip_first_nibble_mut(&mut self.path_left);
                    self.current_path.push_unchecked(n);
                    if self.branch_node_children[children as usize][n as usize] != 0 {
                        self.current_node =
                            self.branch_node_children[children as usize][n as usize];
                        continue;
                    } else {
                        let leaf_ptr = self.nodes.len() as Ptr;
                        let tmp_key = self.insert_key(self.path_left.clone());
                        let tmp_value = self.copy_value(insert_value);
                        self.nodes.push(DiffTrieNode::Leaf {
                            key: tmp_key,
                            value: tmp_value,
                        });
                        self.rlp_ptrs.push(Default::default());
                        self.hashed_nodes.push(false);
                        self.branch_node_children[children as usize][n as usize] = leaf_ptr;
                    }
                }
                DiffTrieNode::Extension { key, next_node } => {
                    if self.path_left.starts_with(&self.keys[key as usize]) {
                        let ext_key_len = self.keys[key as usize].len();
                        self.current_path
                            .extend_from_slice_unchecked(&self.path_left[..ext_key_len]);
                        self.path_left.as_mut_vec_unchecked().drain(..ext_key_len);
                        self.current_node = next_node;
                        continue;
                    }
                    self.extract_prefix_and_suffix(key);

                    let has_extension_node = !self.prefix.is_empty();
                    if has_extension_node {
                        let ext_key = self.insert_key(self.prefix.clone());
                        let next_node = self.nodes.len() as Ptr;
                        self.nodes[self.current_node as usize] = DiffTrieNode::Extension {
                            key: ext_key,
                            next_node,
                        };
                    };
                    let branch_children = self.create_branch_children();
                    let branch_node = DiffTrieNode::Branch {
                        children: branch_children,
                    };
                    if has_extension_node {
                        self.nodes.push(branch_node);
                        self.rlp_ptrs.push(Default::default());
                        self.hashed_nodes.push(false);
                    } else {
                        self.nodes[self.current_node as usize] = branch_node;
                    }
                    let leaf_ptr = self.nodes.len();
                    let tmp_key = self.insert_key(self.suffix1.clone());
                    let tmp_value = self.copy_value(insert_value);
                    self.nodes.push(DiffTrieNode::Leaf {
                        key: tmp_key,
                        value: tmp_value,
                    });
                    self.rlp_ptrs.push(Default::default());
                    self.hashed_nodes.push(false);

                    let branch_child = if !self.suffix2.is_empty() {
                        next_node
                    } else {
                        let new_ext_ptr = self.nodes.len();
                        let tmp_key = self.insert_key(self.suffix2.clone());
                        self.nodes.push(DiffTrieNode::Extension {
                            key: tmp_key,
                            next_node,
                        });
			self.rlp_ptrs.push(Default::default());
			self.hashed_nodes.push(false);
                        new_ext_ptr as Ptr
                    };

                    self.branch_node_children[branch_children as usize][self.nibble1 as usize] =
                        leaf_ptr as Ptr;
                    self.branch_node_children[branch_children as usize][self.nibble2 as usize] =
                        branch_child as Ptr;
                }
                DiffTrieNode::Leaf { key, value } => {
                    if self.keys[key as usize] == self.path_left {
                        // update leaf in place
                        let new_value = self.copy_or_overwrite_value(value, insert_value);
                        self.nodes[self.current_node as usize] = DiffTrieNode::Leaf {
                            key,
                            value: new_value,
                        };
                    }

                    self.extract_prefix_and_suffix(key);

                    let has_extension_node = !self.prefix.is_empty();
                    if has_extension_node {
                        let ext_key = self.insert_key(self.prefix.clone());
                        let next_node = self.nodes.len() as Ptr;
                        self.nodes[self.current_node as usize] = DiffTrieNode::Extension {
                            key: ext_key,
                            next_node,
                        };
                    };
                    let branch_children = self.create_branch_children();
                    let branch_node = DiffTrieNode::Branch {
                        children: branch_children,
                    };
                    if has_extension_node {
                        self.nodes.push(branch_node);
                        self.rlp_ptrs.push(Default::default());
                        self.hashed_nodes.push(false);
                    } else {
                        self.nodes[self.current_node as usize] = branch_node;
                    }
                    let first_leaf_ptr = self.nodes.len();
                    let tmp_key = self.insert_key(self.suffix1.clone());
                    let tmp_value = self.copy_value(insert_value);
                    self.nodes.push(DiffTrieNode::Leaf {
                        key: tmp_key,
                        value: tmp_value,
                    });
                    self.rlp_ptrs.push(Default::default());
                    self.hashed_nodes.push(false);
                    let second_leaf_ptr = self.nodes.len();
                    let tmp_key = self.insert_key(self.suffix2.clone());
                    let tmp_value = value;
                    self.nodes.push(DiffTrieNode::Leaf {
                        key: tmp_key,
                        value: tmp_value,
                    });
                    self.rlp_ptrs.push(Default::default());
                    self.hashed_nodes.push(false);
                    self.branch_node_children[branch_children as usize][self.nibble1 as usize] =
                        first_leaf_ptr as Ptr;
                    self.branch_node_children[branch_children as usize][self.nibble2 as usize] =
                        second_leaf_ptr as Ptr;
                }
                DiffTrieNode::Null => {
                    let tmp_key = self.insert_key(self.path_left.clone());
                    let tmp_value = self.copy_value(insert_value);
                    self.nodes[self.current_node as usize] = DiffTrieNode::Leaf {
                        key: tmp_key,
                        value: tmp_value,
                    };
                }
            }
            break;
        }
    }
}
