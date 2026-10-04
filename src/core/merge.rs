//! Undoable "merge down" command (D59).
//!
//! Merging composites the top layer's buffer over its previous sibling (the
//! bottom layer) using the top layer's opacity and blend mode, then removes
//! the top layer. The command stores the full before/after [`Layer`] objects
//! plus the before/after tree structure, so undo/redo round-trip byte-exact
//! without ever touching pixel buffers directly.

use std::any::Any;

use crate::core::layer_commands::LayerStackController;
use crate::core::model::composite_over;
use crate::core::model::{Layer, LayerId, LayerStack};
use crate::core::undo::{Command, CommandContext};

/// Undoable "merge down": composites `top` over `bottom_id` and removes `top`.
///
/// `before`/`after` are the tree structures captured around the merge;
/// `bottom_before`/`bottom_after` are the bottom layer's full state (buffer
/// included) so undo/redo restore it byte-exact.
pub struct MergeDownCommand {
    top: Layer,
    bottom_id: LayerId,
    bottom_before: Layer,
    bottom_after: Layer,
    before: Vec<(LayerId, Option<LayerId>)>,
    after: Vec<(LayerId, Option<LayerId>)>,
    was_active: LayerId,
    after_active: LayerId,
    undone: bool,
}

impl Command for MergeDownCommand {
    fn name(&self) -> &str {
        "Merge Down"
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
        debug_assert_eq!(self.bottom_before.id, self.bottom_id);
        if !context.layers.replace_layer(self.bottom_before.clone()) {
            return false;
        }
        if !context
            .layers
            .insert_layer(self.top.clone(), context.layers.len())
        {
            return false;
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
        debug_assert_eq!(self.bottom_after.id, self.bottom_id);
        if !context.layers.replace_layer(self.bottom_after.clone()) {
            return false;
        }
        if context.layers.remove_layer(self.top.id).is_none() {
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

impl<'a> LayerStackController<'a> {
    /// Merges the layer `id` down into its previous sibling and pushes exactly
    /// one undoable "Merge Down" command.
    ///
    /// Returns `false` (and pushes nothing) when `id` is unknown, is a group,
    /// is the first child of its parent, or its previous sibling is a group.
    pub fn merge_down(&mut self, layers: &mut LayerStack, id: LayerId) -> bool {
        let Some(top) = layers.layer(id) else {
            return false;
        };
        if top.is_group {
            return false;
        }
        let siblings = layers.children_of(top.parent);
        let Some(sibling_pos) = siblings.iter().position(|&s| s == id) else {
            return false;
        };
        if sibling_pos == 0 {
            return false;
        }
        let bottom_id = siblings[sibling_pos - 1];
        let bottom = layers
            .layer(bottom_id)
            .expect("previous sibling must exist");
        if bottom.is_group {
            return false;
        }

        let before = layers.structure();
        let top_layer = top.clone();
        let bottom_before = bottom.clone();
        let was_active = layers.active_layer_id();

        // Apply: composite the top buffer over the bottom, then drop the top.
        let top_buffer = top_layer.buffer.clone();
        let bottom_mut = layers
            .layer_mut(bottom_id)
            .expect("bottom must exist for merge");
        composite_over(
            &mut bottom_mut.buffer,
            &top_buffer,
            top_layer.opacity,
            top_layer.blend,
        );
        let _ = layers.remove_layer(id);
        let _ = layers.set_active(bottom_id);

        let after = layers.structure();
        let bottom_after = layers
            .layer(bottom_id)
            .expect("bottom must exist after merge")
            .clone();
        let after_active = layers.active_layer_id();

        self.stack.push(Box::new(MergeDownCommand {
            top: top_layer,
            bottom_id,
            bottom_before,
            bottom_after,
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
    use crate::core::color::Color;
    use crate::core::model::BlendMode;
    use crate::core::undo::UndoStack;

    #[test]
    fn merge_down_composites_top_over_bottom() {
        let mut layers = LayerStack::new(2, 2);
        let bottom = layers.active_layer_id();
        layers
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgb(0, 0, 255));
        let top = layers.add_layer("Top");
        layers
            .layer_mut(top)
            .unwrap()
            .buffer
            .fill(Color::rgb(255, 0, 0));
        let expected = layers
            .composite_layers(&crate::core::tilemap::TilePalette::new())
            .as_bytes()
            .to_vec();

        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);
        assert!(ctl.merge_down(&mut layers, top));
        assert_eq!(layers.layer(top), None);
        assert_eq!(
            layers
                .composite_layers(&crate::core::tilemap::TilePalette::new())
                .as_bytes(),
            expected
        );
        assert_eq!(ctl.top_undo_name(), Some("Merge Down"));
    }

    #[test]
    fn merge_down_keeps_bottom_layer_attributes() {
        let mut layers = LayerStack::new(2, 2);
        let bottom = layers.active_layer_id();
        {
            let layer = layers.layer_mut(bottom).unwrap();
            layer.name = "Bottom".to_string();
            layer.opacity = 0.7;
            layer.blend = BlendMode::Multiply;
            layer.visible = false;
            layer.buffer.fill(Color::rgb(0, 0, 255));
        }
        let top = layers.add_layer("Top");
        layers
            .layer_mut(top)
            .unwrap()
            .buffer
            .fill(Color::rgb(255, 0, 0));

        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);
        assert!(ctl.merge_down(&mut layers, top));
        let merged = layers.layer(bottom).unwrap();
        assert_eq!(merged.name, "Bottom");
        assert_eq!(merged.opacity, 0.7);
        assert_eq!(merged.blend, BlendMode::Multiply);
        assert!(!merged.visible);
        assert_eq!(layers.layer(top), None);
    }

    #[test]
    fn merge_down_rejects_group_and_first_child() {
        let mut layers = LayerStack::new(2, 2);
        let _root = layers.active_layer_id();
        let a = layers.add_layer("A");
        let b = layers.add_layer("B");
        let g = layers.create_group_around(&[a, b], "G").unwrap();
        let c = layers.add_layer("C");
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        // Group target rejected.
        assert!(!ctl.merge_down(&mut layers, g));
        // First child of a parent rejected.
        assert!(!ctl.merge_down(&mut layers, a));
        // Previous sibling is a group rejected (c's previous sibling is g).
        assert!(!ctl.merge_down(&mut layers, c));
        // Unknown id rejected.
        assert!(!ctl.merge_down(&mut layers, LayerId::new(999)));
        assert_eq!(ctl.undo_len(), 0);
    }

    #[test]
    fn merge_down_redo_equals_composite_layers() {
        let mut layers = LayerStack::new(2, 2);
        let bottom = layers.active_layer_id();
        layers
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgb(0, 0, 255));
        let top = layers.add_layer("Top");
        {
            let layer = layers.layer_mut(top).unwrap();
            layer.buffer.fill(Color::rgb(255, 0, 0));
            layer.opacity = 0.5;
        }
        let expected = layers
            .composite_layers(&crate::core::tilemap::TilePalette::new())
            .as_bytes()
            .to_vec();

        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);
        assert!(ctl.merge_down(&mut layers, top));
        assert_eq!(
            layers
                .composite_layers(&crate::core::tilemap::TilePalette::new())
                .as_bytes(),
            expected
        );

        assert!(ctl.undo(&mut layers));
        assert!(ctl.redo(&mut layers));
        assert_eq!(layers.layer(top), None);
        assert_eq!(
            layers
                .composite_layers(&crate::core::tilemap::TilePalette::new())
                .as_bytes(),
            expected
        );
    }

    #[test]
    fn merge_down_undo_restores_original_stack() {
        let mut layers = LayerStack::new(2, 2);
        let bottom = layers.active_layer_id();
        layers
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgb(0, 0, 255));
        let top = layers.add_layer("Top");
        layers
            .layer_mut(top)
            .unwrap()
            .buffer
            .fill(Color::rgb(255, 0, 0));
        assert!(layers.set_active(top));
        let before = layers.structure();
        let before_bytes = layers
            .composite_layers(&crate::core::tilemap::TilePalette::new())
            .as_bytes()
            .to_vec();

        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);
        assert!(ctl.merge_down(&mut layers, top));
        assert_eq!(layers.active_layer_id(), bottom);

        assert!(ctl.undo(&mut layers));
        assert_eq!(layers.structure(), before);
        assert_eq!(
            layers
                .composite_layers(&crate::core::tilemap::TilePalette::new())
                .as_bytes(),
            before_bytes
        );
        assert!(layers.layer(top).is_some());
        assert_eq!(layers.active_layer_id(), top);
    }
}
