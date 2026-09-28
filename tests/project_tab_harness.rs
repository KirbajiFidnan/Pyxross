use std::path::PathBuf;

use pyxross::ui::project::{CloseProject, ProjectStore};

#[test]
fn tabs_report_stable_order_and_active_context() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 2, 2).project();
    let beta = store.new_project("beta", 2, 2).project();
    let tabs = store.tabs();
    assert_eq!(
        tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
        vec![alpha, beta]
    );
    assert!(!tabs[0].active);
    assert!(tabs[1].active);
    store.activate(alpha).unwrap();
    assert!(store.tabs()[0].active);
}

#[test]
fn active_close_selects_deterministic_neighbor_and_dirty_close_is_atomic() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 2, 2).project();
    let beta = store.new_project("beta", 2, 2).project();
    let gamma = store.new_project("gamma", 2, 2).project();
    store.activate(beta).unwrap();
    store.current_mut().mark_dirty();
    assert_eq!(
        store.close(beta, CloseProject::Discard),
        Err(pyxross::ui::project::CloseError::Dirty { project: beta })
    );
    assert_eq!(store.active_id(), Some(beta));
    store.current_mut().mark_saved();
    let outcome = store.close(beta, CloseProject::Discard).unwrap();
    assert_eq!(outcome.closed(), beta);
    assert_eq!(store.active_id(), Some(alpha));
    store.close(alpha, CloseProject::Discard).unwrap();
    assert_eq!(store.active_id(), Some(gamma));
}

#[test]
fn project_path_lookup_reuses_existing_tab_identity() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 2, 2).project();
    let path = PathBuf::from("/tmp/alpha.pyxross");
    store.session_mut(alpha).unwrap().path = Some(path.clone());

    assert_eq!(store.find_by_path(&path), Some(alpha));
    assert_eq!(store.tabs().len(), 1);
}
