//! Retained UI tree.
//!
//! [`Tree`] holds the current root [`TreeNode`] and supports full
//! replacement via [`snapshot`](Tree::snapshot) and incremental updates
//! via [`apply_patch`](Tree::apply_patch). The renderer reads the tree
//! during `view()` to produce iced widgets; the host mutates it by
//! sending Snapshot and Patch messages.

use std::collections::HashSet;

use crate::protocol::{PatchOp, TreeNode};
use crate::widgets::MAX_TREE_DEPTH;

/// Retained tree store. Holds the current root node (if any) and supports
/// full replacement (snapshot) and incremental patch application.
#[derive(Debug, Default)]
pub struct Tree {
    root: Option<TreeNode>,
}

impl Tree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the entire tree with a new root (snapshot).
    ///
    /// The tree is always accepted (the renderer needs UI even if the tree
    /// has problems). Returns `Ok(())` if all node IDs are unique, or
    /// `Err` with a list of `"id (type_name)"` strings for each duplicate.
    /// The caller should emit a protocol error so the host can fix the bug.
    pub fn snapshot(&mut self, root: TreeNode) -> Result<(), Vec<String>> {
        self.root = Some(root);
        // Validate after setting -- the tree is accepted regardless, but
        // duplicates are reported as errors.
        if let Some(root) = self.root.as_ref() {
            validate_unique_ids(root)
        } else {
            Ok(())
        }
    }

    /// Return a reference to the current root, if any.
    pub fn root(&self) -> Option<&TreeNode> {
        self.root.as_ref()
    }

    /// Find a window node by its toddy ID, searching the entire tree recursively.
    pub fn find_window(&self, toddy_id: &str) -> Option<&TreeNode> {
        let root = self.root.as_ref()?;
        find_window_recursive(root, toddy_id, 0)
    }

    /// Collect the IDs of all window nodes in the tree (recursive search).
    pub fn window_ids(&self) -> Vec<String> {
        let Some(root) = self.root.as_ref() else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        collect_window_ids_recursive(root, &mut ids, 0);
        ids
    }

    /// Apply a sequence of patch operations to the tree.
    ///
    /// Operations are applied sequentially. If one operation fails, it is
    /// skipped with a warning and subsequent operations are still applied.
    /// This means a partial failure can leave the tree in an intermediate
    /// state. The host should treat patch sequences as best-effort and
    /// use Snapshot for full-state recovery when needed.
    pub fn apply_patch(&mut self, ops: Vec<PatchOp>) {
        for op in ops {
            if let Err(e) = self.apply_op(&op) {
                log::error!("failed to apply patch op {:?}: {}", op.op, e);
            }
        }
    }

    fn apply_op(&mut self, op: &PatchOp) -> Result<(), String> {
        let root = self.root.as_mut().ok_or("no tree to patch")?;

        match op.op.as_str() {
            "replace_node" => {
                let node = op
                    .rest
                    .get("node")
                    .ok_or("replace_node: missing 'node' field")?;
                let new_node: TreeNode = serde_json::from_value(node.clone())
                    .map_err(|e| format!("replace_node: invalid node: {e}"))?;

                if op.path.is_empty() {
                    // Replace root
                    *root = new_node;
                } else {
                    let parent = navigate_mut(root, &op.path[..op.path.len() - 1])?;
                    let idx = *op.path.last().unwrap();
                    if idx < parent.children.len() {
                        parent.children[idx] = new_node;
                    } else {
                        return Err(format!("replace_node: index {idx} out of bounds"));
                    }
                }
                Ok(())
            }
            "update_props" => {
                let target = navigate_mut(root, &op.path)?;
                let props = op
                    .rest
                    .get("props")
                    .ok_or("update_props: missing 'props' field")?;

                if !target.props.is_object() {
                    log::error!(
                        "update_props: target node '{}' props is not an object: {}",
                        target.id,
                        target.props
                    );
                    return Ok(());
                }
                if !props.is_object() {
                    log::error!("update_props: patch props is not an object: {}", props);
                    return Ok(());
                }
                let target_map = target.props.as_object_mut().unwrap();
                let patch_map = props.as_object().unwrap();
                for (k, v) in patch_map {
                    if v.is_null() {
                        target_map.remove(k);
                    } else {
                        target_map.insert(k.clone(), v.clone());
                    }
                }
                Ok(())
            }
            "insert_child" => {
                let parent = navigate_mut(root, &op.path)?;
                let index = op
                    .rest
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .ok_or("insert_child: missing or invalid 'index'")?
                    as usize;
                let node = op
                    .rest
                    .get("node")
                    .ok_or("insert_child: missing 'node' field")?;
                let new_node: TreeNode = serde_json::from_value(node.clone())
                    .map_err(|e| format!("insert_child: invalid node: {e}"))?;

                if index <= parent.children.len() {
                    parent.children.insert(index, new_node);
                } else {
                    log::error!(
                        "insert_child: index {index} is beyond children length {}, appending instead",
                        parent.children.len()
                    );
                    parent.children.push(new_node);
                }
                Ok(())
            }
            "remove_child" => {
                let parent = navigate_mut(root, &op.path)?;
                let index = op
                    .rest
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .ok_or("remove_child: missing or invalid 'index'")?
                    as usize;

                if index < parent.children.len() {
                    parent.children.remove(index);
                    Ok(())
                } else {
                    Err(format!(
                        "remove_child: index {index} out of bounds (len={})",
                        parent.children.len()
                    ))
                }
            }
            other => {
                log::warn!("unknown patch op: {other}");
                Ok(())
            }
        }
    }
}

fn find_window_recursive<'a>(
    node: &'a TreeNode,
    toddy_id: &str,
    depth: usize,
) -> Option<&'a TreeNode> {
    if depth > MAX_TREE_DEPTH {
        log::warn!("find_window_recursive: depth exceeds {MAX_TREE_DEPTH}, stopping search");
        return None;
    }
    if node.type_name == "window" && node.id == toddy_id {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_window_recursive(child, toddy_id, depth + 1) {
            return Some(found);
        }
    }
    None
}

fn collect_window_ids_recursive(node: &TreeNode, ids: &mut Vec<String>, depth: usize) {
    if depth > MAX_TREE_DEPTH {
        log::warn!("collect_window_ids_recursive: depth exceeds {MAX_TREE_DEPTH}, stopping search");
        return;
    }
    if node.type_name == "window" {
        ids.push(node.id.clone());
    }
    for child in &node.children {
        collect_window_ids_recursive(child, ids, depth + 1);
    }
}

/// Walk the tree and check that all node IDs are unique.
///
/// Returns `Ok(())` if no duplicates are found, or `Err` with a list of
/// `"id (type_name)"` strings for each ID that appears more than once.
/// Empty IDs are skipped (some internal nodes may not have meaningful IDs).
fn validate_unique_ids(root: &TreeNode) -> Result<(), Vec<String>> {
    let mut seen = HashSet::new();
    let mut duplicates = Vec::new();
    collect_duplicate_ids(root, &mut seen, &mut duplicates, 0);
    if duplicates.is_empty() {
        Ok(())
    } else {
        Err(duplicates)
    }
}

fn collect_duplicate_ids(
    node: &TreeNode,
    seen: &mut HashSet<String>,
    duplicates: &mut Vec<String>,
    depth: usize,
) {
    if depth > MAX_TREE_DEPTH {
        return;
    }
    if !node.id.is_empty() && !seen.insert(node.id.clone()) {
        duplicates.push(format!("{} ({})", node.id, node.type_name));
    }
    for child in &node.children {
        collect_duplicate_ids(child, seen, duplicates, depth + 1);
    }
}

/// Navigate to a node at the given path of child indices.
fn navigate_mut<'a>(root: &'a mut TreeNode, path: &[usize]) -> Result<&'a mut TreeNode, String> {
    let mut current = root;
    for &idx in path {
        if idx < current.children.len() {
            current = &mut current.children[idx];
        } else {
            return Err(format!(
                "path navigation: index {idx} out of bounds (len={})",
                current.children.len()
            ));
        }
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PatchOp;
    use crate::testing::{node, node_with_children, node_with_props};
    use serde_json::json;

    fn make_patch_op(op: &str, path: Vec<usize>, rest: serde_json::Value) -> PatchOp {
        // Deserialize from JSON to get proper PatchOp with flattened rest
        let mut obj = serde_json::Map::new();
        obj.insert("op".to_string(), json!(op));
        obj.insert("path".to_string(), json!(path));
        if let Some(map) = rest.as_object() {
            for (k, v) in map {
                obj.insert(k.clone(), v.clone());
            }
        }
        serde_json::from_value(serde_json::Value::Object(obj)).unwrap()
    }

    // -----------------------------------------------------------------------
    // Tree basics
    // -----------------------------------------------------------------------

    #[test]
    fn new_tree_is_empty() {
        let tree = Tree::new();
        assert!(tree.root().is_none());
    }

    #[test]
    fn default_tree_is_empty() {
        let tree = Tree::default();
        assert!(tree.root().is_none());
    }

    #[test]
    fn snapshot_sets_root() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("root", "column"));
        assert!(tree.root().is_some());
        assert_eq!(tree.root().unwrap().id, "root");
        assert_eq!(tree.root().unwrap().type_name, "column");
    }

    #[test]
    fn snapshot_replaces_previous_root() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("first", "column"));
        let _ = tree.snapshot(node("second", "row"));
        assert_eq!(tree.root().unwrap().id, "second");
        assert_eq!(tree.root().unwrap().type_name, "row");
    }

    #[test]
    fn snapshot_preserves_children() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node("a", "text"), node("b", "button")],
        );
        let _ = tree.snapshot(root);
        assert_eq!(tree.root().unwrap().children.len(), 2);
        assert_eq!(tree.root().unwrap().children[0].id, "a");
        assert_eq!(tree.root().unwrap().children[1].id, "b");
    }

    // -----------------------------------------------------------------------
    // find_window
    // -----------------------------------------------------------------------

    #[test]
    fn find_window_at_root() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("main", "window"));
        let found = tree.find_window("main");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "main");
        assert_eq!(found.unwrap().type_name, "window");
    }

    #[test]
    fn find_window_root_wrong_id() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("main", "window"));
        assert!(tree.find_window("other").is_none());
    }

    #[test]
    fn find_window_in_children() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node("win1", "window"), node("win2", "window")],
        );
        let _ = tree.snapshot(root);
        assert!(tree.find_window("win1").is_some());
        assert!(tree.find_window("win2").is_some());
        assert_eq!(tree.find_window("win1").unwrap().id, "win1");
    }

    #[test]
    fn find_window_not_found() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("root", "column"));
        assert!(tree.find_window("nope").is_none());
    }

    #[test]
    fn find_window_on_empty_tree() {
        let tree = Tree::new();
        assert!(tree.find_window("anything").is_none());
    }

    #[test]
    fn find_window_ignores_non_window_children() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![
                node("btn", "button"),
                node("win", "window"),
                node("txt", "text"),
            ],
        );
        let _ = tree.snapshot(root);
        assert!(tree.find_window("btn").is_none());
        assert!(tree.find_window("txt").is_none());
        assert!(tree.find_window("win").is_some());
    }

    #[test]
    fn find_window_searches_grandchildren() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node_with_children(
                "inner",
                "row",
                vec![node("deep_win", "window")],
            )],
        );
        let _ = tree.snapshot(root);
        let found = tree.find_window("deep_win");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "deep_win");
    }

    #[test]
    fn find_window_deeply_nested() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node_with_children(
                "l1",
                "row",
                vec![node_with_children(
                    "l2",
                    "column",
                    vec![node_with_children(
                        "l3",
                        "row",
                        vec![node("buried_win", "window")],
                    )],
                )],
            )],
        );
        let _ = tree.snapshot(root);
        let found = tree.find_window("buried_win");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "buried_win");
    }

    #[test]
    fn window_ids_finds_nested_windows() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![
                node("w1", "window"),
                node_with_children("inner", "row", vec![node("w2", "window")]),
            ],
        );
        let _ = tree.snapshot(root);
        let ids = tree.window_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"w1".to_string()));
        assert!(ids.contains(&"w2".to_string()));
    }

    // -----------------------------------------------------------------------
    // window_ids
    // -----------------------------------------------------------------------

    #[test]
    fn window_ids_when_root_is_window() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("main", "window"));
        let ids = tree.window_ids();
        assert_eq!(ids, vec!["main".to_string()]);
    }

    #[test]
    fn window_ids_collects_child_windows() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![
                node("w1", "window"),
                node("w2", "window"),
                node("w3", "window"),
            ],
        );
        let _ = tree.snapshot(root);
        let ids = tree.window_ids();
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"w1".to_string()));
        assert!(ids.contains(&"w2".to_string()));
        assert!(ids.contains(&"w3".to_string()));
    }

    #[test]
    fn window_ids_skips_non_windows() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![
                node("w1", "window"),
                node("btn", "button"),
                node("w2", "window"),
            ],
        );
        let _ = tree.snapshot(root);
        let ids = tree.window_ids();
        assert_eq!(ids.len(), 2);
        assert!(!ids.contains(&"btn".to_string()));
    }

    #[test]
    fn window_ids_empty_when_no_windows() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("root", "column"));
        assert!(tree.window_ids().is_empty());
    }

    #[test]
    fn window_ids_empty_on_empty_tree() {
        let tree = Tree::new();
        assert!(tree.window_ids().is_empty());
    }

    // -----------------------------------------------------------------------
    // apply_patch -- replace_node
    // -----------------------------------------------------------------------

    #[test]
    fn patch_replace_root() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("old", "column"));
        let op = make_patch_op(
            "replace_node",
            vec![],
            json!({
                "node": {"id": "new", "type": "row", "props": {}, "children": []}
            }),
        );
        tree.apply_patch(vec![op]);
        assert_eq!(tree.root().unwrap().id, "new");
        assert_eq!(tree.root().unwrap().type_name, "row");
    }

    #[test]
    fn patch_replace_child() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node("a", "text"), node("b", "button")],
        );
        let _ = tree.snapshot(root);
        let op = make_patch_op(
            "replace_node",
            vec![1],
            json!({
                "node": {"id": "c", "type": "text", "props": {"content": "replaced"}, "children": []}
            }),
        );
        tree.apply_patch(vec![op]);
        assert_eq!(tree.root().unwrap().children[1].id, "c");
        assert_eq!(
            tree.root().unwrap().children[1].props["content"],
            "replaced"
        );
    }

    #[test]
    fn patch_replace_nested_child() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node_with_children(
                "row",
                "row",
                vec![node("inner", "text")],
            )],
        );
        let _ = tree.snapshot(root);
        let op = make_patch_op(
            "replace_node",
            vec![0, 0],
            json!({
                "node": {"id": "replaced", "type": "button", "props": {}, "children": []}
            }),
        );
        tree.apply_patch(vec![op]);
        assert_eq!(tree.root().unwrap().children[0].children[0].id, "replaced");
        assert_eq!(
            tree.root().unwrap().children[0].children[0].type_name,
            "button"
        );
    }

    #[test]
    fn patch_replace_out_of_bounds_does_not_panic() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("root", "column"));
        let op = make_patch_op(
            "replace_node",
            vec![5],
            json!({
                "node": {"id": "x", "type": "text", "props": {}, "children": []}
            }),
        );
        // Should print an error to stderr but not panic
        tree.apply_patch(vec![op]);
        // Root is unchanged
        assert_eq!(tree.root().unwrap().id, "root");
    }

    // -----------------------------------------------------------------------
    // apply_patch -- update_props
    // -----------------------------------------------------------------------

    #[test]
    fn patch_update_props_on_root() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node_with_props("root", "column", json!({"spacing": 5})));
        let op = make_patch_op(
            "update_props",
            vec![],
            json!({
                "props": {"spacing": 10, "padding": 20}
            }),
        );
        tree.apply_patch(vec![op]);
        assert_eq!(tree.root().unwrap().props["spacing"], 10);
        assert_eq!(tree.root().unwrap().props["padding"], 20);
    }

    #[test]
    fn patch_update_props_removes_null_keys() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node_with_props(
            "root",
            "text",
            json!({"content": "hi", "size": 14}),
        ));
        let op = make_patch_op(
            "update_props",
            vec![],
            json!({
                "props": {"size": null}
            }),
        );
        tree.apply_patch(vec![op]);
        assert_eq!(tree.root().unwrap().props["content"], "hi");
        assert!(tree.root().unwrap().props.get("size").is_none());
    }

    #[test]
    fn patch_update_props_on_child() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node_with_props("txt", "text", json!({"content": "old"}))],
        );
        let _ = tree.snapshot(root);
        let op = make_patch_op(
            "update_props",
            vec![0],
            json!({
                "props": {"content": "new"}
            }),
        );
        tree.apply_patch(vec![op]);
        assert_eq!(tree.root().unwrap().children[0].props["content"], "new");
    }

    #[test]
    fn patch_update_props_non_object_target_props_does_not_panic() {
        let mut tree = Tree::new();
        // Target has a non-object props value (a string)
        let _ = tree.snapshot(node_with_props("root", "text", json!("not an object")));
        let op = make_patch_op(
            "update_props",
            vec![],
            json!({
                "props": {"content": "new"}
            }),
        );
        tree.apply_patch(vec![op]);
        // Props unchanged -- the merge was skipped
        assert_eq!(tree.root().unwrap().props, json!("not an object"));
    }

    #[test]
    fn patch_update_props_non_object_patch_props_does_not_panic() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node_with_props("root", "text", json!({"content": "hi"})));
        // Patch props is a string, not an object
        let op = make_patch_op(
            "update_props",
            vec![],
            json!({
                "props": "not an object"
            }),
        );
        tree.apply_patch(vec![op]);
        // Props unchanged -- the merge was skipped
        assert_eq!(tree.root().unwrap().props["content"], "hi");
    }

    // -----------------------------------------------------------------------
    // apply_patch -- insert_child
    // -----------------------------------------------------------------------

    #[test]
    fn patch_insert_child_at_beginning() {
        let mut tree = Tree::new();
        let root = node_with_children("root", "column", vec![node("a", "text")]);
        let _ = tree.snapshot(root);
        let op = make_patch_op(
            "insert_child",
            vec![],
            json!({
                "index": 0,
                "node": {"id": "b", "type": "button", "props": {}, "children": []}
            }),
        );
        tree.apply_patch(vec![op]);
        assert_eq!(tree.root().unwrap().children.len(), 2);
        assert_eq!(tree.root().unwrap().children[0].id, "b");
        assert_eq!(tree.root().unwrap().children[1].id, "a");
    }

    #[test]
    fn patch_insert_child_at_end() {
        let mut tree = Tree::new();
        let root = node_with_children("root", "column", vec![node("a", "text")]);
        let _ = tree.snapshot(root);
        let op = make_patch_op(
            "insert_child",
            vec![],
            json!({
                "index": 1,
                "node": {"id": "b", "type": "button", "props": {}, "children": []}
            }),
        );
        tree.apply_patch(vec![op]);
        assert_eq!(tree.root().unwrap().children.len(), 2);
        assert_eq!(tree.root().unwrap().children[1].id, "b");
    }

    #[test]
    fn patch_insert_child_beyond_length_appends() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("root", "column"));
        let op = make_patch_op(
            "insert_child",
            vec![],
            json!({
                "index": 99,
                "node": {"id": "x", "type": "text", "props": {}, "children": []}
            }),
        );
        tree.apply_patch(vec![op]);
        assert_eq!(tree.root().unwrap().children.len(), 1);
        assert_eq!(tree.root().unwrap().children[0].id, "x");
    }

    #[test]
    fn patch_insert_child_into_nested_parent() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node_with_children(
                "row",
                "row",
                vec![node("existing", "text")],
            )],
        );
        let _ = tree.snapshot(root);
        let op = make_patch_op(
            "insert_child",
            vec![0],
            json!({
                "index": 0,
                "node": {"id": "new", "type": "button", "props": {}, "children": []}
            }),
        );
        tree.apply_patch(vec![op]);
        let row = &tree.root().unwrap().children[0];
        assert_eq!(row.children.len(), 2);
        assert_eq!(row.children[0].id, "new");
        assert_eq!(row.children[1].id, "existing");
    }

    // -----------------------------------------------------------------------
    // apply_patch -- remove_child
    // -----------------------------------------------------------------------

    #[test]
    fn patch_remove_child() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node("a", "text"), node("b", "button"), node("c", "text")],
        );
        let _ = tree.snapshot(root);
        let op = make_patch_op("remove_child", vec![], json!({"index": 1}));
        tree.apply_patch(vec![op]);
        assert_eq!(tree.root().unwrap().children.len(), 2);
        assert_eq!(tree.root().unwrap().children[0].id, "a");
        assert_eq!(tree.root().unwrap().children[1].id, "c");
    }

    #[test]
    fn patch_remove_child_first() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node("a", "text"), node("b", "button")],
        );
        let _ = tree.snapshot(root);
        let op = make_patch_op("remove_child", vec![], json!({"index": 0}));
        tree.apply_patch(vec![op]);
        assert_eq!(tree.root().unwrap().children.len(), 1);
        assert_eq!(tree.root().unwrap().children[0].id, "b");
    }

    #[test]
    fn patch_remove_child_last() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node("a", "text"), node("b", "button")],
        );
        let _ = tree.snapshot(root);
        let op = make_patch_op("remove_child", vec![], json!({"index": 1}));
        tree.apply_patch(vec![op]);
        assert_eq!(tree.root().unwrap().children.len(), 1);
        assert_eq!(tree.root().unwrap().children[0].id, "a");
    }

    #[test]
    fn patch_remove_child_out_of_bounds_does_not_panic() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("root", "column"));
        let op = make_patch_op("remove_child", vec![], json!({"index": 0}));
        // Should log error, not panic
        tree.apply_patch(vec![op]);
        assert!(tree.root().unwrap().children.is_empty());
    }

    // -----------------------------------------------------------------------
    // apply_patch -- unknown op
    // -----------------------------------------------------------------------

    #[test]
    fn patch_unknown_op_does_not_panic() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("root", "column"));
        let op = make_patch_op("frobnicate", vec![], json!({}));
        tree.apply_patch(vec![op]);
        // Tree should be unchanged
        assert_eq!(tree.root().unwrap().id, "root");
    }

    // -----------------------------------------------------------------------
    // apply_patch -- multiple ops in sequence
    // -----------------------------------------------------------------------

    #[test]
    fn patch_multiple_ops_applied_in_order() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("root", "column"));

        let ops = vec![
            make_patch_op(
                "insert_child",
                vec![],
                json!({
                    "index": 0,
                    "node": {"id": "a", "type": "text", "props": {}, "children": []}
                }),
            ),
            make_patch_op(
                "insert_child",
                vec![],
                json!({
                    "index": 1,
                    "node": {"id": "b", "type": "text", "props": {}, "children": []}
                }),
            ),
            make_patch_op(
                "insert_child",
                vec![],
                json!({
                    "index": 1,
                    "node": {"id": "c", "type": "text", "props": {}, "children": []}
                }),
            ),
        ];
        tree.apply_patch(ops);
        let children = &tree.root().unwrap().children;
        assert_eq!(children.len(), 3);
        assert_eq!(children[0].id, "a");
        assert_eq!(children[1].id, "c");
        assert_eq!(children[2].id, "b");
    }

    // -----------------------------------------------------------------------
    // apply_patch on empty tree
    // -----------------------------------------------------------------------

    #[test]
    fn patch_on_empty_tree_does_not_panic() {
        let mut tree = Tree::new();
        let op = make_patch_op(
            "replace_node",
            vec![],
            json!({
                "node": {"id": "x", "type": "text", "props": {}, "children": []}
            }),
        );
        tree.apply_patch(vec![op]);
        // Still empty -- the op should fail gracefully
        assert!(tree.root().is_none());
    }

    // -----------------------------------------------------------------------
    // navigate_mut edge cases (tested indirectly through patch ops)
    // -----------------------------------------------------------------------

    #[test]
    fn patch_deep_path_navigation() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node_with_children(
                "r0",
                "row",
                vec![node_with_children(
                    "r0c0",
                    "column",
                    vec![node("deep", "text")],
                )],
            )],
        );
        let _ = tree.snapshot(root);
        let op = make_patch_op(
            "update_props",
            vec![0, 0, 0],
            json!({
                "props": {"content": "updated deep"}
            }),
        );
        tree.apply_patch(vec![op]);
        let deep = &tree.root().unwrap().children[0].children[0].children[0];
        assert_eq!(deep.props["content"], "updated deep");
    }

    #[test]
    fn patch_invalid_path_does_not_panic() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("root", "column"));
        let op = make_patch_op(
            "update_props",
            vec![0, 1, 2],
            json!({
                "props": {"x": 1}
            }),
        );
        tree.apply_patch(vec![op]);
        // Root unchanged
        assert_eq!(tree.root().unwrap().id, "root");
    }

    // -----------------------------------------------------------------------
    // Malformed patch operations (error paths)
    // -----------------------------------------------------------------------

    #[test]
    fn patch_replace_node_missing_node_field_does_not_panic() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("root", "column"));
        // replace_node without the required "node" field
        let op = make_patch_op("replace_node", vec![], json!({}));
        tree.apply_patch(vec![op]);
        // Tree should be unchanged
        assert_eq!(tree.root().unwrap().id, "root");
    }

    #[test]
    fn patch_replace_node_invalid_node_json_does_not_panic() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("root", "column"));
        // "node" is present but not a valid TreeNode (missing required fields)
        let op = make_patch_op("replace_node", vec![], json!({"node": {"garbage": true}}));
        tree.apply_patch(vec![op]);
        assert_eq!(tree.root().unwrap().id, "root");
    }

    #[test]
    fn patch_update_props_missing_props_field_does_not_panic() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node_with_props("root", "text", json!({"content": "hi"})));
        let op = make_patch_op("update_props", vec![], json!({}));
        tree.apply_patch(vec![op]);
        // Props unchanged -- the missing "props" field is handled gracefully
        assert_eq!(tree.root().unwrap().props["content"], "hi");
    }

    #[test]
    fn patch_insert_child_missing_index_does_not_panic() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("root", "column"));
        let op = make_patch_op(
            "insert_child",
            vec![],
            json!({
                "node": {"id": "x", "type": "text", "props": {}, "children": []}
            }),
        );
        tree.apply_patch(vec![op]);
        // No child inserted because index is missing
        assert!(tree.root().unwrap().children.is_empty());
    }

    #[test]
    fn patch_insert_child_missing_node_does_not_panic() {
        let mut tree = Tree::new();
        let _ = tree.snapshot(node("root", "column"));
        let op = make_patch_op("insert_child", vec![], json!({"index": 0}));
        tree.apply_patch(vec![op]);
        assert!(tree.root().unwrap().children.is_empty());
    }

    #[test]
    fn patch_remove_child_missing_index_does_not_panic() {
        let mut tree = Tree::new();
        let root = node_with_children("root", "column", vec![node("a", "text")]);
        let _ = tree.snapshot(root);
        let op = make_patch_op("remove_child", vec![], json!({}));
        tree.apply_patch(vec![op]);
        // Child should still be present -- the op failed gracefully
        assert_eq!(tree.root().unwrap().children.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Multi-op patch tests
    // -----------------------------------------------------------------------

    #[test]
    fn patch_multi_op_mixed_types() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![
                node_with_props("a", "text", json!({"content": "hello"})),
                node("b", "button"),
            ],
        );
        let _ = tree.snapshot(root);

        let ops = vec![
            // Insert a third child at index 2
            make_patch_op(
                "insert_child",
                vec![],
                json!({
                    "index": 2,
                    "node": {"id": "c", "type": "text", "props": {"content": "new"}, "children": []}
                }),
            ),
            // Remove the first child (index 0 = "a")
            make_patch_op("remove_child", vec![], json!({"index": 0})),
            // Update props on current index 0 (was "b", now shifted to front)
            make_patch_op(
                "update_props",
                vec![0],
                json!({"props": {"label": "updated"}}),
            ),
        ];
        tree.apply_patch(ops);

        let children = &tree.root().unwrap().children;
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].id, "b");
        assert_eq!(children[0].props["label"], "updated");
        assert_eq!(children[1].id, "c");
    }

    #[test]
    fn patch_remove_shifts_indices() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![
                node("first", "text"),
                node("second", "button"),
                node("third", "text"),
            ],
        );
        let _ = tree.snapshot(root);

        let ops = vec![
            // Remove child at index 0 ("first")
            make_patch_op("remove_child", vec![], json!({"index": 0})),
            // Replace node at index 0, which is now "second" after removal
            make_patch_op(
                "replace_node",
                vec![0],
                json!({
                    "node": {"id": "replaced", "type": "row", "props": {}, "children": []}
                }),
            ),
        ];
        tree.apply_patch(ops);

        let children = &tree.root().unwrap().children;
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].id, "replaced");
        assert_eq!(children[0].type_name, "row");
        assert_eq!(children[1].id, "third");
    }

    #[test]
    fn patch_bad_middle_op_continues() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![
                node_with_props("a", "text", json!({"content": "original"})),
                node("b", "button"),
            ],
        );
        let _ = tree.snapshot(root);

        let ops = vec![
            // First op: update props on child "a" (valid)
            make_patch_op(
                "update_props",
                vec![0],
                json!({"props": {"content": "changed"}}),
            ),
            // Second op: invalid path (out of bounds)
            make_patch_op(
                "update_props",
                vec![99, 0],
                json!({"props": {"content": "nope"}}),
            ),
            // Third op: update props on child "b" (valid)
            make_patch_op(
                "update_props",
                vec![1],
                json!({"props": {"label": "click me"}}),
            ),
        ];
        tree.apply_patch(ops);

        let children = &tree.root().unwrap().children;
        assert_eq!(children[0].props["content"], "changed");
        assert_eq!(children[1].props["label"], "click me");
    }

    // -----------------------------------------------------------------------
    // validate_unique_ids / snapshot duplicate detection
    // -----------------------------------------------------------------------

    #[test]
    fn snapshot_unique_ids_returns_ok() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node("a", "text"), node("b", "button")],
        );
        assert!(tree.snapshot(root).is_ok());
    }

    #[test]
    fn snapshot_duplicate_ids_returns_err() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node("dupe", "text"), node("dupe", "button")],
        );
        let result = tree.snapshot(root);
        assert!(result.is_err());
        let dupes = result.unwrap_err();
        assert_eq!(dupes.len(), 1);
        assert!(dupes[0].contains("dupe"));
    }

    #[test]
    fn snapshot_duplicate_ids_still_accepts_tree() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![node("dupe", "text"), node("dupe", "button")],
        );
        let _ = tree.snapshot(root);
        // Tree was still accepted despite duplicates
        assert!(tree.root().is_some());
        assert_eq!(tree.root().unwrap().children.len(), 2);
    }

    #[test]
    fn snapshot_multiple_duplicate_ids() {
        let mut tree = Tree::new();
        let root = node_with_children(
            "root",
            "column",
            vec![
                node("a", "text"),
                node("a", "text"),
                node("b", "button"),
                node("b", "button"),
            ],
        );
        let result = tree.snapshot(root);
        assert!(result.is_err());
        let dupes = result.unwrap_err();
        assert_eq!(dupes.len(), 2);
    }

    #[test]
    fn snapshot_empty_ids_are_ignored() {
        let mut tree = Tree::new();
        let root = node_with_children("root", "column", vec![node("", "text"), node("", "text")]);
        // Empty IDs should not be flagged as duplicates
        assert!(tree.snapshot(root).is_ok());
    }

    #[test]
    fn find_window_returns_none_beyond_max_depth() {
        // Build a chain deeper than MAX_TREE_DEPTH (256).
        let mut deepest = node("deep_win", "window");
        for i in 0..MAX_TREE_DEPTH + 10 {
            deepest = node_with_children(&format!("n{i}"), "column", vec![deepest]);
        }
        let mut tree = Tree::new();
        let _ = tree.snapshot(deepest);

        // The window at depth > 256 should not be found.
        assert!(
            tree.find_window("deep_win").is_none(),
            "window beyond MAX_TREE_DEPTH should not be reachable"
        );
    }

    #[test]
    fn window_ids_skips_windows_beyond_max_depth() {
        let mut deepest = node("deep_win", "window");
        for i in 0..MAX_TREE_DEPTH + 10 {
            deepest = node_with_children(&format!("n{i}"), "column", vec![deepest]);
        }
        let mut tree = Tree::new();
        let _ = tree.snapshot(deepest);

        let ids = tree.window_ids();
        assert!(
            !ids.contains(&"deep_win".to_string()),
            "window beyond MAX_TREE_DEPTH should not appear in window_ids"
        );
    }
}
