//! PreviewManager — owns the active projection list, view sources, monitor picker [D65].

use crate::core::projection::{
    MonitorTarget, ProjectionId, ProjectionMode, ProjectionStatus, Projector, ViewSource,
};

/// Monitor targets the picker can offer without a platform monitor list.
/// `MonitorTarget::Named` needs the platform monitor enumeration, which the
/// integration unit owns; it is rendered as a hint here.
const MONITOR_OPTIONS: [MonitorTarget; 4] = [
    MonitorTarget::Primary,
    MonitorTarget::Largest,
    MonitorTarget::LeftOfPrimary,
    MonitorTarget::RightOfPrimary,
];

/// Display label for a monitor target.
fn monitor_label(target: MonitorTarget) -> &'static str {
    match target {
        MonitorTarget::Primary => "Primary",
        MonitorTarget::Largest => "Largest",
        MonitorTarget::LeftOfPrimary => "Left of primary",
        MonitorTarget::RightOfPrimary => "Right of primary",
        MonitorTarget::Named(_) => "Named",
    }
}

/// Manages the set of active projection surfaces and their configurations.
pub struct PreviewManager {
    projections: std::collections::HashMap<ProjectionId, ProjectionStatus>,
    view_sources: std::collections::HashMap<ProjectionId, ViewSource>,
    capture_mode: bool,
    pending_monitor: Option<MonitorTarget>,
}

impl PreviewManager {
    pub fn new() -> Self {
        Self {
            projections: std::collections::HashMap::new(),
            view_sources: std::collections::HashMap::new(),
            capture_mode: true,
            pending_monitor: None,
        }
    }

    /// Creates a new projection with the given target and mode.
    pub fn create_projection<P: Projector>(
        &mut self,
        projector: &mut P,
        target: MonitorTarget,
        mode: ProjectionMode,
    ) -> Result<ProjectionId, crate::core::projection::ProjectionError> {
        let id = projector.create(target, mode)?;
        if let Some(status) = projector.status(id) {
            self.projections.insert(id, status);
            self.view_sources.insert(id, ViewSource::EditorCanvas);
        }
        Ok(id)
    }

    /// Destroys a projection by ID.
    pub fn destroy_projection<P: Projector>(
        &mut self,
        projector: &mut P,
        id: ProjectionId,
    ) -> Result<(), crate::core::projection::ProjectionError> {
        projector.destroy(id)?;
        self.projections.remove(&id);
        self.view_sources.remove(&id);
        Ok(())
    }

    /// Returns the list of active projections.
    pub fn projections(&self) -> impl Iterator<Item = (&ProjectionId, &ProjectionStatus)> {
        self.projections.iter()
    }

    /// Sets the view source for a projection. Returns false when the id is
    /// unknown.
    pub fn set_view_source(&mut self, id: ProjectionId, source: ViewSource) -> bool {
        if !self.projections.contains_key(&id) {
            return false;
        }
        self.view_sources.insert(id, source);
        true
    }

    /// Returns the view source for a projection, if it exists.
    pub fn view_source(&self, id: ProjectionId) -> Option<ViewSource> {
        self.view_sources.get(&id).copied()
    }

    /// Whether Capture Mode is enabled (D67: captures target the window in
    /// Window mode).
    pub fn capture_mode_enabled(&self) -> bool {
        self.capture_mode
    }

    /// Toggles Capture Mode.
    pub fn set_capture_mode(&mut self, on: bool) {
        self.capture_mode = on;
    }

    /// The monitor selection pending from the last `ui_monitor_picker`
    /// interaction, for the integration unit to consume when creating or
    /// retargeting projections.
    pub fn pending_monitor(&self) -> Option<MonitorTarget> {
        self.pending_monitor
    }

    /// Monitor picker UI: lists the [`MonitorTarget`] variants as selectable
    /// labels and stores the pending selection in `pending_monitor`.
    ///
    /// Thin and side-effect-free: never creates windows or surfaces.
    pub fn ui_monitor_picker(&mut self, ui: &mut egui::Ui) {
        ui.label("Projection monitor");
        for target in MONITOR_OPTIONS {
            let selected = self.pending_monitor == Some(target);
            if ui
                .selectable_label(selected, monitor_label(target))
                .clicked()
            {
                self.pending_monitor = Some(target);
            }
        }
        ui.label("Named monitors are selected by the projection panel (integration unit).");
    }

    /// Processes and handles projection events.
    pub fn poll_events<P: Projector>(&mut self, projector: &mut P) {
        for event in projector.poll_events() {
            use crate::core::projection::ProjectionEvent;
            match event {
                ProjectionEvent::Created { id, .. } => {
                    if let Some(status) = projector.status(id) {
                        self.projections.insert(id, status);
                        self.view_sources.insert(id, ViewSource::EditorCanvas);
                    }
                }
                ProjectionEvent::Destroyed { id } => {
                    self.projections.remove(&id);
                    self.view_sources.remove(&id);
                }
                ProjectionEvent::Resized { id, width, height } => {
                    if let Some(status) = self.projections.get_mut(&id) {
                        status.width = width;
                        status.height = height;
                    }
                }
                ProjectionEvent::Died { id } => {
                    if let Some(status) = self.projections.get_mut(&id) {
                        status.alive = false;
                    }
                }
            }
        }
    }
}

impl Default for PreviewManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::projection::{ProjectionError, ProjectionEvent};

    struct MockProjector {
        projections: std::collections::HashMap<ProjectionId, ProjectionStatus>,
        events: Vec<ProjectionEvent>,
    }

    impl MockProjector {
        fn new() -> Self {
            Self {
                projections: std::collections::HashMap::new(),
                events: Vec::new(),
            }
        }
    }

    impl Projector for MockProjector {
        fn create(
            &mut self,
            target: MonitorTarget,
            mode: ProjectionMode,
        ) -> Result<ProjectionId, ProjectionError> {
            let id = ProjectionId::next();
            let status = ProjectionStatus {
                id,
                mode,
                target,
                width: 1280,
                height: 720,
                capture_mode: mode == ProjectionMode::Capture,
                alive: true,
            };
            self.projections.insert(id, status);
            self.events.push(ProjectionEvent::Created {
                id,
                width: 1280,
                height: 720,
            });
            Ok(id)
        }

        fn destroy(&mut self, id: ProjectionId) -> Result<(), ProjectionError> {
            if self.projections.remove(&id).is_some() {
                self.events.push(ProjectionEvent::Destroyed { id });
                Ok(())
            } else {
                Err(ProjectionError::Backend("not found"))
            }
        }

        fn status(&self, id: ProjectionId) -> Option<ProjectionStatus> {
            self.projections.get(&id).copied()
        }

        fn poll_events(&mut self) -> Vec<ProjectionEvent> {
            std::mem::take(&mut self.events)
        }
    }

    #[test]
    fn set_view_source_unknown_id_returns_false() {
        let mut manager = PreviewManager::new();
        assert!(!manager.set_view_source(ProjectionId::next(), ViewSource::Playback));
    }

    #[test]
    fn set_and_get_view_source_known_id() {
        let mut manager = PreviewManager::new();
        let mut projector = MockProjector::new();
        let id = manager
            .create_projection(
                &mut projector,
                MonitorTarget::Primary,
                ProjectionMode::Editor,
            )
            .unwrap();
        assert_eq!(manager.view_source(id), Some(ViewSource::EditorCanvas));
        assert!(manager.set_view_source(id, ViewSource::Playback));
        assert_eq!(manager.view_source(id), Some(ViewSource::Playback));
    }

    #[test]
    fn capture_mode_defaults_on_and_toggles() {
        let mut manager = PreviewManager::new();
        assert!(manager.capture_mode_enabled());
        manager.set_capture_mode(false);
        assert!(!manager.capture_mode_enabled());
        manager.set_capture_mode(true);
        assert!(manager.capture_mode_enabled());
    }

    #[test]
    fn create_set_destroy_leaves_no_dangling_entry() {
        let mut manager = PreviewManager::new();
        let mut projector = MockProjector::new();
        let id = manager
            .create_projection(
                &mut projector,
                MonitorTarget::Primary,
                ProjectionMode::Editor,
            )
            .unwrap();
        assert!(manager.set_view_source(id, ViewSource::FrameEdit));
        manager.destroy_projection(&mut projector, id).unwrap();
        assert_eq!(manager.view_source(id), None);
        assert!(!manager.set_view_source(id, ViewSource::Playback));
        assert_eq!(manager.projections().count(), 0);
    }

    #[test]
    fn poll_events_created_destroys_view_source() {
        let mut manager = PreviewManager::new();
        let mut projector = MockProjector::new();
        let id = projector
            .create(MonitorTarget::Primary, ProjectionMode::Editor)
            .unwrap();
        manager.poll_events(&mut projector);
        assert_eq!(manager.view_source(id), Some(ViewSource::EditorCanvas));
        projector.destroy(id).unwrap();
        manager.poll_events(&mut projector);
        assert_eq!(manager.view_source(id), None);
    }

    /// Runs one headless frame of the monitor picker in a full-screen central
    /// panel (same harness style as `src/ui/toolbar.rs` tests).
    fn run_picker_frame(
        ctx: &egui::Context,
        manager: &mut PreviewManager,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 300.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| manager.ui_monitor_picker(ui));
        });
        output.textures_delta.clear();
        output
    }

    /// Top-left positions of every rendered text equal to `text`, in paint
    /// order.
    fn text_positions(output: &egui::FullOutput, text: &str) -> Vec<egui::Pos2> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::Shape::Text(text) => out.push((text.galley.text().to_string(), text.pos)),
                egui::Shape::Vec(shapes) => {
                    for s in shapes {
                        walk(s, out);
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts);
        }
        texts
            .into_iter()
            .filter(|(t, _)| t == text)
            .map(|(_, pos)| pos)
            .collect()
    }

    #[test]
    fn monitor_picker_click_stores_pending_selection() {
        let ctx = egui::Context::default();
        let mut manager = PreviewManager::new();
        let output = run_picker_frame(&ctx, &mut manager, vec![]);
        let pos = text_positions(&output, "Primary")
            .into_iter()
            .next()
            .expect("Primary label should be rendered");
        let click_pos = pos + egui::vec2(4.0, 8.0);
        run_picker_frame(
            &ctx,
            &mut manager,
            vec![egui::Event::PointerMoved(click_pos)],
        );
        run_picker_frame(
            &ctx,
            &mut manager,
            vec![egui::Event::PointerButton {
                pos: click_pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        run_picker_frame(
            &ctx,
            &mut manager,
            vec![egui::Event::PointerButton {
                pos: click_pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(manager.pending_monitor(), Some(MonitorTarget::Primary));
    }
}
