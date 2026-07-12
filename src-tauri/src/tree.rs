//! Pure tree logic for branching conversations.
//!
//! A conversation is a tree of nodes; each node is one interaction (a user
//! question + the AI answer). `parentId` is null for a child of the implicit
//! virtual root (a new thread). The CONTEXT-ISOLATION INVARIANT lives here:
//! `ancestor_path` only ever walks `parentId` upward, so a node's generation
//! context is the root->parent chain and siblings are structurally unreachable.
//!
//! Everything is Value-level (matching the rest of the codebase) and pure, so
//! it is exhaustively unit-tested without a running app or Ollama.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

fn node_id(node: &Value) -> Option<u64> {
    node.get("id").and_then(|v| v.as_u64())
}

/// A node's parentId, or None for a child of the virtual root (null/absent).
fn parent_id_of(node: &Value) -> Option<u64> {
    node.get("parentId").and_then(|v| v.as_u64())
}

/// Next node id = max(existing ids) + 1. NOT len()+1 — deletions shrink the
/// list, and a length-based id would recycle a live id after a subtree delete,
/// silently re-parenting orphans or forging a phantom ancestor (a context leak).
pub fn next_node_id(nodes: &[Value]) -> u64 {
    nodes.iter().filter_map(node_id).max().unwrap_or(0) + 1
}

pub fn find_node(nodes: &[Value], id: u64) -> Option<&Value> {
    nodes.iter().find(|n| node_id(n) == Some(id))
}

/// Ordered root->parent ancestor nodes for `parent_id`.
///
/// Defended against corrupted/hand-edited files: a HashMap index (a miss ends
/// the walk), a visited-set (a repeat ends the walk on a cycle), and a hard
/// iteration cap. Returns the partial isolated chain rather than hanging.
/// `parent_id == None` (virtual root) yields an empty path.
pub fn ancestor_path(nodes: &[Value], parent_id: Option<u64>) -> Vec<Value> {
    let index: HashMap<u64, &Value> = nodes
        .iter()
        .filter_map(|n| node_id(n).map(|id| (id, n)))
        .collect();

    let mut chain: Vec<Value> = Vec::new();
    let mut seen: HashSet<u64> = HashSet::new();
    let cap = nodes.len() + 1;
    let mut current = parent_id;
    let mut steps = 0;

    while let Some(id) = current {
        if steps >= cap {
            break; // defensive cap
        }
        steps += 1;
        if !seen.insert(id) {
            break; // cycle
        }
        let node = match index.get(&id) {
            Some(node) => *node,
            None => break, // dangling parent; keep what we have
        };
        chain.push(node.clone());
        current = parent_id_of(node);
    }

    chain.reverse(); // parent->root becomes root->parent  (LOAD-BEARING)
    chain
}

/// Flatten the ancestor path into Ollama chat messages, then append the new
/// question as the final user turn. Every ancestor emits BOTH a user turn
/// (question) and an assistant turn (answer), even when empty, so the strict
/// user/assistant alternation Ollama expects is never broken.
pub fn build_chat_messages(path: &[Value], new_question: &str) -> Vec<Value> {
    let mut messages = Vec::with_capacity(path.len() * 2 + 1);
    for node in path {
        let question = node.get("question").and_then(|v| v.as_str()).unwrap_or("");
        let answer = node.get("answer").and_then(|v| v.as_str()).unwrap_or("");
        messages.push(json!({ "role": "user", "content": question }));
        messages.push(json!({ "role": "assistant", "content": answer }));
    }
    messages.push(json!({ "role": "user", "content": new_question }));
    messages
}

/// Remove `root_id` and its entire subtree. Returns (surviving nodes, removed
/// ids). Surviving ids/edges are preserved (no renumber). Iterative closure so
/// depth can't overflow the stack.
pub fn delete_subtree(nodes: &[Value], root_id: u64) -> (Vec<Value>, Vec<u64>) {
    if find_node(nodes, root_id).is_none() {
        return (nodes.to_vec(), Vec::new());
    }

    let mut removed: HashSet<u64> = HashSet::new();
    removed.insert(root_id);
    loop {
        let mut grew = false;
        for node in nodes {
            let id = match node_id(node) {
                Some(id) => id,
                None => continue,
            };
            if removed.contains(&id) {
                continue;
            }
            if let Some(parent) = parent_id_of(node) {
                if removed.contains(&parent) {
                    removed.insert(id);
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }

    let survivors: Vec<Value> = nodes
        .iter()
        .filter(|n| node_id(n).map(|id| !removed.contains(&id)).unwrap_or(true))
        .cloned()
        .collect();
    let mut removed_ids: Vec<u64> = removed.into_iter().collect();
    removed_ids.sort_unstable();
    (survivors, removed_ids)
}

/// Set a node's canvas position. Returns the updated node clone, or None.
pub fn set_position(nodes: &mut [Value], id: u64, x: f64, y: f64) -> Option<Value> {
    for node in nodes.iter_mut() {
        if node_id(node) == Some(id) {
            node["x"] = json!(x);
            node["y"] = json!(y);
            return Some(node.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u64, parent: Option<u64>, q: &str, a: &str) -> Value {
        json!({
            "id": id,
            "parentId": parent,
            "question": q,
            "answer": a,
            "model": "m",
            "x": 0.0,
            "y": 0.0,
        })
    }

    #[test]
    fn next_node_id_empty_is_one() {
        assert_eq!(next_node_id(&[]), 1);
    }

    #[test]
    fn next_node_id_is_max_plus_one_not_len() {
        let nodes = vec![node(1, None, "", ""), node(2, Some(1), "", ""), node(5, Some(2), "", "")];
        assert_eq!(next_node_id(&nodes), 6); // len+1 would wrongly be 4
    }

    #[test]
    fn next_node_id_after_delete_is_collision_safe() {
        // 2 and 3 are siblings under 1; deleting leaf 2 leaves [1, 3].
        let nodes = vec![node(1, None, "", ""), node(2, Some(1), "", ""), node(3, Some(1), "", "")];
        let (survivors, removed) = delete_subtree(&nodes, 2); // leaves [1, 3]
        assert_eq!(removed, vec![2]);
        // max(1,3)+1 = 4: never collides with a surviving id, and not len+1 (== 3).
        assert_eq!(next_node_id(&survivors), 4);
    }

    #[test]
    fn find_node_hit_and_miss() {
        let nodes = vec![node(1, None, "", "")];
        assert!(find_node(&nodes, 1).is_some());
        assert!(find_node(&nodes, 9).is_none());
    }

    #[test]
    fn ancestor_path_virtual_root_is_empty() {
        let nodes = vec![node(1, None, "q", "a")];
        assert!(ancestor_path(&nodes, None).is_empty());
    }

    #[test]
    fn ancestor_path_root_to_parent_order() {
        // 1 <- 2 <- 3
        let nodes = vec![
            node(1, None, "q1", "a1"),
            node(2, Some(1), "q2", "a2"),
            node(3, Some(2), "q3", "a3"),
        ];
        let path = ancestor_path(&nodes, Some(3));
        let ids: Vec<u64> = path.iter().filter_map(node_id).collect();
        assert_eq!(ids, vec![1, 2, 3]); // would be [3,2,1] without reverse()
    }

    #[test]
    fn ancestor_path_excludes_siblings() {
        // parent 1 with children 2 and 3
        let nodes = vec![
            node(1, None, "q1", "a1"),
            node(2, Some(1), "q2", "a2"),
            node(3, Some(1), "q3", "a3"),
        ];
        // Creating a child under node 2: context is [1, 2]; node 3's branch is invisible.
        let path = ancestor_path(&nodes, Some(2));
        let ids: Vec<u64> = path.iter().filter_map(node_id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert!(!ids.contains(&3)); // sibling branch excluded
    }

    #[test]
    fn ancestor_path_excludes_cousins() {
        // Two branches under root: A(2)->A2(4), B(3)->B2(5). A2's context never sees B.
        let nodes = vec![
            node(1, None, "root_q", "root_a"),
            node(2, Some(1), "a_q", "a_a"),
            node(3, Some(1), "b_q", "b_a"),
            node(4, Some(2), "a2_q", "a2_a"),
            node(5, Some(3), "b2_q", "b2_a"),
        ];
        let path = ancestor_path(&nodes, Some(4));
        let ids: Vec<u64> = path.iter().filter_map(node_id).collect();
        assert_eq!(ids, vec![1, 2, 4]);
        assert!(!ids.contains(&3) && !ids.contains(&5)); // B-branch invisible
    }

    #[test]
    fn ancestor_path_terminates_on_cycle() {
        // 1 -> 2 -> 1 (cycle)
        let nodes = vec![node(1, Some(2), "q1", "a1"), node(2, Some(1), "q2", "a2")];
        let path = ancestor_path(&nodes, Some(1));
        assert!(path.len() <= 2); // terminates, no hang
    }

    #[test]
    fn ancestor_path_self_loop_terminates() {
        let nodes = vec![node(1, Some(1), "q1", "a1")];
        let path = ancestor_path(&nodes, Some(1));
        assert!(path.len() <= 1);
    }

    #[test]
    fn ancestor_path_dangling_parent_stops() {
        let nodes = vec![node(2, Some(999), "q2", "a2")];
        let path = ancestor_path(&nodes, Some(2));
        let ids: Vec<u64> = path.iter().filter_map(node_id).collect();
        assert_eq!(ids, vec![2]); // 999 is dangling; walk stops cleanly
    }

    #[test]
    fn build_chat_messages_empty_path_single_turn() {
        let messages = build_chat_messages(&[], "hi");
        assert_eq!(messages, vec![json!({ "role": "user", "content": "hi" })]);
    }

    #[test]
    fn build_chat_messages_alternates_roles() {
        let path = vec![node(1, None, "q1", "a1"), node(2, Some(1), "q2", "a2")];
        let messages = build_chat_messages(&path, "q3");
        assert_eq!(
            messages,
            vec![
                json!({ "role": "user", "content": "q1" }),
                json!({ "role": "assistant", "content": "a1" }),
                json!({ "role": "user", "content": "q2" }),
                json!({ "role": "assistant", "content": "a2" }),
                json!({ "role": "user", "content": "q3" }),
            ]
        );
    }

    #[test]
    fn build_chat_messages_preserves_empty_turns() {
        let path = vec![node(1, None, "", "")];
        let messages = build_chat_messages(&path, "next");
        assert_eq!(messages.len(), 3); // empty question + empty answer + new question
        assert_eq!(messages[0], json!({ "role": "user", "content": "" }));
        assert_eq!(messages[1], json!({ "role": "assistant", "content": "" }));
    }

    #[test]
    fn delete_subtree_leaf_only() {
        let nodes = vec![node(1, None, "", ""), node(2, Some(1), "", "")];
        let (survivors, removed) = delete_subtree(&nodes, 2);
        assert_eq!(removed, vec![2]);
        assert_eq!(survivors.len(), 1);
    }

    #[test]
    fn delete_subtree_cascades_descendants() {
        // 1 <- 2 <- 3, and sibling 4 under 1
        let nodes = vec![
            node(1, None, "", ""),
            node(2, Some(1), "", ""),
            node(3, Some(2), "", ""),
            node(4, Some(1), "", ""),
        ];
        let (survivors, removed) = delete_subtree(&nodes, 2);
        assert_eq!(removed, vec![2, 3]);
        let ids: Vec<u64> = survivors.iter().filter_map(node_id).collect();
        assert_eq!(ids, vec![1, 4]);
    }

    #[test]
    fn delete_subtree_unknown_id_is_noop() {
        let nodes = vec![node(1, None, "", "")];
        let (survivors, removed) = delete_subtree(&nodes, 42);
        assert!(removed.is_empty());
        assert_eq!(survivors.len(), 1);
    }

    #[test]
    fn set_position_updates_xy() {
        let mut nodes = vec![node(1, None, "", "")];
        let updated = set_position(&mut nodes, 1, 12.5, 34.0).unwrap();
        assert_eq!(updated["x"], 12.5);
        assert_eq!(updated["y"], 34.0);
        assert_eq!(nodes[0]["x"], 12.5);
        assert!(set_position(&mut nodes, 99, 1.0, 1.0).is_none());
    }
}
