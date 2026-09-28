use std::cell::Cell;
use std::rc::Rc;

use egui::{pos2, vec2, Rect, Vec2};
use pyxross::core::camera::Camera;
use pyxross::core::color::Color;
use pyxross::ui::canvas::CanvasWidget;
use pyxross::ui::dock_color_picker_panel::{
    bright_shading_hue_shift, color_picker_panel_spec, color_to_hsl, color_to_hsv, hsl_to_color,
    hsv_to_color, lightness_chips, shading_chips, shading_hue_shift,
};
use pyxross::ui::dock_hosts::{ColorPickerHost, ColorPickerView};
use pyxross::ui::input_capture::{InputCapture, SurfaceId};
use pyxross::ui::panel_dock::nest::NestView;
use pyxross::ui::panel_dock::skin::AtlasCache;
use pyxross::ui::panel_dock::{
    drag_outside_threshold, DockAction, DockError, DockManager, DockSide, PanelChrome,
    PanelContent, PanelHeaderAction, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};
use pyxross::ui::panel_dock::{
    NestReset, NestResetHandle, PreviewControls, PreviewFeed, PreviewImage,
};
use pyxross::ui::theme::Theme;

/// Headless chrome harness: owns the gesture capture (so multi-frame gestures
/// keep their owner) and binds each view to a panel surface whose target is
/// set, mirroring the app's per-frame routing.
struct ChromeHarness {
    capture: InputCapture,
}

impl ChromeHarness {
    fn new() -> Self {
        Self {
            capture: InputCapture::new(),
        }
    }

    fn view<'a>(&'a self, theme: &'a Theme) -> PanelChrome<'a> {
        let surface = SurfaceId::Panel(PanelId::new(1));
        self.capture.set_target(Some(surface));
        PanelChrome::flat(theme, &self.capture, surface)
    }

    fn skinned<'a>(&'a self, theme: &'a Theme, atlases: &'a AtlasCache) -> PanelChrome<'a> {
        let surface = SurfaceId::Panel(PanelId::new(1));
        self.capture.set_target(Some(surface));
        PanelChrome {
            colors: &theme.colors,
            theme,
            atlases: Some(atlases),
            capture: &self.capture,
            surface,
        }
    }
}

struct EmptyContent;

impl PanelContent for EmptyContent {
    fn ui(&mut self, _ui: &mut egui::Ui, _chrome: &PanelChrome) {}
}

struct RecordingContent {
    available_size: Rc<Cell<egui::Vec2>>,
}

impl PanelContent for RecordingContent {
    fn ui(&mut self, ui: &mut egui::Ui, _chrome: &PanelChrome) {
        self.available_size.set(ui.available_size());
    }
}

fn floating_manager() -> DockManager {
    DockManager::try_new([PanelSpec {
        id: PanelId::new(9),
        metadata: PanelMetadata::new("Floating", true, vec2(180.0, 120.0)),
        placement: PanelPlacement::Floating,
        floating_rect: Rect::from_min_size(pos2(20.0, 20.0), vec2(260.0, 190.0)),
        content: Box::new(EmptyContent),
    }])
    .unwrap()
}

fn rendered_texts(output: &egui::FullOutput) -> Vec<String> {
    fn visit(shape: &egui::Shape, texts: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => texts.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| visit(shape, texts)),
            _ => {}
        }
    }
    let mut texts = Vec::new();
    output
        .shapes
        .iter()
        .for_each(|clipped| visit(&clipped.shape, &mut texts));
    texts
}

#[test]
fn demo_keeps_one_stable_instance_per_panel_across_dock_transitions() {
    let mut manager = DockManager::demo();
    let panel = PanelId::new(2);
    let identity = manager.instance_identity(panel);

    manager
        .transition_panel(panel, PanelPlacement::DockedBottom)
        .unwrap();
    manager
        .transition_panel(panel, PanelPlacement::DockedRight)
        .unwrap();

    assert_eq!(manager.instance_identity(panel), identity);
    assert_eq!(manager.panel_count(), 4);
    assert_eq!(manager.placement(panel), Some(PanelPlacement::DockedRight));
}

#[test]
fn floating_geometry_clamps_both_axes_to_minimum_and_bounds() {
    let mut manager = floating_manager();
    let panel = PanelId::new(9);
    let bounds = Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 480.0));

    manager.set_floating_rect(
        panel,
        Rect::from_min_size(pos2(-20.0, -10.0), vec2(4.0, 6.0)),
    );
    manager.clamp_floating(panel, bounds);
    let rect = manager.floating_rect(panel).unwrap();

    assert!(rect.width() >= 180.0);
    assert!(rect.height() >= 120.0);
    assert!(bounds.contains(rect.min));
    assert!(bounds.contains(rect.max - vec2(0.1, 0.1)));
}

#[test]
fn dock_panel_resize_uses_only_the_dock_axis_and_reorder_stays_in_one_dock() {
    let mut manager = DockManager::demo();
    let panel = PanelId::new(2);

    manager.resize_dock_panel(panel, 40.0, vec2(320.0, 480.0));
    let right_size = manager.dock_panel_size(DockSide::Right, panel).unwrap();
    assert_eq!(right_size.x, 240.0);
    assert_eq!(right_size.y, 160.0);

    manager.reorder(DockSide::Right, panel, 2).unwrap();
    assert_eq!(
        manager.dock_order(DockSide::Right),
        &[PanelId::new(1), PanelId::new(4), panel]
    );
    assert_eq!(manager.placement(panel), Some(PanelPlacement::DockedRight));
}

#[test]
fn dock_area_resize_is_axis_specific() {
    let mut manager = DockManager::demo();

    manager.resize_dock_area(DockSide::Right, -500.0, vec2(640.0, 480.0));
    manager.resize_dock_area(DockSide::Bottom, 500.0, vec2(640.0, 480.0));

    // A non-empty dock collapses to the frame strip, not the old 180pt floor.
    assert_eq!(manager.dock_extent(DockSide::Right), 8.0);
    // The bottom dock grows to the window height, not a 50% fraction.
    assert_eq!(manager.dock_extent(DockSide::Bottom), 480.0);
}

#[test]
fn empty_dock_areas_can_collapse_to_zero_extent() {
    let mut manager = DockManager::try_new([]).unwrap();

    manager.resize_dock_area(DockSide::Right, -500.0, vec2(640.0, 480.0));
    manager.resize_dock_area(DockSide::Bottom, -500.0, vec2(640.0, 480.0));

    assert_eq!(manager.dock_extent(DockSide::Right), 0.0);
    assert_eq!(manager.dock_extent(DockSide::Bottom), 0.0);
}

#[test]
fn thin_docks_render_a_full_frame_without_panicking() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut manager = DockManager::demo();
    manager
        .transition_panel(PanelId::new(4), PanelPlacement::DockedLeft)
        .unwrap();
    let viewport = vec2(1280.0, 720.0);
    manager.resize_dock_area(DockSide::Left, -10_000.0, viewport);
    manager.resize_dock_area(DockSide::Right, -10_000.0, viewport);
    manager.resize_dock_area(DockSide::Bottom, -10_000.0, viewport);

    run_demo_frame(&ctx, &mut manager, &chrome.view(&theme));

    assert_eq!(manager.dock_extent(DockSide::Left), 8.0);
    assert_eq!(manager.dock_extent(DockSide::Right), 8.0);
    assert_eq!(manager.dock_extent(DockSide::Bottom), 8.0);
    let viewport_rect = Rect::from_min_size(pos2(0.0, 0.0), viewport);
    for side in [DockSide::Left, DockSide::Right, DockSide::Bottom] {
        let dock = match side {
            DockSide::Left => {
                Rect::from_min_max(viewport_rect.min, pos2(8.0, viewport_rect.bottom()))
            }
            DockSide::Right => Rect::from_min_max(
                pos2(viewport_rect.right() - 8.0, viewport_rect.top()),
                viewport_rect.max,
            ),
            DockSide::Bottom => Rect::from_min_max(
                pos2(0.0, viewport_rect.bottom() - 8.0),
                pos2(viewport_rect.right(), viewport_rect.bottom()),
            ),
        };
        for (_, rect) in manager.panel_rects(side, dock) {
            assert!(
                rect.width() >= 0.0 && rect.height() >= 0.0,
                "{side:?} inverted rect: {rect:?}"
            );
            assert!(
                rect.width().is_finite() && rect.height().is_finite(),
                "{side:?} NaN rect: {rect:?}"
            );
        }
    }
}

#[test]
fn dock_areas_shrink_to_a_frame_strip() {
    let mut manager = DockManager::demo();
    manager
        .transition_panel(PanelId::new(4), PanelPlacement::DockedLeft)
        .unwrap();
    let viewport = vec2(640.0, 480.0);

    manager.resize_dock_area(DockSide::Left, -10_000.0, viewport);
    manager.resize_dock_area(DockSide::Right, -10_000.0, viewport);
    manager.resize_dock_area(DockSide::Bottom, -10_000.0, viewport);

    // A non-empty dock collapses to the 8pt frame strip, not the old 180/140 floors.
    assert_eq!(manager.dock_extent(DockSide::Left), 8.0);
    assert_eq!(manager.dock_extent(DockSide::Right), 8.0);
    assert_eq!(manager.dock_extent(DockSide::Bottom), 8.0);
}

#[test]
fn left_dock_expands_until_the_right_dock_frame() {
    let mut manager = DockManager::demo();
    manager
        .transition_panel(PanelId::new(4), PanelPlacement::DockedLeft)
        .unwrap();
    let viewport = vec2(640.0, 480.0);
    let right_extent = manager.dock_extent(DockSide::Right);

    manager.resize_dock_area(DockSide::Left, 10_000.0, viewport);

    assert_eq!(
        manager.dock_extent(DockSide::Left),
        viewport.x - right_extent
    );
}

#[test]
fn right_dock_expands_until_the_left_dock_frame() {
    let mut manager = DockManager::demo();
    manager
        .transition_panel(PanelId::new(4), PanelPlacement::DockedLeft)
        .unwrap();
    let viewport = vec2(640.0, 480.0);
    let left_extent = manager.dock_extent(DockSide::Left);

    manager.resize_dock_area(DockSide::Right, 10_000.0, viewport);

    assert_eq!(
        manager.dock_extent(DockSide::Right),
        viewport.x - left_extent
    );
}

#[test]
fn bottom_dock_expands_to_the_window_height() {
    let mut manager = DockManager::demo();
    let viewport = vec2(640.0, 480.0);

    manager.resize_dock_area(DockSide::Bottom, 10_000.0, viewport);

    assert_eq!(manager.dock_extent(DockSide::Bottom), viewport.y);
}

#[test]
fn thin_dock_panels_render_without_panicking() {
    let mut manager = DockManager::demo();
    manager
        .transition_panel(PanelId::new(4), PanelPlacement::DockedLeft)
        .unwrap();
    let viewport = vec2(640.0, 480.0);
    manager.resize_dock_area(DockSide::Left, -10_000.0, viewport);
    manager.resize_dock_area(DockSide::Right, -10_000.0, viewport);
    manager.resize_dock_area(DockSide::Bottom, -10_000.0, viewport);

    let left_dock = Rect::from_min_max(
        pos2(0.0, 0.0),
        pos2(manager.dock_extent(DockSide::Left), viewport.y),
    );
    let right_dock = Rect::from_min_max(
        pos2(viewport.x - manager.dock_extent(DockSide::Right), 0.0),
        pos2(viewport.x, viewport.y),
    );
    let bottom_dock = Rect::from_min_max(
        pos2(0.0, viewport.y - manager.dock_extent(DockSide::Bottom)),
        pos2(viewport.x, viewport.y),
    );

    for (side, dock) in [
        (DockSide::Left, left_dock),
        (DockSide::Right, right_dock),
        (DockSide::Bottom, bottom_dock),
    ] {
        let rects = manager.panel_rects(side, dock);
        assert!(!rects.is_empty(), "{side:?} must still resolve its panels");
        for (_, rect) in rects {
            assert!(
                rect.width() >= 0.0 && rect.height() >= 0.0,
                "{side:?} inverted rect: {rect:?}"
            );
            assert!(
                rect.width().is_finite() && rect.height().is_finite(),
                "{side:?} NaN rect: {rect:?}"
            );
        }
    }
}

#[test]
fn panel_resize_on_a_thin_dock_does_not_panic() {
    let mut manager = DockManager::try_new([
        panel_spec(
            1,
            "L1",
            PanelPlacement::DockedLeft,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
            Box::new(EmptyContent),
        ),
        panel_spec(
            2,
            "L2",
            PanelPlacement::DockedLeft,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
            Box::new(EmptyContent),
        ),
        panel_spec(
            3,
            "R1",
            PanelPlacement::DockedRight,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
            Box::new(EmptyContent),
        ),
        panel_spec(
            4,
            "R2",
            PanelPlacement::DockedRight,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
            Box::new(EmptyContent),
        ),
        panel_spec(
            5,
            "B1",
            PanelPlacement::DockedBottom,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
            Box::new(EmptyContent),
        ),
        panel_spec(
            6,
            "B2",
            PanelPlacement::DockedBottom,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
            Box::new(EmptyContent),
        ),
    ])
    .unwrap();
    let viewport = vec2(8.0, 8.0);
    manager.resize_dock_area(DockSide::Left, -10_000.0, viewport);
    manager.resize_dock_area(DockSide::Right, -10_000.0, viewport);
    manager.resize_dock_area(DockSide::Bottom, -10_000.0, viewport);
    assert_eq!(manager.dock_extent(DockSide::Left), 8.0);
    assert_eq!(manager.dock_extent(DockSide::Right), 8.0);
    assert_eq!(manager.dock_extent(DockSide::Bottom), 8.0);

    // Every side's first panel has a next panel; drive both directions. The
    // tiny viewport resolves panel extents below their minimums, which used to
    // invert the clamp range in `resize_dock_panel` and panic.
    for delta in [-10_000.0, 10_000.0] {
        for side in [DockSide::Left, DockSide::Right, DockSide::Bottom] {
            let first = manager.dock_order(side)[0];
            manager.resize_dock_panel(first, delta, viewport);
        }
    }
}

#[test]
fn thin_dock_renders_a_full_frame_without_panicking() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut manager = DockManager::demo();
    manager
        .transition_panel(PanelId::new(4), PanelPlacement::DockedLeft)
        .unwrap();
    let viewport = vec2(1280.0, 720.0);
    manager.resize_dock_area(DockSide::Left, -10_000.0, viewport);
    manager.resize_dock_area(DockSide::Right, -10_000.0, viewport);
    manager.resize_dock_area(DockSide::Bottom, -10_000.0, viewport);

    run_demo_frame(&ctx, &mut manager, &chrome.view(&theme));

    assert_eq!(manager.dock_extent(DockSide::Left), 8.0);
    assert_eq!(manager.dock_extent(DockSide::Right), 8.0);
    assert_eq!(manager.dock_extent(DockSide::Bottom), 8.0);

    // A static nest in a usable viewport still paints its content and bars;
    // the full frame must complete. (Sub-24pt viewports are collapsed and
    // render nothing by design; the degenerate scrollbar geometry they used to
    // exercise is covered by `scrollbar_geometry_is_safe_for_a_degenerate_track`.)
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0)),
        color: egui::Color32::RED,
    });
    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(200.0, 200.0),
        vec![],
    );
    assert!(!output.shapes.is_empty(), "the nest frame must still paint");
}

#[test]
fn scrollbar_geometry_is_safe_for_a_degenerate_track() {
    use pyxross::ui::panel_dock::nest::scrollbar::{offset_from_thumb_pos, thumb_geometry};

    // A track narrower than MIN_THUMB must not panic; the bar stays hidden.
    assert_eq!(thumb_geometry(2.0, 600.0, 0.0, 2.0), None);
    assert_eq!(thumb_geometry(2.0, 600.0, 0.0, 0.0), None);
    assert_eq!(thumb_geometry(2.0, 600.0, 0.0, 15.0), None);
    // A track exactly MIN_THUMB still yields a full-track thumb.
    let geo = thumb_geometry(2.0, 600.0, 0.0, 16.0).unwrap();
    assert_eq!(geo.thumb_len, 16.0);
    assert_eq!(geo.thumb_pos, 0.0);
    // The inverse mapping is safe for degenerate inputs too.
    let offset = offset_from_thumb_pos(0.0, 16.0, 16.0, 600.0, 2.0);
    assert!(offset.is_finite());
}

#[test]
fn drag_out_requires_a_deterministic_two_axis_threshold() {
    assert!(!drag_outside_threshold(
        pos2(100.0, 100.0),
        pos2(108.0, 108.0)
    ));
    assert!(drag_outside_threshold(
        pos2(100.0, 100.0),
        pos2(112.0, 112.0)
    ));
}

#[test]
fn capability_controls_pop_out_without_removing_the_panel_instance() {
    let mut manager = DockManager::demo();
    let panel = PanelId::new(2);

    assert!(!manager.metadata(panel).unwrap().can_pop_out);
    assert!(manager
        .transition_panel(panel, PanelPlacement::Floating)
        .is_err());
    assert_eq!(manager.placement(panel), Some(PanelPlacement::DockedRight));
    assert_eq!(manager.panel_count(), 4);
}

#[test]
fn panel_d_supports_dock_and_floating_transitions() {
    let mut manager = DockManager::demo();
    let panel = PanelId::new(4);

    assert!(manager.metadata(panel).unwrap().can_pop_out);
    manager
        .transition_panel(panel, PanelPlacement::Floating)
        .unwrap();
    assert_eq!(manager.placement(panel), Some(PanelPlacement::Floating));
    manager
        .transition_panel(panel, PanelPlacement::DockedRight)
        .unwrap();
    assert_eq!(manager.placement(panel), Some(PanelPlacement::DockedRight));
}

#[test]
fn panel_headers_derive_action_labels_from_capability_and_placement() {
    let mut manager = DockManager::demo();

    assert_eq!(
        manager.header_action(PanelId::new(1)),
        Some(PanelHeaderAction::Float)
    );
    assert_eq!(
        manager.header_action(PanelId::new(4)),
        Some(PanelHeaderAction::Float)
    );
    assert_eq!(manager.header_action(PanelId::new(2)), None);

    manager
        .transition_panel(PanelId::new(4), PanelPlacement::Floating)
        .unwrap();
    assert_eq!(
        manager.header_action(PanelId::new(4)),
        Some(PanelHeaderAction::Dock)
    );
}

#[test]
fn panel_a_no_longer_offers_native_pop_out() {
    let manager = DockManager::demo();
    let panel_a = PanelId::new(1);

    assert!(!manager.metadata(panel_a).unwrap().can_native_pop_out);
    assert_ne!(
        manager.header_action(panel_a),
        Some(PanelHeaderAction::PopOut)
    );
}

#[test]
fn detached_panels_restore_their_previous_dock_side_and_order() {
    let mut manager = DockManager::demo();
    let panel_a = PanelId::new(1);
    let panel_d = PanelId::new(4);

    manager
        .transition_panel(panel_d, PanelPlacement::DockedBottom)
        .unwrap();
    manager.reorder(DockSide::Bottom, panel_d, 0).unwrap();
    manager
        .transition_panel(panel_d, PanelPlacement::Floating)
        .unwrap();
    manager.restore_panel(panel_d).unwrap();
    assert_eq!(
        manager.placement(panel_d),
        Some(PanelPlacement::DockedBottom)
    );
    assert_eq!(
        manager.dock_order(DockSide::Bottom),
        &[panel_d, PanelId::new(3)]
    );

    manager
        .transition_panel(panel_a, PanelPlacement::DockedBottom)
        .unwrap();
    manager.reorder(DockSide::Bottom, panel_a, 1).unwrap();
    manager.transition_panel_a_to_native().unwrap();
    manager.restore_panel_a().unwrap();
    assert_eq!(
        manager.placement(panel_a),
        Some(PanelPlacement::DockedBottom)
    );
    assert_eq!(
        manager.dock_order(DockSide::Bottom),
        &[panel_d, panel_a, PanelId::new(3)]
    );
}

#[test]
fn panel_a_supports_float_and_pop_out_transitions() {
    let mut manager = DockManager::demo();
    let panel = PanelId::new(1);

    assert!(manager.metadata(panel).unwrap().can_pop_out);
    manager
        .transition_panel(panel, PanelPlacement::Floating)
        .unwrap();
    assert_eq!(manager.placement(panel), Some(PanelPlacement::Floating));
    manager
        .transition_panel(panel, PanelPlacement::DockedRight)
        .unwrap();
    assert_eq!(manager.placement(panel), Some(PanelPlacement::DockedRight));
}

#[test]
fn panel_a_dock_action_maps_to_the_canonical_workspace_identity() {
    assert_eq!(
        DockAction::PopOut(PanelId::new(1)).canonical_panel(),
        Some(pyxross::ui::panel_registry::PanelId::PanelA)
    );
    assert_eq!(DockAction::PopOut(PanelId::new(2)).canonical_panel(), None);
}

#[test]
fn only_panel_a_can_enter_native_presentation_and_restore_without_duplication() {
    let mut manager = DockManager::demo();

    manager.transition_panel_a_to_native().unwrap();
    assert_eq!(
        manager.placement(PanelId::new(1)),
        Some(PanelPlacement::Native)
    );
    assert_eq!(manager.panel_count(), 4);
    assert!(manager
        .transition_panel(PanelId::new(2), PanelPlacement::Native)
        .is_err());

    manager.restore_panel_a().unwrap();
    assert_eq!(
        manager.placement(PanelId::new(1)),
        Some(PanelPlacement::DockedRight)
    );
    assert_eq!(manager.panel_count(), 4);
}

#[test]
fn floating_panel_keeps_a_resizable_body_when_content_is_empty() {
    let ctx = egui::Context::default();
    let available_size = Rc::new(Cell::new(egui::Vec2::ZERO));
    let mut manager = DockManager::try_new([PanelSpec {
        id: PanelId::new(4),
        metadata: PanelMetadata::new("Floating", true, vec2(180.0, 120.0)),
        placement: PanelPlacement::Floating,
        floating_rect: Rect::from_min_size(pos2(20.0, 20.0), vec2(260.0, 190.0)),
        content: Box::new(RecordingContent {
            available_size: Rc::clone(&available_size),
        }),
    }])
    .unwrap();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();

    for _ in 0..2 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .show(ui, |ui| manager.show_inside(ui, &chrome.view(&theme)));
            },
        );
        output.textures_delta.clear();
    }

    assert!(
        available_size.get().y > 0.0,
        "floating body height: {:?}",
        available_size.get()
    );
    let window_rect =
        ctx.memory(|memory| memory.area_rect(egui::Id::new(("panel-dock-floating", 4))));
    assert!(window_rect.expect("floating window rect").height() > 40.0);
}

#[test]
fn floating_panel_header_drag_moves_the_window() {
    let ctx = egui::Context::default();
    let mut manager = floating_manager();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let frame = |manager: &mut DockManager, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .show(ui, |ui| manager.show_inside(ui, &chrome.view(&theme)));
            },
        );
        output.textures_delta.clear();
        ctx.memory(|memory| memory.area_rect(egui::Id::new(("panel-dock-floating", 9))))
            .unwrap()
    };

    let initial = frame(
        &mut manager,
        vec![egui::Event::PointerMoved(pos2(50.0, 35.0))],
    );
    frame(
        &mut manager,
        vec![egui::Event::PointerButton {
            pos: pos2(50.0, 35.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    frame(
        &mut manager,
        vec![egui::Event::PointerMoved(pos2(100.0, 85.0))],
    );
    let moved = frame(&mut manager, Vec::new());

    assert!(
        moved.min.x > initial.min.x,
        "floating window did not move: {initial:?} -> {moved:?}"
    );
    assert!(
        moved.min.y > initial.min.y,
        "floating window did not move: {initial:?} -> {moved:?}"
    );
}

#[test]
fn floating_panel_body_press_does_not_move_the_window() {
    let ctx = egui::Context::default();
    let mut manager = floating_manager();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let frame = |manager: &mut DockManager, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .show(ui, |ui| manager.show_inside(ui, &chrome.view(&theme)));
            },
        );
        output.textures_delta.clear();
        ctx.memory(|memory| memory.area_rect(egui::Id::new(("panel-dock-floating", 9))))
            .unwrap()
    };

    let initial = frame(
        &mut manager,
        vec![egui::Event::PointerMoved(pos2(80.0, 100.0))],
    );
    frame(
        &mut manager,
        vec![egui::Event::PointerButton {
            pos: pos2(80.0, 100.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    frame(
        &mut manager,
        vec![egui::Event::PointerMoved(pos2(140.0, 150.0))],
    );
    let settled = frame(&mut manager, Vec::new());

    assert_eq!(
        settled.min, initial.min,
        "floating body press moved the window: {initial:?} -> {settled:?}"
    );
}

#[test]
fn floating_panel_pop_out_button_press_does_not_start_a_drag() {
    let ctx = egui::Context::default();
    let mut manager = floating_manager();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let frame = |manager: &mut DockManager, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .show(ui, |ui| manager.show_inside(ui, &chrome.view(&theme)));
            },
        );
        output.textures_delta.clear();
        ctx.memory(|memory| memory.area_rect(egui::Id::new(("panel-dock-floating", 9))))
            .unwrap()
    };

    let initial = frame(
        &mut manager,
        vec![egui::Event::PointerMoved(pos2(245.0, 35.0))],
    );
    frame(
        &mut manager,
        vec![egui::Event::PointerButton {
            pos: pos2(245.0, 35.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    frame(
        &mut manager,
        vec![egui::Event::PointerMoved(pos2(320.0, 95.0))],
    );
    let settled = frame(&mut manager, Vec::new());

    assert_eq!(
        manager.placement(PanelId::new(9)),
        Some(PanelPlacement::Floating)
    );
    assert_eq!(
        settled.min, initial.min,
        "Pop out button press moved the window: {initial:?} -> {settled:?}"
    );
}

#[test]
fn docked_panel_drag_out_preserves_the_grab_offset() {
    let ctx = egui::Context::default();
    let mut manager = DockManager::demo();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let frame = |manager: &mut DockManager, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .show(ui, |ui| manager.show_inside(ui, &chrome.view(&theme)));
            },
        );
        output.textures_delta.clear();
    };

    frame(
        &mut manager,
        vec![egui::Event::PointerMoved(pos2(600.0, 250.0))],
    );
    frame(
        &mut manager,
        vec![egui::Event::PointerButton {
            pos: pos2(600.0, 250.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    frame(
        &mut manager,
        vec![egui::Event::PointerMoved(pos2(400.0, 300.0))],
    );
    frame(
        &mut manager,
        vec![egui::Event::PointerButton {
            pos: pos2(400.0, 300.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );

    assert_eq!(
        manager.placement(PanelId::new(4)),
        Some(PanelPlacement::Floating)
    );
    assert_eq!(
        manager.floating_rect(PanelId::new(4)).unwrap().min,
        pos2(352.0, 298.0)
    );
}

#[test]
fn headless_demo_renders_each_stable_dummy_panel_once() {
    let ctx = egui::Context::default();
    let mut manager = DockManager::demo();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let first = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default()
                .show(ui, |ui| manager.show_inside(ui, &chrome.view(&theme)));
        },
    );
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default()
                .show(ui, |ui| manager.show_inside(ui, &chrome.view(&theme)));
        },
    );
    let mut first = first;
    first.textures_delta.clear();
    let texts = rendered_texts(&output);
    output.textures_delta.clear();

    assert!(texts.iter().any(|text| text == "Preview"), "{texts:?}");
    assert!(texts.iter().any(|text| text == "Panel B"), "{texts:?}");
    assert!(texts.iter().any(|text| text == "Panel C"), "{texts:?}");
    assert!(texts.iter().any(|text| text == "Panel D"), "{texts:?}");
    assert!(
        !texts.iter().any(|text| text == "Pulse content"),
        "{texts:?}"
    );
}

/// Risk validation for the dynamic nest: egui 0.36 `Context::set_transform_layer`
/// + `TSTransform` must map a known world rect to the expected screen rect at
/// scale 2, and the layer's clip rect must land on the viewport. If this test
/// fails, the dynamic nest falls back to manual painting with an explicit
/// scale passed to components (documented contingency).
#[test]
fn transform_layer_at_scale_two_maps_world_rects_and_clips_to_the_viewport() {
    let ctx = egui::Context::default();
    let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::new("nest-transform-probe"));
    // translation (100, 50), scale 2: screen = 2 * world + (100, 50).
    let transform = egui::emath::TSTransform::new(vec2(100.0, 50.0), 2.0);
    ctx.set_transform_layer(layer, transform);

    let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0));
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(viewport),
            ..Default::default()
        },
        |ctx| {
            // The nest clips its content layer to the world-space viewport
            // rect; after the transform it must land exactly on the screen
            // viewport. With translation (100,50) and scale 2 the world clip
            // is (-50,-25,640x360): 2*(-50,-25)+(100,50) = (0,0).
            let mut painter = ctx.layer_painter(layer);
            painter.set_clip_rect(Rect::from_min_size(pos2(-50.0, -25.0), vec2(640.0, 360.0)));
            // World rect (10,10,20x20) -> screen (120,70,40x40).
            painter.rect_filled(
                Rect::from_min_size(pos2(10.0, 10.0), vec2(20.0, 20.0)),
                0.0,
                egui::Color32::RED,
            );
            // World rect far outside the viewport: (1000,1000,20x20) -> (2100,2050).
            painter.rect_filled(
                Rect::from_min_size(pos2(1000.0, 1000.0), vec2(20.0, 20.0)),
                0.0,
                egui::Color32::GREEN,
            );
        },
    );

    let rects: Vec<(Rect, Rect)> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) => Some((clipped.clip_rect, rect.rect)),
            _ => None,
        })
        .collect();
    output.textures_delta.clear();
    assert_eq!(
        rects.len(),
        2,
        "both probe rects must be emitted: {rects:?}"
    );
    for (clip_rect, _) in &rects {
        assert_eq!(
            *clip_rect, viewport,
            "world-space clip rect must land on the viewport after the transform"
        );
    }
    assert_eq!(
        rects[0].1,
        Rect::from_min_size(pos2(120.0, 70.0), vec2(40.0, 40.0)),
        "world rect must land at the expected screen rect at scale 2"
    );
    assert_eq!(
        rects[1].1,
        Rect::from_min_size(pos2(2100.0, 2050.0), vec2(40.0, 40.0)),
        "out-of-viewport world rect must still transform correctly"
    );
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("pyxross_dock_skin_{}_{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_rgba_png(path: &std::path::Path, width: u32, height: u32) {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut out), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        let data = vec![255u8; (width * height * 4) as usize];
        writer.write_image_data(&data).expect("png data");
        writer.finish().expect("png finish");
    }
    std::fs::write(path, out).unwrap();
}

fn skin_with_atlas(atlas: &str) -> pyxross::ui::theme::Skin {
    let src = |x: u32| pyxross::ui::theme::NineSliceSource {
        x,
        y: 0,
        width: 32,
        height: 32,
        corner_size: 4,
    };
    pyxross::ui::theme::Skin {
        atlas_path: atlas.to_string(),
        normal: src(0),
        hover: src(32),
        pressed: src(64),
        disabled: src(96),
    }
}

fn skinned_theme() -> pyxross::ui::theme::Theme {
    let mut theme = pyxross::ui::theme::Theme::default_dark();
    for name in ["panel_bg", "panel_header", "button", "scrollbar_handle"] {
        theme
            .skins
            .insert(name.to_string(), skin_with_atlas("atlas.png"));
    }
    theme
}

fn run_demo_frame(
    ctx: &egui::Context,
    manager: &mut DockManager,
    chrome: &pyxross::ui::panel_dock::skin::PanelChrome,
) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| manager.show_inside(ui, chrome));
        },
    );
    output.textures_delta.clear();
    output
}

/// Like [`run_demo_frame`] but also captures the CentralPanel's actual
/// viewport rect, which the dock geometry is computed against.
fn run_demo_frame_capturing_viewport(
    ctx: &egui::Context,
    manager: &mut DockManager,
    chrome: &pyxross::ui::panel_dock::skin::PanelChrome,
) -> (egui::FullOutput, Rect) {
    let mut viewport = Rect::NOTHING;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                viewport = ui.max_rect();
                manager.show_inside(ui, chrome);
            });
        },
    );
    output.textures_delta.clear();
    (output, viewport)
}

/// Whether two rects share more than a shared edge (positive-area overlap).
fn overlaps(a: Rect, b: Rect) -> bool {
    let overlap = a.intersect(b);
    overlap.width() > 0.5 && overlap.height() > 0.5
}

fn mesh_bounds(mesh: &egui::Mesh) -> Rect {
    let mut min = mesh.vertices[0].pos;
    let mut max = mesh.vertices[0].pos;
    for vertex in &mesh.vertices {
        min = min.min(vertex.pos);
        max = max.max(vertex.pos);
    }
    Rect::from_min_max(min, max)
}

fn atlas_mesh_bounds(output: &egui::FullOutput, atlas_id: egui::TextureId) -> Vec<Rect> {
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Mesh(mesh) if mesh.texture_id == atlas_id => Some(mesh_bounds(mesh)),
            _ => None,
        })
        .collect()
}

fn dock_rects(manager: &DockManager, viewport: Rect) -> (Rect, Rect) {
    let right_rect = Rect::from_min_max(
        pos2(
            viewport.right() - manager.dock_extent(DockSide::Right),
            viewport.top(),
        ),
        viewport.max,
    );
    let bottom_rect = Rect::from_min_max(
        pos2(
            viewport.left(),
            viewport.bottom() - manager.dock_extent(DockSide::Bottom),
        ),
        pos2(right_rect.left(), viewport.bottom()),
    );
    (right_rect, bottom_rect)
}

#[test]
fn skinned_chrome_emits_atlas_textured_meshes() {
    let ctx = egui::Context::default();
    let dir = temp_dir("skinned");
    let atlas_path = dir.join("atlas.png");
    write_rgba_png(&atlas_path, 128, 192);
    let (bytes, size) = pyxross::ui::theme::load_atlas_with_size(&atlas_path).unwrap();
    let image =
        egui::ColorImage::from_rgba_unmultiplied([size.0 as usize, size.1 as usize], &bytes);
    let texture = ctx.load_texture("test-atlas", image, egui::TextureOptions::NEAREST);
    let mut cache = pyxross::ui::panel_dock::skin::AtlasCache::new();
    cache.insert(
        std::path::PathBuf::from("atlas.png"),
        pyxross::ui::panel_dock::skin::SkinAtlas::new(texture.clone(), size),
    );
    let theme = skinned_theme();
    let chrome = ChromeHarness::new();
    let mut manager = DockManager::demo();
    let output = run_demo_frame(&ctx, &mut manager, &chrome.skinned(&theme, &cache));

    let atlas_id = texture.id();
    let mut mesh_count = 0;
    for clipped in &output.shapes {
        if let egui::Shape::Mesh(mesh) = &clipped.shape {
            if mesh.texture_id == atlas_id {
                mesh_count += 1;
            }
        }
    }
    assert!(
        mesh_count > 0,
        "skinned chrome must emit at least one atlas-textured mesh"
    );
}

#[test]
fn skinned_docked_panels_each_paint_their_own_frame() {
    let ctx = egui::Context::default();
    let dir = temp_dir("skinned-frames");
    let atlas_path = dir.join("atlas.png");
    write_rgba_png(&atlas_path, 128, 192);
    let (bytes, size) = pyxross::ui::theme::load_atlas_with_size(&atlas_path).unwrap();
    let image =
        egui::ColorImage::from_rgba_unmultiplied([size.0 as usize, size.1 as usize], &bytes);
    let texture = ctx.load_texture("test-atlas", image, egui::TextureOptions::NEAREST);
    let mut cache = pyxross::ui::panel_dock::skin::AtlasCache::new();
    cache.insert(
        std::path::PathBuf::from("atlas.png"),
        pyxross::ui::panel_dock::skin::SkinAtlas::new(texture.clone(), size),
    );
    let theme = skinned_theme();
    let chrome = ChromeHarness::new();
    let mut manager = DockManager::demo();
    let mut viewport = Rect::NOTHING;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                viewport = ui.max_rect();
                manager.show_inside(ui, &chrome.skinned(&theme, &cache));
            });
        },
    );
    output.textures_delta.clear();

    let (right_rect, bottom_rect) = dock_rects(&manager, viewport);
    let mut panel_rects: Vec<Rect> = manager
        .panel_rects(DockSide::Right, right_rect)
        .into_iter()
        .map(|(_, rect)| rect)
        .collect();
    panel_rects.extend(
        manager
            .panel_rects(DockSide::Bottom, bottom_rect)
            .into_iter()
            .map(|(_, rect)| rect),
    );
    assert_eq!(panel_rects.len(), 4, "demo must dock 4 panels");

    let frame_bounds = atlas_mesh_bounds(&output, texture.id());
    for panel_rect in &panel_rects {
        assert!(
            frame_bounds.contains(panel_rect),
            "missing panel_bg frame mesh for {panel_rect:?}; atlas mesh bounds: {frame_bounds:?}"
        );
    }

    let border_strokes: Vec<egui::Stroke> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) => Some(rect.stroke),
            _ => None,
        })
        .collect();
    assert!(
        !border_strokes.contains(&egui::Stroke::new(1.0, theme.colors.panel_border32())),
        "skinned dock must skip the flat outline stroke"
    );
}

#[test]
fn flat_chrome_emits_no_meshes_and_uses_theme_tokens() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut manager = DockManager::demo();
    let output = run_demo_frame(&ctx, &mut manager, &chrome.view(&theme));

    let meshes: Vec<_> = output
        .shapes
        .iter()
        .filter(|clipped| matches!(&clipped.shape, egui::Shape::Mesh(_)))
        .collect();
    assert!(meshes.is_empty(), "flat chrome must not emit meshes");

    let fills: Vec<egui::Color32> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) => Some(rect.fill),
            _ => None,
        })
        .collect();
    assert!(
        fills.contains(&theme.colors.panel_bg32()),
        "dock background fill must use panel_bg"
    );
    assert!(
        fills.contains(&theme.colors.panel_header_bg32()),
        "panel header fill must use panel_header_bg"
    );
    assert!(
        fills.contains(&theme.colors.panel_border32()),
        "resize handle fill must use panel_border"
    );

    let strokes: Vec<egui::Stroke> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) => Some(rect.stroke),
            _ => None,
        })
        .collect();
    assert!(
        strokes.contains(&egui::Stroke::new(1.0, theme.colors.panel_border32())),
        "flat dock must keep its outline stroke"
    );
}

// ---------------------------------------------------------------------------
// Collapsed-dock panel visibility tests
// ---------------------------------------------------------------------------

#[test]
fn collapsed_dock_paints_no_panel_chrome() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut manager = DockManager::demo();
    let screen = vec2(1280.0, 720.0);
    manager.resize_dock_area(DockSide::Right, -10_000.0, screen);
    assert_eq!(manager.dock_extent(DockSide::Right), 8.0);

    let (output, viewport) =
        run_demo_frame_capturing_viewport(&ctx, &mut manager, &chrome.view(&theme));
    let (right_rect, bottom_rect) = dock_rects(&manager, viewport);

    // The dock's own frame strip still paints.
    let backgrounds = rect_fills(&output, theme.colors.panel_bg32());
    assert!(
        backgrounds.contains(&right_rect),
        "the collapsed dock strip must still paint: {backgrounds:?}"
    );

    // No panel chrome may paint inside the collapsed strip.
    let headers = rect_fills(&output, theme.colors.panel_header_bg32());
    for header in &headers {
        assert!(
            !overlaps(*header, right_rect),
            "collapsed right dock painted a panel header band: {header:?}"
        );
    }
    let texts = rendered_texts(&output);
    for title in ["Preview", "Panel B", "Panel D"] {
        assert!(
            !texts.iter().any(|text| text == title),
            "collapsed right dock painted title {title}: {texts:?}"
        );
    }

    // Control: the expanded bottom dock still renders its panel.
    assert!(
        texts.iter().any(|text| text == "Panel C"),
        "the expanded bottom dock must still paint: {texts:?}"
    );
    assert!(
        headers.iter().any(|header| overlaps(*header, bottom_rect)),
        "the expanded bottom dock must keep its header band"
    );
}

#[test]
fn collapsed_bottom_dock_paints_no_panel_chrome() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut manager = DockManager::demo();
    let screen = vec2(1280.0, 720.0);
    manager.resize_dock_area(DockSide::Bottom, -10_000.0, screen);
    assert_eq!(manager.dock_extent(DockSide::Bottom), 8.0);

    let (output, viewport) =
        run_demo_frame_capturing_viewport(&ctx, &mut manager, &chrome.view(&theme));
    let (right_rect, bottom_rect) = dock_rects(&manager, viewport);

    let backgrounds = rect_fills(&output, theme.colors.panel_bg32());
    assert!(
        backgrounds.contains(&bottom_rect),
        "the collapsed dock strip must still paint: {backgrounds:?}"
    );

    let headers = rect_fills(&output, theme.colors.panel_header_bg32());
    for header in &headers {
        assert!(
            !overlaps(*header, bottom_rect),
            "collapsed bottom dock painted a panel header band: {header:?}"
        );
    }
    let texts = rendered_texts(&output);
    assert!(
        !texts.iter().any(|text| text == "Panel C"),
        "collapsed bottom dock painted its title: {texts:?}"
    );
    for title in ["Preview", "Panel B", "Panel D"] {
        assert!(
            texts.iter().any(|text| text == title),
            "the expanded right dock must still paint {title}: {texts:?}"
        );
    }
    assert!(
        headers.iter().any(|header| overlaps(*header, right_rect)),
        "the expanded right dock must keep its header bands"
    );
}

#[test]
fn all_demo_panels_are_hidden_when_their_dock_is_collapsed() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut manager = DockManager::demo();
    let screen = vec2(1280.0, 720.0);
    manager.resize_dock_area(DockSide::Right, -10_000.0, screen);
    manager.resize_dock_area(DockSide::Bottom, -10_000.0, screen);

    let (output, viewport) =
        run_demo_frame_capturing_viewport(&ctx, &mut manager, &chrome.view(&theme));
    let (right_rect, bottom_rect) = dock_rects(&manager, viewport);

    let texts = rendered_texts(&output);
    for title in ["Preview", "Panel B", "Panel C", "Panel D"] {
        assert!(
            !texts.iter().any(|text| text == title),
            "collapsed docks must hide {title}: {texts:?}"
        );
    }
    assert!(
        rect_fills(&output, theme.colors.panel_header_bg32()).is_empty(),
        "collapsed docks must paint no header bands"
    );
    let backgrounds = rect_fills(&output, theme.colors.panel_bg32());
    assert!(
        backgrounds.contains(&right_rect) && backgrounds.contains(&bottom_rect),
        "both dock frame strips must still paint: {backgrounds:?}"
    );

    // Growing the docks back restores every demo panel.
    manager.resize_dock_area(DockSide::Right, 232.0, screen);
    manager.resize_dock_area(DockSide::Bottom, 172.0, screen);
    let (output, _) = run_demo_frame_capturing_viewport(&ctx, &mut manager, &chrome.view(&theme));
    let texts = rendered_texts(&output);
    for title in ["Preview", "Panel B", "Panel C", "Panel D"] {
        assert!(
            texts.iter().any(|text| text == title),
            "grown docks must restore {title}: {texts:?}"
        );
    }
}

#[test]
fn panel_chrome_returns_after_growing_the_dock() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let dynamic_nest = Rc::new(std::cell::RefCell::new(NestContent::new(NestMode::Dynamic)));
    dynamic_nest
        .borrow_mut()
        .camera_mut()
        .zoom_around(pos2(0.0, 0.0), pos2(0.0, 0.0), 2.0);
    dynamic_nest
        .borrow_mut()
        .camera_mut()
        .pan_by(vec2(30.0, -12.0));
    let static_nest = Rc::new(std::cell::RefCell::new(NestContent::new(NestMode::Static)));
    static_nest.borrow_mut().push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0)),
        color: egui::Color32::RED,
    });
    let capture = InputCapture::new();
    let surface = SurfaceId::Panel(PanelId::new(2));
    capture.set_target(Some(surface));
    let chrome = PanelChrome::flat(&theme, &capture, surface);

    let mut manager = DockManager::try_new([
        panel_spec(
            1,
            "Docked",
            PanelPlacement::DockedRight,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
            Box::new(SharedNest(Rc::clone(&dynamic_nest))),
        ),
        panel_spec(
            2,
            "Scrolling",
            PanelPlacement::DockedBottom,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
            Box::new(SharedNest(Rc::clone(&static_nest))),
        ),
        panel_spec(
            3,
            "Floating",
            PanelPlacement::Floating,
            Rect::from_min_size(pos2(20.0, 20.0), vec2(260.0, 190.0)),
            Box::new(EmptyContent),
        ),
    ])
    .unwrap();
    let screen = vec2(1280.0, 720.0);
    let right_before = manager.dock_extent(DockSide::Right);
    let bottom_before = manager.dock_extent(DockSide::Bottom);
    let (camera_scale, camera_offset) = (
        dynamic_nest.borrow().camera().scale(),
        dynamic_nest.borrow().camera().offset(),
    );

    // Scroll the bottom static nest so it carries a non-zero offset.
    let frame = |manager: &mut DockManager, events: Vec<egui::Event>| {
        let mut viewport = Rect::NOTHING;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    viewport = ui.max_rect();
                    manager.show_inside(ui, &chrome);
                });
            },
        );
        output.textures_delta.clear();
        (output, viewport)
    };
    let (_, viewport) = frame(&mut manager, vec![]);
    let (right_rect_before, bottom_rect_before) = dock_rects(&manager, viewport);
    let right_panels_before = manager.panel_rects(DockSide::Right, right_rect_before);
    let bottom_panels_before = manager.panel_rects(DockSide::Bottom, bottom_rect_before);
    let floating_before = manager.floating_rect(PanelId::new(3)).unwrap();
    let bottom_center = bottom_panels_before[0].1.center();
    frame(
        &mut manager,
        vec![
            egui::Event::PointerMoved(bottom_center),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -5.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            },
        ],
    );
    let offset_before = static_nest.borrow().offset();
    assert!(
        offset_before.y > 0.0,
        "the static nest must scroll: {offset_before:?}"
    );

    // Collapse both docks: the panels vanish, the floating panel does not.
    manager.resize_dock_area(DockSide::Right, -10_000.0, screen);
    manager.resize_dock_area(DockSide::Bottom, -10_000.0, screen);
    let (output, _) = run_demo_frame_capturing_viewport(&ctx, &mut manager, &chrome);
    let texts = rendered_texts(&output);
    for title in ["Docked", "Scrolling"] {
        assert!(
            !texts.iter().any(|text| text == title),
            "collapsed docks must hide {title}: {texts:?}"
        );
    }
    assert!(
        texts.iter().any(|text| text == "Floating"),
        "floating panels are not in a dock and must keep painting: {texts:?}"
    );

    // Grow the docks back: geometry, camera, scroll offset and the floating
    // rect are all untouched.
    manager.resize_dock_area(DockSide::Right, right_before - 8.0, screen);
    manager.resize_dock_area(DockSide::Bottom, bottom_before - 8.0, screen);
    let (output, _) = run_demo_frame_capturing_viewport(&ctx, &mut manager, &chrome);

    assert_eq!(manager.dock_extent(DockSide::Right), right_before);
    assert_eq!(manager.dock_extent(DockSide::Bottom), bottom_before);
    let (right_rect_after, bottom_rect_after) = dock_rects(&manager, viewport);
    assert_eq!(
        manager.panel_rects(DockSide::Right, right_rect_after),
        right_panels_before
    );
    assert_eq!(
        manager.panel_rects(DockSide::Bottom, bottom_rect_after),
        bottom_panels_before
    );
    assert_eq!(
        manager.floating_rect(PanelId::new(3)).unwrap(),
        floating_before
    );
    assert_eq!(dynamic_nest.borrow().camera().scale(), camera_scale);
    assert_eq!(dynamic_nest.borrow().camera().offset(), camera_offset);
    assert_eq!(static_nest.borrow().offset(), offset_before);

    let texts = rendered_texts(&output);
    for title in ["Docked", "Scrolling", "Floating"] {
        assert!(
            texts.iter().any(|text| text == title),
            "grown docks must restore {title}: {texts:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Panel/nest collapse-threshold alignment tests
// ---------------------------------------------------------------------------

fn static_probe_nest(color: egui::Color32) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0)),
        color,
    });
    nest
}

fn dynamic_probe_nest(color: egui::Color32) -> NestContent {
    let mut nest = NestContent::new(NestMode::Dynamic);
    nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(20.0, 20.0)),
        color,
    });
    nest
}

/// Painted content shapes of `color` with their clip rects.
fn content_shapes(output: &egui::FullOutput, color: egui::Color32) -> Vec<(Rect, Rect)> {
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) if rect.fill == color => Some((rect.rect, clipped.clip_rect)),
            _ => None,
        })
        .collect()
}

/// Asserts the panel chrome (its title) and its nest content switch together
/// at this dock extent, and that painted content never overflows the panel
/// rect (its clip stays inside, on a usable nest viewport).
fn assert_panel_and_content_switch(
    output: &egui::FullOutput,
    panel_rect: Rect,
    title: &str,
    content_color: egui::Color32,
    extent: i32,
) {
    let chrome_painted = rendered_texts(output).iter().any(|text| text == title);
    let content = content_shapes(output, content_color);
    let content_painted = !content.is_empty();
    assert_eq!(
        chrome_painted, content_painted,
        "panel chrome and nest content must switch together at extent {extent}: panel {panel_rect:?}, chrome={chrome_painted}, content={content_painted}"
    );
    if content_painted {
        for (_, clip) in &content {
            assert!(
                nest_is_usable(*clip),
                "painted content implies a usable nest viewport at extent {extent}: {clip:?}"
            );
            assert!(
                panel_rect.contains_rect(*clip),
                "content clip must stay inside the panel at extent {extent}: {clip:?} vs {panel_rect:?}"
            );
        }
    }
}

/// Sweeps the dock extent across the collapse boundary in 1px steps and
/// asserts the panel chrome and its nest content appear/disappear together at
/// every step, with no overflow while content is painted.
#[test]
fn panel_and_nest_content_switch_at_the_same_threshold() {
    let ctx = egui::Context::default();
    let dir = temp_dir("threshold-width");
    let atlas_path = dir.join("atlas.png");
    write_rgba_png(&atlas_path, 128, 192);
    let (bytes, size) = pyxross::ui::theme::load_atlas_with_size(&atlas_path).unwrap();
    let image =
        egui::ColorImage::from_rgba_unmultiplied([size.0 as usize, size.1 as usize], &bytes);
    let texture = ctx.load_texture("threshold-atlas", image, egui::TextureOptions::NEAREST);
    let mut cache = pyxross::ui::panel_dock::skin::AtlasCache::new();
    cache.insert(
        std::path::PathBuf::from("atlas.png"),
        pyxross::ui::panel_dock::skin::SkinAtlas::new(texture.clone(), size),
    );
    let theme = skinned_theme();
    let chrome = ChromeHarness::new();
    let screen = vec2(1280.0, 720.0);

    // Width axis: a right-docked panel's width is the dock extent; its height
    // is the full viewport, so only the width crosses the boundary.
    let mut width_manager = DockManager::try_new([panel_spec(
        1,
        "Width probe",
        PanelPlacement::DockedRight,
        Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
        Box::new(static_probe_nest(egui::Color32::RED)),
    )])
    .unwrap();
    for extent in 8..=80 {
        width_manager.resize_dock_area(
            DockSide::Right,
            extent as f32 - width_manager.dock_extent(DockSide::Right),
            screen,
        );
        let (output, viewport) = run_demo_frame_capturing_viewport(
            &ctx,
            &mut width_manager,
            &chrome.skinned(&theme, &cache),
        );
        let (right_rect, _) = dock_rects(&width_manager, viewport);
        let panel_rect = width_manager.panel_rects(DockSide::Right, right_rect)[0].1;
        assert_panel_and_content_switch(
            &output,
            panel_rect,
            "Width probe",
            egui::Color32::RED,
            extent,
        );
    }

    // Height axis: a bottom-docked panel's height is the dock extent.
    let mut height_manager = DockManager::try_new([panel_spec(
        1,
        "Height probe",
        PanelPlacement::DockedBottom,
        Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
        Box::new(static_probe_nest(egui::Color32::RED)),
    )])
    .unwrap();
    for extent in 8..=100 {
        height_manager.resize_dock_area(
            DockSide::Bottom,
            extent as f32 - height_manager.dock_extent(DockSide::Bottom),
            screen,
        );
        let (output, viewport) = run_demo_frame_capturing_viewport(
            &ctx,
            &mut height_manager,
            &chrome.skinned(&theme, &cache),
        );
        let (_, bottom_rect) = dock_rects(&height_manager, viewport);
        let panel_rect = height_manager.panel_rects(DockSide::Bottom, bottom_rect)[0].1;
        assert_panel_and_content_switch(
            &output,
            panel_rect,
            "Height probe",
            egui::Color32::RED,
            extent,
        );
    }
}

/// The two panels the user named — the layers panel (static nest) and Panel B
/// (dynamic nest) — hide together with their content at the same boundary.
#[test]
fn layers_panel_and_panel_b_hide_together_with_their_content() {
    let ctx = egui::Context::default();
    let dir = temp_dir("threshold-panels");
    let atlas_path = dir.join("atlas.png");
    write_rgba_png(&atlas_path, 128, 192);
    let (bytes, size) = pyxross::ui::theme::load_atlas_with_size(&atlas_path).unwrap();
    let image =
        egui::ColorImage::from_rgba_unmultiplied([size.0 as usize, size.1 as usize], &bytes);
    let texture = ctx.load_texture(
        "threshold-panels-atlas",
        image,
        egui::TextureOptions::NEAREST,
    );
    let mut cache = pyxross::ui::panel_dock::skin::AtlasCache::new();
    cache.insert(
        std::path::PathBuf::from("atlas.png"),
        pyxross::ui::panel_dock::skin::SkinAtlas::new(texture.clone(), size),
    );
    let theme = skinned_theme();
    let chrome = ChromeHarness::new();
    let screen = vec2(1280.0, 720.0);

    let mut manager = DockManager::try_new([
        panel_spec(
            1,
            "Layers",
            PanelPlacement::DockedRight,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
            Box::new(static_probe_nest(egui::Color32::RED)),
        ),
        panel_spec(
            2,
            "Panel B",
            PanelPlacement::DockedRight,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
            Box::new(dynamic_probe_nest(egui::Color32::BLUE)),
        ),
    ])
    .unwrap();

    for extent in 8..=80 {
        manager.resize_dock_area(
            DockSide::Right,
            extent as f32 - manager.dock_extent(DockSide::Right),
            screen,
        );
        let (output, viewport) =
            run_demo_frame_capturing_viewport(&ctx, &mut manager, &chrome.skinned(&theme, &cache));
        let (right_rect, _) = dock_rects(&manager, viewport);
        let panels = manager.panel_rects(DockSide::Right, right_rect);
        assert_eq!(
            panels.len(),
            2,
            "both panels must stay docked at extent {extent}"
        );
        for (id, panel_rect) in &panels {
            let (title, color) = if *id == PanelId::new(1) {
                ("Layers", egui::Color32::RED)
            } else {
                ("Panel B", egui::Color32::BLUE)
            };
            assert_panel_and_content_switch(&output, *panel_rect, title, color, extent);
        }
    }
}

// ---------------------------------------------------------------------------
// Nest content tests (dynamic + static modes)
// ---------------------------------------------------------------------------

use pyxross::ui::panel_dock::nest::camera::NestCamera;
use pyxross::ui::panel_dock::nest::{
    nest_is_usable, nest_viewport, Component, ComponentPlacement, NestContent, NestMode,
};

/// A content-space probe that paints a known rect.
struct ProbeRect {
    rect: egui::Rect,
    color: egui::Color32,
}

impl Component for ProbeRect {
    fn ui(&mut self, ui: &mut egui::Ui, view: &NestView, _chrome: &PanelChrome) {
        let screen = view.world_rect_to_screen(self.rect);
        ui.allocate_rect(screen, egui::Sense::hover());
        view.painter(ui).rect_filled(screen, 0.0, self.color);
    }
}

/// A screen-space overlay probe.
struct OverlayProbe {
    rect: egui::Rect,
    color: egui::Color32,
}

impl Component for OverlayProbe {
    fn placement(&self) -> ComponentPlacement {
        ComponentPlacement::Overlay
    }

    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, _chrome: &PanelChrome) {
        ui.painter().rect_filled(self.rect, 0.0, self.color);
    }
}

fn run_nest_frame(
    ctx: &egui::Context,
    nest: &mut NestContent,
    chrome: &PanelChrome,
    screen: Vec2,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Rect) {
    let mut viewport = Rect::NOTHING;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), screen)),
            events,
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                viewport = ui.max_rect();
                nest.ui(ui, chrome);
            });
        },
    );
    output.textures_delta.clear();
    (output, viewport)
}

fn count_fills(output: &egui::FullOutput, color: egui::Color32) -> usize {
    output
        .shapes
        .iter()
        .filter(|clipped| matches!(&clipped.shape, egui::Shape::Rect(rect) if rect.fill == color))
        .count()
}

fn rect_fills(output: &egui::FullOutput, color: egui::Color32) -> Vec<Rect> {
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) if rect.fill == color => Some(rect.rect),
            _ => None,
        })
        .collect()
}

/// Like [`rect_fills`] but descends into nested shape groups (egui windows
/// paint their content inside a `Shape::Vec` clip group).
fn rect_fills_recursive(output: &egui::FullOutput, color: egui::Color32) -> Vec<Rect> {
    fn visit(shape: &egui::Shape, color: egui::Color32, out: &mut Vec<Rect>) {
        match shape {
            egui::Shape::Rect(rect) if rect.fill == color => out.push(rect.rect),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| visit(shape, color, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    output
        .shapes
        .iter()
        .for_each(|clipped| visit(&clipped.shape, color, &mut out));
    out
}

fn scroll(delta: Vec2) -> egui::Event {
    // Point unit with a sub-8px delta: egui feeds it straight into the smooth
    // scroll delta (no multi-frame smoothing queue), so one event = one zoom.
    egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta,
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    }
}

fn ctrl_scroll(delta: Vec2) -> egui::Event {
    egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta,
        modifiers: egui::Modifiers::CTRL,
        phase: egui::TouchPhase::Move,
    }
}

#[test]
fn dynamic_nest_scroll_zooms_around_the_cursor_within_bounds() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Dynamic);
    let frame = |ctx: &egui::Context, nest: &mut NestContent, events: Vec<egui::Event>| {
        let (_, viewport) =
            run_nest_frame(ctx, nest, &chrome.view(&theme), vec2(800.0, 600.0), events);
        nest_viewport(viewport).min
    };

    let origin = frame(&ctx, &mut nest, vec![]);
    let pointer = pos2(300.0, 200.0);
    let world_before =
        ((pointer - origin - nest.camera().offset()) / nest.camera().scale()).to_pos2();

    // One notch of plain scroll zooms in around the cursor.
    frame(
        &ctx,
        &mut nest,
        vec![egui::Event::PointerMoved(pointer), scroll(vec2(0.0, 7.0))],
    );
    assert!(
        nest.camera().scale() > 1.0,
        "scale must grow: {}",
        nest.camera().scale()
    );
    let screen_after = nest.camera().world_to_screen(origin, world_before);
    assert!(
        (screen_after - pointer).length() < 0.01,
        "cursor anchor drifted: {screen_after:?} vs {pointer:?}"
    );

    // Zooming out repeatedly clamps at MIN_SCALE (continuous zoom needs more
    // frames than the old stepped zoom did).
    for _ in 0..600 {
        frame(
            &ctx,
            &mut nest,
            vec![egui::Event::PointerMoved(pointer), scroll(vec2(0.0, -7.0))],
        );
    }
    assert_eq!(nest.camera().scale(), NestCamera::MIN_SCALE);

    // Zooming in repeatedly clamps at MAX_SCALE.
    for _ in 0..1200 {
        frame(
            &ctx,
            &mut nest,
            vec![egui::Event::PointerMoved(pointer), scroll(vec2(0.0, 7.0))],
        );
    }
    assert_eq!(nest.camera().scale(), NestCamera::MAX_SCALE);
}

#[test]
fn dynamic_nest_middle_drag_pans_the_camera() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Dynamic);

    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let start = pos2(300.0, 200.0);
    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Middle,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerMoved(pos2(340.0, 230.0))],
    );

    assert_eq!(nest.camera().offset(), vec2(40.0, 30.0));
}

#[test]
fn dynamic_nest_consumes_scroll_so_parents_do_not_react() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Dynamic);

    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    // Plain (non-Ctrl) scroll over the nest: egui reports it as scroll delta,
    // which the nest must consume so the parent ScrollArea does not react.
    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![
            egui::Event::PointerMoved(pos2(300.0, 200.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: vec2(0.0, 1.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            },
        ],
    );

    let remaining = ctx.input(|input| input.smooth_scroll_delta);
    assert_eq!(remaining, Vec2::ZERO, "nest must consume scroll input");
}

#[test]
fn dynamic_nest_transform_layer_maps_world_rects_and_clips_to_the_viewport() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Dynamic);
    nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(10.0, 10.0), vec2(20.0, 20.0)),
        color: egui::Color32::RED,
    });
    nest.camera_mut()
        .zoom_around(pos2(0.0, 0.0), pos2(0.0, 0.0), 2.0);
    nest.camera_mut().pan_by(vec2(100.0, 50.0));

    let (output, viewport) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let inset = nest_viewport(viewport);

    let red_rects: Vec<(Rect, Rect)> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) if rect.fill == egui::Color32::RED => {
                Some((clipped.clip_rect, rect.rect))
            }
            _ => None,
        })
        .collect();
    assert_eq!(red_rects.len(), 1, "probe rect must be emitted");
    // World (10,10,20x20) at scale 2 + offset (100,50): inset origin + (120,70,40x40).
    assert_eq!(
        red_rects[0].1,
        Rect::from_min_size(inset.min + vec2(120.0, 70.0), vec2(40.0, 40.0)),
        "content must land at the expected screen rect"
    );
    assert_eq!(
        red_rects[0].0, inset,
        "content must be clipped to the inset nest viewport"
    );
}

#[test]
fn dynamic_nest_overlay_components_stay_fixed_while_content_transforms() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Dynamic);
    nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(10.0, 10.0), vec2(20.0, 20.0)),
        color: egui::Color32::RED,
    });
    nest.push(OverlayProbe {
        rect: Rect::from_min_size(pos2(5.0, 5.0), vec2(30.0, 20.0)),
        color: egui::Color32::GREEN,
    });

    let (output, viewport) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let overlay_before = rect_fills(&output, egui::Color32::GREEN);
    assert_eq!(
        overlay_before,
        vec![Rect::from_min_size(pos2(5.0, 5.0), vec2(30.0, 20.0))],
        "overlay paints in screen space"
    );

    // Zoom in around the viewport center.
    let center = viewport.center();
    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![
            egui::Event::PointerMoved(center),
            ctrl_scroll(vec2(0.0, 0.9)),
        ],
    );
    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );

    let overlay_after = rect_fills(&output, egui::Color32::GREEN);
    assert_eq!(
        overlay_after,
        vec![Rect::from_min_size(pos2(5.0, 5.0), vec2(30.0, 20.0))],
        "overlay must stay fixed in screen space"
    );
    let content_after = rect_fills(&output, egui::Color32::RED);
    assert_ne!(
        content_after,
        vec![Rect::from_min_size(
            viewport.min + vec2(10.0, 10.0),
            vec2(20.0, 20.0)
        )],
        "content must move with the camera"
    );
}

#[test]
fn dynamic_nest_camera_survives_panel_resize() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Dynamic);

    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![
            egui::Event::PointerMoved(pos2(300.0, 200.0)),
            ctrl_scroll(vec2(0.0, 0.9)),
        ],
    );
    let (scale, offset) = (nest.camera().scale(), nest.camera().offset());

    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(500.0, 400.0),
        vec![],
    );
    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(900.0, 700.0),
        vec![],
    );

    assert_eq!(
        nest.camera().scale(),
        scale,
        "resize must not change the zoom"
    );
    assert_eq!(
        nest.camera().offset(),
        offset,
        "resize must not change the pan"
    );
}

#[test]
fn static_nest_shows_bars_only_on_overflow() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();

    let mut small = NestContent::new(NestMode::Static);
    small.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0)),
        color: egui::Color32::RED,
    });
    let (output, _) = run_nest_frame(
        &ctx,
        &mut small,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert_eq!(
        count_fills(&output, theme.colors.selection_bg_fill32()),
        0,
        "no thumb when content fits"
    );
    assert_eq!(
        count_fills(&output, theme.colors.panel_border32()),
        0,
        "no track when content fits"
    );

    let mut big = NestContent::new(NestMode::Static);
    big.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0)),
        color: egui::Color32::RED,
    });
    let (output, viewport) = run_nest_frame(
        &ctx,
        &mut big,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let inset = nest_viewport(viewport);
    assert_eq!(
        count_fills(&output, theme.colors.selection_bg_fill32()),
        2,
        "both thumbs on overflow"
    );
    assert_eq!(
        count_fills(&output, theme.colors.panel_border32()),
        2,
        "both tracks on overflow"
    );

    let thumbs = rect_fills(&output, theme.colors.selection_bg_fill32());
    let vertical = thumbs
        .iter()
        .find(|r| r.width() < r.height())
        .expect("vertical thumb");
    let horizontal = thumbs
        .iter()
        .find(|r| r.width() > r.height())
        .expect("horizontal thumb");
    assert!(
        (vertical.right() - inset.right()).abs() < 1.0,
        "vertical thumb must hug the inset right edge: {vertical:?} vs {inset:?}"
    );
    assert!(
        (horizontal.bottom() - inset.bottom()).abs() < 1.0,
        "horizontal thumb must hug the inset bottom edge: {horizontal:?} vs {inset:?}"
    );
}

#[test]
fn static_nest_middle_drag_pan_shares_the_scrollbar_offset() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0)),
        color: egui::Color32::RED,
    });

    let (_, viewport) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let inset = nest_viewport(viewport);
    let start = pos2(400.0, 300.0);
    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Middle,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerMoved(pos2(440.0, 340.0))],
    );

    assert_eq!(
        nest.offset(),
        vec2(40.0, 40.0),
        "middle-drag must write the shared offset"
    );

    // The thumb geometry must reflect the panned offset (same value the bars read).
    let thumbs = rect_fills(&output, theme.colors.selection_bg_fill32());
    let vertical = thumbs
        .iter()
        .find(|r| r.width() < r.height())
        .expect("vertical thumb");
    let geo = pyxross::ui::panel_dock::nest::scrollbar::thumb_geometry(
        inset.height(),
        700.0,
        nest.offset().y,
        inset.height(),
    )
    .expect("vertical overflow");
    assert!(
        (vertical.top() - (inset.top() + geo.thumb_pos)).abs() < 1.0,
        "thumb must sit at the panned offset: {vertical:?} vs pos {}",
        inset.top() + geo.thumb_pos
    );
}

#[test]
fn static_nest_flat_scrollbars_use_theme_tokens_without_meshes() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0)),
        color: egui::Color32::RED,
    });

    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );

    let meshes: Vec<_> = output
        .shapes
        .iter()
        .filter(|clipped| matches!(&clipped.shape, egui::Shape::Mesh(_)))
        .collect();
    assert!(meshes.is_empty(), "flat static nest must not emit meshes");
    assert_eq!(
        count_fills(&output, theme.colors.panel_border32()),
        2,
        "tracks use panel_border"
    );
    assert_eq!(
        count_fills(&output, theme.colors.selection_bg_fill32()),
        2,
        "thumbs use selection_bg_fill"
    );
}

#[test]
fn static_nest_skinned_scrollbars_emit_atlas_meshes() {
    let ctx = egui::Context::default();
    let dir = temp_dir("nest-skinned-bars");
    let atlas_path = dir.join("atlas.png");
    write_rgba_png(&atlas_path, 128, 192);
    let (bytes, size) = pyxross::ui::theme::load_atlas_with_size(&atlas_path).unwrap();
    let image =
        egui::ColorImage::from_rgba_unmultiplied([size.0 as usize, size.1 as usize], &bytes);
    let texture = ctx.load_texture("nest-atlas", image, egui::TextureOptions::NEAREST);
    let mut cache = pyxross::ui::panel_dock::skin::AtlasCache::new();
    cache.insert(
        std::path::PathBuf::from("atlas.png"),
        pyxross::ui::panel_dock::skin::SkinAtlas::new(texture.clone(), size),
    );
    let mut theme = skinned_theme();
    theme
        .skins
        .insert("scrollbar_track".to_string(), skin_with_atlas("atlas.png"));
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0)),
        color: egui::Color32::RED,
    });

    let (output, viewport) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.skinned(&theme, &cache),
        vec2(800.0, 600.0),
        vec![],
    );
    let inset = nest_viewport(viewport);

    let atlas_id = texture.id();
    let bounds = atlas_mesh_bounds(&output, atlas_id);
    assert!(
        bounds
            .iter()
            .any(|b| (b.right() - inset.right()).abs() < 1.0 && b.height() > 100.0),
        "vertical bar must be an atlas mesh hugging the inset right edge: {bounds:?}"
    );
    assert!(
        bounds
            .iter()
            .any(|b| (b.bottom() - inset.bottom()).abs() < 1.0 && b.width() > 100.0),
        "horizontal bar must be an atlas mesh hugging the inset bottom edge: {bounds:?}"
    );
}

#[test]
fn static_nest_overlay_stays_fixed_while_content_scrolls() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0)),
        color: egui::Color32::RED,
    });
    nest.push(OverlayProbe {
        rect: Rect::from_min_size(pos2(20.0, 20.0), vec2(30.0, 20.0)),
        color: egui::Color32::GREEN,
    });

    let (output, viewport) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let inset = nest_viewport(viewport);
    let overlay_before = rect_fills(&output, egui::Color32::GREEN);
    assert_eq!(
        overlay_before,
        vec![Rect::from_min_size(pos2(20.0, 20.0), vec2(30.0, 20.0))],
        "overlay paints in screen space pinned to the viewport"
    );

    // Wheel scroll moves the content but leaves the overlay pinned.
    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![
            egui::Event::PointerMoved(pos2(400.0, 300.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: vec2(0.0, -3.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            },
        ],
    );

    let overlay_after = rect_fills(&output, egui::Color32::GREEN);
    assert_eq!(
        overlay_after,
        vec![Rect::from_min_size(pos2(20.0, 20.0), vec2(30.0, 20.0))],
        "overlay must not scroll with the content"
    );
    assert!(
        nest.offset().y > 0.0,
        "content must have scrolled: {:?}",
        nest.offset()
    );
    for rect in overlay_after {
        assert!(
            inset.contains_rect(rect),
            "overlay must stay inside the inset viewport: {rect:?} vs {inset:?}"
        );
    }
    // Content is clipped to the inset viewport.
    let content_clips: Vec<Rect> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) if rect.fill == egui::Color32::RED => Some(clipped.clip_rect),
            _ => None,
        })
        .collect();
    assert_eq!(
        content_clips,
        vec![inset],
        "content must be clipped to the inset viewport"
    );
}

#[test]
fn dynamic_nest_content_starts_inside_the_inset_viewport() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Dynamic);
    nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(20.0, 20.0)),
        color: egui::Color32::RED,
    });
    nest.push(OverlayProbe {
        rect: Rect::from_min_size(pos2(20.0, 20.0), vec2(30.0, 20.0)),
        color: egui::Color32::GREEN,
    });

    let (output, viewport) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let inset = nest_viewport(viewport);

    // Identity camera: world (0,0) lands exactly at the inset origin, and the
    // content layer is clipped to the inset viewport.
    let red_rects: Vec<(Rect, Rect)> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) if rect.fill == egui::Color32::RED => {
                Some((clipped.clip_rect, rect.rect))
            }
            _ => None,
        })
        .collect();
    assert_eq!(red_rects.len(), 1, "content probe must be emitted");
    assert_eq!(
        red_rects[0].1.min, inset.min,
        "content must start at the inset viewport origin: {:?} vs {:?}",
        red_rects[0].1.min, inset.min
    );
    assert_eq!(
        red_rects[0].0, inset,
        "content must be clipped to the inset viewport"
    );

    // The overlay stays fixed in screen space and does not transform with pan/zoom.
    let overlay_before = rect_fills(&output, egui::Color32::GREEN);
    assert_eq!(
        overlay_before,
        vec![Rect::from_min_size(pos2(20.0, 20.0), vec2(30.0, 20.0))],
        "overlay paints in screen space"
    );

    let center = viewport.center();
    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![
            egui::Event::PointerMoved(center),
            ctrl_scroll(vec2(0.0, 0.9)),
        ],
    );
    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let overlay_after = rect_fills(&output, egui::Color32::GREEN);
    assert_eq!(
        overlay_after,
        vec![Rect::from_min_size(pos2(20.0, 20.0), vec2(30.0, 20.0))],
        "overlay must stay fixed while content transforms"
    );
}

#[test]
fn collapsed_panel_hides_all_nest_components() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0)),
        color: egui::Color32::RED,
    });
    nest.push(OverlayProbe {
        rect: Rect::from_min_size(pos2(20.0, 20.0), vec2(30.0, 20.0)),
        color: egui::Color32::GREEN,
    });

    // An 8x8 panel collapses to a 4x4 nest viewport: nothing may paint.
    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(8.0, 8.0),
        vec![],
    );
    assert_eq!(
        count_fills(&output, egui::Color32::RED),
        0,
        "collapsed nest must hide content components"
    );
    assert_eq!(
        count_fills(&output, egui::Color32::GREEN),
        0,
        "collapsed nest must hide overlay components"
    );
    assert_eq!(
        count_fills(&output, theme.colors.selection_bg_fill32()),
        0,
        "collapsed nest must hide the scrollbars"
    );

    // Growing the panel back restores the content exactly as before.
    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert_eq!(
        count_fills(&output, egui::Color32::RED),
        1,
        "content probe must paint in a usable viewport"
    );
    assert_eq!(
        count_fills(&output, egui::Color32::GREEN),
        1,
        "overlay probe must paint in a usable viewport"
    );
}

#[test]
fn collapsed_dynamic_nest_hides_all_components() {
    let ctx = egui::Context::default();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Dynamic);
    nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(20.0, 20.0)),
        color: egui::Color32::RED,
    });
    nest.push(OverlayProbe {
        rect: Rect::from_min_size(pos2(20.0, 20.0), vec2(30.0, 20.0)),
        color: egui::Color32::GREEN,
    });

    // Collapsed: no components paint and no input is handled — the camera
    // ignores scroll while the panel is collapsed.
    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(8.0, 8.0),
        vec![
            egui::Event::PointerMoved(pos2(4.0, 4.0)),
            scroll(vec2(0.0, 7.0)),
        ],
    );
    assert_eq!(
        count_fills(&output, egui::Color32::RED),
        0,
        "collapsed nest must hide content components"
    );
    assert_eq!(
        count_fills(&output, egui::Color32::GREEN),
        0,
        "collapsed nest must hide overlay components"
    );
    assert_eq!(nest.camera().scale(), 1.0, "collapsed nest must not zoom");
    assert_eq!(
        nest.camera().offset(),
        Vec2::ZERO,
        "collapsed nest must not pan"
    );

    // Growing the panel back restores the content.
    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert_eq!(
        count_fills(&output, egui::Color32::RED),
        1,
        "content probe must paint in a usable viewport"
    );
    assert_eq!(
        count_fills(&output, egui::Color32::GREEN),
        1,
        "overlay probe must paint in a usable viewport"
    );
}

#[test]
fn demo_panels_a_and_d_host_nests() {
    let ctx = egui::Context::default();
    let mut manager = DockManager::demo();
    let theme = pyxross::ui::theme::Theme::default_dark();
    let chrome = ChromeHarness::new();
    let output = run_demo_frame(&ctx, &mut manager, &chrome.view(&theme));
    let texts = rendered_texts(&output);

    assert!(
        texts.iter().any(|text| text == "Reset pan")
            && texts.iter().any(|text| text == "Reset zoom"),
        "the preview panel must render its reset buttons: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text.starts_with("zoom ")),
        "Panel B must render the dynamic nest zoom overlay: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.starts_with("Row 0 of the static nest")),
        "Panel D must render the static nest rows: {texts:?}"
    );
}

#[test]
fn surface_at_resolves_floating_over_dock_over_canvas() {
    let mut manager = DockManager::demo();
    let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0));
    let dock_panel = PanelId::new(1);
    let floating = PanelId::new(4);

    // A docked panel under the pointer resolves to the panel.
    assert_eq!(
        manager.surface_at(pos2(1150.0, 60.0), viewport),
        Some(dock_panel)
    );
    // The canvas area is not a panel surface.
    assert_eq!(manager.surface_at(pos2(400.0, 300.0), viewport), None);

    // Floating panel D dragged over the dock wins over the docked panel.
    manager
        .transition_panel(floating, PanelPlacement::Floating)
        .unwrap();
    manager.set_floating_rect(
        floating,
        Rect::from_min_size(pos2(1040.0, 40.0), vec2(200.0, 300.0)),
    );
    assert_eq!(
        manager.surface_at(pos2(1150.0, 60.0), viewport),
        Some(floating)
    );

    // Between two overlapping floating panels the most recently clicked wins.
    manager
        .transition_panel(dock_panel, PanelPlacement::Floating)
        .unwrap();
    manager.set_floating_rect(
        dock_panel,
        Rect::from_min_size(pos2(1100.0, 100.0), vec2(200.0, 200.0)),
    );
    let overlap = pos2(1150.0, 150.0);
    assert_eq!(manager.surface_at(overlap, viewport), Some(dock_panel));
    manager.raise_floating(floating);
    assert_eq!(manager.surface_at(overlap, viewport), Some(floating));
}

/// A nest shared with the test so its camera can be asserted after a frame.
struct SharedNest(Rc<std::cell::RefCell<NestContent>>);

impl PanelContent for SharedNest {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.0.borrow_mut().ui(ui, chrome);
    }
}

/// Frames a dock with one dynamic nest plus the canvas, routing gestures
/// through a shared capture exactly like the app does.
struct NestCanvasFrame {
    ctx: egui::Context,
    theme: Theme,
    manager: DockManager,
    capture: InputCapture,
    canvas: CanvasWidget,
    camera: Camera,
    canvas_pan: (i32, i32),
    canvas_zoomed: bool,
}

impl NestCanvasFrame {
    fn new(nest: Rc<std::cell::RefCell<NestContent>>) -> Self {
        let theme = Theme::default_dark();
        let manager = DockManager::try_new([PanelSpec {
            id: PanelId::new(1),
            metadata: PanelMetadata::new("Nest", false, vec2(180.0, 120.0)),
            placement: PanelPlacement::DockedRight,
            floating_rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0)),
            content: Box::new(SharedNest(nest)),
        }])
        .unwrap();
        Self {
            ctx: egui::Context::default(),
            theme,
            manager,
            capture: InputCapture::new(),
            canvas: CanvasWidget::new(),
            camera: Camera::new(),
            canvas_pan: (0, 0),
            canvas_zoomed: false,
        }
    }

    /// Runs one frame, routing the target by `DockManager::surface_at`.
    fn frame(&mut self, events: Vec<egui::Event>) {
        let mut canvas_pan = (0, 0);
        let mut canvas_zoomed = false;
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        let frame_rect = ui.max_rect();
                        let pointer = ui.ctx().input(|input| input.pointer.latest_pos());
                        let target = pointer.filter(|p| frame_rect.contains(*p)).map(|p| {
                            self.manager
                                .surface_at(p, frame_rect)
                                .map_or(SurfaceId::Canvas, SurfaceId::Panel)
                        });
                        self.capture.set_target(target);
                        let chrome = PanelChrome::flat(
                            &self.theme,
                            &self.capture,
                            SurfaceId::Panel(PanelId::new(1)),
                        );
                        let interactions = self.canvas.ui(
                            ui,
                            &self.theme.colors,
                            &self.capture,
                            egui::TextureId::default(),
                            pyxross::ui::canvas::CanvasView {
                                canvas_size: (32, 32),
                                grid_visible: false,
                                ..Default::default()
                            },
                            self.camera,
                        );
                        self.camera = interactions.updated_camera;
                        canvas_pan = interactions.pan_by;
                        canvas_zoomed = interactions.zoom.is_some();
                        self.manager.show_inside(ui, &chrome);
                    });
            },
        );
        output.textures_delta.clear();
        self.canvas_pan = canvas_pan;
        self.canvas_zoomed = canvas_zoomed;
    }
}

fn middle_button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Middle,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn middle_drag_in_a_nest_masks_the_canvas_and_keeps_tracking_outside() {
    let nest = Rc::new(std::cell::RefCell::new(NestContent::new(NestMode::Dynamic)));
    let mut frame = NestCanvasFrame::new(Rc::clone(&nest));
    let surface = SurfaceId::Panel(PanelId::new(1));
    let inside_panel = pos2(720.0, 300.0);
    let over_canvas = pos2(300.0, 300.0);

    frame.frame(vec![egui::Event::PointerMoved(inside_panel)]);
    frame.frame(vec![middle_button(inside_panel, true)]);
    assert_eq!(frame.capture.owner(), Some(surface));
    let offset_before = nest.borrow().camera().offset();

    // The pointer leaves the nest but the gesture keeps tracking, and the
    // canvas stays untouched.
    frame.frame(vec![egui::Event::PointerMoved(over_canvas)]);
    assert!(nest.borrow().camera().offset().x < offset_before.x);
    assert_eq!(frame.canvas_pan, (0, 0));
    assert_eq!(frame.capture.owner(), Some(surface));

    frame.frame(vec![middle_button(over_canvas, false)]);
    assert_eq!(frame.capture.owner(), None);
}

#[test]
fn scroll_is_routed_only_to_the_topmost_surface() {
    let nest = Rc::new(std::cell::RefCell::new(NestContent::new(NestMode::Dynamic)));
    let mut frame = NestCanvasFrame::new(Rc::clone(&nest));
    let inside_panel = pos2(720.0, 300.0);
    let over_canvas = pos2(300.0, 300.0);

    // Over the nest: the nest zooms, the canvas does not.
    frame.frame(vec![egui::Event::PointerMoved(inside_panel)]);
    let scale_before = nest.borrow().camera().scale();
    frame.frame(vec![scroll(vec2(0.0, 20.0))]);
    assert!(nest.borrow().camera().scale() != scale_before);
    assert!(!frame.canvas_zoomed);
    assert_eq!(frame.camera.zoom_percent(), 100);

    // Ctrl+scroll is reserved for the pen brush size: it must not zoom a nest.
    let scale_before_ctrl = nest.borrow().camera().scale();
    frame.frame(vec![ctrl_scroll(vec2(0.0, 20.0))]);
    assert_eq!(nest.borrow().camera().scale(), scale_before_ctrl);

    // Over the canvas: the canvas zooms, the nest does not.
    let scale_after_nest = nest.borrow().camera().scale();
    frame.frame(vec![egui::Event::PointerMoved(over_canvas)]);
    frame.frame(vec![ctrl_scroll(vec2(0.0, 20.0))]);
    assert!(frame.camera.zoom_percent() > 100);
    assert_eq!(nest.borrow().camera().scale(), scale_after_nest);
}

#[test]
fn clicking_a_floating_panel_raises_it_above_the_other() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let front = PanelId::new(1);
    let back = PanelId::new(2);
    let mut manager = DockManager::try_new([
        PanelSpec {
            id: front,
            metadata: PanelMetadata::new("Front", false, vec2(100.0, 100.0)),
            placement: PanelPlacement::Floating,
            floating_rect: Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 200.0)),
            content: Box::new(EmptyContent),
        },
        PanelSpec {
            id: back,
            metadata: PanelMetadata::new("Back", false, vec2(100.0, 100.0)),
            placement: PanelPlacement::Floating,
            floating_rect: Rect::from_min_size(pos2(150.0, 150.0), vec2(200.0, 200.0)),
            content: Box::new(EmptyContent),
        },
    ])
    .unwrap();
    let capture = InputCapture::new();
    let chrome = PanelChrome::flat(&theme, &capture, SurfaceId::Panel(front));
    let frame = |manager: &mut DockManager, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| manager.show_inside(ui, &chrome));
            },
        );
        output.textures_delta.clear();
    };
    let front_layer = egui::LayerId::new(
        egui::Order::Middle,
        egui::Id::new(("panel-dock-floating", front.raw())),
    );

    // `back` starts on top.
    manager.raise_floating(back);
    frame(&mut manager, vec![]);

    // Clicking `front` where `back` does not cover raises `front` to the top.
    frame(
        &mut manager,
        vec![egui::Event::PointerMoved(pos2(120.0, 120.0))],
    );
    frame(
        &mut manager,
        vec![egui::Event::PointerButton {
            pos: pos2(120.0, 120.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    frame(&mut manager, vec![]);
    assert_eq!(manager.floating_order().last(), Some(&front));
    assert_eq!(ctx.layer_id_at(pos2(250.0, 250.0)), Some(front_layer));
}

/// The static nest masks the wheel while another surface owns a gesture.
#[test]
fn static_nest_wheel_is_masked_while_another_surface_owns_the_gesture() {
    let capture = InputCapture::new();
    let surface = SurfaceId::Panel(PanelId::new(1));
    capture.claim(SurfaceId::Canvas);

    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = PanelChrome::flat(&theme, &capture, surface);
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0)),
        color: egui::Color32::RED,
    });

    let (mut output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome,
        vec2(800.0, 600.0),
        vec![
            egui::Event::PointerMoved(pos2(400.0, 300.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: vec2(0.0, -3.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            },
        ],
    );
    output.textures_delta.clear();
    assert_eq!(nest.offset(), Vec2::ZERO);
}

/// Runs enough passes with advancing time for egui to finish a freshly shown
/// window's fade-in: until it completes, the window paints its content at
/// reduced opacity.
fn frame_twice(
    ctx: &egui::Context,
    manager: &mut DockManager,
    chrome: &PanelChrome,
) -> egui::FullOutput {
    let mut last = None;
    for step in 0..16 {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
            time: Some(step as f64 / 60.0),
            predicted_dt: 1.0 / 60.0,
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show(ui, |ui| manager.show_inside(ui, chrome));
        });
        output.textures_delta.clear();
        last = Some(output);
    }
    last.expect("at least one frame")
}

/// Runs enough passes with advancing time for a freshly shown window's fade-in
/// to fully complete, so its painted fills carry their exact theme colors.
fn frame_until_settled(
    ctx: &egui::Context,
    manager: &mut DockManager,
    chrome: &PanelChrome,
) -> egui::FullOutput {
    let mut last = None;
    for step in 0..24 {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
            time: Some(step as f64 / 60.0),
            predicted_dt: 1.0 / 60.0,
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show(ui, |ui| manager.show_inside(ui, chrome));
        });
        output.textures_delta.clear();
        last = Some(output);
    }
    last.expect("at least one frame")
}

/// Number of meshes painted with `texture` (images are textured meshes).
fn textured_mesh_count(output: &egui::FullOutput, texture: egui::TextureId) -> usize {
    fn visit(shape: &egui::Shape, texture: egui::TextureId, count: &mut usize) {
        match shape {
            egui::Shape::Mesh(mesh) if mesh.texture_id == texture => *count += 1,
            egui::Shape::Vec(shapes) => {
                shapes.iter().for_each(|shape| visit(shape, texture, count))
            }
            _ => {}
        }
    }
    let mut count = 0;
    output
        .shapes
        .iter()
        .for_each(|clipped| visit(&clipped.shape, texture, &mut count));
    count
}

/// Position of the first text shape equal to `needle`.
fn text_pos(output: &egui::FullOutput, needle: &str) -> Option<egui::Pos2> {
    fn visit(shape: &egui::Shape, needle: &str, found: &mut Option<egui::Pos2>) {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == needle && found.is_none() => {
                *found = Some(text.pos);
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| visit(shape, needle, found)),
            _ => {}
        }
    }
    let mut found = None;
    output
        .shapes
        .iter()
        .for_each(|clipped| visit(&clipped.shape, needle, &mut found));
    found
}

/// The galley rect (top-left + size) of the first text shape equal to `needle`.
fn text_rect(output: &egui::FullOutput, needle: &str) -> Option<egui::Rect> {
    fn visit(shape: &egui::Shape, needle: &str, found: &mut Option<egui::Rect>) {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == needle && found.is_none() => {
                *found = Some(egui::Rect::from_min_size(text.pos, text.galley.size()));
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| visit(shape, needle, found)),
            _ => {}
        }
    }
    let mut found = None;
    output
        .shapes
        .iter()
        .for_each(|clipped| visit(&clipped.shape, needle, &mut found));
    found
}

/// Rects of every text shape equal to `needle`.
fn text_rects(output: &egui::FullOutput, needle: &str) -> Vec<egui::Rect> {
    fn visit(shape: &egui::Shape, needle: &str, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == needle => {
                out.push(egui::Rect::from_min_size(text.pos, text.galley.size()));
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| visit(shape, needle, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    output
        .shapes
        .iter()
        .for_each(|clipped| visit(&clipped.shape, needle, &mut out));
    out
}

/// Positions of filled rects with `color` in paint order.
fn fill_positions(output: &egui::FullOutput, color: egui::Color32) -> Vec<usize> {
    fn visit(shape: &egui::Shape, color: egui::Color32, index: &mut usize, out: &mut Vec<usize>) {
        match shape {
            egui::Shape::Rect(rect) => {
                if rect.fill == color {
                    out.push(*index);
                }
                *index += 1;
            }
            egui::Shape::Vec(shapes) => shapes
                .iter()
                .for_each(|shape| visit(shape, color, index, out)),
            _ => *index += 1,
        }
    }
    let mut positions = Vec::new();
    let mut index = 0;
    output
        .shapes
        .iter()
        .for_each(|clipped| visit(&clipped.shape, color, &mut index, &mut positions));
    positions
}

/// Positions of text shapes equal to `needle` in paint order.
fn text_positions(output: &egui::FullOutput, needle: &str) -> Vec<usize> {
    fn visit(shape: &egui::Shape, needle: &str, index: &mut usize, out: &mut Vec<usize>) {
        match shape {
            egui::Shape::Text(text) => {
                if text.galley.text() == needle {
                    out.push(*index);
                }
                *index += 1;
            }
            egui::Shape::Vec(shapes) => shapes
                .iter()
                .for_each(|shape| visit(shape, needle, index, out)),
            _ => *index += 1,
        }
    }
    let mut positions = Vec::new();
    let mut index = 0;
    output
        .shapes
        .iter()
        .for_each(|clipped| visit(&clipped.shape, needle, &mut index, &mut positions));
    positions
}

fn panel_spec(
    id: u64,
    title: &'static str,
    placement: PanelPlacement,
    rect: Rect,
    content: Box<dyn PanelContent>,
) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(id),
        metadata: PanelMetadata::new(title, true, vec2(120.0, 80.0)),
        placement,
        floating_rect: rect,
        content,
    }
}

#[test]
fn docked_nest_paints_below_floating_panels() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();

    let mut docked_nest = NestContent::new(NestMode::Dynamic);
    docked_nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 400.0)),
        color: egui::Color32::RED,
    });
    let mut floating_nest = NestContent::new(NestMode::Dynamic);
    floating_nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(180.0, 120.0)),
        color: egui::Color32::BLUE,
    });

    let mut manager = DockManager::try_new([
        panel_spec(
            1,
            "Docked nest",
            PanelPlacement::DockedRight,
            Rect::from_min_size(pos2(40.0, 40.0), vec2(200.0, 200.0)),
            Box::new(docked_nest),
        ),
        panel_spec(
            2,
            "Floating over dock",
            PanelPlacement::Floating,
            Rect::from_min_size(pos2(600.0, 100.0), vec2(200.0, 200.0)),
            Box::new(floating_nest),
        ),
    ])
    .unwrap();

    let mut output = frame_twice(&ctx, &mut manager, &chrome.view(&theme));
    output.textures_delta.clear();

    let nested = fill_positions(&output, egui::Color32::RED);
    let floating = fill_positions(&output, egui::Color32::BLUE);
    assert!(!nested.is_empty(), "docked nest content must be painted");
    assert!(
        !floating.is_empty(),
        "floating panel content must be painted"
    );
    assert!(
        nested[0] < floating[0],
        "docked nest (at {:?}) must paint below the floating panel (at {:?})",
        nested.first(),
        floating.first()
    );
}

#[test]
fn floating_nest_paints_above_its_own_panel_chrome() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();

    let mut floating_nest = NestContent::new(NestMode::Dynamic);
    floating_nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(120.0, 80.0)),
        color: egui::Color32::BLUE,
    });
    let mut manager = DockManager::try_new([panel_spec(
        2,
        "Floating nest",
        PanelPlacement::Floating,
        Rect::from_min_size(pos2(200.0, 200.0), vec2(200.0, 200.0)),
        Box::new(floating_nest),
    )])
    .unwrap();

    let mut output = frame_twice(&ctx, &mut manager, &chrome.view(&theme));
    output.textures_delta.clear();

    let header = text_positions(&output, "Floating nest");
    let content = fill_positions(&output, egui::Color32::BLUE);
    assert!(!header.is_empty(), "floating panel header must be painted");
    assert!(!content.is_empty(), "floating nest content must be painted");
    assert!(
        header[0] < content[0],
        "floating nest content (at {:?}) must paint above its own panel chrome (at {:?})",
        content.first(),
        header.first()
    );
}

#[test]
fn floating_nests_stay_between_their_host_and_the_panel_above_it() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();

    let mut lower = NestContent::new(NestMode::Dynamic);
    lower.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(150.0, 100.0)),
        color: egui::Color32::BLUE,
    });
    let mut upper = NestContent::new(NestMode::Dynamic);
    upper.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(150.0, 100.0)),
        color: egui::Color32::GREEN,
    });

    let mut manager = DockManager::try_new([
        panel_spec(
            1,
            "Lower",
            PanelPlacement::Floating,
            Rect::from_min_size(pos2(100.0, 100.0), vec2(220.0, 220.0)),
            Box::new(lower),
        ),
        panel_spec(
            2,
            "Upper",
            PanelPlacement::Floating,
            Rect::from_min_size(pos2(180.0, 150.0), vec2(220.0, 220.0)),
            Box::new(upper),
        ),
    ])
    .unwrap();
    manager.raise_floating(PanelId::new(2));

    let mut output = frame_twice(&ctx, &mut manager, &chrome.view(&theme));
    output.textures_delta.clear();

    let lower_header = text_positions(&output, "Lower");
    let upper_header = text_positions(&output, "Upper");
    let lower_nest = fill_positions(&output, egui::Color32::BLUE);
    let upper_nest = fill_positions(&output, egui::Color32::GREEN);

    assert!(
        !lower_header.is_empty() && !upper_header.is_empty(),
        "both panel headers paint"
    );
    assert!(
        !lower_nest.is_empty() && !upper_nest.is_empty(),
        "both nests paint"
    );
    assert!(
        lower_header[0] < lower_nest[0],
        "the lower nest must paint above its own panel chrome"
    );
    assert!(
        lower_nest[0] < upper_header[0],
        "the lower nest must paint below the panel above it (nest {:?}, upper chrome {:?})",
        lower_nest.first(),
        upper_header.first()
    );
    assert!(
        upper_header[0] < upper_nest[0],
        "the upper nest must paint above its own panel chrome"
    );
}

#[test]
fn dynamic_nest_applies_camera_reset_requests() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Dynamic);

    // Zoom and pan, then request a zoom reset through the component channel.
    nest.camera_mut()
        .zoom_around(pos2(100.0, 100.0), pos2(10.0, 10.0), 2.0);
    nest.camera_mut().pan_by(vec2(-30.0, 15.0));
    nest.reset_handle().request(NestReset::Zoom);
    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert_eq!(
        nest.camera().scale(),
        1.0,
        "zoom reset applies on the next frame"
    );
    assert_ne!(
        nest.camera().offset(),
        Vec2::ZERO,
        "zoom reset keeps the pan"
    );

    nest.reset_handle().request(NestReset::Pan);
    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert_eq!(
        nest.camera().offset(),
        Vec2::ZERO,
        "pan reset applies on the next frame"
    );
}

#[test]
fn preview_panel_paints_the_canvas_image() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let feed = PreviewFeed::default();
    let mut nest = NestContent::new(NestMode::Dynamic);
    nest.push(PreviewImage::new(feed.clone()));

    // Without a published image the panel shows a placeholder.
    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert!(rendered_texts(&output)
        .iter()
        .any(|text| text == "No preview"));

    // With an image the canvas texture is painted.
    let texture = ctx.load_texture(
        "preview-test",
        egui::ColorImage::new([4, 4], vec![egui::Color32::RED; 16]),
        egui::TextureOptions::NEAREST,
    );
    feed.set(Some((texture.id(), vec2(128.0, 96.0))));
    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert!(!rendered_texts(&output)
        .iter()
        .any(|text| text == "No preview"));
    assert!(
        textured_mesh_count(&output, texture.id()) > 0,
        "the preview image must be painted"
    );
}

#[test]
fn preview_reset_buttons_request_their_resets() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Dynamic);
    let reset = nest.reset_handle();
    nest.push(PreviewControls::new(reset.clone()));
    nest.camera_mut()
        .zoom_around(pos2(50.0, 50.0), pos2(0.0, 0.0), 2.0);

    // Locate the button by its label and click it.
    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let zoom_button = text_pos(&output, "Reset zoom").expect("reset zoom button paints");

    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![
            egui::Event::PointerMoved(zoom_button),
            egui::Event::PointerButton {
                pos: zoom_button,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerButton {
            pos: zoom_button,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    // The click is handled while painting; the nest applies the request on a
    // following frame.
    for _ in 0..2 {
        run_nest_frame(
            &ctx,
            &mut nest,
            &chrome.view(&theme),
            vec2(800.0, 600.0),
            vec![],
        );
    }
    assert_eq!(
        nest.camera().scale(),
        1.0,
        "the button requests a zoom reset the nest applies"
    );
}

#[test]
fn preview_controls_click_requests_reset_in_isolation() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let capture = InputCapture::new();
    capture.set_target(Some(SurfaceId::Canvas));
    let chrome = PanelChrome::flat(&theme, &capture, SurfaceId::Canvas);
    let reset = NestResetHandle::default();
    let mut controls = PreviewControls::new(reset.clone());
    let mut frame = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        controls.ui(ui, &NestView::screen(ui.max_rect()), &chrome)
                    });
            },
        );
        output.textures_delta.clear();
        output
    };
    let output = frame(vec![]);
    let button = text_pos(&output, "Reset zoom").expect("button paints");
    frame(vec![
        egui::Event::PointerMoved(button),
        egui::Event::PointerButton {
            pos: button,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    frame(vec![egui::Event::PointerButton {
        pos: button,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    assert_eq!(reset.take(), Some(NestReset::Zoom));
}

#[test]
fn nest_overlays_paint_above_zoomed_content() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Dynamic);
    // Content and overlay cover the same world rect: the overlay must win.
    nest.push(ProbeRect {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(120.0, 80.0)),
        color: egui::Color32::BLUE,
    });
    nest.push(OverlayProbe {
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(120.0, 80.0)),
        color: egui::Color32::GREEN,
    });

    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let content = fill_positions(&output, egui::Color32::BLUE);
    let overlay = fill_positions(&output, egui::Color32::GREEN);
    assert!(
        !content.is_empty() && !overlay.is_empty(),
        "both must paint"
    );
    assert!(
        content[0] < overlay[0],
        "fixed overlay components must paint above zoomable content ({content:?} vs {overlay:?})"
    );

    // Zooming and panning the content must not change that ordering.
    nest.camera_mut()
        .zoom_around(pos2(100.0, 100.0), pos2(0.0, 0.0), 2.0);
    nest.camera_mut().pan_by(vec2(-30.0, 20.0));
    let (output, _) = run_nest_frame(
        &ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let content = fill_positions(&output, egui::Color32::BLUE);
    let overlay = fill_positions(&output, egui::Color32::GREEN);
    assert!(
        content[0] < overlay[0],
        "ordering must survive zoom/pan ({content:?} vs {overlay:?})"
    );
}

#[test]
fn dynamic_nest_zooms_at_the_same_rate_as_the_canvas() {
    use pyxross::ui::canvas::CanvasWidget;
    let event = || scroll(vec2(0.0, 20.0));

    // Canvas: one scroll event over the canvas.
    let canvas_ctx = egui::Context::default();
    let mut widget = CanvasWidget::new();
    let mut camera = Camera::new();
    let mut output = canvas_ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
            events: vec![egui::Event::PointerMoved(pos2(50.0, 50.0)), event()],
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    let capture = InputCapture::new();
                    capture.set_target(Some(SurfaceId::Canvas));
                    let interactions = widget.ui(
                        ui,
                        &Theme::default_dark().colors,
                        &capture,
                        egui::TextureId::default(),
                        pyxross::ui::canvas::CanvasView {
                            canvas_size: (64, 64),
                            grid_visible: false,
                            ..Default::default()
                        },
                        camera,
                    );
                    camera = interactions.updated_camera;
                });
        },
    );
    output.textures_delta.clear();
    let canvas_percent = camera.zoom_percent();

    // Nest: the same event over a dynamic nest.
    let nest_ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut nest = NestContent::new(NestMode::Dynamic);
    run_nest_frame(
        &nest_ctx,
        &mut nest,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerMoved(pos2(300.0, 200.0)), event()],
    );
    let nest_percent = (nest.camera().scale() * 100.0).round() as i32;

    assert!(
        (nest_percent - canvas_percent as i32).abs() <= 1,
        "nest zoom {nest_percent}% must match the canvas rate {canvas_percent}%"
    );
}

// ---------------------------------------------------------------------------
// Toolbox dock panel tests
// ---------------------------------------------------------------------------

use pyxross::core::brush::{BrushShape, BrushSpec};
use pyxross::input::{FieldierChild, Tool, WandSettings};
use pyxross::ui::dock_hosts::ToolboxHost;
use pyxross::ui::dock_tool_property_panel::tool_property_panel_spec;
use pyxross::ui::dock_toolbox_panel::toolbox_panel_spec;
use pyxross::ui::toolbar::ToolbarEvent;

/// Frames a dock holding the Toolbox panel (dock id 100) docked right.
fn run_toolbox_frame(
    ctx: &egui::Context,
    manager: &mut DockManager,
    chrome: &PanelChrome,
    screen: Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), screen)),
            events,
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| manager.show_inside(ui, chrome));
        },
    );
    output.textures_delta.clear();
    output
}

fn toolbox_manager(host: &ToolboxHost) -> DockManager {
    DockManager::try_new([toolbox_panel_spec(host, PanelPlacement::DockedRight)]).unwrap()
}

#[test]
fn toolbox_lists_exactly_the_real_tools() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    let mut manager = toolbox_manager(&host);

    let output = run_toolbox_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let texts = rendered_texts(&output);
    for label in ["Pencil", "Eraser", "Fill", "Eyedropper", "Fieldier"] {
        assert_eq!(
            texts.iter().filter(|text| *text == label).count(),
            1,
            "tool label {label} must appear exactly once: {texts:?}"
        );
    }
    let others: Vec<&String> = texts
        .iter()
        .filter(|text| {
            !matches!(
                text.as_str(),
                "Pencil" | "Eraser" | "Fill" | "Eyedropper" | "Fieldier"
            )
        })
        .collect();
    assert_eq!(
        others,
        vec!["Toolbox"],
        "no text besides the tool labels and the panel title: {texts:?}"
    );
}

#[test]
fn tool_buttons_scroll_with_nest_content() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    let mut manager = toolbox_manager(&host);

    // A short viewport: the 360pt content overflows the docked panel, so the
    // buttons scroll with the nest content instead of being pinned.
    let screen = vec2(400.0, 150.0);
    let output = run_toolbox_frame(&ctx, &mut manager, &chrome.view(&theme), screen, vec![]);
    let before = text_pos(&output, "Pencil").expect("Pencil label paints");

    run_toolbox_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        screen,
        vec![
            egui::Event::PointerMoved(pos2(300.0, 75.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -5.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            },
        ],
    );
    // The wheel frame consumes the delta and updates the nest offset; the
    // content paints at the new offset on the following frame.
    let output = run_toolbox_frame(&ctx, &mut manager, &chrome.view(&theme), screen, vec![]);
    let after = text_pos(&output, "Pencil").expect("Pencil label still paints");

    assert!(
        after.y < before.y,
        "the tool buttons must scroll up with the content: {before:?} -> {after:?}"
    );
}

#[test]
fn toolbox_content_shrinks_with_the_panel_width() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    let mut manager = toolbox_manager(&host);

    // A narrow-but-usable dock: a 40pt wide panel leaves a ~30pt nest viewport.
    let screen = vec2(400.0, 120.0);
    manager.resize_dock_area(DockSide::Right, -200.0, screen);
    assert_eq!(manager.dock_extent(DockSide::Right), 40.0);

    let output = run_toolbox_frame(&ctx, &mut manager, &chrome.view(&theme), screen, vec![]);

    // The 360pt-tall content overflows the short viewport vertically (one
    // vertical thumb) but the width follows the panel, so no horizontal thumb
    // is painted: the content must not force a width wider than the panel.
    let thumbs = rect_fills(&output, theme.colors.selection_bg_fill32());
    assert_eq!(
        thumbs.len(),
        1,
        "exactly the vertical thumb, no horizontal overflow: {thumbs:?}"
    );
    assert!(
        thumbs[0].width() < thumbs[0].height(),
        "the thumb must be vertical: {thumbs:?}"
    );
}

#[test]
fn clicking_tool_emits_tool_selected() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    let mut manager = toolbox_manager(&host);

    let output = run_toolbox_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let fill = text_pos(&output, "Fill").expect("Fill label paints");

    run_toolbox_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![
            egui::Event::PointerMoved(fill),
            egui::Event::PointerButton {
                pos: fill,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    run_toolbox_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerButton {
            pos: fill,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );

    let events: Vec<ToolbarEvent> = host.events.borrow().clone();
    assert_eq!(events, vec![ToolbarEvent::ToolSelected(Tool::Fill)]);
}

#[test]
fn plain_frame_emits_no_events() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    let mut manager = toolbox_manager(&host);

    run_toolbox_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert!(
        host.events.borrow().is_empty(),
        "a plain frame must emit no events"
    );
}

#[test]
fn toolbox_panel_spec_registers_dock_id_100() {
    let host = ToolboxHost::default();
    let spec = toolbox_panel_spec(&host, PanelPlacement::DockedRight);
    assert_eq!(spec.id, PanelId::new(100));
    assert_eq!(spec.metadata.title, "Toolbox");
    assert_eq!(spec.placement, PanelPlacement::DockedRight);

    let manager = DockManager::try_new([spec]).unwrap();
    assert_eq!(manager.panel_count(), 1);
    assert_eq!(
        manager.placement(PanelId::new(100)),
        Some(PanelPlacement::DockedRight)
    );
}

// ---------------------------------------------------------------------------
// Tool Property dock panel tests
// ---------------------------------------------------------------------------

/// Frames a dock holding the Tool Property panel (dock id 103) docked right.
fn run_tool_property_frame(
    ctx: &egui::Context,
    manager: &mut DockManager,
    chrome: &PanelChrome,
    screen: Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), screen)),
            events,
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| manager.show_inside(ui, chrome));
        },
    );
    output.textures_delta.clear();
    output
}

fn tool_property_manager(host: &ToolboxHost) -> DockManager {
    DockManager::try_new([tool_property_panel_spec(host, PanelPlacement::DockedRight)]).unwrap()
}

#[test]
fn tool_property_rows_put_labels_above_their_inputs() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    {
        let mut view = host.view.borrow_mut();
        view.tool = Tool::Pencil;
        view.draw.size = 20;
        view.draw.tail = 5;
        view.draw.scatter = 7;
    }
    let mut manager = tool_property_manager(&host);
    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );

    // Each row: the label on its own line, its first input below it and
    // left-aligned with it. The slider rows start with their numeric input
    // box; the shape rows show their first button.
    let inactive_fill = ctx
        .style_of(egui::Theme::Dark)
        .visuals
        .widgets
        .inactive
        .bg_fill;
    let rows: [(&str, Option<&str>); 5] = [
        ("Size", None),
        ("Shape", Some("Square")),
        ("Tail", None),
        ("Scatter", None),
        ("Spread", Some("Square")),
    ];
    for (label, button) in rows {
        let label_rect =
            text_rect(&output, label).unwrap_or_else(|| panic!("missing row label {label}"));
        let input_rect = match button {
            None => row_numeric_input(&output, inactive_fill, label),
            Some(text) => text_rects(&output, text)
                .into_iter()
                .filter(|rect| rect.top() > label_rect.bottom())
                .min_by(|a, b| a.top().total_cmp(&b.top()))
                .unwrap_or_else(|| {
                    panic!(
                        "{label}: no {text:?} button below its label: {:?}",
                        rendered_texts(&output)
                    )
                }),
        };
        assert!(
            (input_rect.left() - label_rect.left()).abs() < 24.0,
            "{label}: the input must be left-aligned with its label: {label_rect:?} vs {input_rect:?}"
        );
    }
}

#[test]
fn slider_rows_have_a_numeric_input_left_of_the_slider() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    {
        let mut view = host.view.borrow_mut();
        view.tool = Tool::Pencil;
        view.draw.size = 20;
        view.draw.tail = 5;
        view.draw.scatter = 7;
    }
    let mut manager = tool_property_manager(&host);
    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );

    let inactive_fill = ctx
        .style_of(egui::Theme::Dark)
        .visuals
        .widgets
        .inactive
        .bg_fill;
    for (label, shown) in [("Size", "20"), ("Tail", "5"), ("Scatter", "7")] {
        let input = row_numeric_input(&output, inactive_fill, label);
        let rail = row_rail(&output, inactive_fill, label);
        assert!(
            input.right() <= rail.left(),
            "{label}: the numeric input must be left of the slider rail: {input:?} vs {rail:?}"
        );
        assert!(
            (input.center().y - rail.center().y).abs() < input.height(),
            "{label}: the input and the slider must share a line: {input:?} vs {rail:?}"
        );
        let text = text_rect(&output, shown)
            .unwrap_or_else(|| panic!("{label}: the input box must show {shown:?}"));
        assert!(
            input.contains(text.center()),
            "{label}: {shown:?} must be painted inside the input box: {text:?} vs {input:?}"
        );
    }

    // The sliders keep their built-in value display off: each value is painted
    // exactly once, by the input box.
    for shown in ["20", "5", "7"] {
        assert_eq!(
            text_rects(&output, shown).len(),
            1,
            "only the input box may paint {shown:?}: {:?}",
            rendered_texts(&output)
        );
    }
}

/// The slider rail of the property row whose label is `label`: the nearest
/// rail below the label's line.
fn row_rail(output: &egui::FullOutput, inactive_fill: egui::Color32, label: &str) -> egui::Rect {
    let label_rect =
        text_rect(output, label).unwrap_or_else(|| panic!("missing row label {label}"));
    rect_fills(output, inactive_fill)
        .into_iter()
        .filter(|rect| rect.height() <= 10.0 && rect.center().y > label_rect.bottom())
        .min_by(|a, b| a.center().y.total_cmp(&b.center().y))
        .unwrap_or_else(|| panic!("the {label:?} row's slider rail should be painted"))
}

/// The numeric input box of the property row whose label is `label`: the
/// button-framed fill on the rail's line and left of the rail.
fn row_numeric_input(
    output: &egui::FullOutput,
    inactive_fill: egui::Color32,
    label: &str,
) -> egui::Rect {
    let rail = row_rail(output, inactive_fill, label);
    rect_fills(output, inactive_fill)
        .into_iter()
        .filter(|rect| rect.height() > 10.0 && rect.right() <= rail.left())
        .min_by(|a, b| {
            (a.center().y - rail.center().y)
                .abs()
                .total_cmp(&(b.center().y - rail.center().y).abs())
        })
        .unwrap_or_else(|| {
            panic!("the {label:?} row's input box should be painted left of its slider")
        })
}

/// The reset button of the property row whose label is `label`: the nearest
/// reset button below the label's line.
fn row_reset(output: &egui::FullOutput, label: &str) -> egui::Rect {
    let label_rect =
        text_rect(output, label).unwrap_or_else(|| panic!("missing row label {label}"));
    text_rects(output, "Reset")
        .into_iter()
        .filter(|rect| rect.top() > label_rect.bottom())
        .min_by(|a, b| a.top().total_cmp(&b.top()))
        .unwrap_or_else(|| panic!("the {label:?} row's reset button should be painted"))
}

/// Clicks the primary button at `pos` on the Tool Property panel (move, press,
/// release; one frame each).
fn click_tool_property(
    ctx: &egui::Context,
    manager: &mut DockManager,
    chrome: &PanelChrome,
    pos: egui::Pos2,
) {
    let screen = vec2(800.0, 600.0);
    let events = [
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ];
    for event in events {
        run_tool_property_frame(ctx, manager, chrome, screen, vec![event]);
    }
}

/// The right-docked panel's rect at the given viewport.
fn right_panel_rect(manager: &DockManager, viewport: Rect) -> Rect {
    let right_rect = Rect::from_min_max(
        pos2(
            viewport.right() - manager.dock_extent(DockSide::Right),
            viewport.top(),
        ),
        viewport.max,
    );
    manager.panel_rects(DockSide::Right, right_rect)[0].1
}

#[test]
fn tool_property_panel_shows_the_current_tool_name() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    host.view.borrow_mut().tool = Tool::Fill;
    let mut manager = tool_property_manager(&host);

    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );

    let name = text_pos(&output, "Fill").expect("the active tool name must paint");
    let title = text_pos(&output, "Tool Property").expect("the panel header must paint");
    assert!(
        name.y > title.y,
        "the tool name must sit below the panel header: {name:?} vs {title:?}"
    );
    assert!(
        text_pos(&output, "Pencil").is_none(),
        "only the active tool's name may paint: {:?}",
        rendered_texts(&output)
    );
}

#[test]
fn tool_property_panel_follows_tool_changes() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    let mut manager = tool_property_manager(&host);

    host.view.borrow_mut().tool = Tool::Pencil;
    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert!(
        text_pos(&output, "Pencil").is_some(),
        "the panel must show the current tool"
    );
    assert!(
        text_pos(&output, "Eraser").is_none(),
        "no other tool name may paint"
    );

    host.view.borrow_mut().tool = Tool::Eraser;
    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert!(
        text_pos(&output, "Eraser").is_some(),
        "the panel must follow the tool switch"
    );
    assert!(
        text_pos(&output, "Pencil").is_none(),
        "the old tool name must disappear"
    );
}

#[test]
fn tool_property_panel_has_a_placeholder_property_area() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    host.view.borrow_mut().tool = Tool::Fill;
    let mut manager = tool_property_manager(&host);

    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let name = text_pos(&output, "Fill").expect("the tool name paints");
    let placeholder = text_pos(&output, "No editable properties for this tool yet.")
        .expect("the placeholder property area paints");
    assert!(
        placeholder.y > name.y,
        "the placeholder must sit below the tool name: {name:?} vs {placeholder:?}"
    );

    // Both stay left-aligned inside the panel.
    let panel_rect = right_panel_rect(
        &manager,
        Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)),
    );
    assert!(
        name.x - panel_rect.left() < 60.0,
        "the tool name must start near the left edge: {name:?}"
    );
    assert!(
        placeholder.x - panel_rect.left() < 60.0,
        "the placeholder must start near the left edge: {placeholder:?}"
    );
}

#[test]
fn tool_property_panel_shows_draw_properties_for_pencil_and_eraser() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    let mut manager = tool_property_manager(&host);

    for tool in [Tool::Pencil, Tool::Eraser] {
        host.view.borrow_mut().tool = tool;
        let output = run_tool_property_frame(
            &ctx,
            &mut manager,
            &chrome.view(&theme),
            vec2(800.0, 600.0),
            vec![],
        );
        for label in ["Size", "Shape", "Square", "Circle"] {
            assert!(
                text_pos(&output, label).is_some(),
                "{tool:?} must show the {label} control: {:?}",
                rendered_texts(&output)
            );
        }
        assert!(
            text_pos(&output, "No editable properties for this tool yet.").is_none(),
            "{tool:?} must not show the placeholder"
        );
    }
}

#[test]
fn tool_property_panel_keeps_placeholder_for_non_draw_tools() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    let mut manager = tool_property_manager(&host);

    for tool in [Tool::Fill, Tool::Eyedropper] {
        host.view.borrow_mut().tool = tool;
        let output = run_tool_property_frame(
            &ctx,
            &mut manager,
            &chrome.view(&theme),
            vec2(800.0, 600.0),
            vec![],
        );
        assert!(
            text_pos(&output, "No editable properties for this tool yet.").is_some(),
            "{tool:?} must keep the placeholder"
        );
        assert!(
            text_pos(&output, "Size").is_none(),
            "{tool:?} must not show brush size"
        );
        assert!(
            text_pos(&output, "Shape").is_none(),
            "{tool:?} must not show brush shape"
        );
    }
}

#[test]
fn clicking_shape_button_emits_brush_shape_changed() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    let mut manager = tool_property_manager(&host);

    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let circle = text_pos(&output, "Circle").expect("the Circle button paints");
    run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![
            egui::Event::PointerMoved(circle),
            egui::Event::PointerButton {
                pos: circle,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerButton {
            pos: circle,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );

    let events: Vec<ToolbarEvent> = host.events.borrow().clone();
    assert!(
        events.contains(&ToolbarEvent::BrushShapeChanged(BrushShape::Round)),
        "clicking Circle must emit BrushShapeChanged(Round), got {events:?}"
    );
}

#[test]
fn transform_active_shows_algorithm_selector_instead_of_tool_properties() {
    use pyxross::core::transform::TransformAlgorithm;
    use pyxross::input::Tool;

    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    {
        let mut view = host.view.borrow_mut();
        view.tool = Tool::Pencil;
        view.transform_active = true;
        view.transform_algorithm = TransformAlgorithm::CleanEdge;
    }
    let mut manager = tool_property_manager(&host);
    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );

    assert!(
        text_pos(&output, "Transform").is_some(),
        "a live transform must paint the Transform header: {:?}",
        rendered_texts(&output)
    );
    assert!(text_pos(&output, "Algorithm").is_some());
    for label in ["RotSprite", "CleanEdge", "Rotxel"] {
        assert!(
            text_pos(&output, label).is_some(),
            "{label} button must paint while transforming"
        );
    }
    for absent in ["Pencil", "Size", "Shape", "Tail", "Scatter", "Spread"] {
        assert!(
            text_pos(&output, absent).is_none(),
            "{absent:?} must be hidden while a transform is live"
        );
    }
}

#[test]
fn transform_inactive_keeps_tool_properties_and_hides_algorithm() {
    use pyxross::core::transform::TransformAlgorithm;
    use pyxross::input::Tool;

    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    {
        let mut view = host.view.borrow_mut();
        view.tool = Tool::Pencil;
        view.transform_active = false;
        // The stored algorithm is irrelevant while no transform is live.
        view.transform_algorithm = TransformAlgorithm::CleanEdge;
    }
    let mut manager = tool_property_manager(&host);
    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );

    assert!(
        text_pos(&output, "Pencil").is_some(),
        "without a transform the tool label path stays"
    );
    assert!(
        text_pos(&output, "Size").is_some(),
        "the draw properties must stay intact"
    );
    for absent in ["Transform", "Algorithm", "RotSprite", "CleanEdge", "Rotxel"] {
        assert!(
            text_pos(&output, absent).is_none(),
            "{absent:?} must not paint without a transform"
        );
    }
}

#[test]
fn clicking_algorithm_button_emits_transform_algorithm_changed() {
    use pyxross::core::transform::TransformAlgorithm;

    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    host.view.borrow_mut().transform_active = true;
    let mut manager = tool_property_manager(&host);

    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let clean = text_pos(&output, "CleanEdge").expect("the CleanEdge button paints");
    let press = |pressed| egui::Event::PointerButton {
        pos: clean,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerMoved(clean), press(true)],
    );
    run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![press(false)],
    );

    let events: Vec<ToolbarEvent> = host.events.borrow().clone();
    assert!(
        events.contains(&ToolbarEvent::TransformAlgorithmChanged(
            TransformAlgorithm::CleanEdge
        )),
        "clicking CleanEdge must emit TransformAlgorithmChanged(CleanEdge), got {events:?}"
    );
}

#[test]
fn dragging_brush_slider_emits_brush_size_changed() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    let mut manager = tool_property_manager(&host);

    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let inactive_fill = ctx
        .style_of(egui::Theme::Dark)
        .visuals
        .widgets
        .inactive
        .bg_fill;
    let rail = rect_fills(&output, inactive_fill)
        .into_iter()
        .filter(|rect| rect.height() <= 10.0)
        .min_by(|a, b| a.center().y.total_cmp(&b.center().y))
        .expect("the brush-size slider rail should be painted");
    // The rail now starts at the panel's left edge, where the panel resize grip
    // covers its first point: drag from just inside it.
    let start = egui::pos2(rail.left() + 4.0, rail.center().y);
    let end = egui::pos2(rail.right() - 4.0, rail.center().y);
    let mid = egui::pos2((start.x + end.x) / 2.0, start.y);
    let press = |pos| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    };
    let release = |pos| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    };
    run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerMoved(start)],
    );
    run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![press(start)],
    );
    run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerMoved(mid)],
    );
    run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerMoved(end)],
    );
    run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![release(end)],
    );

    let events: Vec<ToolbarEvent> = host.events.borrow().clone();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ToolbarEvent::BrushSizeChanged(v) if *v != 1)),
        "dragging the slider must emit a changed brush size, got {events:?}"
    );
}

#[test]
fn numeric_input_and_slider_stay_in_sync() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    {
        let mut view = host.view.borrow_mut();
        view.tool = Tool::Pencil;
        view.draw.size = 20;
        view.draw.tail = 0;
        view.draw.scatter = 0;
    }
    let mut manager = tool_property_manager(&host);
    let screen = vec2(800.0, 600.0);
    let inactive_fill = ctx
        .style_of(egui::Theme::Dark)
        .visuals
        .widgets
        .inactive
        .bg_fill;
    let press = |pos| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    };
    let release = |pos| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    };

    // Given the Size row at 20, drag its input box to the right.
    let output = run_tool_property_frame(&ctx, &mut manager, &chrome.view(&theme), screen, vec![]);
    let input = row_numeric_input(&output, inactive_fill, "Size");
    let from = input.center();
    let to = egui::pos2(from.x + 40.0, from.y);
    for event in [
        egui::Event::PointerMoved(from),
        press(from),
        egui::Event::PointerMoved(egui::pos2(from.x + 20.0, from.y)),
        egui::Event::PointerMoved(to),
        release(to),
    ] {
        run_tool_property_frame(
            &ctx,
            &mut manager,
            &chrome.view(&theme),
            screen,
            vec![event],
        );
    }

    // Then the box that edits the slider's value also emits the slider's event.
    let events: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
    let boxed = events
        .iter()
        .find_map(|event| match event {
            ToolbarEvent::BrushSizeChanged(size) if *size > 20 => Some(*size),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!("dragging the input box must emit a larger size, got {events:?}")
        });
    assert!(boxed <= 64, "the input box must clamp to 64, got {boxed}");

    // Applying the event as the App would leaves the box showing the new size.
    host.view.borrow_mut().draw.size = boxed;
    let output = run_tool_property_frame(&ctx, &mut manager, &chrome.view(&theme), screen, vec![]);
    let input = row_numeric_input(&output, inactive_fill, "Size");
    let edited = text_rect(&output, &boxed.to_string())
        .unwrap_or_else(|| panic!("the box must show {boxed}: {:?}", rendered_texts(&output)));
    assert!(
        input.contains(edited.center()),
        "the box must show the value it dragged: {edited:?} vs {input:?}"
    );

    // When the slider is dragged to its right end instead.
    let rail = row_rail(&output, inactive_fill, "Size");
    let from = egui::pos2(rail.left() + 4.0, rail.center().y);
    let to = egui::pos2(rail.right() - 4.0, rail.center().y);
    for event in [
        egui::Event::PointerMoved(from),
        press(from),
        egui::Event::PointerMoved(egui::pos2(rail.center().x, rail.center().y)),
        egui::Event::PointerMoved(to),
        release(to),
    ] {
        run_tool_property_frame(
            &ctx,
            &mut manager,
            &chrome.view(&theme),
            screen,
            vec![event],
        );
    }

    // Then applying the event leaves the input box showing the slider's value.
    let events: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
    let slid = events
        .iter()
        .rev()
        .find_map(|event| match event {
            ToolbarEvent::BrushSizeChanged(size) => Some(*size),
            _ => None,
        })
        .unwrap_or_else(|| panic!("dragging the slider must emit a size, got {events:?}"));
    assert!(
        slid > boxed && slid <= 64,
        "dragging to the right end must raise {boxed}, got {slid}"
    );

    host.view.borrow_mut().draw.size = slid;
    let output = run_tool_property_frame(&ctx, &mut manager, &chrome.view(&theme), screen, vec![]);
    let input = row_numeric_input(&output, inactive_fill, "Size");
    let moved = text_rect(&output, &slid.to_string()).unwrap_or_else(|| {
        panic!(
            "the box must follow the slider to {slid}: {:?}",
            rendered_texts(&output)
        )
    });
    assert!(
        input.contains(moved.center()),
        "the box must show the slider's value: {moved:?} vs {input:?}"
    );
}

#[test]
fn reset_buttons_still_zero_the_value() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    {
        let mut view = host.view.borrow_mut();
        view.tool = Tool::Pencil;
        view.draw.size = 20;
        view.draw.tail = 5;
        view.draw.scatter = 7;
    }
    let mut manager = tool_property_manager(&host);
    let screen = vec2(800.0, 600.0);
    let inactive_fill = ctx
        .style_of(egui::Theme::Dark)
        .visuals
        .widgets
        .inactive
        .bg_fill;

    for label in ["Size", "Tail", "Scatter"] {
        let output =
            run_tool_property_frame(&ctx, &mut manager, &chrome.view(&theme), screen, vec![]);
        let reset = row_reset(&output, label);
        let rail = row_rail(&output, inactive_fill, label);
        assert!(
            (reset.center().y - rail.center().y).abs() < 24.0,
            "{label}: the reset button must sit on the slider's line: {reset:?} vs {rail:?}"
        );

        click_tool_property(&ctx, &mut manager, &chrome.view(&theme), reset.center());
        let events: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
        let expected = match label {
            "Size" => ToolbarEvent::BrushSizeChanged(BrushSpec::MIN_SIZE),
            "Tail" => ToolbarEvent::BrushTailChanged(0),
            _ => ToolbarEvent::BrushScatterChanged(0),
        };
        assert_eq!(
            events,
            vec![expected],
            "the {label} reset must emit exactly {expected:?}"
        );

        // Applying the reset as the App would leaves the input box on the
        // reset value, so the numeric input mirrors the slider's zeroing.
        let shown = match label {
            "Size" => {
                host.view.borrow_mut().draw.size = BrushSpec::MIN_SIZE;
                "1"
            }
            "Tail" => {
                host.view.borrow_mut().draw.tail = 0;
                "0"
            }
            _ => {
                host.view.borrow_mut().draw.scatter = 0;
                "0"
            }
        };
        let output =
            run_tool_property_frame(&ctx, &mut manager, &chrome.view(&theme), screen, vec![]);
        let input = row_numeric_input(&output, inactive_fill, label);
        assert!(
            text_rects(&output, shown)
                .into_iter()
                .any(|rect| input.contains(rect.center())),
            "the {label} input box must show {shown:?} after the reset: {:?}",
            rendered_texts(&output)
        );
    }
}

#[test]
fn tail_and_scatter_rows_reset_to_zero() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    {
        let mut view = host.view.borrow_mut();
        view.tool = Tool::Pencil;
        view.draw.tail = 5;
        view.draw.scatter = 7;
    }
    let mut manager = tool_property_manager(&host);

    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let tail_reset = row_reset(&output, "Tail");
    click_tool_property(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        tail_reset.center(),
    );
    let events: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
    assert_eq!(
        events,
        vec![ToolbarEvent::BrushTailChanged(0)],
        "the Tail reset must emit exactly BrushTailChanged(0)"
    );

    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let scatter_reset = row_reset(&output, "Scatter");
    click_tool_property(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        scatter_reset.center(),
    );
    let events: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
    assert_eq!(
        events,
        vec![ToolbarEvent::BrushScatterChanged(0)],
        "the Scatter reset must emit exactly BrushScatterChanged(0)"
    );
}

#[test]
fn tool_property_panel_exposes_scatter_for_draw_tools() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    let mut manager = tool_property_manager(&host);

    for tool in [Tool::Pencil, Tool::Eraser] {
        host.view.borrow_mut().tool = tool;
        let output = run_tool_property_frame(
            &ctx,
            &mut manager,
            &chrome.view(&theme),
            vec2(800.0, 600.0),
            vec![],
        );
        let scatter = text_pos(&output, "Scatter").expect("the Scatter row must paint");
        let shape = text_pos(&output, "Shape").expect("the Shape row must paint");
        assert!(
            scatter.y > shape.y,
            "{tool:?}: Scatter must sit below Shape: {scatter:?} vs {shape:?}"
        );
    }

    let press = |pos| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    };
    let release = |pos| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    };

    // The Scatter slider is the bottom-most slider rail; dragging it end to
    // end must emit BrushScatterChanged within 0..=32.
    host.view.borrow_mut().tool = Tool::Pencil;
    host.view.borrow_mut().draw.scatter = 0;
    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let inactive_fill = ctx
        .style_of(egui::Theme::Dark)
        .visuals
        .widgets
        .inactive
        .bg_fill;
    let rail = rect_fills(&output, inactive_fill)
        .into_iter()
        .filter(|rect| rect.height() <= 10.0)
        .max_by(|a, b| a.center().y.total_cmp(&b.center().y))
        .expect("the scatter slider rail should be painted");
    // The rail starts at the panel's left edge; the resize grip covers its first
    // point, so the drag starts just inside it.
    let start = egui::pos2(rail.left() + 4.0, rail.center().y);
    let end = egui::pos2(rail.right() - 4.0, rail.center().y);
    run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerMoved(start)],
    );
    run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![press(start)],
    );
    run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerMoved(end)],
    );
    run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![release(end)],
    );
    let events: Vec<ToolbarEvent> = host.events.borrow().clone();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ToolbarEvent::BrushScatterChanged(v) if *v > 0 && *v <= 32)),
        "dragging the Scatter slider must emit BrushScatterChanged within 0..=32, got {events:?}"
    );

    // The numeric input sits left of the rail and shows the scatter value; the
    // reset button's click is covered by `reset_buttons_still_zero_the_value`.
    host.view.borrow_mut().draw.scatter = 7;
    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let input = row_numeric_input(&output, inactive_fill, "Scatter");
    let rail = row_rail(&output, inactive_fill, "Scatter");
    assert!(
        input.right() <= rail.left(),
        "the Scatter input box must sit left of the rail: {input:?} vs {rail:?}"
    );
    let shown = text_rect(&output, "7").expect("the Scatter input box must show 7");
    assert!(
        input.contains(shown.center()),
        "7 must be painted inside the Scatter input box: {shown:?} vs {input:?}"
    );
    row_reset(&output, "Scatter");

    // Non-draw tools keep the placeholder: no Scatter row at all.
    host.view.borrow_mut().tool = Tool::Fill;
    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert!(
        text_pos(&output, "No editable properties for this tool yet.").is_some(),
        "the placeholder path must stay unchanged for other tools"
    );
    assert!(
        text_pos(&output, "Scatter").is_none(),
        "non-draw tools must not show the Scatter row: {:?}",
        rendered_texts(&output)
    );
}

#[test]
fn tool_property_content_shrinks_with_the_panel_width() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    host.view.borrow_mut().tool = Tool::Fill;
    let mut manager = tool_property_manager(&host);

    // A narrow-but-usable dock: a 40pt wide panel leaves a ~30pt nest viewport.
    let screen = vec2(400.0, 120.0);
    manager.resize_dock_area(DockSide::Right, -200.0, screen);
    assert_eq!(manager.dock_extent(DockSide::Right), 40.0);

    let output = run_tool_property_frame(&ctx, &mut manager, &chrome.view(&theme), screen, vec![]);

    // The 160pt-tall content overflows the short viewport vertically (one
    // vertical thumb) but the width follows the panel, so no horizontal thumb
    // is painted: the content must not force a width wider than the panel.
    let thumbs = rect_fills(&output, theme.colors.selection_bg_fill32());
    assert_eq!(
        thumbs.len(),
        1,
        "exactly the vertical thumb, no horizontal overflow: {thumbs:?}"
    );
    assert!(
        thumbs[0].width() < thumbs[0].height(),
        "the thumb must be vertical: {thumbs:?}"
    );
}

// ---------------------------------------------------------------------------
// Fieldier child picker + wand property tests
// ---------------------------------------------------------------------------

#[test]
fn fieldier_child_picker_and_wand_properties_render_together() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    {
        let mut view = host.view.borrow_mut();
        view.tool = Tool::Fieldier;
        view.child = FieldierChild::Wand;
    }
    let mut manager = tool_property_manager(&host);

    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );

    for label in [
        "Rectangle",
        "Wand",
        "Lasso",
        "Contiguous",
        "Tolerance",
        "Restrict to region",
    ] {
        assert!(
            text_pos(&output, label).is_some(),
            "the Fieldier/Wand panel must show {label:?}: {:?}",
            rendered_texts(&output)
        );
    }
    assert!(
        text_pos(&output, "No editable properties for this tool yet.").is_none(),
        "the Fieldier must not keep the placeholder"
    );
}

#[test]
fn wand_rows_hidden_for_non_wand_children() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    {
        let mut view = host.view.borrow_mut();
        view.tool = Tool::Fieldier;
    }
    let mut manager = tool_property_manager(&host);

    for child in [FieldierChild::Rectangle, FieldierChild::Lasso] {
        host.view.borrow_mut().child = child;
        let output = run_tool_property_frame(
            &ctx,
            &mut manager,
            &chrome.view(&theme),
            vec2(800.0, 600.0),
            vec![],
        );
        for label in ["Rectangle", "Wand", "Lasso"] {
            assert!(
                text_pos(&output, label).is_some(),
                "{child:?} must keep the {label:?} child picker entry: {:?}",
                rendered_texts(&output)
            );
        }
        for label in ["Contiguous", "Tolerance", "Restrict to region"] {
            assert!(
                text_pos(&output, label).is_none(),
                "{child:?} must not show the wand row {label:?}: {:?}",
                rendered_texts(&output)
            );
        }
    }
}

#[test]
fn wand_contiguous_checkbox_is_display_only() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    {
        let mut view = host.view.borrow_mut();
        view.tool = Tool::Fieldier;
        view.child = FieldierChild::Wand;
        view.wand_contiguous_effective = true;
    }
    let mut manager = tool_property_manager(&host);

    let output = run_tool_property_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let contiguous = text_rect(&output, "Contiguous")
        .expect("the Contiguous checkbox paints")
        .center();

    click_tool_property(&ctx, &mut manager, &chrome.view(&theme), contiguous);

    let events: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
    assert!(
        events.is_empty(),
        "a click on the display-only Contiguous checkbox must emit nothing, got {events:?}"
    );
    assert_eq!(
        host.view.borrow().wand,
        WandSettings::default(),
        "the display-only checkbox must not change the stored wand settings"
    );
}

#[test]
fn wand_tolerance_control_emits_event() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    {
        let mut view = host.view.borrow_mut();
        view.tool = Tool::Fieldier;
        view.child = FieldierChild::Wand;
        view.wand.tolerance = 0;
    }
    let mut manager = tool_property_manager(&host);
    let screen = vec2(800.0, 600.0);

    let output = run_tool_property_frame(&ctx, &mut manager, &chrome.view(&theme), screen, vec![]);
    let inactive_fill = ctx
        .style_of(egui::Theme::Dark)
        .visuals
        .widgets
        .inactive
        .bg_fill;
    let rail = row_rail(&output, inactive_fill, "Tolerance");
    // The rail starts at the panel's left edge, where the panel resize grip
    // covers its first point: drag from just inside it.
    let start = egui::pos2(rail.left() + 4.0, rail.center().y);
    let end = egui::pos2(rail.right() - 4.0, rail.center().y);
    let mid = egui::pos2((start.x + end.x) / 2.0, start.y);
    let press = |pos| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    };
    let release = |pos| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    };
    for event in [
        egui::Event::PointerMoved(start),
        press(start),
        egui::Event::PointerMoved(mid),
        egui::Event::PointerMoved(end),
        release(end),
    ] {
        run_tool_property_frame(
            &ctx,
            &mut manager,
            &chrome.view(&theme),
            screen,
            vec![event],
        );
    }

    let events: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ToolbarEvent::WandToleranceChanged(v) if *v > 0)),
        "dragging the Tolerance slider must emit WandToleranceChanged, got {events:?}"
    );
}

#[test]
fn wand_restrict_checkbox_emits_event() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = ToolboxHost::default();
    {
        let mut view = host.view.borrow_mut();
        view.tool = Tool::Fieldier;
        view.child = FieldierChild::Wand;
    }
    let mut manager = tool_property_manager(&host);
    let screen = vec2(800.0, 600.0);

    let output = run_tool_property_frame(&ctx, &mut manager, &chrome.view(&theme), screen, vec![]);
    let restrict =
        text_pos(&output, "Restrict to region").expect("the Restrict to region checkbox paints");
    click_tool_property(&ctx, &mut manager, &chrome.view(&theme), restrict);
    let events: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
    assert_eq!(
        events,
        vec![ToolbarEvent::WandRestrictToRegionChanged(true)],
        "clicking Restrict to region must emit WandRestrictToRegionChanged(true)"
    );

    // Applying the event as the App would flips the checkbox; clicking again
    // must emit the false transition.
    host.view.borrow_mut().wand.restrict_to_region = true;
    let output = run_tool_property_frame(&ctx, &mut manager, &chrome.view(&theme), screen, vec![]);
    let restrict =
        text_pos(&output, "Restrict to region").expect("the Restrict to region checkbox paints");
    click_tool_property(&ctx, &mut manager, &chrome.view(&theme), restrict);
    let events: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
    assert_eq!(
        events,
        vec![ToolbarEvent::WandRestrictToRegionChanged(false)],
        "the second click must emit WandRestrictToRegionChanged(false)"
    );
}

// ---------------------------------------------------------------------------
// Layers dock panel tests
// ---------------------------------------------------------------------------

use pyxross::core::model::{BlendMode, LayerId};
use pyxross::ui::dock_hosts::{LayerPanelHost, LayerRow, LayerView};
use pyxross::ui::dock_layers_panel::{
    layer_settings_panel_spec, layers_panel_spec, LAYER_SETTINGS_PANEL_ID,
};
use pyxross::ui::layers::LayerPanelEvent;

fn layers_manager(host: &LayerPanelHost) -> DockManager {
    DockManager::try_new([layers_panel_spec(host, PanelPlacement::DockedRight)]).unwrap()
}

/// A dock holding the Layers panel plus the per-layer settings floating panel
/// (id 102), mirroring the App's open state.
fn layers_settings_manager(host: &LayerPanelHost) -> DockManager {
    DockManager::try_new([
        layers_panel_spec(host, PanelPlacement::DockedRight),
        layer_settings_panel_spec(
            host,
            Rect::from_min_size(pos2(360.0, 140.0), vec2(280.0, 200.0)),
        ),
    ])
    .unwrap()
}

fn run_layers_frame(
    ctx: &egui::Context,
    manager: &mut DockManager,
    chrome: &PanelChrome,
    screen: Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), screen)),
            events,
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| manager.show_inside(ui, chrome));
        },
    );
    output.textures_delta.clear();
    output
}

/// A full click (hover, press, release) at `pos`.
fn click_at(ctx: &egui::Context, manager: &mut DockManager, chrome: &PanelChrome, pos: egui::Pos2) {
    run_layers_frame(
        ctx,
        manager,
        chrome,
        vec2(800.0, 600.0),
        vec![egui::Event::PointerMoved(pos)],
    );
    run_layers_frame(
        ctx,
        manager,
        chrome,
        vec2(800.0, 600.0),
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    run_layers_frame(
        ctx,
        manager,
        chrome,
        vec2(800.0, 600.0),
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
}

fn layer_row(
    id: u64,
    name: &str,
    is_group: bool,
    depth: usize,
    has_children: bool,
    expanded: bool,
) -> LayerRow {
    LayerRow {
        id: LayerId::new(id),
        name: name.to_string(),
        visible: true,
        opacity: 1.0,
        blend: BlendMode::Normal,
        is_group,
        depth,
        has_children,
        expanded,
    }
}

fn set_view(
    host: &LayerPanelHost,
    rows: Vec<LayerRow>,
    active: Option<LayerId>,
    merge_enabled: bool,
    delete_enabled: bool,
) {
    *host.view.borrow_mut() = LayerView {
        rows,
        active,
        merge_enabled,
        delete_enabled,
    };
}

#[test]
fn layers_panel_lists_rows_in_view_order_with_indent() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = LayerPanelHost::default();
    set_view(
        &host,
        vec![
            layer_row(1, "Group", true, 0, true, true),
            layer_row(2, "Child", false, 1, false, true),
            layer_row(3, "Top", false, 0, false, true),
        ],
        Some(LayerId::new(2)),
        false,
        false,
    );
    let mut manager = layers_manager(&host);
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );

    let y = |name: &str| {
        text_pos(&output, name)
            .unwrap_or_else(|| panic!("missing row {name}"))
            .y
    };
    assert!(
        y("Group") < y("Child"),
        "rows render in view order (topmost first)"
    );
    assert!(
        y("Child") < y("Top"),
        "rows render in view order (topmost first)"
    );
    let x = |name: &str| {
        text_pos(&output, name)
            .unwrap_or_else(|| panic!("missing row {name}"))
            .x
    };
    assert!(
        x("Child") > x("Group"),
        "a deeper row must start further right than its parent: {} vs {}",
        x("Child"),
        x("Group")
    );
}

#[test]
fn header_add_and_delete_emit_events() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = LayerPanelHost::default();
    set_view(
        &host,
        vec![layer_row(1, "Layer 1", false, 0, false, true)],
        Some(LayerId::new(1)),
        false,
        false,
    );
    let mut manager = layers_manager(&host);

    // Add always emits.
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let add = text_pos(&output, "+ Add").expect("add button paints");
    click_at(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        add + vec2(4.0, 8.0),
    );
    assert_eq!(host.events.borrow().clone(), vec![LayerPanelEvent::Add]);

    // Delete is disabled when delete_enabled is false: clicking emits nothing.
    host.events.borrow_mut().clear();
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let delete = text_pos(&output, "🗑 Delete").expect("delete button paints");
    click_at(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        delete + vec2(4.0, 8.0),
    );
    assert!(
        host.events.borrow().is_empty(),
        "disabled delete must not emit"
    );

    // With delete_enabled, clicking emits Remove(active).
    set_view(
        &host,
        vec![layer_row(1, "Layer 1", false, 0, false, true)],
        Some(LayerId::new(1)),
        false,
        true,
    );
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let delete = text_pos(&output, "🗑 Delete").expect("delete button paints");
    click_at(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        delete + vec2(4.0, 8.0),
    );
    assert_eq!(
        host.events.borrow().clone(),
        vec![LayerPanelEvent::Remove(LayerId::new(1))]
    );
}

#[test]
fn merge_disabled_when_not_mergeable() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = LayerPanelHost::default();
    set_view(
        &host,
        vec![layer_row(1, "Layer 1", false, 0, false, true)],
        Some(LayerId::new(1)),
        false,
        false,
    );
    let mut manager = layers_manager(&host);

    // Not mergeable: the button is disabled and clicking emits nothing.
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let merge = text_pos(&output, "Merge").expect("merge button paints");
    click_at(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        merge + vec2(4.0, 8.0),
    );
    assert!(
        host.events.borrow().is_empty(),
        "disabled merge must not emit"
    );

    // Mergeable: clicking emits MergeDown.
    set_view(
        &host,
        vec![layer_row(1, "Layer 1", false, 0, false, true)],
        Some(LayerId::new(1)),
        true,
        false,
    );
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let merge = text_pos(&output, "Merge").expect("merge button paints");
    click_at(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        merge + vec2(4.0, 8.0),
    );
    assert_eq!(
        host.events.borrow().clone(),
        vec![LayerPanelEvent::MergeDown]
    );
}

#[test]
fn group_row_arrow_emits_toggle_expanded() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = LayerPanelHost::default();
    set_view(
        &host,
        vec![
            layer_row(1, "Group", true, 0, true, true),
            layer_row(2, "Child", false, 1, false, true),
        ],
        None,
        false,
        false,
    );
    let mut manager = layers_manager(&host);

    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let arrow = text_pos(&output, "▼").expect("expanded group arrow paints");
    click_at(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        arrow + vec2(4.0, 8.0),
    );
    assert_eq!(
        host.events.borrow().clone(),
        vec![LayerPanelEvent::ToggleExpanded(LayerId::new(1))]
    );
}

#[test]
fn eye_and_settings_buttons_emit_their_events() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = LayerPanelHost::default();
    set_view(
        &host,
        vec![
            layer_row(1, "A", false, 0, false, true),
            layer_row(2, "B", false, 0, false, true),
        ],
        None,
        false,
        false,
    );
    let mut manager = layers_manager(&host);

    // The first eye belongs to the topmost row.
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let eye = text_pos(&output, "👁").expect("eye paints");
    click_at(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        eye + vec2(4.0, 8.0),
    );
    assert_eq!(
        host.events.borrow().clone(),
        vec![LayerPanelEvent::ToggleVisible(LayerId::new(1))]
    );

    // The first settings button belongs to the topmost row.
    host.events.borrow_mut().clear();
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let gear = text_pos(&output, "⚙").expect("settings button paints");
    click_at(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        gear + vec2(4.0, 8.0),
    );
    assert_eq!(
        host.events.borrow().clone(),
        vec![LayerPanelEvent::SettingsToggle(LayerId::new(1))]
    );
}

#[test]
fn drop_onto_a_row_emits_reorder_dropped() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = LayerPanelHost::default();
    set_view(
        &host,
        vec![
            layer_row(1, "A", false, 0, false, true),
            layer_row(2, "B", false, 0, false, true),
        ],
        None,
        false,
        false,
    );
    let mut manager = layers_manager(&host);

    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let a = text_pos(&output, "A").expect("row A paints") + vec2(4.0, 8.0);
    let b = text_pos(&output, "B").expect("row B paints") + vec2(4.0, 8.0);

    // Press on A's name, drag to B's name, release.
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerMoved(a)],
    );
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerButton {
            pos: a,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerMoved(b)],
    );
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![egui::Event::PointerButton {
            pos: b,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );

    assert_eq!(
        host.events.borrow().clone(),
        vec![LayerPanelEvent::ReorderDropped {
            dragged: LayerId::new(1),
            target: LayerId::new(2),
        }]
    );
}

#[test]
fn settings_popup_renders_for_the_open_layer_and_hides_when_closed() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = LayerPanelHost::default();
    set_view(
        &host,
        vec![layer_row(1, "Layer 1", false, 0, false, true)],
        Some(LayerId::new(1)),
        false,
        false,
    );
    let mut manager = layers_settings_manager(&host);

    // Closed: no popup widgets.
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert!(
        text_pos(&output, "Opacity").is_none(),
        "no popup when closed"
    );

    // Open for the layer: the popup's name/blend widgets render. The window
    // paints its content from the second frame (egui's fade-in).
    *host.open_settings.borrow_mut() = Some(LayerId::new(1));
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert!(
        text_pos(&output, "Opacity").is_some(),
        "opacity label renders"
    );
    assert!(text_pos(&output, "Blend").is_some(), "blend label renders");
    assert!(
        text_pos(&output, "Normal").is_some(),
        "blend combo shows the current mode"
    );

    // Closed again: gone.
    *host.open_settings.borrow_mut() = None;
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert!(
        text_pos(&output, "Opacity").is_none(),
        "popup hides when closed"
    );
}

#[test]
fn settings_popup_shows_ungroup_only_for_groups() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = LayerPanelHost::default();
    set_view(
        &host,
        vec![
            layer_row(1, "Group", true, 0, true, true),
            layer_row(2, "Leaf", false, 1, false, true),
        ],
        None,
        false,
        false,
    );
    let mut manager = layers_settings_manager(&host);

    // A group's popup offers ungroup.
    *host.open_settings.borrow_mut() = Some(LayerId::new(1));
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert!(
        text_pos(&output, "Ungroup").is_some(),
        "group popup must offer ungroup"
    );

    // A leaf's popup does not.
    *host.open_settings.borrow_mut() = Some(LayerId::new(2));
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert!(
        text_pos(&output, "Ungroup").is_none(),
        "leaf popup must not offer ungroup"
    );
}

#[test]
fn settings_popup_shows_the_persistent_rename_buffer() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = LayerPanelHost::default();
    set_view(
        &host,
        vec![layer_row(1, "Layer 1", false, 0, false, true)],
        Some(LayerId::new(1)),
        false,
        false,
    );
    let mut manager = layers_settings_manager(&host);

    // The in-progress rename text lives in the host buffer (the App seeds it
    // when the popup opens); the popup must show the buffer, not the row name.
    *host.open_settings.borrow_mut() = Some(LayerId::new(1));
    *host.rename_buffer.borrow_mut() = "Sketch".to_string();
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );

    assert!(
        text_pos(&output, "Sketch").is_some(),
        "the popup must render the persistent rename buffer"
    );
}

#[test]
fn layer_settings_nest_has_no_close_button() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = LayerPanelHost::default();
    set_view(
        &host,
        vec![layer_row(1, "Layer 1", false, 0, false, true)],
        Some(LayerId::new(1)),
        false,
        false,
    );
    let mut manager = layers_settings_manager(&host);

    // Open the popup with the buffer seeded (the App does this on open).
    *host.open_settings.borrow_mut() = Some(LayerId::new(1));
    *host.rename_buffer.borrow_mut() = "Layer 1".to_string();
    let output = frame_until_settled(&ctx, &mut manager, &chrome.view(&theme));

    // Closing moved to the top bar: the nest paints no ✕ close affordance.
    let texts = rendered_texts(&output);
    assert!(
        !texts.iter().any(|text| text.contains('✕')),
        "the layer-settings nest must not paint a close button: {texts:?}"
    );

    // The popup still renders its fields under the panel's own header band
    // (the layers panel's header is far above the centered floating panel).
    let header_fills = rect_fills_recursive(&output, theme.colors.panel_header_bg32());
    let opacity = text_pos(&output, "Opacity").expect("popup content renders");
    assert!(
        header_fills.iter().any(|rect| {
            let gap = opacity.y - rect.bottom();
            (0.0..100.0).contains(&gap)
        }),
        "the popup must paint a header band above its content: {header_fills:?} vs {opacity:?}"
    );
    assert!(
        text_pos(&output, "Blend").is_some(),
        "the blend row still renders"
    );
    assert!(
        text_pos(&output, "Normal").is_some(),
        "the blend combo still renders"
    );
}

#[test]
fn settings_panel_is_registered_as_a_floating_dock_panel() {
    let host = LayerPanelHost::default();
    let mut manager = layers_settings_manager(&host);
    let id = PanelId::new(LAYER_SETTINGS_PANEL_ID);
    manager.raise_floating(id);

    assert_eq!(manager.placement(id), Some(PanelPlacement::Floating));
    assert!(manager.floating_order().contains(&id));
    assert_eq!(
        manager.header_action(id),
        Some(PanelHeaderAction::Close),
        "floating-only panels close from the header"
    );
}

#[test]
fn floating_only_panel_offers_close_instead_of_dock() {
    // A floating-only panel (cannot dock) exposes Close.
    let host = LayerPanelHost::default();
    let manager = layers_settings_manager(&host);
    let settings_id = PanelId::new(LAYER_SETTINGS_PANEL_ID);
    assert_eq!(
        manager.header_action(settings_id),
        Some(PanelHeaderAction::Close)
    );
    assert_eq!(PanelHeaderAction::Close.label(), "Close");

    // A dockable floating panel keeps the Dock action.
    let dockable = floating_manager();
    let panel = PanelId::new(9);
    assert!(dockable.metadata(panel).unwrap().can_pop_out);
    assert_eq!(dockable.header_action(panel), Some(PanelHeaderAction::Dock));
    assert_eq!(PanelHeaderAction::Dock.label(), "Dock");

    // Clicking the header's Close button drives the ClosePanel action.
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut manager = layers_settings_manager(&host);
    let output = frame_until_settled(&ctx, &mut manager, &chrome.view(&theme));
    let close = text_pos(&output, "Close").expect("the Close header action paints");
    click_at(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        close + vec2(4.0, 8.0),
    );
    assert_eq!(
        manager.drain_actions(),
        vec![DockAction::ClosePanel(settings_id)]
    );
}

/// A dock holding one panel on the right plus one floating panel whose
/// `can_pop_out` is given, mirroring the App's docked + floating mix.
fn floating_plus_dock_manager(can_pop_out: bool) -> DockManager {
    DockManager::try_new([
        PanelSpec {
            id: PanelId::new(7),
            metadata: PanelMetadata::new("Docked", false, vec2(120.0, 80.0)),
            placement: PanelPlacement::DockedRight,
            floating_rect: Rect::from_min_size(pos2(560.0, 40.0), vec2(200.0, 160.0)),
            content: Box::new(EmptyContent),
        },
        PanelSpec {
            id: PanelId::new(8),
            metadata: PanelMetadata::new("Floating", can_pop_out, vec2(120.0, 80.0)),
            placement: PanelPlacement::Floating,
            floating_rect: Rect::from_min_size(pos2(80.0, 120.0), vec2(220.0, 160.0)),
            content: Box::new(EmptyContent),
        },
    ])
    .unwrap()
}

/// Drags `id`'s header to `drop` and releases there, returning the frame that
/// painted while the pointer was over `drop` (the drop-preview frame).
fn drag_header_to(
    ctx: &egui::Context,
    manager: &mut DockManager,
    chrome: &PanelChrome,
    id: PanelId,
    drop: egui::Pos2,
) -> egui::FullOutput {
    let screen = vec2(800.0, 600.0);
    let frame = |manager: &mut DockManager, events: Vec<egui::Event>| {
        run_layers_frame(ctx, manager, chrome, screen, events)
    };
    for _ in 0..4 {
        frame(manager, vec![]);
    }
    let start = manager.floating_rect(id).expect("the dragged panel floats");
    let grab = pos2(start.left() + 40.0, start.top() + 14.0);
    frame(manager, vec![pointer_move(grab)]);
    frame(manager, vec![pointer_press(grab)]);
    let preview = frame(manager, vec![pointer_move(drop)]);
    frame(manager, vec![pointer_release(drop)]);
    for _ in 0..2 {
        frame(manager, vec![]);
    }
    preview
}

#[test]
fn float_only_panel_is_never_docked_by_drag_and_drop() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut manager = floating_plus_dock_manager(false);
    let id = PanelId::new(8);

    // Given: a floating panel with can_pop_out == false and a wide right dock.
    assert!(
        manager.is_float_only(id),
        "a floating can_pop_out == false panel is float-only"
    );
    let start = manager
        .floating_rect(id)
        .expect("the float-only panel floats");

    // When: its header is dragged onto the right dock band and released.
    let over_dock = pos2(700.0, 300.0);
    let preview = drag_header_to(&ctx, &mut manager, &chrome.view(&theme), id, over_dock);

    // Then: no drop preview was offered and no dock transition happened.
    assert!(
        rect_fills_recursive(&preview, theme.colors.selection_fill32()).is_empty(),
        "a float-only drag must not paint a drop preview"
    );
    assert_eq!(
        manager.placement(id),
        Some(PanelPlacement::Floating),
        "the float-only panel must stay floating after the drop"
    );
    assert!(
        matches!(
            manager.transition_panel(id, PanelPlacement::DockedRight),
            Err(DockError::DockNotAllowed(panel)) if panel == id
        ),
        "a direct dock transition must be rejected"
    );
    assert_eq!(manager.placement(id), Some(PanelPlacement::Floating));

    // And: dragging it still moves it (the header drag keeps working).
    let moved = manager
        .floating_rect(id)
        .expect("the float-only panel still floats");
    assert_ne!(
        moved.min, start.min,
        "the header drag must move the float-only panel"
    );
}

#[test]
fn dockable_floating_panel_still_docks() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut manager = floating_plus_dock_manager(true);
    let id = PanelId::new(8);

    // Given: a dockable (can_pop_out == true) floating panel.
    assert!(
        !manager.is_float_only(id),
        "a can_pop_out panel is never float-only"
    );

    // When: its header is dragged onto the right dock band and released.
    let over_dock = pos2(700.0, 300.0);
    let preview = drag_header_to(&ctx, &mut manager, &chrome.view(&theme), id, over_dock);

    // Then: the drop preview was offered and the panel docked right.
    assert!(
        !rect_fills_recursive(&preview, theme.colors.selection_fill32()).is_empty(),
        "a dockable drag must paint a drop preview"
    );
    assert_eq!(
        manager.placement(id),
        Some(PanelPlacement::DockedRight),
        "a can_pop_out floating panel must still dock"
    );
    assert!(manager.dock_order(DockSide::Right).contains(&id));
}

#[test]
fn docked_panels_with_can_pop_out_false_stay_docked() {
    // Given: the demo dock, whose Panel B (right) and Panel C (bottom) are
    // docked with can_pop_out == false (the Toolbox/Layers shape).
    let mut manager = DockManager::demo();
    let panel_b = PanelId::new(2);
    let panel_c = PanelId::new(3);

    // Then: they are not float-only — the rule needs `placement == Floating`.
    assert_eq!(
        manager.placement(panel_b),
        Some(PanelPlacement::DockedRight)
    );
    assert_eq!(
        manager.placement(panel_c),
        Some(PanelPlacement::DockedBottom)
    );
    assert!(!manager.is_float_only(panel_b));
    assert!(!manager.is_float_only(panel_c));
    assert_eq!(
        manager.header_action(panel_b),
        None,
        "docked no-pop-out panels offer no header action"
    );

    // And: they cannot float, but they still dock between dock areas.
    assert!(matches!(
        manager.transition_panel(panel_b, PanelPlacement::Floating),
        Err(DockError::PopOutNotAllowed(panel)) if panel == panel_b
    ));
    manager
        .transition_panel(panel_b, PanelPlacement::DockedLeft)
        .unwrap();
    assert_eq!(manager.placement(panel_b), Some(PanelPlacement::DockedLeft));

    // And: dragging a docked no-pop-out panel to the canvas leaves it docked.
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let (_, viewport) = run_demo_frame_capturing_viewport(&ctx, &mut manager, &chrome.view(&theme));
    let bottom_rect = Rect::from_min_max(
        pos2(
            viewport.left(),
            viewport.bottom() - manager.dock_extent(DockSide::Bottom),
        ),
        pos2(
            viewport.right() - manager.dock_extent(DockSide::Right),
            viewport.bottom(),
        ),
    );
    let grab = manager
        .panel_rects(DockSide::Bottom, bottom_rect)
        .into_iter()
        .find(|(id, _)| *id == panel_c)
        .map(|(_, rect)| pos2(rect.center().x, rect.top() + 14.0))
        .expect("Panel C paints in the bottom dock");
    let canvas = pos2(300.0, 300.0);
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![pointer_move(grab)],
    );
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![pointer_press(grab)],
    );
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![pointer_move(canvas)],
    );
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![pointer_release(canvas)],
    );

    assert_eq!(
        manager.placement(panel_c),
        Some(PanelPlacement::DockedBottom),
        "a docked can_pop_out == false panel must never float"
    );
}

#[test]
fn settings_panel_moves_with_a_header_drag() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = LayerPanelHost::default();
    set_view(
        &host,
        vec![layer_row(1, "Layer 1", false, 0, false, true)],
        Some(LayerId::new(1)),
        false,
        false,
    );
    *host.open_settings.borrow_mut() = Some(LayerId::new(1));
    let mut manager = layers_settings_manager(&host);
    let id = PanelId::new(LAYER_SETTINGS_PANEL_ID);
    manager.raise_floating(id);

    let frame = |manager: &mut DockManager, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .show(ui, |ui| manager.show_inside(ui, &chrome.view(&theme)));
            },
        );
        output.textures_delta.clear();
    };

    // Settle the window, then grab its header band and drag it.
    frame(&mut manager, vec![]);
    let initial = manager.floating_rect(id).unwrap();
    let header = pos2(initial.center().x, initial.top() + 14.0);
    frame(&mut manager, vec![egui::Event::PointerMoved(header)]);
    frame(
        &mut manager,
        vec![egui::Event::PointerButton {
            pos: header,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    frame(
        &mut manager,
        vec![egui::Event::PointerMoved(header + vec2(60.0, 40.0))],
    );
    frame(&mut manager, Vec::new());

    let moved = manager.floating_rect(id).unwrap();
    assert!(
        moved.min.x > initial.min.x,
        "settings panel did not move: {initial:?} -> {moved:?}"
    );
    assert!(
        moved.min.y > initial.min.y,
        "settings panel did not move: {initial:?} -> {moved:?}"
    );
}

#[test]
fn settings_panel_close_removes_it_from_the_dock() {
    let host = LayerPanelHost::default();
    let mut manager = layers_settings_manager(&host);
    let id = PanelId::new(LAYER_SETTINGS_PANEL_ID);
    *host.open_settings.borrow_mut() = Some(LayerId::new(1));
    let count_before = manager.panel_count();

    // The App's close path: remove the panel and clear the open layer.
    assert!(manager.remove_spec(id));
    *host.open_settings.borrow_mut() = None;

    assert_eq!(manager.panel_count(), count_before - 1);
    assert_eq!(manager.placement(id), None);
    assert_eq!(*host.open_settings.borrow(), None);
}

#[test]
fn settings_panel_title_follows_the_layer_name() {
    let host = LayerPanelHost::default();
    let mut manager = layers_settings_manager(&host);
    let id = PanelId::new(LAYER_SETTINGS_PANEL_ID);
    assert_eq!(manager.metadata(id).unwrap().title, "Layer settings");

    // The App re-titles the panel from the layer name every frame.
    manager.set_title(id, "Layer: Sketch");
    assert_eq!(manager.metadata(id).unwrap().title, "Layer: Sketch");
}

#[test]
fn settings_panel_switch_moves_to_the_new_layer() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = LayerPanelHost::default();
    set_view(
        &host,
        vec![
            layer_row(1, "A", false, 0, false, true),
            layer_row(2, "B", false, 0, false, true),
        ],
        None,
        false,
        false,
    );
    let mut manager = layers_settings_manager(&host);
    let id = PanelId::new(LAYER_SETTINGS_PANEL_ID);
    manager.raise_floating(id);

    // Open on layer A: the panel renders its settings.
    *host.open_settings.borrow_mut() = Some(LayerId::new(1));
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert!(
        text_pos(&output, "Opacity").is_some(),
        "settings panel renders for layer A"
    );

    // Switch to layer B: the same single panel now targets B.
    *host.open_settings.borrow_mut() = Some(LayerId::new(2));
    run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );
    assert!(
        text_pos(&output, "Opacity").is_some(),
        "settings panel still renders after the switch"
    );
    assert_eq!(
        manager.panel_count(),
        2,
        "switching must not add a second settings panel"
    );
}

#[test]
fn layer_rows_fill_the_nest_width_and_pin_settings_right() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = LayerPanelHost::default();
    set_view(
        &host,
        vec![
            layer_row(1, "A", false, 0, false, true),
            layer_row(2, "B", false, 0, false, true),
        ],
        Some(LayerId::new(1)),
        false,
        false,
    );
    let mut manager = layers_manager(&host);
    let output = run_layers_frame(
        &ctx,
        &mut manager,
        &chrome.view(&theme),
        vec2(800.0, 600.0),
        vec![],
    );

    // The layers panel fills the right dock (240pt wide at this viewport).
    let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
    let right_rect = Rect::from_min_max(
        pos2(
            viewport.right() - manager.dock_extent(DockSide::Right),
            viewport.top(),
        ),
        viewport.max,
    );
    let panel_rect = manager.panel_rects(DockSide::Right, right_rect)[0].1;

    // The gear is pinned to the far right of the row.
    let gear = text_rect(&output, "⚙").expect("settings button paints");
    assert!(
        (panel_rect.right() - gear.right()).abs() < 30.0,
        "gear must hug the nest's right edge: gear {gear:?} vs panel {panel_rect:?}"
    );

    // The eye and name stay near the left edge, far from the gear.
    let eye = text_rect(&output, "👁").expect("eye paints");
    let name = text_rect(&output, "A").expect("name paints");
    assert!(
        eye.left() - panel_rect.left() < 80.0,
        "eye must stay near the left edge: {eye:?} vs {panel_rect:?}"
    );
    assert!(
        name.left() - panel_rect.left() < 120.0,
        "name must stay near the left edge: {name:?} vs {panel_rect:?}"
    );
    assert!(
        gear.left() > name.right() + 80.0,
        "gear must sit far right of the name: gear {gear:?} vs name {name:?}"
    );

    // The active row's background spans the full nest width. The row paints
    // with egui's default selection fill (the harness never installs the app
    // theme into the context visuals).
    let selection = rect_fills(&output, egui::Visuals::dark().selection.bg_fill);
    assert!(
        selection
            .iter()
            .any(|rect| (panel_rect.right() - rect.right()).abs() < 30.0),
        "the active row background must cover the full row width: {selection:?}"
    );
}

#[test]
fn layers_content_shrinks_with_the_panel_width() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let host = LayerPanelHost::default();
    set_view(
        &host,
        vec![
            layer_row(1, "A", false, 0, false, true),
            layer_row(2, "B", false, 0, false, true),
        ],
        Some(LayerId::new(1)),
        false,
        false,
    );
    let mut manager = layers_manager(&host);

    // A narrow-but-usable dock: a 40pt wide panel leaves a ~30pt nest viewport.
    let screen = vec2(400.0, 120.0);
    manager.resize_dock_area(DockSide::Right, -200.0, screen);
    assert_eq!(manager.dock_extent(DockSide::Right), 40.0);

    let output = run_layers_frame(&ctx, &mut manager, &chrome.view(&theme), screen, vec![]);

    // Two rows (136pt tall) overflow the short viewport vertically (one
    // vertical thumb) but the width follows the panel, so no horizontal thumb
    // is painted: the list must not force the old 240pt minimum width.
    let thumbs = rect_fills(&output, theme.colors.selection_bg_fill32());
    assert_eq!(
        thumbs.len(),
        1,
        "exactly the vertical thumb, no horizontal overflow: {thumbs:?}"
    );
    assert!(
        thumbs[0].width() < thumbs[0].height(),
        "the thumb must be vertical: {thumbs:?}"
    );
}

#[test]
fn panel_titles_are_centered_in_the_header_band() {
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut manager = DockManager::demo();
    let output = run_demo_frame(&ctx, &mut manager, &chrome.view(&theme));

    // The title must sit inside a painted header band and be horizontally
    // centered within it (the band is the header rect itself, so this proves
    // the title is centered in the header).
    let title = text_rect(&output, "Preview").expect("panel title paints");
    let header_fills = rect_fills(&output, theme.colors.panel_header_bg32());
    let band = header_fills
        .iter()
        .find(|rect| rect.top() <= title.center().y && title.center().y <= rect.bottom())
        .expect("the title must sit inside a painted header band");
    assert!(
        (title.center().x - band.center().x).abs() < 3.0,
        "title must be horizontally centered in the header band: title {title:?} vs band {band:?}"
    );
}

#[test]
fn brush_shape_selector_still_works_from_the_panel() {
    use pyxross::core::brush::BrushShape;
    use pyxross::input::Tool;
    use pyxross::ui::dock_hosts::{ToolboxHost, ToolboxView};
    use pyxross::ui::dock_tool_property_panel::tool_property_panel_spec;
    use pyxross::ui::panel_dock::{PanelId, PanelPlacement};
    use pyxross::ui::project::DrawSettings;
    use pyxross::ui::toolbar::ToolbarEvent;

    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let host = ToolboxHost::default();
    *host.view.borrow_mut() = ToolboxView {
        tool: Tool::Pencil,
        draw: DrawSettings {
            size: 4,
            shape: BrushShape::Square,
            scatter: 0,
            ..DrawSettings::default()
        },
        ..ToolboxView::default()
    };
    let surface = SurfaceId::Panel(PanelId::new(103));
    let capture = InputCapture::new();
    capture.set_target(Some(surface));
    let chrome = PanelChrome::flat(&theme, &capture, surface);
    let mut manager =
        DockManager::try_new([tool_property_panel_spec(&host, PanelPlacement::DockedRight)])
            .unwrap();
    let frame = |manager: &mut DockManager, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| manager.show_inside(ui, &chrome));
            },
        );
        output.textures_delta.clear();
        output
    };

    let output = frame(&mut manager, vec![]);
    let circle = text_rect(&output, "Circle").expect("the Circle shape button paints");
    let center = circle.center();

    frame(&mut manager, vec![egui::Event::PointerMoved(center)]);
    frame(
        &mut manager,
        vec![egui::Event::PointerButton {
            pos: center,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    frame(
        &mut manager,
        vec![egui::Event::PointerButton {
            pos: center,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );

    let events: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ToolbarEvent::BrushShapeChanged(BrushShape::Round))),
        "clicking Circle must emit BrushShapeChanged(Round): {events:?}"
    );
}

#[test]
fn tool_property_panel_exposes_tail_for_draw_tools() {
    use pyxross::input::Tool;
    use pyxross::ui::dock_hosts::ToolboxHost;
    use pyxross::ui::dock_tool_property_panel::tool_property_panel_spec;
    use pyxross::ui::panel_dock::{PanelId, PanelPlacement};
    use pyxross::ui::toolbar::ToolbarEvent;

    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let host = ToolboxHost::default();
    let surface = SurfaceId::Panel(PanelId::new(103));
    let capture = InputCapture::new();
    capture.set_target(Some(surface));
    let chrome = PanelChrome::flat(&theme, &capture, surface);
    let mut manager =
        DockManager::try_new([tool_property_panel_spec(&host, PanelPlacement::DockedRight)])
            .unwrap();
    let frame = |manager: &mut DockManager, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| manager.show_inside(ui, &chrome));
            },
        );
        output.textures_delta.clear();
        output
    };

    for tool in [Tool::Pencil, Tool::Eraser] {
        host.view.borrow_mut().tool = tool;
        let output = frame(&mut manager, vec![]);
        assert!(
            text_rect(&output, "Tail").is_some(),
            "{tool:?} must expose the Tail control: {:?}",
            rendered_texts(&output)
        );
    }

    host.view.borrow_mut().tool = Tool::Fill;
    let output = frame(&mut manager, vec![]);
    assert!(
        text_rect(&output, "Tail").is_none(),
        "non-draw tools must not show the Tail row"
    );

    host.view.borrow_mut().tool = Tool::Pencil;
    host.view.borrow_mut().draw.tail = 0;
    let output = frame(&mut manager, vec![]);
    let inactive_fill = ctx
        .style_of(egui::Theme::Dark)
        .visuals
        .widgets
        .inactive
        .bg_fill;
    let rail = row_rail(&output, inactive_fill, "Tail");
    // The panel's resize grip covers the rail's first point, so the "left end"
    // press starts just inside it; it still clamps to the extreme position.
    let left_end = egui::pos2(rail.left() + 4.0, rail.center().y);
    let near_centre = egui::pos2(rail.center().x + 2.0, rail.center().y);
    let press = |pos| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    };
    let release = |pos| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    };

    // The thumb drives the mapped tail value: the left end is the minimum
    // magnitude (-1) and approaching the centre it grows toward the 100 cap.
    frame(&mut manager, vec![egui::Event::PointerMoved(left_end)]);
    frame(&mut manager, vec![press(left_end)]);
    let at_left_end: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
    assert!(
        at_left_end.contains(&ToolbarEvent::BrushTailChanged(-1)),
        "pressing the Tail slider's left end must emit BrushTailChanged(-1), got {at_left_end:?}"
    );
    frame(&mut manager, vec![egui::Event::PointerMoved(near_centre)]);
    let near_centre_events: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
    assert!(
        near_centre_events
            .iter()
            .any(|e| matches!(e, ToolbarEvent::BrushTailChanged(v) if v.abs() >= 40)),
        "approaching the centre must emit a large tail magnitude, got {near_centre_events:?}"
    );
    frame(&mut manager, vec![release(near_centre)]);
}

#[test]
fn panel_buttons_use_the_nine_slice_skin() {
    let ctx = egui::Context::default();
    let dir = temp_dir("panel-buttons-skin");
    let atlas_path = dir.join("atlas.png");
    write_rgba_png(&atlas_path, 128, 192);
    let (bytes, size) = pyxross::ui::theme::load_atlas_with_size(&atlas_path).unwrap();
    let image =
        egui::ColorImage::from_rgba_unmultiplied([size.0 as usize, size.1 as usize], &bytes);
    let texture = ctx.load_texture("panel-buttons-atlas", image, egui::TextureOptions::NEAREST);
    let mut cache = AtlasCache::new();
    cache.insert(
        std::path::PathBuf::from("atlas.png"),
        pyxross::ui::panel_dock::skin::SkinAtlas::new(texture.clone(), size),
    );
    // Only the button skin is declared, so every atlas-textured mesh in these
    // panels is a button nine-slice.
    let mut theme = Theme::default_dark();
    theme
        .skins
        .insert("button".to_string(), skin_with_atlas("atlas.png"));
    let chrome = ChromeHarness::new();
    let atlas_id = texture.id();
    let screen = vec2(800.0, 600.0);

    let toolbox_host = ToolboxHost::default();
    let mut toolbox = toolbox_manager(&toolbox_host);
    let output = run_toolbox_frame(
        &ctx,
        &mut toolbox,
        &chrome.skinned(&theme, &cache),
        screen,
        vec![],
    );
    assert!(
        atlas_mesh_bounds(&output, atlas_id).len() >= 5,
        "the five tool buttons must be nine-slice meshes"
    );

    let layers_host = LayerPanelHost::default();
    set_view(
        &layers_host,
        vec![
            layer_row(1, "Group", true, 0, true, true),
            layer_row(2, "Leaf", false, 1, false, true),
        ],
        Some(LayerId::new(2)),
        true,
        true,
    );
    let mut layers = layers_manager(&layers_host);
    let output = run_layers_frame(
        &ctx,
        &mut layers,
        &chrome.skinned(&theme, &cache),
        screen,
        vec![],
    );
    assert!(
        atlas_mesh_bounds(&output, atlas_id).len() >= 7,
        "the layers header and row buttons must be nine-slice meshes"
    );

    let property_host = ToolboxHost::default();
    property_host.view.borrow_mut().tool = Tool::Pencil;
    let mut property = tool_property_manager(&property_host);
    let output = run_tool_property_frame(
        &ctx,
        &mut property,
        &chrome.skinned(&theme, &cache),
        screen,
        vec![],
    );
    assert!(
        atlas_mesh_bounds(&output, atlas_id).len() >= 5,
        "the shape and spread selectors must be nine-slice meshes"
    );

    // Flat chrome (no atlases): the panels fall back to plain widgets.
    let flat_theme = Theme::default_dark();
    let flat = chrome.view(&flat_theme);
    let mut toolbox = toolbox_manager(&toolbox_host);
    let output = run_toolbox_frame(&ctx, &mut toolbox, &flat, screen, vec![]);
    assert!(
        atlas_mesh_bounds(&output, atlas_id).is_empty(),
        "flat chrome must not paint button meshes"
    );
    let mut layers = layers_manager(&layers_host);
    let output = run_layers_frame(&ctx, &mut layers, &flat, screen, vec![]);
    assert!(
        atlas_mesh_bounds(&output, atlas_id).is_empty(),
        "flat chrome must not paint button meshes"
    );
    let mut property = tool_property_manager(&property_host);
    let output = run_tool_property_frame(&ctx, &mut property, &flat, screen, vec![]);
    assert!(
        atlas_mesh_bounds(&output, atlas_id).is_empty(),
        "flat chrome must not paint button meshes"
    );
}

/// Runs one frame of a panel content (no dock window) in a full-screen central
/// panel, mirroring the App's `PanelContent::ui` call path.
fn run_content_frame(
    ctx: &egui::Context,
    content: &mut dyn PanelContent,
    chrome: &PanelChrome,
    screen: Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), screen)),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| content.ui(ui, chrome));
        },
    );
    output.textures_delta.clear();
    output
}

/// Number of gradient meshes (textured images) painted anywhere in the output.
fn content_mesh_count(output: &egui::FullOutput) -> usize {
    fn visit(shape: &egui::Shape, count: &mut usize) {
        match shape {
            egui::Shape::Mesh(_) => *count += 1,
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| visit(shape, count)),
            _ => {}
        }
    }
    let mut count = 0;
    output
        .shapes
        .iter()
        .for_each(|clipped| visit(&clipped.shape, &mut count));
    count
}

#[test]
fn color_picker_is_floating_only_and_hidden_when_collapsed() {
    let host = ColorPickerHost::default();
    *host.view.borrow_mut() = ColorPickerView {
        color: Color::rgb(0, 0, 255),
        old_color: Color::BLACK,
        new_color: Color::rgb(0, 0, 255),
        secondary_color: Color::WHITE,
        open: true,
    };
    let spec = color_picker_panel_spec(
        &host,
        Rect::from_min_size(pos2(40.0, 40.0), vec2(470.0, 300.0)),
    );

    // The spec is floating-only: no dock/float action, just Close.
    assert_eq!(spec.placement, PanelPlacement::Floating);
    assert!(!spec.metadata.can_pop_out);
    assert_eq!(spec.metadata.title, "Color Picker");
    assert_eq!(
        spec.metadata.header_action(PanelPlacement::Floating),
        Some(PanelHeaderAction::Close)
    );

    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut content = spec.content;

    // A collapsed nest hides every component: no gradients, no chips.
    let collapsed = run_content_frame(
        &ctx,
        &mut *content,
        &chrome.view(&theme),
        vec2(8.0, 8.0),
        vec![],
    );
    assert_eq!(
        content_mesh_count(&collapsed),
        0,
        "a collapsed picker must paint no gradients"
    );
    for index in 0..32 {
        assert!(
            ctx.read_response(egui::Id::new(("color-picker-chip", index)))
                .is_none(),
            "a collapsed picker must not register chip {index}"
        );
    }

    // A usable viewport paints the square and hue bar gradients and all chips.
    let usable = run_content_frame(
        &ctx,
        &mut *content,
        &chrome.view(&theme),
        vec2(470.0, 300.0),
        vec![],
    );
    assert_eq!(
        content_mesh_count(&usable),
        2,
        "the square and hue bar paint one gradient mesh each"
    );
    for index in 0..32 {
        assert!(
            ctx.read_response(egui::Id::new(("color-picker-chip", index)))
                .is_some(),
            "chip {index} must be interactive"
        );
    }
}

#[test]
fn clicking_a_chip_sets_the_color() {
    let host = ColorPickerHost::default();
    let base = Color::rgb(0, 0, 255);
    *host.view.borrow_mut() = ColorPickerView {
        color: base,
        old_color: Color::BLACK,
        new_color: base,
        secondary_color: Color::WHITE,
        open: true,
    };
    let spec = color_picker_panel_spec(
        &host,
        Rect::from_min_size(pos2(40.0, 40.0), vec2(470.0, 300.0)),
    );
    let mut content = spec.content;
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let screen = vec2(470.0, 300.0);
    let mut frame = |events: Vec<egui::Event>| {
        run_content_frame(&ctx, &mut *content, &chrome.view(&theme), screen, events)
    };

    frame(vec![]);
    let chips = shading_chips(base);
    let darkest = chips.len() - 1;
    let chip = ctx
        .read_response(egui::Id::new(("color-picker-chip", darkest)))
        .expect("the darkest shading chip renders")
        .rect;
    let expected = chips[darkest];
    assert_ne!(
        expected, base,
        "the darkest shading chip must not be the base color"
    );

    frame(vec![egui::Event::PointerMoved(chip.center())]);
    frame(vec![egui::Event::PointerButton {
        pos: chip.center(),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    }]);
    frame(vec![egui::Event::PointerButton {
        pos: chip.center(),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }]);

    let events: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
    assert_eq!(
        events,
        vec![ToolbarEvent::ColorChanged(expected)],
        "clicking a chip must emit its color"
    );
}

#[test]
fn palette_panel_paints_nothing_when_collapsed() {
    use pyxross::ui::dock_color_palette_panel::palette_panel_spec;
    use pyxross::ui::dock_hosts::{PalettePanelHost, PaletteView};

    let host = PalettePanelHost::default();
    *host.view.borrow_mut() = PaletteView {
        entries: vec![Color::rgb(10, 20, 30), Color::rgb(40, 50, 60)],
        primary: Color::rgb(40, 50, 60),
        secondary: Color::WHITE,
    };
    let spec = palette_panel_spec(&host, PanelPlacement::Floating);
    assert_eq!(spec.id, PanelId::new(105));
    assert_eq!(spec.metadata.title, "Palette");
    assert!(
        spec.metadata.can_pop_out,
        "the palette panel must be dockable and floatable"
    );
    let mut content = spec.content;
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();

    // A collapsed nest hides every component: no buttons, no swatches.
    let collapsed = run_content_frame(
        &ctx,
        &mut *content,
        &chrome.view(&theme),
        vec2(8.0, 8.0),
        vec![],
    );
    assert!(
        rendered_texts(&collapsed).is_empty(),
        "a collapsed palette must paint no buttons: {:?}",
        rendered_texts(&collapsed)
    );
    for index in 0..2 {
        assert!(
            ctx.read_response(egui::Id::new(("palette-swatch", index)))
                .is_none(),
            "a collapsed palette must not register swatch {index}"
        );
    }

    // A usable viewport paints the pinned header and every swatch.
    let usable = run_content_frame(
        &ctx,
        &mut *content,
        &chrome.view(&theme),
        vec2(240.0, 300.0),
        vec![],
    );
    let texts = rendered_texts(&usable);
    assert!(texts.iter().any(|text| text == "Add color"), "{texts:?}");
    assert!(texts.iter().any(|text| text == "Remove color"), "{texts:?}");
    for index in 0..2 {
        assert!(
            ctx.read_response(egui::Id::new(("palette-swatch", index)))
                .is_some(),
            "swatch {index} must be interactive"
        );
    }
}

// ---------------------------------------------------------------------------
// Color Picker gesture tests (New/Old previews, FG/BG swatches, swap)
// ---------------------------------------------------------------------------

use pyxross::input::{Action, Keymap, LogicalKey, Modifiers};

fn pointer_move(pos: egui::Pos2) -> egui::Event {
    egui::Event::PointerMoved(pos)
}

fn pointer_press(pos: egui::Pos2) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    }
}

fn pointer_release(pos: egui::Pos2) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn color32(color: Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

/// A color-picker host snapshot with the given preview colors.
fn picker_host(view: ColorPickerView) -> ColorPickerHost {
    let host = ColorPickerHost::default();
    *host.view.borrow_mut() = view;
    host
}

/// Applies the color-picker `ColorChanged` events the way the App shell does.
fn applied_primary(host: &ColorPickerHost, mut primary: Color) -> Color {
    for event in host.events.borrow_mut().drain(..) {
        if let ToolbarEvent::ColorChanged(color) = event {
            primary = color;
        }
    }
    primary
}

#[test]
fn clicking_old_preview_reverts_the_primary_color() {
    let opened_with = Color::rgb(255, 0, 0);
    let working = Color::rgb(0, 0, 255);
    let host = picker_host(ColorPickerView {
        color: working,
        old_color: opened_with,
        new_color: working,
        secondary_color: Color::WHITE,
        open: true,
    });
    let spec = color_picker_panel_spec(
        &host,
        Rect::from_min_size(pos2(40.0, 40.0), vec2(470.0, 300.0)),
    );
    let mut content = spec.content;
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut frame = |events: Vec<egui::Event>| {
        run_content_frame(
            &ctx,
            &mut *content,
            &chrome.view(&theme),
            vec2(470.0, 300.0),
            events,
        )
    };

    frame(vec![]);
    let old = ctx
        .read_response(egui::Id::new(("color-picker-preview", "Old")))
        .expect("the Old preview renders")
        .rect;

    // Given: the panel opened with `opened_with` and now working on `working`.
    // When: the Old preview is clicked and the emitted event is applied.
    frame(vec![pointer_move(old.center())]);
    frame(vec![pointer_press(old.center())]);
    frame(vec![pointer_release(old.center())]);
    let primary = applied_primary(&host, working);

    // Then: the primary reverts to the color the panel was opened with.
    assert_eq!(
        primary, opened_with,
        "clicking Old must revert the primary color"
    );
}

#[test]
fn clicking_new_preview_restores_the_working_color() {
    let opened_with = Color::rgb(255, 0, 0);
    let working = Color::rgb(0, 0, 255);
    let host = picker_host(ColorPickerView {
        color: working,
        old_color: opened_with,
        new_color: working,
        secondary_color: Color::WHITE,
        open: true,
    });
    let spec = color_picker_panel_spec(
        &host,
        Rect::from_min_size(pos2(40.0, 40.0), vec2(470.0, 300.0)),
    );
    let mut content = spec.content;
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut frame = |events: Vec<egui::Event>| {
        run_content_frame(
            &ctx,
            &mut *content,
            &chrome.view(&theme),
            vec2(470.0, 300.0),
            events,
        )
    };

    frame(vec![]);
    let old = ctx
        .read_response(egui::Id::new(("color-picker-preview", "Old")))
        .expect("the Old preview renders")
        .rect;
    let new = ctx
        .read_response(egui::Id::new(("color-picker-preview", "New")))
        .expect("the New preview renders")
        .rect;

    // Given: the panel working on `working`. When: Old is clicked (revert).
    frame(vec![pointer_move(old.center())]);
    frame(vec![pointer_press(old.center())]);
    frame(vec![pointer_release(old.center())]);
    let reverted = applied_primary(&host, working);
    assert_eq!(reverted, opened_with);

    // The App publishes the reverted primary into the next snapshot.
    host.view.borrow_mut().color = reverted;

    // Then: the New preview still shows the working color (the "back" target).
    let output = frame(vec![]);
    assert!(
        rect_fills_recursive(&output, color32(working)).contains(&new),
        "the New preview must keep showing the working color after a revert"
    );

    // When: New is clicked. Then: the working color is restored.
    frame(vec![pointer_move(new.center())]);
    frame(vec![pointer_press(new.center())]);
    frame(vec![pointer_release(new.center())]);
    let restored = applied_primary(&host, reverted);
    assert_eq!(
        restored, working,
        "clicking New must restore the last working color"
    );
}

#[test]
fn picker_shows_primary_and_secondary_swatches_at_twice_the_chip_size() {
    let primary = Color::rgb(10, 20, 30);
    let secondary = Color::rgb(200, 210, 220);
    let host = picker_host(ColorPickerView {
        color: primary,
        old_color: Color::BLACK,
        new_color: primary,
        secondary_color: secondary,
        open: true,
    });
    let spec = color_picker_panel_spec(
        &host,
        Rect::from_min_size(pos2(40.0, 40.0), vec2(470.0, 300.0)),
    );
    let mut content = spec.content;
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let output = run_content_frame(
        &ctx,
        &mut *content,
        &chrome.view(&theme),
        vec2(470.0, 300.0),
        vec![],
    );

    // Given: a rendered picker. Then: the FG/BG squares are twice a chip and
    // the BACKGROUND square sits down-right of the FOREGROUND square.
    let chip = ctx
        .read_response(egui::Id::new(("color-picker-chip", 0usize)))
        .expect("a variation chip renders")
        .rect;
    let fg = ctx
        .read_response(egui::Id::new("color-picker-fg-swatch"))
        .expect("the FOREGROUND swatch renders")
        .rect;
    let bg = ctx
        .read_response(egui::Id::new("color-picker-bg-swatch"))
        .expect("the BACKGROUND swatch renders")
        .rect;

    assert_eq!(
        fg.size(),
        2.0 * chip.size(),
        "the FG square must be twice the chip size"
    );
    assert_eq!(
        bg.size(),
        fg.size(),
        "the BG square must match the FG square"
    );
    assert!(
        bg.min.x > fg.min.x && bg.min.y > fg.min.y,
        "BACKGROUND must sit down-right of FOREGROUND: {fg:?} {bg:?}"
    );

    let texts = rendered_texts(&output);
    assert!(texts.iter().any(|text| text == "FOREGROUND"), "{texts:?}");
    assert!(texts.iter().any(|text| text == "BACKGROUND"), "{texts:?}");
    assert!(
        rect_fills_recursive(&output, color32(primary)).contains(&fg),
        "the FG square must show the primary color"
    );
    assert!(
        rect_fills_recursive(&output, color32(secondary)).contains(&bg),
        "the BG square must show the secondary color"
    );
}

#[test]
fn swap_button_and_x_hotkey_swap_primary_and_secondary() {
    let primary = Color::rgb(10, 20, 30);
    let secondary = Color::rgb(200, 210, 220);
    let host = picker_host(ColorPickerView {
        color: primary,
        old_color: Color::BLACK,
        new_color: primary,
        secondary_color: secondary,
        open: true,
    });
    let spec = color_picker_panel_spec(
        &host,
        Rect::from_min_size(pos2(40.0, 40.0), vec2(470.0, 300.0)),
    );
    let mut content = spec.content;
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut frame = |events: Vec<egui::Event>| {
        run_content_frame(
            &ctx,
            &mut *content,
            &chrome.view(&theme),
            vec2(470.0, 300.0),
            events,
        )
    };

    frame(vec![]);
    let swap = ctx
        .read_response(egui::Id::new("color-picker-swap-colors"))
        .expect("the swap button renders")
        .rect;

    // When: the swap button is clicked.
    frame(vec![pointer_move(swap.center())]);
    frame(vec![pointer_press(swap.center())]);
    frame(vec![pointer_release(swap.center())]);
    let events: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();

    // Then: the panel asks the App to swap, which exchanges the two colors.
    assert_eq!(events, vec![ToolbarEvent::SwapColors]);
    let (mut first, mut second) = (primary, secondary);
    std::mem::swap(&mut first, &mut second);
    assert_eq!((first, second), (secondary, primary));

    // And: plain X is bound to the same action, with no clash.
    let keymap = Keymap::defaults();
    assert!(keymap.validate().is_ok());
    let binding = keymap
        .binding(Action::SwapColors)
        .expect("X must be bound to SwapColors");
    assert_eq!(binding.key, LogicalKey::X);
    assert_eq!(binding.modifiers, Modifiers::default());
    let plain_x: Vec<Action> = keymap
        .bindings
        .iter()
        .filter(|(_, binding)| {
            binding.key == LogicalKey::X && binding.modifiers == Modifiers::default()
        })
        .map(|(action, _)| *action)
        .collect();
    assert_eq!(
        plain_x,
        vec![Action::SwapColors],
        "plain X must not clash with another binding"
    );

    let mut fired = false;
    let mut output = egui::Context::default().run_ui(
        egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(egui::Modifiers::NONE),
                egui::Event::Key {
                    key: egui::Key::X,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        },
        |ui| {
            fired = ui
                .ctx()
                .input(|input| keymap.pressed(Action::SwapColors, input))
        },
    );
    output.textures_delta.clear();
    assert!(fired, "a plain X press must trigger the swap action");
}

/// Frames a dock holding only the Color Picker, returning the settled window
/// rect from the manager.
fn run_picker_dock_frame(
    ctx: &egui::Context,
    manager: &mut DockManager,
    chrome: &PanelChrome,
    events: Vec<egui::Event>,
) -> Option<Rect> {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
            events,
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| manager.show_inside(ui, chrome));
        },
    );
    output.textures_delta.clear();
    manager.floating_rect(PanelId::new(104))
}

#[test]
fn color_picker_is_not_resizable() {
    let host = ColorPickerHost::default();
    let spec = color_picker_panel_spec(
        &host,
        Rect::from_min_size(pos2(40.0, 40.0), vec2(470.0, 300.0)),
    );
    assert!(
        !spec.metadata.resizable,
        "the Color Picker metadata must be fixed-size"
    );

    let id = PanelId::new(104);
    let mut manager = DockManager::try_new([spec]).unwrap();
    assert_eq!(
        manager.metadata(id).map(|metadata| metadata.resizable),
        Some(false)
    );

    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let frame = |manager: &mut DockManager, events: Vec<egui::Event>| {
        run_picker_dock_frame(&ctx, manager, &chrome.view(&theme), events)
    };

    for _ in 0..8 {
        frame(&mut manager, vec![]);
    }
    let settled = manager.floating_rect(id).expect("the picker floats");

    // When: the pointer drags the window's right edge outwards.
    let edge = pos2(settled.right() - 2.0, settled.center().y);
    let dragged = edge + vec2(90.0, 60.0);
    frame(&mut manager, vec![pointer_move(edge)]);
    frame(&mut manager, vec![pointer_press(edge)]);
    frame(&mut manager, vec![pointer_move(dragged)]);
    frame(&mut manager, vec![pointer_release(dragged)]);
    for _ in 0..4 {
        frame(&mut manager, vec![]);
    }

    // Then: the fixed-size picker keeps its rect.
    assert_eq!(
        manager.floating_rect(id),
        Some(settled),
        "an edge resize drag must not change the non-resizable picker rect"
    );
}

#[test]
fn color_picker_reopens_where_it_was_closed() {
    let host = ColorPickerHost::default();
    let id = PanelId::new(104);
    let mut manager = DockManager::try_new([color_picker_panel_spec(
        &host,
        Rect::from_min_size(pos2(40.0, 40.0), vec2(470.0, 300.0)),
    )])
    .unwrap();
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let frame = |manager: &mut DockManager, events: Vec<egui::Event>| {
        run_picker_dock_frame(&ctx, manager, &chrome.view(&theme), events)
    };

    for _ in 0..8 {
        frame(&mut manager, vec![]);
    }
    let opened = manager.floating_rect(id).expect("the picker floats");

    // When: its header is dragged to a new spot.
    let grab = pos2(opened.left() + 40.0, opened.top() + 14.0);
    let drop = grab + vec2(90.0, 70.0);
    frame(&mut manager, vec![pointer_move(grab)]);
    frame(&mut manager, vec![pointer_press(grab)]);
    frame(&mut manager, vec![pointer_move(drop)]);
    frame(&mut manager, vec![pointer_release(drop)]);
    for _ in 0..4 {
        frame(&mut manager, vec![]);
    }
    let moved = manager.floating_rect(id).expect("the picker still floats");
    assert_ne!(
        moved.min, opened.min,
        "the header drag must move the picker"
    );

    // When: the panel is closed and reopened from the remembered rect.
    assert!(manager.remove_spec(id));
    assert_eq!(
        manager.last_floating_rect(id),
        Some(moved),
        "closing must remember the rect"
    );
    let remembered = manager.last_floating_rect(id).expect("a remembered rect");
    manager
        .add_spec(color_picker_panel_spec(&host, remembered))
        .unwrap();
    assert_eq!(
        manager.floating_rect(id),
        Some(moved),
        "the reopened spec must reuse the remembered rect"
    );

    // Then: the reopened window settles at the same spot.
    for _ in 0..4 {
        frame(&mut manager, vec![]);
    }
    let reopened = manager
        .floating_rect(id)
        .expect("the reopened picker floats");
    assert!(
        (reopened.min - moved.min).length() < 1.0,
        "reopened at {reopened:?}, expected {moved:?}"
    );
}

// ---------------------------------------------------------------------------
// Canvas brush footprint: painting continues while the footprint overlaps
// ---------------------------------------------------------------------------

#[test]
fn canvas_stroke_keeps_reporting_while_the_footprint_overlaps_outside_the_rect() {
    use pyxross::core::brush::{BrushShape, BrushSpec, DrawMode};
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let capture = InputCapture::new();
    capture.set_target(Some(SurfaceId::Canvas));
    let mut widget = CanvasWidget::new();
    let mut camera = Camera::new();
    let spec = BrushSpec::new(16, BrushShape::Square);
    let mut frame = |events: Vec<egui::Event>| -> pyxross::ui::canvas::CanvasInteractions {
        let mut result = pyxross::ui::canvas::CanvasInteractions::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        result = widget.ui(
                            ui,
                            &theme.colors,
                            &capture,
                            egui::TextureId::default(),
                            pyxross::ui::canvas::CanvasView {
                                canvas_size: (32, 32),
                                grid_visible: false,
                                brush: Some(spec),
                                draw_mode: DrawMode::Pen,
                                ..Default::default()
                            },
                            camera,
                        );
                    });
            },
        );
        output.textures_delta.clear();
        camera = result.updated_camera;
        result
    };
    let primary = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };

    // Given: a primary drag that started on the canvas.
    frame(vec![egui::Event::PointerMoved(pos2(10.0, 10.0))]);
    frame(vec![primary(pos2(10.0, 10.0), true)]);
    let started = frame(vec![egui::Event::PointerMoved(pos2(20.0, 10.0))]);
    assert!(started.stroke_started, "the drag must start on the canvas");
    assert_eq!(started.stroke_point, Some((20, 10)));

    // When: the cursor leaves the draw rect while the 16 px footprint
    // (screen 28..=43 at 100%) still overlaps the 32 px canvas.
    let outside = frame(vec![egui::Event::PointerMoved(pos2(36.0, 10.0))]);

    // Then: the stroke keeps reporting its outside anchor so the in-bounds
    // part of the footprint is painted.
    assert_eq!(
        outside.stroke_point,
        Some((36, 10)),
        "an outside cursor with an overlapping footprint must keep the stroke alive"
    );

    // When: the cursor is far enough out that the footprint misses the canvas
    // (48..=63 clears the 32 px edge).
    let clear = frame(vec![egui::Event::PointerMoved(pos2(56.0, 10.0))]);

    // Then: reporting pauses (the App re-enters as a new segment later).
    assert_eq!(
        clear.stroke_point, None,
        "a footprint clear of the canvas pauses the stroke"
    );

    let released = frame(vec![primary(pos2(56.0, 10.0), false)]);
    assert!(
        released.stroke_ended,
        "the release must still end the gesture"
    );
}

/// Hovers a numeric field and turns the wheel `notches` notches, returning
/// the color the panel emitted (published back into the host view).
fn wheel_field(
    ctx: &egui::Context,
    content: &mut dyn PanelContent,
    chrome: &PanelChrome,
    host: &ColorPickerHost,
    field: &str,
    notches: f32,
    current: Color,
) -> Color {
    let rect = ctx
        .read_response(egui::Id::new(("color-picker-field", field)))
        .unwrap_or_else(|| panic!("the {field} field must render"))
        .rect;
    run_content_frame(
        ctx,
        content,
        chrome,
        vec2(540.0, 300.0),
        vec![pointer_move(rect.center())],
    );
    run_content_frame(
        ctx,
        content,
        chrome,
        vec2(540.0, 300.0),
        vec![scroll(vec2(0.0, notches))],
    );
    let next = applied_primary(host, current);
    host.view.borrow_mut().color = next;
    next
}

/// Hovers the hue bar and turns the wheel `notches` notches, returning the
/// color the panel emitted (published back into the host view).
fn wheel_hue_bar(
    ctx: &egui::Context,
    content: &mut dyn PanelContent,
    chrome: &PanelChrome,
    host: &ColorPickerHost,
    notches: f32,
    current: Color,
) -> Color {
    let rect = ctx
        .read_response(egui::Id::new("color-picker-hue"))
        .expect("the hue bar must render")
        .rect;
    run_content_frame(
        ctx,
        content,
        chrome,
        vec2(540.0, 300.0),
        vec![pointer_move(rect.center())],
    );
    run_content_frame(
        ctx,
        content,
        chrome,
        vec2(540.0, 300.0),
        vec![scroll(vec2(0.0, notches))],
    );
    let next = applied_primary(host, current);
    host.view.borrow_mut().color = next;
    next
}

#[test]
fn hue_slider_wheel_changes_the_hue() {
    let base = Color::rgb(255, 0, 0);
    let host = picker_host(ColorPickerView {
        color: base,
        old_color: base,
        new_color: base,
        secondary_color: Color::WHITE,
        open: true,
    });
    let spec = color_picker_panel_spec(
        &host,
        Rect::from_min_size(pos2(40.0, 40.0), vec2(540.0, 300.0)),
    );
    let mut content = spec.content;
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    run_content_frame(
        &ctx,
        &mut *content,
        &chrome.view(&theme),
        vec2(540.0, 300.0),
        vec![],
    );

    // When: one wheel notch up while hovering the hue bar.
    let (_, saturation, value) = color_to_hsv(base);
    let up = wheel_hue_bar(&ctx, &mut *content, &chrome.view(&theme), &host, 2.0, base);
    assert_eq!(
        up,
        hsv_to_color(1.0, saturation, value, base.a),
        "one wheel notch must move the hue by one step"
    );

    // And: one notch down returns to the base hue.
    let down = wheel_hue_bar(&ctx, &mut *content, &chrome.view(&theme), &host, -2.0, up);
    assert_eq!(down, base, "one notch down must return to the base hue");

    // And: the hue wraps below zero.
    let wrapped = wheel_hue_bar(&ctx, &mut *content, &chrome.view(&theme), &host, -2.0, down);
    assert_eq!(
        wrapped,
        hsv_to_color(359.0, saturation, value, base.a),
        "the hue must wrap from 0 to 359"
    );
}

#[test]
fn numeric_fields_wheel_change_their_value() {
    let base = Color::rgba(10, 20, 30, 200);
    let host = picker_host(ColorPickerView {
        color: base,
        old_color: base,
        new_color: base,
        secondary_color: Color::WHITE,
        open: true,
    });
    let spec = color_picker_panel_spec(
        &host,
        Rect::from_min_size(pos2(40.0, 40.0), vec2(540.0, 300.0)),
    );
    let mut content = spec.content;
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let view = chrome.view(&theme);
    run_content_frame(&ctx, &mut *content, &view, vec2(540.0, 300.0), vec![]);

    // When: the wheel turns over the RGB/A fields. Then: each steps one unit.
    let red = wheel_field(&ctx, &mut *content, &view, &host, "R:", 2.0, base);
    assert_eq!(red, Color::rgba(11, 20, 30, 200), "R must step up by one");
    let green = wheel_field(&ctx, &mut *content, &view, &host, "G:", -2.0, red);
    assert_eq!(
        green,
        Color::rgba(11, 19, 30, 200),
        "G must step down by one"
    );
    let blue = wheel_field(&ctx, &mut *content, &view, &host, "B:", 2.0, green);
    assert_eq!(blue, Color::rgba(11, 19, 31, 200), "B must step up by one");
    let alpha = wheel_field(&ctx, &mut *content, &view, &host, "A:", -2.0, blue);
    assert_eq!(
        alpha,
        Color::rgba(11, 19, 31, 199),
        "A must step down by one"
    );

    // And: the H/S/L fields step one display unit (degree / percent).
    let (hue, saturation, lightness) = color_to_hsl(alpha);
    let hue_up = wheel_field(&ctx, &mut *content, &view, &host, "H:", 2.0, alpha);
    assert_eq!(
        hue_up,
        hsl_to_color(hue + 1.0, saturation, lightness, alpha.a),
        "H must step up by one degree"
    );
    let (hue, saturation, lightness) = color_to_hsl(hue_up);
    let saturation_down = wheel_field(&ctx, &mut *content, &view, &host, "S:", -2.0, hue_up);
    assert_eq!(
        saturation_down,
        hsl_to_color(hue, saturation - 0.01, lightness, hue_up.a),
        "S must step down by one percent"
    );
    let (hue, saturation, lightness) = color_to_hsl(saturation_down);
    let lightness_up = wheel_field(
        &ctx,
        &mut *content,
        &view,
        &host,
        "L:",
        2.0,
        saturation_down,
    );
    assert_eq!(
        lightness_up,
        hsl_to_color(hue, saturation, lightness + 0.01, saturation_down.a),
        "L must step up by one percent"
    );

    // And: the steps respect each field's range.
    let opaque = Color::rgba(lightness_up.r, lightness_up.g, lightness_up.b, 255);
    host.view.borrow_mut().color = opaque;
    let clamped = wheel_field(&ctx, &mut *content, &view, &host, "A:", 2.0, opaque);
    assert_eq!(clamped, opaque, "alpha must clamp at 255");
    let black = Color::rgba(0, lightness_up.g, lightness_up.b, 255);
    host.view.borrow_mut().color = black;
    let clamped = wheel_field(&ctx, &mut *content, &view, &host, "R:", -2.0, black);
    assert_eq!(clamped, black, "R must clamp at 0");
}

#[test]
fn clicking_the_secondary_swatch_swaps_the_colors() {
    let primary = Color::rgb(10, 20, 30);
    let secondary = Color::rgb(200, 210, 220);
    let host = picker_host(ColorPickerView {
        color: primary,
        old_color: Color::BLACK,
        new_color: primary,
        secondary_color: secondary,
        open: true,
    });
    let spec = color_picker_panel_spec(
        &host,
        Rect::from_min_size(pos2(40.0, 40.0), vec2(540.0, 300.0)),
    );
    let mut content = spec.content;
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut frame = |events: Vec<egui::Event>| {
        run_content_frame(
            &ctx,
            &mut *content,
            &chrome.view(&theme),
            vec2(540.0, 300.0),
            events,
        )
    };

    frame(vec![]);
    let fg = ctx
        .read_response(egui::Id::new("color-picker-fg-swatch"))
        .expect("the FOREGROUND swatch renders")
        .rect;
    let bg = ctx
        .read_response(egui::Id::new("color-picker-bg-swatch"))
        .expect("the BACKGROUND swatch renders")
        .rect;

    // When: the BACKGROUND square's exposed corner is clicked.
    let corner = pos2(bg.right() - 3.0, bg.bottom() - 3.0);
    frame(vec![pointer_move(corner)]);
    frame(vec![pointer_press(corner)]);
    frame(vec![pointer_release(corner)]);

    // Then: the panel asks the App to swap, exactly like the Swap button.
    let events: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
    assert_eq!(events, vec![ToolbarEvent::SwapColors]);

    // And: clicking the FOREGROUND square keeps its current (inert) behaviour.
    frame(vec![pointer_move(fg.center())]);
    frame(vec![pointer_press(fg.center())]);
    frame(vec![pointer_release(fg.center())]);
    assert!(
        host.events.borrow().is_empty(),
        "clicking FOREGROUND must not swap: {:?}",
        host.events.borrow()
    );
}

#[test]
fn shading_row_spans_brighter_to_darker_with_hue_shifts() {
    let blue = Color::rgb(0, 0, 255);
    let chips = shading_chips(blue);

    // Then: four brighter chips, the base in the middle, four darker ones.
    assert_eq!(
        chips.len(),
        9,
        "the Shading row must span four steps each way"
    );
    assert_eq!(chips[4], blue, "the base color sits in the middle");

    // And: the bright end shifts toward yellow (the warm-highlight rule) while
    // the dark end walks the existing map (blue -> red).
    assert_eq!(shading_hue_shift(240.0), 120.0);
    assert_eq!(bright_shading_hue_shift(240.0), 120.0);
    assert_eq!(
        chips,
        [
            Color::rgb(255, 0, 0),
            Color::rgb(255, 0, 128),
            Color::rgb(255, 0, 255),
            Color::rgb(128, 0, 255),
            Color::rgb(0, 0, 255),
            Color::rgb(102, 0, 204),
            Color::rgb(153, 0, 153),
            Color::rgb(102, 0, 51),
            Color::rgb(51, 0, 0),
        ],
        "blue must brighten toward yellow and darken toward red"
    );

    let distance_to_yellow = |hue: f32| {
        let delta = (hue - 60.0).abs().rem_euclid(360.0);
        delta.min(360.0 - delta)
    };
    let (base_hue, _, _) = color_to_hsv(blue);
    let (bright_hue, _, _) = color_to_hsv(chips[0]);
    let (dark_hue, _, _) = color_to_hsv(chips[8]);
    assert!(
        distance_to_yellow(bright_hue) < distance_to_yellow(base_hue),
        "the bright end must move toward yellow: {bright_hue} vs base {base_hue}"
    );
    assert!(
        (dark_hue - 0.0).abs() < 1.0,
        "the dark end must keep the map's blue -> red shift: {dark_hue}"
    );

    // And: value never increases from bright to dark.
    let values: Vec<f32> = chips.iter().map(|chip| color_to_hsv(*chip).2).collect();
    assert!(
        values.windows(2).all(|pair| pair[0] >= pair[1]),
        "{values:?}"
    );
    assert!(
        values[0] > values[8],
        "the bright end must be lighter: {values:?}"
    );
}

#[test]
fn shading_and_lightness_rows_have_finer_steps() {
    let base = Color::rgb(51, 102, 204);

    // Then: both rows carry nine chips.
    assert_eq!(shading_chips(base).len(), 9);
    assert_eq!(lightness_chips(base).len(), 9);
    assert_eq!(
        shading_chips(base)[4],
        base,
        "the base chip stays in the middle"
    );

    // And: the Lightness row is nine even steps from 0.9 to 0.1.
    let levels: Vec<f32> = lightness_chips(base)
        .iter()
        .map(|chip| color_to_hsl(*chip).2)
        .collect();
    assert!((levels[0] - 0.9).abs() < 0.01, "{levels:?}");
    assert!((levels[8] - 0.1).abs() < 0.01, "{levels:?}");
    for pair in levels.windows(2) {
        assert!(
            (pair[0] - pair[1] - 0.1).abs() < 0.02,
            "the ladder must step evenly: {pair:?}"
        );
    }

    // And: the panel renders all 32 chips (9 + 9 + 7 + 7).
    let host = picker_host(ColorPickerView {
        color: base,
        old_color: Color::BLACK,
        new_color: base,
        secondary_color: Color::WHITE,
        open: true,
    });
    let spec = color_picker_panel_spec(
        &host,
        Rect::from_min_size(pos2(40.0, 40.0), vec2(540.0, 300.0)),
    );
    let mut content = spec.content;
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    run_content_frame(
        &ctx,
        &mut *content,
        &chrome.view(&theme),
        vec2(540.0, 300.0),
        vec![],
    );
    for index in 0..32 {
        assert!(
            ctx.read_response(egui::Id::new(("color-picker-chip", index)))
                .is_some(),
            "chip {index} must render"
        );
    }
    assert!(
        ctx.read_response(egui::Id::new(("color-picker-chip", 32usize)))
            .is_none(),
        "there must be no chip beyond the finer rows"
    );
}

#[test]
fn picker_bottom_has_no_excess_space() {
    let host = picker_host(ColorPickerView {
        color: Color::rgb(51, 102, 204),
        old_color: Color::BLACK,
        new_color: Color::rgb(51, 102, 204),
        secondary_color: Color::WHITE,
        open: true,
    });
    let spec = color_picker_panel_spec(
        &host,
        Rect::from_min_size(pos2(120.0, 80.0), vec2(540.0, 300.0)),
    );
    let id = PanelId::new(104);
    let mut manager = DockManager::try_new([spec]).unwrap();
    let ctx = egui::Context::default();
    let theme = Theme::default_dark();
    let chrome = ChromeHarness::new();
    let mut output = None;
    for _ in 0..12 {
        output = Some(run_layers_frame(
            &ctx,
            &mut manager,
            &chrome.view(&theme),
            vec2(800.0, 600.0),
            vec![],
        ));
    }

    // Given: the settled picker window. Then: its content (the lowest rect is
    // the swap button under the FG/BG pair) ends just above the window bottom.
    let panel = manager.floating_rect(id).expect("the picker floats");
    let mut content_bottom = f32::NEG_INFINITY;
    for name in [
        "color-picker-fg-swatch",
        "color-picker-bg-swatch",
        "color-picker-swap-colors",
        "color-picker-sv-square",
        "color-picker-hue",
    ] {
        content_bottom = content_bottom.max(
            ctx.read_response(egui::Id::new(name))
                .unwrap_or_else(|| panic!("{name} must render"))
                .rect
                .bottom(),
        );
    }
    for label in ["R:", "G:", "B:", "A:", "H:", "S:", "L:"] {
        content_bottom = content_bottom.max(
            ctx.read_response(egui::Id::new(("color-picker-field", label)))
                .unwrap_or_else(|| panic!("the {label} field must render"))
                .rect
                .bottom(),
        );
    }
    let gap = panel.bottom() - content_bottom;
    assert!(gap >= 8.0, "the content must stay inside the panel: {gap}");
    assert!(
        gap <= 16.0,
        "the panel must not reserve empty space below the content: {gap}"
    );
    assert!(
        panel.height() < 300.0,
        "the trimmed panel must be shorter than the old rect"
    );

    // And: the trimmed layout fits without scrollbars.
    let output = output.expect("at least one frame");
    assert!(
        rect_fills_recursive(&output, theme.colors.selection_bg_fill32()).is_empty(),
        "a snug picker must paint no scrollbar thumb"
    );
}
