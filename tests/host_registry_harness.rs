use egui::ViewportId;
use pyxross::ui::host_registry::{ContextGenerationService, HostRegistryError, WindowHostRegistry};
use pyxross::ui::panel_registry::PanelId;
use winit::window::WindowId;

const WORKSPACE_PANELS: [PanelId; 4] = [
    PanelId::PanelA,
    PanelId::Preview,
    PanelId::Playback,
    PanelId::FrameEditor,
];

#[test]
fn workspace_host_identity_and_viewport_mapping_are_unique() {
    let mut registry = WindowHostRegistry::new();
    let viewport = ViewportId::from_hash_of("preview");
    let host = registry.create(PanelId::Preview, viewport).unwrap();
    let binding = registry.resolve_panel(PanelId::Preview).unwrap();
    assert_eq!(binding.host(), host);
    assert_eq!(registry.resolve_viewport(viewport).unwrap().host(), host);
    assert_eq!(
        registry.create(PanelId::Preview, ViewportId::from_hash_of("preview-2")),
        Err(HostRegistryError::DuplicatePanel(PanelId::Preview))
    );
    assert_eq!(
        registry.create(PanelId::Playback, viewport),
        Err(HostRegistryError::DuplicateViewport(viewport))
    );
}

#[test]
fn each_workspace_panel_has_one_live_host() {
    let mut registry = WindowHostRegistry::new();

    for panel in WORKSPACE_PANELS {
        let viewport = ViewportId::from_hash_of(panel);
        let host = registry.create(panel, viewport).unwrap();
        assert_eq!(registry.resolve_panel(panel).unwrap().host(), host);
    }

    assert_eq!(registry.len(), WORKSPACE_PANELS.len());
    assert!(registry.is_consistent());
}

#[test]
fn tool_hosts_are_rejected_and_retirement_removes_all_routes() {
    let mut registry = WindowHostRegistry::new();
    assert_eq!(
        registry.create(PanelId::Layers, ViewportId::from_hash_of("layers")),
        Err(HostRegistryError::UnsupportedPanel(PanelId::Layers))
    );
    let viewport = ViewportId::from_hash_of("playback");
    let window = WindowId::from(42);
    let host = registry.create(PanelId::Playback, viewport).unwrap();
    registry.bind_window(host, window).unwrap();
    assert_eq!(registry.resolve_window(window).unwrap().host(), host);
    let binding = registry.retire(host).unwrap();
    assert_eq!(binding.host(), host);
    assert_eq!(registry.len(), 0);
    assert!(registry.resolve_host(host).is_none());
    assert!(registry.resolve_panel(PanelId::Playback).is_none());
    assert!(registry.resolve_viewport(viewport).is_none());
    assert!(registry.resolve_window(window).is_none());
    assert!(registry.is_retired(host));
    assert!(registry.is_consistent());
    assert_eq!(
        registry.retire(host),
        Err(HostRegistryError::UnknownHost(host))
    );
}

#[test]
fn context_generation_rejects_old_host_events() {
    let mut generations = ContextGenerationService::new();
    let first = generations.current();
    let second = generations.advance();
    assert!(second > first);
    assert!(!generations.accepts(first));
    assert!(generations.accepts(second));
}

#[test]
fn a_host_cannot_be_rebound_without_retiring_its_old_window_route() {
    let mut registry = WindowHostRegistry::new();
    let host = registry
        .create(PanelId::Preview, ViewportId::from_hash_of("preview"))
        .unwrap();
    let first = WindowId::from(42);
    let second = WindowId::from(43);
    registry.bind_window(host, first).unwrap();

    assert_eq!(
        registry.bind_window(host, second),
        Err(HostRegistryError::WindowAlreadyBound(host))
    );
    assert_eq!(registry.resolve_window(first).unwrap().host(), host);
    assert!(registry.resolve_window(second).is_none());
}
