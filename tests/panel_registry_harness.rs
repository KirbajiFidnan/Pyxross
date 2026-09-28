use pyxross::ui::panel_registry::{
    PanelCategory, PanelId, PanelPlacement, PanelRegistry, PanelRegistryError,
};

const WORKSPACE_PANELS: [PanelId; 4] = [
    PanelId::PanelA,
    PanelId::Preview,
    PanelId::Playback,
    PanelId::FrameEditor,
];
const TOOL_PANELS: [PanelId; 5] = [
    PanelId::Layers,
    PanelId::ColorPalette,
    PanelId::TilePalette,
    PanelId::Animations,
    PanelId::Toolbox,
];

#[test]
fn registry_has_exactly_one_instance_per_panel() {
    let registry = PanelRegistry::new();
    assert_eq!(registry.len(), PanelId::ALL.len());
    assert_eq!(registry.iter().count(), PanelId::ALL.len());
    for panel in PanelId::ALL {
        assert_eq!(registry.get(panel).expect("registered panel").id(), panel);
    }
}

#[test]
fn panel_a_is_the_canonical_workspace_identity() {
    let registry = PanelRegistry::new();

    assert_eq!(PanelId::PanelA.category(), PanelCategory::Workspace);
    assert_eq!(
        registry.get(PanelId::PanelA).expect("panel A exists").id(),
        PanelId::PanelA
    );
}

#[test]
fn workspace_panels_preserve_identity_through_repeated_transitions() {
    let mut registry = PanelRegistry::new();
    for _ in 0..100 {
        for panel in WORKSPACE_PANELS {
            registry
                .transition(panel, PanelPlacement::FloatingInMainWindow)
                .unwrap();
            registry
                .transition(panel, PanelPlacement::PopOutWindow)
                .unwrap();
            registry.dock_back(panel).unwrap();
            let instance = registry.get(panel).unwrap();
            assert_eq!(instance.id(), panel);
            assert_eq!(instance.category(), PanelCategory::Workspace);
            assert_eq!(instance.placement(), PanelPlacement::FloatingInMainWindow);
        }
    }
    assert_eq!(registry.len(), PanelId::ALL.len());
}

#[test]
fn dock_back_restores_docked_or_floating_main_placement() {
    let mut registry = PanelRegistry::new();

    for panel in WORKSPACE_PANELS {
        registry
            .transition(panel, PanelPlacement::PopOutWindow)
            .unwrap();
        registry.dock_back(panel).unwrap();
        assert_eq!(
            registry.get(panel).unwrap().placement(),
            PanelPlacement::Docked
        );

        registry
            .transition(panel, PanelPlacement::FloatingInMainWindow)
            .unwrap();
        registry
            .transition(panel, PanelPlacement::PopOutWindow)
            .unwrap();
        registry.dock_back(panel).unwrap();
        assert_eq!(
            registry.get(panel).unwrap().placement(),
            PanelPlacement::FloatingInMainWindow
        );
    }
}

#[test]
fn tool_panels_reject_popout_without_mutating_registry() {
    let mut registry = PanelRegistry::new();
    for panel in TOOL_PANELS {
        let before = registry.get(panel).unwrap();
        let error = registry
            .transition(panel, PanelPlacement::PopOutWindow)
            .unwrap_err();
        assert_eq!(
            error,
            PanelRegistryError::UnsupportedPlacement {
                panel,
                placement: PanelPlacement::PopOutWindow
            }
        );
        assert_eq!(registry.get(panel).unwrap(), before);
        assert_eq!(before.category(), PanelCategory::Tool);
    }
}

#[test]
fn duplicate_and_unknown_panels_are_typed_errors() {
    let mut registry = PanelRegistry::default();
    registry.register(PanelId::Preview).unwrap();
    assert_eq!(
        registry.register(PanelId::Preview),
        Err(PanelRegistryError::DuplicatePanel(PanelId::Preview))
    );
    assert_eq!(
        registry.get(PanelId::Layers),
        Err(PanelRegistryError::UnknownPanel(PanelId::Layers))
    );
    assert_eq!(
        registry.transition(PanelId::Layers, PanelPlacement::Docked),
        Err(PanelRegistryError::UnknownPanel(PanelId::Layers))
    );
}
