//! Layer tree operations: groups, parent/child relationships, and the DFS
//! pre-order covenant.
//!
//! [`LayerStack::layers`] stays a flat `Vec<Layer>` kept in DFS pre-order: a
//! group's descendants occupy a contiguous subrange immediately after the
//! group, in child order, before the group's next sibling. Every operation in
//! this module preserves that invariant, so sibling moves remain plain index
//! moves inside a parent's block.

use std::collections::{HashMap, HashSet};

use crate::core::buffer::PixelBuffer;

use super::{BlendMode, Layer, LayerId, LayerStack};

/// A subtree detached from a [`LayerStack`], ready to be re-attached.
///
/// `nodes` is in DFS pre-order; each entry's `Option` is that node's parent
/// id *within the subtree* (`None` for the subtree root). `parent` and
/// `child_index` record where the root used to live so
/// [`LayerStack::attach_subtree`] can restore it exactly.
#[derive(Clone, Debug, PartialEq)]
pub struct DetachedSubtree {
    pub nodes: Vec<(Layer, Option<LayerId>)>,
    pub parent: Option<LayerId>,
    pub child_index: usize,
}

impl LayerStack {
    /// Returns true when `id` references a group layer.
    pub fn is_group(&self, id: LayerId) -> bool {
        self.layer(id).is_some_and(|layer| layer.is_group)
    }

    /// The parent of `id`, or `None` for a root-level layer.
    pub fn parent_of(&self, id: LayerId) -> Option<LayerId> {
        self.layer(id).and_then(|layer| layer.parent)
    }

    /// The children of `parent` in child order. `None` yields the root-level
    /// layers.
    pub fn children_of(&self, parent: Option<LayerId>) -> Vec<LayerId> {
        self.layers
            .iter()
            .filter(|layer| layer.parent == parent)
            .map(|layer| layer.id)
            .collect()
    }

    /// Depth of `id` in the tree; roots are depth 0.
    pub fn depth_of(&self, id: LayerId) -> usize {
        let mut depth = 0;
        let mut current = id;
        while let Some(parent) = self.parent_of(current) {
            depth += 1;
            current = parent;
        }
        depth
    }

    /// True when `ancestor` is a strict ancestor of `node`.
    pub fn is_ancestor(&self, ancestor: LayerId, node: LayerId) -> bool {
        let mut current = node;
        while let Some(parent) = self.parent_of(current) {
            if parent == ancestor {
                return true;
            }
            current = parent;
        }
        false
    }

    /// Every node in the subtree rooted at `root`, DFS pre-order, including
    /// `root` itself.
    pub fn subtree_ids(&self, root: LayerId) -> Vec<LayerId> {
        let mut out = Vec::new();
        self.collect_subtree(root, &mut out);
        out
    }

    fn collect_subtree(&self, root: LayerId, out: &mut Vec<LayerId>) {
        out.push(root);
        for child in self.children_of(Some(root)) {
            self.collect_subtree(child, out);
        }
    }

    /// Every node in the stack, DFS pre-order.
    pub fn depth_first(&self) -> Vec<LayerId> {
        let mut out = Vec::with_capacity(self.layers.len());
        for root in self.children_of(None) {
            self.collect_subtree(root, &mut out);
        }
        out
    }

    /// The current tree structure in DFS pre-order: `(layer id, that layer's parent)`.
    /// Every layer appears exactly once; a group's descendants immediately follow it.
    pub fn structure(&self) -> Vec<(LayerId, Option<LayerId>)> {
        self.layers
            .iter()
            .map(|layer| (layer.id, layer.parent))
            .collect()
    }

    /// True when the stack contains at least one group layer.
    pub fn has_groups(&self) -> bool {
        self.layers.iter().any(|layer| layer.is_group)
    }

    /// Appends a new empty root-level group and makes it active.
    pub fn add_group(&mut self, name: &str) -> LayerId {
        let id = LayerId(self.next_id);
        self.next_id = match self.next_id.checked_add(1) {
            Some(next_id) => next_id,
            None => panic!("layer id space exhausted"),
        };
        self.layers.push(self.new_group_layer(id, name));
        self.active = id;
        self.changed = true;
        id
    }

    fn new_group_layer(&self, id: LayerId, name: &str) -> Layer {
        Layer {
            id,
            name: name.to_string(),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            buffer: PixelBuffer::new(self.width, self.height),
            parent: None,
            is_group: true,
            tilemap: None,
            locked: false,
            tilemap_cache: std::cell::RefCell::new(None),
        }
    }

    /// Wraps `ids` in a new group inserted at the lowest index among them.
    ///
    /// Each id is reparented into the group as a direct child, in its existing
    /// relative order (the order the ids appear in the stack). Returns `None`
    /// when `ids` is empty or any id is unknown.
    pub fn create_group_around(&mut self, ids: &[LayerId], name: &str) -> Option<LayerId> {
        if ids.is_empty() {
            return None;
        }
        let mut entries: Vec<(LayerId, usize)> = Vec::with_capacity(ids.len());
        for id in ids {
            entries.push((*id, self.position_of(*id)?));
        }
        entries.sort_by_key(|(_, pos)| *pos);
        let min_pos = entries[0].1;

        let group_id = LayerId(self.next_id);
        self.next_id = self.next_id.checked_add(1)?;
        let group = self.new_group_layer(group_id, name);
        self.layers.insert(min_pos, group);

        let mut insert_at = min_pos + 1;
        for (id, _) in entries {
            let pos = self.position_of(id)?;
            let end = self.subtree_end(id);
            let mut subtree: Vec<Layer> = self.layers.drain(pos..end).collect();
            if pos < insert_at {
                insert_at -= end - pos;
            }
            subtree[0].parent = Some(group_id);
            let len = subtree.len();
            self.layers.splice(insert_at..insert_at, subtree);
            insert_at += len;
        }
        self.active = group_id;
        self.changed = true;
        Some(group_id)
    }

    /// Removes `group`, promoting its children into the group's parent at the
    /// group's position. Returns false when `group` is unknown or not a group.
    pub fn ungroup(&mut self, group: LayerId) -> bool {
        let Some(pos) = self.position_of(group) else {
            return false;
        };
        if !self.is_group(group) {
            return false;
        }
        let parent = self.parent_of(group);
        let end = self.subtree_end(group);
        let mut subtree: Vec<Layer> = self.layers.drain(pos..end).collect();
        subtree.remove(0); // the group itself
        for layer in &mut subtree {
            if layer.parent == Some(group) {
                layer.parent = parent;
            }
        }
        self.layers.splice(pos..pos, subtree);
        self.changed = true;
        true
    }

    /// Moves `id`'s whole subtree to become the `child_index`-th child of
    /// `new_parent` (clamped to the parent's current child count). Cycle-safe:
    /// rejects when `new_parent` is `id` itself or a descendant of `id`.
    pub fn set_parent_and_position(
        &mut self,
        id: LayerId,
        new_parent: Option<LayerId>,
        child_index: usize,
    ) -> bool {
        let Some(pos) = self.position_of(id) else {
            return false;
        };
        if let Some(parent) = new_parent {
            if self.layer(parent).is_none() {
                return false;
            }
        }
        if new_parent == Some(id) || new_parent.is_some_and(|p| self.is_ancestor(id, p)) {
            return false;
        }

        let end = self.subtree_end(id);
        let mut subtree: Vec<Layer> = self.layers.drain(pos..end).collect();

        // `insert_position_for` runs AFTER the drain, so `child_index` is
        // already in post-removal coordinates; shifting it again would move a
        // layer dragged upward back to where it started.
        let insert_at = self.insert_position_for(new_parent, child_index);

        subtree[0].parent = new_parent;
        self.layers.splice(insert_at..insert_at, subtree);
        self.changed = true;
        true
    }

    /// Detaches the subtree rooted at `root`, returning it with enough
    /// context to re-attach it exactly.
    pub fn detach_subtree(&mut self, root: LayerId) -> Option<DetachedSubtree> {
        let pos = self.position_of(root)?;
        let end = self.subtree_end(root);
        let parent = self.parent_of(root);
        let child_index = self
            .children_of(parent)
            .iter()
            .position(|child| *child == root)?;
        let mut nodes: Vec<(Layer, Option<LayerId>)> = self
            .layers
            .drain(pos..end)
            .map(|layer| {
                let parent = layer.parent;
                (layer, parent)
            })
            .collect();
        nodes[0].1 = None;
        self.changed = true;
        Some(DetachedSubtree {
            nodes,
            parent,
            child_index,
        })
    }

    /// Re-attaches a subtree previously detached with
    /// [`LayerStack::detach_subtree`], restoring it to its former parent and
    /// child index. Returns false when any id collides with an existing layer
    /// or the former parent is unknown.
    pub fn attach_subtree(&mut self, subtree: DetachedSubtree) -> bool {
        let DetachedSubtree {
            nodes,
            parent,
            child_index,
        } = subtree;
        if nodes.is_empty() {
            return false;
        }
        for (layer, _) in &nodes {
            if self.layer(layer.id).is_some() {
                return false;
            }
        }
        if let Some(parent) = parent {
            if self.layer(parent).is_none() {
                return false;
            }
        }
        let insert_at = self.insert_position_for(parent, child_index);
        let mut layers: Vec<Layer> = nodes
            .into_iter()
            .map(|(mut layer, parent)| {
                layer.parent = parent;
                layer
            })
            .collect();
        layers[0].parent = parent;
        self.layers.splice(insert_at..insert_at, layers);
        self.changed = true;
        true
    }

    /// Wholesale structural restore used by undo/redo.
    ///
    /// Reorders the EXISTING layers into the given DFS pre-order and re-applies each layer's
    /// `parent` link. `entries` must contain every current layer id exactly once, every
    /// `parent` must resolve to an id inside `entries`, and the parent graph must be acyclic
    /// (a parent must appear before its children in the given order, matching DFS pre-order).
    ///
    /// Pixel buffers are untouched (no clone, no copy). Recomputes `next_id` as `max(id) + 1`.
    /// Keeps `active` if it still exists; otherwise selects the LAST root-level layer.
    /// Marks the stack changed on success.
    ///
    /// Returns `false` and leaves the stack COMPLETELY UNCHANGED when validation fails.
    pub fn restore_structure(&mut self, entries: &[(LayerId, Option<LayerId>)]) -> bool {
        if entries.len() != self.layers.len() {
            return false;
        }

        // Rule 1: every current layer id appears exactly once.
        let positions: HashMap<LayerId, usize> = self
            .layers
            .iter()
            .enumerate()
            .map(|(index, layer)| (layer.id, index))
            .collect();
        let mut seen = vec![false; self.layers.len()];
        for (id, _) in entries {
            let Some(&pos) = positions.get(id) else {
                return false; // unknown id
            };
            if seen[pos] {
                return false; // duplicate id
            }
            seen[pos] = true;
        }

        // Rules 2 & 3: a parent must differ from its child, resolve to an id
        // inside `entries`, and appear before its child (which also rules out
        // cycles).
        let mut appeared = HashSet::with_capacity(entries.len());
        for (id, parent) in entries {
            if let Some(parent) = parent {
                if *parent == *id || !appeared.contains(parent) {
                    return false;
                }
            }
            appeared.insert(*id);
        }

        // Recompute `next_id` before mutating so a failure leaves nothing touched.
        let next_id = match self.layers.iter().map(|l| l.id.as_u64()).max() {
            Some(max) => match max.checked_add(1) {
                Some(next) => next,
                None => return false,
            },
            None => 0,
        };

        // Rebuild `layers` in the requested order, moving each existing `Layer`
        // (pixel buffer included) rather than cloning it.
        let mut by_id: HashMap<LayerId, Layer> = std::mem::take(&mut self.layers)
            .into_iter()
            .map(|layer| (layer.id, layer))
            .collect();
        let mut rebuilt = Vec::with_capacity(entries.len());
        for (id, parent) in entries {
            let mut layer = by_id
                .remove(id)
                .expect("validated: every id appears exactly once");
            layer.parent = *parent;
            rebuilt.push(layer);
        }
        self.layers = rebuilt;
        self.next_id = next_id;

        // Rule 1 guarantees the active id is still present; the fallback guards
        // the unreachable case where it is not.
        if !self.layers.iter().any(|l| l.id == self.active) {
            self.active = self
                .layers
                .iter()
                .rev()
                .find(|l| l.parent.is_none())
                .map(|l| l.id)
                .unwrap_or(self.active);
        }
        self.changed = true;
        true
    }

    /// Overwrites the layer with `layer.id` in place, preserving its id.
    /// Returns false when the id is unknown.
    pub fn replace_layer(&mut self, layer: Layer) -> bool {
        let Some(existing) = self.layers.iter_mut().find(|l| l.id == layer.id) else {
            return false;
        };
        *existing = layer;
        self.changed = true;
        true
    }

    /// Index of `id` in the flat `layers` vec.
    pub(super) fn position_of(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|layer| layer.id == id)
    }

    /// Insertion index for a new `child_index`-th child of `parent` (clamped
    /// to the parent's current child count).
    fn insert_position_for(&self, parent: Option<LayerId>, child_index: usize) -> usize {
        let children = self.children_of(parent);
        let child_index = child_index.min(children.len());
        if child_index == 0 {
            match parent {
                Some(parent) => self.position_of(parent).unwrap() + 1,
                None => 0,
            }
        } else {
            self.subtree_end(children[child_index - 1])
        }
    }

    /// Exclusive end index of the subtree rooted at `id`. Relies on the DFS
    /// pre-order covenant: the subtree is a contiguous range.
    pub(super) fn subtree_end(&self, id: LayerId) -> usize {
        let pos = self.position_of(id).expect("subtree root must exist");
        let root_depth = self.depth_of(id);
        let mut end = pos + 1;
        while end < self.layers.len() && self.depth_of(self.layers[end].id) > root_depth {
            end += 1;
        }
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::color::Color;
    use std::collections::HashMap;

    #[test]
    fn depth_first_is_dfs_preorder() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        let c = stack.add_layer("C");
        let g = stack.create_group_around(&[a, b], "G").unwrap();
        assert_eq!(stack.depth_first(), vec![root, g, a, b, c]);
    }

    #[test]
    fn children_of_returns_contiguous_subrange() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        let c = stack.add_layer("C");
        let g = stack.create_group_around(&[a, b], "G").unwrap();
        assert_eq!(stack.children_of(None), vec![root, g, c]);
        assert_eq!(stack.children_of(Some(g)), vec![a, b]);
        // The group's subtree is a contiguous range in the flat vec.
        let ids: Vec<LayerId> = stack.iter().map(|l| l.id).collect();
        assert_eq!(ids, vec![root, g, a, b, c]);
        assert_eq!(stack.subtree_ids(g), vec![g, a, b]);
    }

    #[test]
    fn sibling_reorder_inside_group_contiguous() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        let c = stack.add_layer("C");
        let g = stack.create_group_around(&[a, b, c], "G").unwrap();
        // [root, G, A, B, C]; move A (index 2) to index 4 (after C).
        assert!(stack.reorder(a, 4));
        let ids: Vec<LayerId> = stack.iter().map(|l| l.id).collect();
        assert_eq!(ids, vec![root, g, b, c, a]);
        assert_eq!(stack.children_of(Some(g)), vec![b, c, a]);
        // Contiguity preserved: the subtree of G is still one block.
        assert_eq!(stack.subtree_ids(g), vec![g, b, c, a]);
    }

    #[test]
    fn set_parent_rejects_cycle() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        let g = stack.create_group_around(&[a, b], "G").unwrap();
        // new_parent == id
        assert!(!stack.set_parent_and_position(g, Some(g), 0));
        // new_parent is a descendant of id (would create a cycle)
        assert!(!stack.set_parent_and_position(g, Some(a), 0));
        // unknown ids
        assert!(!stack.set_parent_and_position(LayerId::new(999), Some(g), 0));
        assert!(!stack.set_parent_and_position(a, Some(LayerId::new(999)), 0));
        // valid moves still work
        assert!(stack.set_parent_and_position(a, None, 0));
        assert!(stack.set_parent_and_position(g, None, 0));
        assert_eq!(stack.depth_first().len(), 4);
        assert_eq!(stack.layer(root).unwrap().id, root);
    }

    #[test]
    fn set_parent_and_position_moves_a_layer_up_past_a_lower_target() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        // [root, A, B]: A bottom, B top. Move A up past B (child index 2 in
        // post-removal coordinates: after B).
        assert!(stack.set_parent_and_position(a, None, 2));
        assert_eq!(stack.children_of(None), vec![root, b, a]);
        assert_eq!(stack.structure(), vec![(root, None), (b, None), (a, None)]);
    }

    #[test]
    fn set_parent_and_position_moves_a_layer_down_past_a_higher_target() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        // [root, A, B]: move B (top) down past A (child index 1 in
        // post-removal coordinates: after root, before A).
        assert!(stack.set_parent_and_position(b, None, 1));
        assert_eq!(stack.children_of(None), vec![root, b, a]);
        assert_eq!(stack.structure(), vec![(root, None), (b, None), (a, None)]);
    }

    #[test]
    fn set_parent_and_position_up_then_down_round_trips() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        assert!(stack.set_parent_and_position(a, None, 2));
        assert_eq!(stack.structure(), vec![(root, None), (b, None), (a, None)]);
        assert!(stack.set_parent_and_position(a, None, 1));
        assert_eq!(stack.structure(), vec![(root, None), (a, None), (b, None)]);
    }

    #[test]
    fn create_group_around_reparents_selected() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        let c = stack.add_layer("C");
        let g = stack.create_group_around(&[a, c], "G").unwrap();
        // Group inserted at the lowest index among the selected (a at index 1).
        let ids: Vec<LayerId> = stack.iter().map(|l| l.id).collect();
        assert_eq!(ids, vec![root, g, a, c, b]);
        assert_eq!(stack.children_of(Some(g)), vec![a, c]);
        assert_eq!(stack.parent_of(a), Some(g));
        assert_eq!(stack.parent_of(c), Some(g));
        assert_eq!(stack.parent_of(b), None);
        assert_eq!(stack.depth_of(a), 1);
        assert_eq!(stack.depth_of(root), 0);
        // Empty or unknown ids are rejected.
        assert!(stack.create_group_around(&[], "Empty").is_none());
        assert!(stack
            .create_group_around(&[LayerId::new(999)], "X")
            .is_none());
    }

    #[test]
    fn ungroup_promotes_children() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        let c = stack.add_layer("C");
        let g = stack.create_group_around(&[a, b], "G").unwrap();
        assert!(stack.ungroup(g));
        let ids: Vec<LayerId> = stack.iter().map(|l| l.id).collect();
        assert_eq!(ids, vec![root, a, b, c]);
        assert_eq!(stack.parent_of(a), None);
        assert_eq!(stack.parent_of(b), None);
        assert_eq!(stack.layer(g), None);
        // Ungrouping a non-group or unknown id fails.
        assert!(!stack.ungroup(a));
        assert!(!stack.ungroup(LayerId::new(999)));
    }

    #[test]
    fn replace_layer_overwrites_by_id_preserving_id() {
        let mut stack = LayerStack::new(2, 2);
        let id = stack.active_layer_id();
        let mut replacement = stack.layer(id).unwrap().clone();
        replacement.name = "Replaced".to_string();
        replacement.opacity = 0.25;
        replacement.buffer.fill(Color::rgb(9, 8, 7));
        assert!(stack.replace_layer(replacement.clone()));
        let layer = stack.layer(id).unwrap();
        assert_eq!(layer.id, id);
        assert_eq!(layer.name, "Replaced");
        assert_eq!(layer.opacity, 0.25);
        assert_eq!(layer.buffer.get_pixel(0, 0), Some(Color::rgb(9, 8, 7)));
        let mut unknown = replacement;
        unknown.id = LayerId::new(999);
        assert!(!stack.replace_layer(unknown));
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn detach_then_attach_restores_stack_byte_exact() {
        let mut stack = LayerStack::new(2, 2);
        let root = stack.active_layer_id();
        stack
            .layer_mut(root)
            .unwrap()
            .buffer
            .fill(Color::rgb(1, 2, 3));
        let a = stack.add_layer("A");
        stack.layer_mut(a).unwrap().buffer.fill(Color::rgb(4, 5, 6));
        let b = stack.add_layer("B");
        stack.layer_mut(b).unwrap().buffer.fill(Color::rgb(7, 8, 9));
        let g = stack.create_group_around(&[a, b], "G").unwrap();
        let before = stack
            .composite_layers(&crate::core::tilemap::TilePalette::new())
            .as_bytes()
            .to_vec();
        let ids_before: Vec<LayerId> = stack.iter().map(|l| l.id).collect();

        let detached = stack.detach_subtree(g).unwrap();
        assert_eq!(stack.layer(g), None);
        let ids_after_detach: Vec<LayerId> = stack.iter().map(|l| l.id).collect();
        assert_eq!(ids_after_detach, vec![root]);

        assert!(stack.attach_subtree(detached));
        let ids_after: Vec<LayerId> = stack.iter().map(|l| l.id).collect();
        assert_eq!(ids_after, ids_before);
        assert_eq!(
            stack
                .composite_layers(&crate::core::tilemap::TilePalette::new())
                .as_bytes(),
            before
        );
    }

    #[test]
    fn detach_single_layer_records_parent_and_index() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        let g = stack.create_group_around(&[a, b], "G").unwrap();
        let detached = stack.detach_subtree(a).unwrap();
        assert_eq!(detached.nodes.len(), 1);
        assert_eq!(detached.nodes[0].0.id, a);
        assert_eq!(detached.nodes[0].1, None);
        assert_eq!(detached.parent, Some(g));
        assert_eq!(detached.child_index, 0);
        assert!(stack.attach_subtree(detached));
        assert_eq!(stack.children_of(Some(g)), vec![a, b]);
        assert_eq!(stack.layer(root).unwrap().id, root);
    }

    #[test]
    fn add_group_creates_empty_root_group_and_activates() {
        let mut stack = LayerStack::new(2, 2);
        let g = stack.add_group("G");
        assert!(stack.is_group(g));
        assert_eq!(stack.parent_of(g), None);
        assert_eq!(stack.depth_of(g), 0);
        assert_eq!(stack.active_layer_id(), g);
        assert!(stack.has_groups());
        assert_eq!(stack.children_of(Some(g)), Vec::<LayerId>::new());
        // A group with no children composites as transparent.
        assert_eq!(
            stack
                .composite_layers(&crate::core::tilemap::TilePalette::new())
                .as_bytes(),
            vec![0; 2 * 2 * 4]
        );
    }

    #[test]
    fn ancestry_and_depth_queries() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        let g = stack.create_group_around(&[a, b], "G").unwrap();
        assert!(stack.is_ancestor(g, a));
        assert!(stack.is_ancestor(g, b));
        assert!(!stack.is_ancestor(a, g));
        assert!(!stack.is_ancestor(g, g));
        assert_eq!(stack.depth_of(root), 0);
        assert_eq!(stack.depth_of(g), 0);
        assert_eq!(stack.depth_of(a), 1);
        assert_eq!(stack.depth_of(b), 1);
    }

    #[test]
    fn structure_is_dfs_preorder_with_parents() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        let c = stack.add_layer("C");
        let g = stack.create_group_around(&[a, b], "G").unwrap();
        assert_eq!(
            stack.structure(),
            vec![
                (root, None),
                (g, None),
                (a, Some(g)),
                (b, Some(g)),
                (c, None),
            ]
        );
    }

    #[test]
    fn restore_structure_round_trips_flat_order_and_parents() {
        let mut stack = LayerStack::new(2, 2);
        let root = stack.active_layer_id();
        stack
            .layer_mut(root)
            .unwrap()
            .buffer
            .fill(Color::rgb(1, 2, 3));
        let a = stack.add_layer("A");
        stack.layer_mut(a).unwrap().buffer.fill(Color::rgb(4, 5, 6));
        let b = stack.add_layer("B");
        stack.layer_mut(b).unwrap().buffer.fill(Color::rgb(7, 8, 9));
        let c = stack.add_layer("C");
        stack
            .layer_mut(c)
            .unwrap()
            .buffer
            .fill(Color::rgb(10, 11, 12));
        let g = stack.create_group_around(&[a, b], "G").unwrap();

        let captured = stack.structure();
        let captured_bytes = stack
            .composite_layers(&crate::core::tilemap::TilePalette::new())
            .as_bytes()
            .to_vec();

        // Shuffle the stack with existing ops until it no longer matches.
        assert!(stack.set_parent_and_position(c, Some(g), 0));
        assert!(stack.reorder(root, stack.len() - 1));
        assert!(stack.set_parent_and_position(a, None, 0));
        assert_ne!(stack.structure(), captured);

        assert!(stack.restore_structure(&captured));
        assert_eq!(stack.structure(), captured);
        assert_eq!(
            stack
                .composite_layers(&crate::core::tilemap::TilePalette::new())
                .as_bytes(),
            captured_bytes
        );

        // `next_id` was recomputed: a fresh layer gets a brand-new id.
        let max_id = captured.iter().map(|(id, _)| id.as_u64()).max().unwrap();
        let fresh = stack.add_layer("Fresh");
        assert!(fresh.as_u64() > max_id);
    }

    #[test]
    fn restore_structure_keeps_buffers_untouched() {
        let mut stack = LayerStack::new(2, 2);
        let root = stack.active_layer_id();
        stack
            .layer_mut(root)
            .unwrap()
            .buffer
            .fill(Color::rgb(1, 2, 3));
        let a = stack.add_layer("A");
        stack.layer_mut(a).unwrap().buffer.fill(Color::rgb(4, 5, 6));
        let b = stack.add_layer("B");
        stack.layer_mut(b).unwrap().buffer.fill(Color::rgb(7, 8, 9));
        let g = stack.create_group_around(&[a, b], "G").unwrap();

        let structure = stack.structure();

        // Shuffle so the restore actually reorders layers.
        assert!(stack.set_parent_and_position(b, None, 0));

        // Mutate a pixel after capturing the structure. A restore that cloned
        // or replaced buffers would lose this mutation; a move-based restore
        // keeps it.
        stack
            .layer_mut(a)
            .unwrap()
            .buffer
            .fill(Color::rgb(99, 99, 99));
        let bytes_before: HashMap<LayerId, Vec<u8>> = stack
            .iter()
            .map(|layer| (layer.id, layer.buffer.as_bytes().to_vec()))
            .collect();

        assert!(stack.restore_structure(&structure));

        let bytes_after: HashMap<LayerId, Vec<u8>> = stack
            .iter()
            .map(|layer| (layer.id, layer.buffer.as_bytes().to_vec()))
            .collect();
        assert_eq!(bytes_after, bytes_before);
        // The mutated pixel survives: the Layer value (buffer included) was
        // moved, not cloned.
        assert_eq!(
            stack.layer(a).unwrap().buffer.as_bytes(),
            [99, 99, 99, 255].repeat(4)
        );
        // The group structure was restored too.
        assert_eq!(stack.children_of(Some(g)), vec![a, b]);
    }

    #[test]
    fn restore_structure_rejects_missing_or_duplicate_ids() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        let c = stack.add_layer("C");
        let g = stack.create_group_around(&[a, b], "G").unwrap();
        let before = stack.structure();
        stack.clear_changed();

        // Missing id: `a` is absent, replaced by an unknown id.
        assert!(!stack.restore_structure(&[
            (root, None),
            (g, None),
            (b, Some(g)),
            (c, None),
            (LayerId::new(999), None),
        ]));
        assert!(!stack.changed());
        // Duplicate id: `a` appears twice, `c` absent.
        assert!(!stack.restore_structure(&[
            (root, None),
            (g, None),
            (a, Some(g)),
            (a, Some(g)),
            (b, Some(g)),
        ]));
        assert!(!stack.changed());
        // Wrong length.
        assert!(!stack.restore_structure(&[(root, None)]));
        assert!(!stack.changed());

        assert_eq!(stack.structure(), before);
    }

    #[test]
    fn restore_structure_rejects_unknown_parent() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        let g = stack.create_group_around(&[a, b], "G").unwrap();
        let before = stack.structure();
        stack.clear_changed();

        // `a`'s parent is not an id inside `entries`.
        assert!(!stack.restore_structure(&[
            (root, None),
            (g, None),
            (a, Some(LayerId::new(999))),
            (b, Some(g)),
        ]));
        assert!(!stack.changed());
        // `a`'s parent is itself.
        assert!(!stack.restore_structure(&[(root, None), (g, None), (a, Some(a)), (b, Some(g)),]));
        assert!(!stack.changed());

        assert_eq!(stack.structure(), before);
    }

    #[test]
    fn restore_structure_rejects_forward_parent_reference() {
        let mut stack = LayerStack::new(1, 1);
        let root = stack.active_layer_id();
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        let g = stack.create_group_around(&[a, b], "G").unwrap();
        let before = stack.structure();
        stack.clear_changed();

        // `a` is listed before its parent `g`.
        assert!(!stack.restore_structure(&[(root, None), (a, Some(g)), (g, None), (b, Some(g)),]));
        assert!(!stack.changed());

        assert_eq!(stack.structure(), before);
    }

    #[test]
    fn restore_structure_selects_last_root_when_active_vanishes() {
        // Rule 1 requires every current id — including the active one — to be
        // present in `entries`, so a valid restore can never drop the active
        // layer. Assert instead that it is preserved when present.
        let mut stack = LayerStack::new(1, 1);
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        let g = stack.create_group_around(&[a, b], "G").unwrap();
        assert!(stack.set_active(a));

        let structure = stack.structure();
        assert!(stack.restore_structure(&structure));
        assert_eq!(stack.active_layer_id(), a);
        assert!(stack.is_group(g));
    }
}
