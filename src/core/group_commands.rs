//! Undoable group commands (D59): create-group, ungroup, move-subtree, and
//! remove-subtree.
//!
//! Every command stores the before/after tree structure captured via
//! [`LayerStack::structure`] and replays it with
//! [`LayerStack::restore_structure`], which reorders the EXISTING layers
//! byte-exact without touching pixel buffers. When a layer must come back
//! (undo of a removal, redo of a creation), it is first re-inserted with
//! [`LayerStack::insert_layer`] and then `restore_structure` fixes its exact
//! position.

use std::any::Any;

use crate::core::layer_commands::LayerStackController;
use crate::core::model::{Layer, LayerId, LayerStack};
use crate::core::undo::{Command, CommandContext};

// ---------------------------------------------------------------------------
// CreateGroupCommand
// ---------------------------------------------------------------------------

/// Undoable "create group around ids". Undo ungroups the group and restores
/// the flat order; redo re-inserts the group and restores the grouped order.
pub struct CreateGroupCommand {
    group: Layer,
    before: Vec<(LayerId, Option<LayerId>)>,
    after: Vec<(LayerId, Option<LayerId>)>,
    undone: bool,
}

impl Command for CreateGroupCommand {
    fn name(&self) -> &str {
        "Create Group"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn undo(&mut self, context: &mut CommandContext) -> bool {
        if self.undone {
            return false;
        }
        if !context.layers.ungroup(self.group.id) {
            return false;
        }
        if !context.layers.restore_structure(&self.before) {
            return false;
        }
        self.undone = true;
        true
    }

    fn redo(&mut self, context: &mut CommandContext) -> bool {
        if !self.undone {
            return false;
        }
        if !context
            .layers
            .insert_layer(self.group.clone(), context.layers.len())
        {
            return false;
        }
        if !context.layers.restore_structure(&self.after) {
            return false;
        }
        self.undone = false;
        true
    }
}

// ---------------------------------------------------------------------------
// UngroupCommand
// ---------------------------------------------------------------------------

/// Undoable "ungroup". Undo re-inserts the group node and restores the
/// grouped structure; redo ungroups again.
pub struct UngroupCommand {
    group: Layer,
    before: Vec<(LayerId, Option<LayerId>)>,
    after: Vec<(LayerId, Option<LayerId>)>,
    undone: bool,
}

impl Command for UngroupCommand {
    fn name(&self) -> &str {
        "Ungroup"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn undo(&mut self, context: &mut CommandContext) -> bool {
        if self.undone {
            return false;
        }
        if !context
            .layers
            .insert_layer(self.group.clone(), context.layers.len())
        {
            return false;
        }
        if !context.layers.restore_structure(&self.before) {
            return false;
        }
        self.undone = true;
        true
    }

    fn redo(&mut self, context: &mut CommandContext) -> bool {
        if !self.undone {
            return false;
        }
        if !context.layers.ungroup(self.group.id) {
            return false;
        }
        if !context.layers.restore_structure(&self.after) {
            return false;
        }
        self.undone = false;
        true
    }
}

// ---------------------------------------------------------------------------
// MoveSubtreeCommand
// ---------------------------------------------------------------------------

/// Undoable "move subtree to a new parent/child index". Both directions are
/// pure structure restores.
pub struct MoveSubtreeCommand {
    before: Vec<(LayerId, Option<LayerId>)>,
    after: Vec<(LayerId, Option<LayerId>)>,
    undone: bool,
}

impl Command for MoveSubtreeCommand {
    fn name(&self) -> &str {
        "Move Layer"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn undo(&mut self, context: &mut CommandContext) -> bool {
        if self.undone {
            return false;
        }
        if !context.layers.restore_structure(&self.before) {
            return false;
        }
        self.undone = true;
        true
    }

    fn redo(&mut self, context: &mut CommandContext) -> bool {
        if !self.undone {
            return false;
        }
        if !context.layers.restore_structure(&self.after) {
            return false;
        }
        self.undone = false;
        true
    }
}

// ---------------------------------------------------------------------------
// RemoveSubtreeCommand
// ---------------------------------------------------------------------------

/// Undoable "remove subtree". Stores every removed [`Layer`] (DFS pre-order)
/// plus the before/after structure and active layer, so undo restores the
/// whole subtree byte-exact.
pub struct RemoveSubtreeCommand {
    removed: Vec<Layer>,
    before: Vec<(LayerId, Option<LayerId>)>,
    after: Vec<(LayerId, Option<LayerId>)>,
    was_active: LayerId,
    after_active: LayerId,
    undone: bool,
}

impl Command for RemoveSubtreeCommand {
    fn name(&self) -> &str {
        "Remove Layer"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn undo(&mut self, context: &mut CommandContext) -> bool {
        if self.undone {
            return false;
        }
        for layer in &self.removed {
            if !context
                .layers
                .insert_layer(layer.clone(), context.layers.len())
            {
                return false;
            }
        }
        if !context.layers.restore_structure(&self.before) {
            return false;
        }
        context.layers.set_active(self.was_active);
        self.undone = true;
        true
    }

    fn redo(&mut self, context: &mut CommandContext) -> bool {
        if !self.undone {
            return false;
        }
        let root_id = self.removed[0].id;
        if context.layers.remove_layer(root_id).is_none() {
            return false;
        }
        if !context.layers.restore_structure(&self.after) {
            return false;
        }
        context.layers.set_active(self.after_active);
        self.undone = false;
        true
    }
}

// ---------------------------------------------------------------------------
// Controller methods
// ---------------------------------------------------------------------------

impl<'a> LayerStackController<'a> {
    /// Wraps `ids` in a new group and pushes exactly one undoable "Create
    /// Group" command. Returns `None` (and pushes nothing) when `ids` is empty
    /// or any id is unknown.
    pub fn create_group_around(
        &mut self,
        layers: &mut LayerStack,
        ids: &[LayerId],
        name: &str,
    ) -> Option<LayerId> {
        let before = layers.structure();
        let group_id = layers.create_group_around(ids, name)?;
        let after = layers.structure();
        let group = layers
            .layer(group_id)
            .expect("just-created group must exist")
            .clone();
        self.stack.push(Box::new(CreateGroupCommand {
            group,
            before,
            after,
            undone: false,
        }));
        Some(group_id)
    }

    /// Removes `group`, promoting its children, and pushes exactly one
    /// undoable "Ungroup" command. Returns `false` (and pushes nothing) when
    /// `group` is unknown or not a group.
    pub fn ungroup(&mut self, layers: &mut LayerStack, group: LayerId) -> bool {
        let Some(group_layer) = layers.layer(group) else {
            return false;
        };
        if !group_layer.is_group {
            return false;
        }
        let group_layer = group_layer.clone();
        let before = layers.structure();
        if !layers.ungroup(group) {
            return false;
        }
        let after = layers.structure();
        self.stack.push(Box::new(UngroupCommand {
            group: group_layer,
            before,
            after,
            undone: false,
        }));
        true
    }

    /// Moves `id`'s whole subtree to become the `child_index`-th child of
    /// `new_parent` and pushes exactly one undoable "Move Layer" command.
    /// Returns `false` (and pushes nothing) when the move is rejected (unknown
    /// id, unknown parent, or a cycle).
    pub fn move_subtree(
        &mut self,
        layers: &mut LayerStack,
        id: LayerId,
        new_parent: Option<LayerId>,
        child_index: usize,
    ) -> bool {
        let before = layers.structure();
        if !layers.set_parent_and_position(id, new_parent, child_index) {
            return false;
        }
        let after = layers.structure();
        self.stack.push(Box::new(MoveSubtreeCommand {
            before,
            after,
            undone: false,
        }));
        true
    }

    /// Removes the whole subtree rooted at `root` and pushes exactly one
    /// undoable "Remove Layer" command. Returns `false` (and pushes nothing)
    /// when `root` is unknown or it is the last remaining root-level layer.
    pub fn remove_subtree(&mut self, layers: &mut LayerStack, root: LayerId) -> bool {
        if layers.layer(root).is_none() {
            return false;
        }
        if layers.children_of(None).len() == 1 && layers.parent_of(root).is_none() {
            return false;
        }
        let removed: Vec<Layer> = layers
            .subtree_ids(root)
            .iter()
            .map(|id| layers.layer(*id).expect("subtree id must exist").clone())
            .collect();
        let before = layers.structure();
        let was_active = layers.active_layer_id();
        let _ = layers.remove_layer(root);
        let after = layers.structure();
        let after_active = layers.active_layer_id();
        self.stack.push(Box::new(RemoveSubtreeCommand {
            removed,
            before,
            after,
            was_active,
            after_active,
            undone: false,
        }));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::undo::UndoStack;

    #[test]
    fn create_group_around_undo_restores_flat_order() {
        let mut layers = LayerStack::new(2, 2);
        let _root = layers.active_layer_id();
        let a = layers.add_layer("A");
        let _b = layers.add_layer("B");
        let c = layers.add_layer("C");
        let before = layers.structure();
        let before_bytes = layers
            .composite_layers(&crate::core::tilemap::TilePalette::new())
            .as_bytes()
            .to_vec();

        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);
        let g = ctl.create_group_around(&mut layers, &[a, c], "G").unwrap();
        assert!(layers.is_group(g));

        assert!(ctl.undo(&mut layers));
        assert_eq!(layers.structure(), before);
        assert_eq!(
            layers
                .composite_layers(&crate::core::tilemap::TilePalette::new())
                .as_bytes(),
            before_bytes
        );
        assert_eq!(layers.layer(g), None);
        assert_eq!(layers.parent_of(a), None);
        assert_eq!(layers.parent_of(c), None);
    }

    #[test]
    fn ungroup_undo_reinserts_group_and_children() {
        let mut layers = LayerStack::new(2, 2);
        let _root = layers.active_layer_id();
        let a = layers.add_layer("A");
        let b = layers.add_layer("B");
        let g = layers.create_group_around(&[a, b], "G").unwrap();
        let before = layers.structure();
        let before_bytes = layers
            .composite_layers(&crate::core::tilemap::TilePalette::new())
            .as_bytes()
            .to_vec();

        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);
        assert!(ctl.ungroup(&mut layers, g));
        assert_eq!(layers.layer(g), None);

        assert!(ctl.undo(&mut layers));
        assert_eq!(layers.structure(), before);
        assert_eq!(
            layers
                .composite_layers(&crate::core::tilemap::TilePalette::new())
                .as_bytes(),
            before_bytes
        );
        assert!(layers.is_group(g));
        assert_eq!(layers.parent_of(a), Some(g));
        assert_eq!(layers.parent_of(b), Some(g));
    }

    #[test]
    fn move_subtree_into_group_and_back_round_trips() {
        let mut layers = LayerStack::new(2, 2);
        let _root = layers.active_layer_id();
        let a = layers.add_layer("A");
        let b = layers.add_layer("B");
        let g = layers.create_group_around(&[a], "G").unwrap();
        let original = layers.structure();
        let original_bytes = layers
            .composite_layers(&crate::core::tilemap::TilePalette::new())
            .as_bytes()
            .to_vec();

        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        assert!(ctl.move_subtree(&mut layers, b, Some(g), 0));
        assert_eq!(layers.parent_of(b), Some(g));

        assert!(ctl.move_subtree(&mut layers, b, None, 0));
        assert_eq!(layers.parent_of(b), None);

        assert!(ctl.undo(&mut layers));
        assert_eq!(layers.parent_of(b), Some(g));
        assert!(ctl.undo(&mut layers));
        assert_eq!(layers.structure(), original);
        assert_eq!(
            layers
                .composite_layers(&crate::core::tilemap::TilePalette::new())
                .as_bytes(),
            original_bytes
        );
    }

    #[test]
    fn move_subtree_rejects_cycle() {
        let mut layers = LayerStack::new(2, 2);
        let _root = layers.active_layer_id();
        let a = layers.add_layer("A");
        let b = layers.add_layer("B");
        let g = layers.create_group_around(&[a, b], "G").unwrap();
        let before = layers.structure();
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        assert!(!ctl.move_subtree(&mut layers, g, Some(g), 0));
        assert!(!ctl.move_subtree(&mut layers, g, Some(a), 0));
        assert!(!ctl.move_subtree(&mut layers, LayerId::new(999), None, 0));
        assert_eq!(layers.structure(), before);
        assert_eq!(ctl.undo_len(), 0);
    }

    #[test]
    fn remove_subtree_undo_restores_children() {
        let mut layers = LayerStack::new(2, 2);
        let _root = layers.active_layer_id();
        let a = layers.add_layer("A");
        let b = layers.add_layer("B");
        let g = layers.create_group_around(&[a, b], "G").unwrap();
        let before = layers.structure();
        let before_bytes = layers
            .composite_layers(&crate::core::tilemap::TilePalette::new())
            .as_bytes()
            .to_vec();

        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);
        assert!(ctl.remove_subtree(&mut layers, g));
        assert_eq!(layers.layer(g), None);
        assert_eq!(layers.layer(a), None);
        assert_eq!(layers.layer(b), None);

        assert!(ctl.undo(&mut layers));
        assert_eq!(layers.structure(), before);
        assert_eq!(
            layers
                .composite_layers(&crate::core::tilemap::TilePalette::new())
                .as_bytes(),
            before_bytes
        );
        assert!(layers.is_group(g));
        assert_eq!(layers.parent_of(a), Some(g));
        assert_eq!(layers.parent_of(b), Some(g));
    }

    #[test]
    fn remove_subtree_rejects_last_root_layer() {
        let mut layers = LayerStack::new(2, 2);
        let root = layers.active_layer_id();
        let a = layers.add_layer("A");
        let b = layers.add_layer("B");
        let g = layers.create_group_around(&[a, b], "G").unwrap();
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        assert!(ctl.remove_subtree(&mut layers, g));
        assert!(!ctl.remove_subtree(&mut layers, root));
        assert_eq!(layers.len(), 1);
        assert_eq!(ctl.undo_len(), 1);
    }

    #[test]
    fn rename_is_undoable() {
        let mut layers = LayerStack::new(2, 2);
        let id = layers.active_layer_id();
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        assert!(ctl.set_name(&mut layers, id, "Renamed".to_string()));
        assert_eq!(layers.layer(id).unwrap().name, "Renamed");
        assert_eq!(ctl.top_undo_name(), Some("Rename Layer"));

        assert!(ctl.undo(&mut layers));
        assert_eq!(layers.layer(id).unwrap().name, "Layer 1");

        assert!(ctl.redo(&mut layers));
        assert_eq!(layers.layer(id).unwrap().name, "Renamed");

        // No-op rename rejected without push.
        assert!(!ctl.set_name(&mut layers, id, "Renamed".to_string()));
        assert_eq!(ctl.undo_len(), 1);
    }

    #[test]
    fn each_group_command_pushes_exactly_one_undo_step() {
        let mut layers = LayerStack::new(2, 2);
        let root = layers.active_layer_id();
        let a = layers.add_layer("A");
        let b = layers.add_layer("B");
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        let g = ctl.create_group_around(&mut layers, &[a, b], "G").unwrap();
        assert_eq!(ctl.undo_len(), 1);

        assert!(ctl.ungroup(&mut layers, g));
        assert_eq!(ctl.undo_len(), 2);

        let g2 = ctl.create_group_around(&mut layers, &[a, b], "G2").unwrap();
        assert_eq!(ctl.undo_len(), 3);

        assert!(ctl.move_subtree(&mut layers, a, Some(g2), 0));
        assert_eq!(ctl.undo_len(), 4);

        assert!(ctl.remove_subtree(&mut layers, g2));
        assert_eq!(ctl.undo_len(), 5);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers.active_layer_id(), root);
    }
}
