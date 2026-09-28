use pyxross::ui::panel_layout::{PanelLayout, PanelLayoutError};
use pyxross::ui::panel_registry::{PanelId, PanelPlacement, PanelRegistryError};

const WORKSPACE_PANELS: [PanelId; 3] = [PanelId::Preview, PanelId::Playback, PanelId::FrameEditor];

#[test]
fn canonical_layout_contains_each_panel_once() {
    let layout = PanelLayout::new();
    layout.validate().unwrap();
    assert_eq!(
        layout.dock_state().iter_all_tabs().count(),
        PanelId::ALL.len()
    );
}

#[test]
fn close_reopen_reorder_and_same_window_float_preserve_identity() {
    let mut layout = PanelLayout::new();
    layout
        .transition(PanelId::Preview, PanelPlacement::FloatingInMainWindow)
        .unwrap();
    layout.close(PanelId::Playback).unwrap();
    assert!(!layout.open_panels().contains(&PanelId::Playback));
    layout.reopen(PanelId::Playback).unwrap();
    layout.reorder(PanelId::Playback, 0).unwrap();
    layout.validate().unwrap();
    assert_eq!(
        layout.registry().get(PanelId::Preview).unwrap().placement(),
        PanelPlacement::FloatingInMainWindow
    );
    assert_eq!(
        layout.registry().get(PanelId::Playback).unwrap().id(),
        PanelId::Playback
    );
}

#[test]
fn workspace_popout_updates_placement_without_breaking_main_window_topology() {
    let mut layout = PanelLayout::new();

    for panel in WORKSPACE_PANELS {
        layout
            .transition(panel, PanelPlacement::PopOutWindow)
            .unwrap();

        let instance = layout.registry().get(panel).unwrap();
        assert_eq!(instance.id(), panel);
        assert_eq!(instance.placement(), PanelPlacement::PopOutWindow);
        assert_eq!(instance.last_main_placement(), PanelPlacement::Docked);
        assert_eq!(
            layout.dock_state().iter_all_tabs().count(),
            PanelId::ALL.len()
        );
        layout.validate().unwrap();
    }
}

#[test]
fn tool_popout_and_closed_panel_operations_are_rejected() {
    let mut layout = PanelLayout::new();
    let before = layout.registry().get(PanelId::Layers).unwrap();
    assert_eq!(
        layout.transition(PanelId::Layers, PanelPlacement::PopOutWindow),
        Err(PanelLayoutError::Registry(
            PanelRegistryError::UnsupportedPlacement {
                panel: PanelId::Layers,
                placement: PanelPlacement::PopOutWindow,
            }
        ))
    );
    assert_eq!(layout.registry().get(PanelId::Layers).unwrap(), before);
    layout.validate().unwrap();
    layout.close(PanelId::Layers).unwrap();
    assert_eq!(
        layout.transition(PanelId::Layers, PanelPlacement::Docked),
        Err(PanelLayoutError::ClosedPanel(PanelId::Layers))
    );
    assert_eq!(layout.reopen(PanelId::Layers), Ok(()));
    assert_eq!(
        layout.reopen(PanelId::Layers),
        Err(PanelLayoutError::DuplicatePanel(PanelId::Layers))
    );
}
