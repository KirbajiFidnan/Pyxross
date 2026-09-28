use egui::ViewportId;
use pyxross::ui::host_registry::{ContextGenerationService, ContextSnapshot, WindowHostRegistry};
use pyxross::ui::panel_registry::PanelId;
use pyxross::ui::project::ProjectStore;

#[test]
fn snapshots_accept_only_the_current_project_and_generation() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 2, 2).project();
    let beta = store.new_project("beta", 2, 2).project();
    let mut generation = ContextGenerationService::new();
    let first = ContextSnapshot::new(beta, generation.current());
    assert!(first.accepts(beta, generation.current()));
    store.activate(alpha).unwrap();
    let next_generation = generation.advance();
    assert!(!first.accepts(alpha, next_generation));
    assert!(!first.accepts(beta, next_generation));
}

#[test]
fn host_identity_survives_context_switch_contract() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 2, 2).project();
    let beta = store.new_project("beta", 2, 2).project();
    let mut hosts = WindowHostRegistry::new();
    let host = hosts
        .create(PanelId::Preview, ViewportId::from_hash_of("preview"))
        .unwrap();
    store.activate(alpha).unwrap();
    store.activate(beta).unwrap();
    assert_eq!(hosts.resolve_host(host).unwrap().host(), host);
    assert_eq!(
        hosts.resolve_panel(PanelId::Preview).unwrap().panel(),
        PanelId::Preview
    );
}
