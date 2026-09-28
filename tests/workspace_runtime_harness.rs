use egui::ViewportId;
use winit::window::WindowId;

use pyxross::ui::host_registry::{ContextGenerationService, WindowHostRegistry};
use pyxross::ui::panel_layout::PanelLayout;
use pyxross::ui::panel_registry::{PanelId, PanelPlacement};
use pyxross::ui::project::ProjectStore;
use pyxross::ui::workspace_runtime::{
    WorkspacePanelAction, WorkspacePanelCoordinator, WorkspacePanelState, WorkspaceRuntimeError,
};

#[test]
fn workspace_popout_reuses_one_host_and_native_close_preserves_canonical_placement() {
    let mut layout = PanelLayout::new();
    let mut hosts = WindowHostRegistry::new();
    let coordinator = WorkspacePanelCoordinator::new();

    let first = coordinator
        .apply(
            WorkspacePanelAction::PopOut(PanelId::Preview),
            &mut layout,
            &mut hosts,
        )
        .expect("preview can pop out");
    let second = coordinator
        .apply(
            WorkspacePanelAction::PopOut(PanelId::Preview),
            &mut layout,
            &mut hosts,
        )
        .expect("repeated pop out is idempotent");

    assert_eq!(first, second);
    assert_eq!(hosts.len(), 1);
    assert_eq!(
        layout
            .registry()
            .get(PanelId::Preview)
            .expect("preview exists")
            .placement(),
        PanelPlacement::Docked
    );
    assert_eq!(
        coordinator.state(PanelId::Preview, &layout, &hosts),
        Ok(first)
    );

    let docked = coordinator
        .apply(
            WorkspacePanelAction::CloseNativeCopy {
                panel: PanelId::Preview,
                host: first.host().expect("native host"),
            },
            &mut layout,
            &mut hosts,
        )
        .expect("native close retires the copy");
    assert_eq!(
        docked,
        WorkspacePanelState::Main {
            placement: PanelPlacement::Docked,
            native_host: None
        }
    );
    assert_eq!(hosts.len(), 0);
    assert_eq!(
        layout
            .registry()
            .get(PanelId::Preview)
            .expect("preview exists")
            .id(),
        PanelId::Preview
    );
}

#[test]
fn os_close_restores_the_previous_floating_main_placement() {
    let mut layout = PanelLayout::new();
    let mut hosts = WindowHostRegistry::new();
    let coordinator = WorkspacePanelCoordinator::new();
    let panel = PanelId::Preview;

    layout
        .transition(panel, PanelPlacement::FloatingInMainWindow)
        .unwrap();
    let popout = coordinator
        .apply(WorkspacePanelAction::PopOut(panel), &mut layout, &mut hosts)
        .expect("floating preview can pop out");
    let host = popout.host().expect("pop-out has a host");
    let window = WindowId::from(48);
    hosts.bind_window(host, window).expect("window route binds");

    let action = coordinator
        .os_close_action_for_window(window, &hosts)
        .expect("child window routes to its panel");
    let restored = coordinator
        .apply(action, &mut layout, &mut hosts)
        .expect("OS close retires the native copy");

    assert_eq!(
        restored,
        WorkspacePanelState::Main {
            placement: PanelPlacement::FloatingInMainWindow,
            native_host: None
        }
    );
    assert_eq!(hosts.len(), 0);
}

#[test]
fn panel_a_has_stable_viewport_and_idempotent_native_lifecycle() {
    let mut layout = PanelLayout::new();
    let mut hosts = WindowHostRegistry::new();
    let coordinator = WorkspacePanelCoordinator::new();

    let first = coordinator
        .apply(
            WorkspacePanelAction::PopOut(PanelId::PanelA),
            &mut layout,
            &mut hosts,
        )
        .expect("panel A can pop out");
    let second = coordinator
        .apply(
            WorkspacePanelAction::PopOut(PanelId::PanelA),
            &mut layout,
            &mut hosts,
        )
        .expect("panel A pop out is idempotent");

    assert_eq!(first, second);
    assert_eq!(first.host(), second.host());
    assert_eq!(
        first,
        coordinator
            .state(PanelId::PanelA, &layout, &hosts)
            .expect("panel A host state")
    );
    assert_eq!(
        coordinator.viewport_id(PanelId::PanelA),
        coordinator.viewport_id(PanelId::PanelA)
    );

    let docked = coordinator
        .apply(
            WorkspacePanelAction::DockBack(PanelId::PanelA),
            &mut layout,
            &mut hosts,
        )
        .expect("dock back restores panel A");
    assert_eq!(
        docked,
        WorkspacePanelState::Main {
            placement: PanelPlacement::Docked,
            native_host: None
        }
    );
    assert_eq!(hosts.len(), 0);
}

#[test]
fn workspace_context_follows_active_store_without_recreating_host() {
    let mut layout = PanelLayout::new();
    let mut hosts = WindowHostRegistry::new();
    let coordinator = WorkspacePanelCoordinator::new();
    let host = coordinator
        .apply(
            WorkspacePanelAction::PopOut(PanelId::Playback),
            &mut layout,
            &mut hosts,
        )
        .expect("playback can pop out");
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 2, 2).project();
    let beta = store.new_project("beta", 2, 2).project();
    let mut generations = ContextGenerationService::new();

    store.activate(alpha).expect("alpha is open");
    let first = coordinator
        .context_for(PanelId::Playback, &store, generations.current())
        .expect("active context exists");
    store.activate(beta).expect("beta is open");
    let second = coordinator
        .context_for(PanelId::Playback, &store, generations.advance())
        .expect("active context exists");

    assert_eq!(first.panel(), PanelId::Playback);
    assert_eq!(first.project(), alpha);
    assert_eq!(second.project(), beta);
    assert_ne!(first.snapshot(), second.snapshot());
    assert_eq!(
        hosts
            .resolve_panel(PanelId::Playback)
            .expect("host survives switch")
            .host(),
        host.host().expect("pop-out state has a host")
    );
}

#[test]
fn child_window_close_routes_to_panel_and_docks_back_without_closing_project() {
    let mut layout = PanelLayout::new();
    let mut hosts = WindowHostRegistry::new();
    let coordinator = WorkspacePanelCoordinator::new();
    let mut store = ProjectStore::new();
    let project = store.new_project("alpha", 2, 2).project();
    let popout = coordinator
        .apply(
            WorkspacePanelAction::PopOut(PanelId::Preview),
            &mut layout,
            &mut hosts,
        )
        .expect("preview can pop out");
    let host = popout.host().expect("pop-out has a host");
    let window = WindowId::from(47);
    hosts.bind_window(host, window).expect("window route binds");

    let action = coordinator
        .os_close_action_for_window(window, &hosts)
        .expect("child window routes to its panel");
    assert_eq!(
        action,
        WorkspacePanelAction::CloseNativeCopy {
            panel: PanelId::Preview,
            host
        }
    );

    let docked = coordinator
        .apply(action, &mut layout, &mut hosts)
        .expect("OS close docks the panel back");

    assert_eq!(
        docked,
        WorkspacePanelState::Main {
            placement: PanelPlacement::Docked,
            native_host: None
        }
    );
    assert_eq!(store.active_id(), Some(project));
    assert!(hosts.resolve_window(window).is_none());
    assert!(hosts.resolve_host(host).is_none());
}

#[test]
fn stale_native_close_cannot_retire_a_replacement_host() {
    let mut layout = PanelLayout::new();
    let mut hosts = WindowHostRegistry::new();
    let coordinator = WorkspacePanelCoordinator::new();
    let panel = PanelId::Preview;

    let first = coordinator
        .apply(WorkspacePanelAction::PopOut(panel), &mut layout, &mut hosts)
        .expect("first host");
    let first_host = first.host().expect("first host identity");
    coordinator
        .apply(
            WorkspacePanelAction::CloseNativeCopy {
                panel,
                host: first_host,
            },
            &mut layout,
            &mut hosts,
        )
        .expect("first host closes");
    let replacement = coordinator
        .apply(WorkspacePanelAction::PopOut(panel), &mut layout, &mut hosts)
        .expect("replacement host");
    let replacement_host = replacement.host().expect("replacement host identity");

    assert_eq!(
        coordinator.apply(
            WorkspacePanelAction::CloseNativeCopy {
                panel,
                host: first_host
            },
            &mut layout,
            &mut hosts
        ),
        Err(WorkspaceRuntimeError::HostMismatch {
            panel,
            host: first_host
        }),
    );
    assert_eq!(
        hosts
            .resolve_panel(panel)
            .expect("replacement remains")
            .host(),
        replacement_host
    );
}

#[test]
fn tool_popout_is_rejected_without_allocating_a_host() {
    let mut layout = PanelLayout::new();
    let mut hosts = WindowHostRegistry::new();
    let coordinator = WorkspacePanelCoordinator::new();
    let before = layout.registry().get(PanelId::Layers).expect("tool exists");

    assert_eq!(
        coordinator.apply(
            WorkspacePanelAction::PopOut(PanelId::Layers),
            &mut layout,
            &mut hosts
        ),
        Err(WorkspaceRuntimeError::UnsupportedPanel(PanelId::Layers)),
    );
    assert_eq!(
        layout.registry().get(PanelId::Layers).expect("tool exists"),
        before
    );
    assert_eq!(hosts.len(), 0);
}

#[test]
fn no_active_project_has_no_workspace_context() {
    let coordinator = WorkspacePanelCoordinator::new();
    let store = ProjectStore::new();
    let generation = ContextGenerationService::new();

    assert_eq!(
        coordinator.context_for(PanelId::FrameEditor, &store, generation.current()),
        None
    );
}

#[test]
fn viewport_identity_is_stable_and_workspace_scoped() {
    let coordinator = WorkspacePanelCoordinator::new();

    assert_eq!(
        coordinator.viewport_id(PanelId::Preview),
        coordinator.viewport_id(PanelId::Preview)
    );
    assert_ne!(
        coordinator.viewport_id(PanelId::Preview),
        coordinator.viewport_id(PanelId::Playback)
    );
    assert_eq!(coordinator.viewport_id(PanelId::Layers), None);
    assert_eq!(
        coordinator.viewport_id(PanelId::Preview),
        Some(ViewportId::from_hash_of((
            "pyxross.workspace",
            PanelId::Preview
        )))
    );
}
