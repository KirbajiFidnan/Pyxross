use std::fs;

use pyxross::ui::panel_registry::{PanelId, PanelPlacement};
use pyxross::ui::workspace_persistence::{PanelGeometry, WorkspaceLayoutError, WorkspaceLayoutV1};

#[test]
fn layout_roundtrip_excludes_runtime_identifiers() {
    let layout = WorkspaceLayoutV1::default();
    let json = layout.to_json().unwrap();
    let decoded = WorkspaceLayoutV1::from_json(&json).unwrap();
    assert_eq!(decoded.to_json().unwrap(), json);
    assert!(!json.contains("WindowId"));
    assert!(!json.contains("HostId"));
    assert!(!json.contains("ViewportId"));
}

#[test]
fn invalid_version_geometry_and_tool_popout_fall_back_or_reject() {
    let mut layout = WorkspaceLayoutV1::default();
    layout.version = 2;
    assert!(matches!(
        layout.validate(),
        Err(WorkspaceLayoutError::UnsupportedVersion(2))
    ));

    let mut layout = WorkspaceLayoutV1::default();
    layout.panels[0].geometry = Some(PanelGeometry::new(0.0, 0.0, f32::NAN, 100.0));
    assert!(matches!(
        layout.validate(),
        Err(WorkspaceLayoutError::Invalid(_))
    ));

    let mut layout = WorkspaceLayoutV1::default();
    let layers = layout
        .panels
        .iter_mut()
        .find(|record| record.panel == PanelId::Layers)
        .unwrap();
    layers.placement = PanelPlacement::PopOutWindow;
    assert!(matches!(
        layout.validate(),
        Err(WorkspaceLayoutError::Invalid(_))
    ));
}

#[test]
fn atomic_save_load_and_corrupt_fallback_are_deterministic() {
    let root = std::env::temp_dir().join(format!("pyxross-layout-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let path = root.join("workspace.json");
    let layout = WorkspaceLayoutV1::default();
    layout.save_atomic(&path).unwrap();
    assert_eq!(
        WorkspaceLayoutV1::load_or_default(&path)
            .unwrap()
            .to_json()
            .unwrap(),
        layout.to_json().unwrap()
    );
    fs::write(&path, "{not-json").unwrap();
    assert_eq!(
        WorkspaceLayoutV1::load_or_default(&path)
            .unwrap()
            .to_json()
            .unwrap(),
        WorkspaceLayoutV1::default().to_json().unwrap()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn startup_normalization_restores_legacy_popouts_to_main_placement_and_geometry() {
    let mut layout = WorkspaceLayoutV1::default();
    let preview = layout
        .panels
        .iter_mut()
        .find(|record| record.panel == PanelId::Preview)
        .unwrap();
    preview.placement = PanelPlacement::PopOutWindow;
    preview.last_main_placement = PanelPlacement::FloatingInMainWindow;
    preview.desired_popout = true;
    preview.geometry = Some(PanelGeometry::new(12.0, 24.0, 640.0, 480.0));

    let playback = layout
        .panels
        .iter_mut()
        .find(|record| record.panel == PanelId::Playback)
        .unwrap();
    playback.placement = PanelPlacement::FloatingInMainWindow;
    playback.last_main_placement = PanelPlacement::FloatingInMainWindow;

    let normalized = layout.normalized_for_startup();
    let preview = normalized
        .panels
        .iter()
        .find(|record| record.panel == PanelId::Preview)
        .unwrap();
    assert_eq!(preview.placement, PanelPlacement::FloatingInMainWindow);
    assert_eq!(
        preview.last_main_placement,
        PanelPlacement::FloatingInMainWindow
    );
    assert!(!preview.desired_popout);
    assert_eq!(
        preview.geometry,
        Some(PanelGeometry::new(12.0, 24.0, 640.0, 480.0))
    );

    let playback = normalized
        .panels
        .iter()
        .find(|record| record.panel == PanelId::Playback)
        .unwrap();
    assert_eq!(playback.placement, PanelPlacement::FloatingInMainWindow);
}
