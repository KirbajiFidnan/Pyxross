use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Instant;

use pyxross::core::brush::{BrushShape, BrushSpec, DrawMode, ScatterShape, Stroke};
use pyxross::core::clipboard::ClipboardRegion;
use pyxross::core::color::Color;
use pyxross::core::document::FORMAT_VERSION;
use pyxross::core::math::Rect2i;
use pyxross::core::select::Selection;
use pyxross::core::stroke_command::StrokeSession;
use pyxross::core::transform::{LayerBuffer, TransformObject};
use pyxross::input::Tool;
use pyxross::io::{
    clear_recovery_journal, read_recovery_journal, write_recovery_journal, RecoveryJournal,
};
use pyxross::render::gizmo::GizmoHit;
use pyxross::ui::project::{
    AnchorEnd, CloseProject, DrawSettings, Ownership, ProjectId, ProjectStore, SelectGesture,
    SelectionTransform, TransformSession,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("pyx_task2_harness_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn struct_fields(source: &str, name: &str) -> BTreeSet<String> {
    let marker = format!("struct {name}");
    let start = source.find(&marker).expect("source struct exists");
    let body_start = source[start..]
        .find('{')
        .expect("source struct body exists")
        + start
        + 1;
    let mut depth = 1usize;
    let mut body_end = body_start;
    for (offset, character) in source[body_start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    body_end = body_start + offset;
                    break;
                }
            }
            _ => {}
        }
    }
    source[body_start..body_end]
        .lines()
        .filter_map(|line| {
            let field = line.trim().strip_prefix("pub ")?;
            let name = field.split_once(':')?.0.trim();
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

fn struct_bodies(source: &str) -> Vec<&str> {
    let mut bodies = Vec::new();
    let mut search_from = 0;
    while let Some(relative_start) = source[search_from..].find("struct ") {
        let start = search_from + relative_start;
        let body_start = source[start..]
            .find('{')
            .expect("source struct body exists")
            + start
            + 1;
        let mut depth = 1usize;
        let mut body_end = body_start;
        for (offset, character) in source[body_start..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        body_end = body_start + offset;
                        break;
                    }
                }
                _ => {}
            }
        }
        bodies.push(&source[body_start..body_end]);
        search_from = body_end + 1;
    }
    bodies
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AppOwner {
    ProjectLocal,
    ShellGlobal,
    HostLocal,
}

struct AppOwnership {
    field: &'static str,
    owner: AppOwner,
    rationale: &'static str,
    lifecycle: &'static str,
}

const APP_OWNERSHIP: &[AppOwnership] = &[
    AppOwnership {
        field: "window",
        owner: AppOwner::HostLocal,
        rationale: "winit window handle belongs to one host",
        lifecycle: "created and retired with the host",
    },
    AppOwnership {
        field: "renderer",
        owner: AppOwner::HostLocal,
        rationale: "GPU renderer is host-bound",
        lifecycle: "created and retired with the host",
    },
    AppOwnership {
        field: "egui_state",
        owner: AppOwner::HostLocal,
        rationale: "egui-winit input state is host-bound",
        lifecycle: "created and retired with the host",
    },
    AppOwnership {
        field: "ctx",
        owner: AppOwner::HostLocal,
        rationale: "egui context is host runtime state",
        lifecycle: "lives for the host process",
    },
    AppOwnership {
        field: "projects",
        owner: AppOwner::ShellGlobal,
        rationale: "shell-owned store routes the active project session",
        lifecycle: "survives project switches and removes closed sessions",
    },
    AppOwnership {
        field: "canvas_widget",
        owner: AppOwner::HostLocal,
        rationale: "canvas widget interaction state is host-local",
        lifecycle: "resets with the host widget",
    },
    AppOwnership {
        field: "canvas_texture",
        owner: AppOwner::HostLocal,
        rationale: "GPU texture handle is an approved host cache",
        lifecycle: "invalidated and rebuilt on host/project changes",
    },
    AppOwnership {
        field: "texture_dirty",
        owner: AppOwner::HostLocal,
        rationale: "texture invalidation flag is host-local",
        lifecycle: "consumed by host texture synchronization",
    },
    AppOwnership {
        field: "canvas_cache_token",
        owner: AppOwner::HostLocal,
        rationale: "cache identity is an approved host cache",
        lifecycle: "cleared on activation changes and rebuilt on sync",
    },
    AppOwnership {
        field: "toolbar",
        owner: AppOwner::ShellGlobal,
        rationale: "panel UI memory is shell-global",
        lifecycle: "survives project switches",
    },
    AppOwnership {
        field: "layer_panel",
        owner: AppOwner::ShellGlobal,
        rationale: "panel UI memory is shell-global",
        lifecycle: "survives project switches",
    },
    AppOwnership {
        field: "toolbox_host",
        owner: AppOwner::ShellGlobal,
        rationale: "dock panel host cells are shell-global view state",
        lifecycle: "survives project switches without recreating panel entries",
    },
    AppOwnership {
        field: "layers_host",
        owner: AppOwner::ShellGlobal,
        rationale: "dock panel host cells are shell-global view state",
        lifecycle: "survives project switches without recreating panel entries",
    },
    AppOwnership {
        field: "color_picker_host",
        owner: AppOwner::ShellGlobal,
        rationale: "dock panel host cells are shell-global view state",
        lifecycle: "survives project switches without recreating panel entries",
    },
    AppOwnership {
        field: "palette_host",
        owner: AppOwner::ShellGlobal,
        rationale: "dock panel host cells are shell-global view state",
        lifecycle: "survives project switches without recreating panel entries",
    },
    AppOwnership {
        field: "settings",
        owner: AppOwner::ShellGlobal,
        rationale: "panel UI memory is shell-global",
        lifecycle: "survives project switches",
    },
    AppOwnership {
        field: "keybindings",
        owner: AppOwner::ShellGlobal,
        rationale: "panel UI memory is shell-global",
        lifecycle: "survives project switches",
    },
    AppOwnership {
        field: "keymap",
        owner: AppOwner::ShellGlobal,
        rationale: "user keymap is shell-global",
        lifecycle: "loaded and persisted independently of projects",
    },
    AppOwnership {
        field: "theme",
        owner: AppOwner::ShellGlobal,
        rationale: "theme manager is shell-global",
        lifecycle: "survives project switches",
    },
    AppOwnership {
        field: "skin_atlas_cache",
        owner: AppOwner::ShellGlobal,
        rationale: "skin atlas textures are shell-global theme caches",
        lifecycle: "rebuilt when the theme or its dir changes",
    },
    AppOwnership {
        field: "timeline",
        owner: AppOwner::ShellGlobal,
        rationale: "panel UI memory is shell-global",
        lifecycle: "survives project switches",
    },
    AppOwnership {
        field: "panel_dock",
        owner: AppOwner::ShellGlobal,
        rationale: "generic dock instances and content are shell-global demo state",
        lifecycle: "survives project switches without recreating panel entries",
    },
    AppOwnership {
        field: "panel_dock_demo",
        owner: AppOwner::HostLocal,
        rationale: "demo visibility is transient host UI state",
        lifecycle: "toggles with the current egui host",
    },
    AppOwnership {
        field: "input_capture",
        owner: AppOwner::HostLocal,
        rationale: "gesture ownership is transient host input state",
        lifecycle: "claimed on pointer press, released on button-up or window exit",
    },
    AppOwnership {
        field: "panel_layout",
        owner: AppOwner::ShellGlobal,
        rationale: "docking topology and panel placement are shell-global",
        lifecycle: "survives project switches and preserves one instance per panel",
    },
    AppOwnership {
        field: "host_registry",
        owner: AppOwner::ShellGlobal,
        rationale:
            "runtime panel hosts are shell-global and keyed independently of project sessions",
        lifecycle: "creates and retires hosts without cloning panel or project state",
    },
    AppOwnership {
        field: "native_hosts",
        owner: AppOwner::ShellGlobal,
        rationale: "native window runtimes are keyed by stable host identity",
        lifecycle: "created from lifecycle callbacks and retired on dock or OS close",
    },
    AppOwnership {
        field: "native_host_retirement",
        owner: AppOwner::ShellGlobal,
        rationale: "retired native window runtimes remain owned across one event-loop turn",
        lifecycle: "drops each retired host exactly once before the following turn",
    },
    AppOwnership {
        field: "pending_native_hosts",
        owner: AppOwner::ShellGlobal,
        rationale: "queued native creation requests cross the egui and winit callback boundary",
        lifecycle: "consumed by active event-loop callbacks without duplicating logical panels",
    },
    AppOwnership {
        field: "deferred_workspace_actions",
        owner: AppOwner::ShellGlobal,
        rationale: "native dock and close actions cross the winit callback boundary",
        lifecycle: "drained after event callbacks before native hosts are retired",
    },
    AppOwnership {
        field: "workspace_runtime",
        owner: AppOwner::ShellGlobal,
        rationale: "workspace placement actions coordinate shell registries",
        lifecycle: "survives project switches without owning project state",
    },
    AppOwnership {
        field: "context_generation",
        owner: AppOwner::ShellGlobal,
        rationale: "context invalidation generation is shared by every host",
        lifecycle: "advances on active-context changes and rejects stale queued events",
    },
    AppOwnership {
        field: "project_tabs",
        owner: AppOwner::ShellGlobal,
        rationale: "tab view state is shell-global and references stable ProjectId values",
        lifecycle: "survives project switching without recreating panels or hosts",
    },
    AppOwnership {
        field: "workspace_layout",
        owner: AppOwner::ShellGlobal,
        rationale: "global panel topology and geometry are independent of project files",
        lifecycle: "persists across project saves and restores with fresh runtime IDs",
    },
    AppOwnership {
        field: "gizmo_hovered",
        owner: AppOwner::HostLocal,
        rationale: "pointer hover is host-local transient state",
        lifecycle: "resets with the host frame",
    },
    AppOwnership {
        field: "tool_armed",
        owner: AppOwner::HostLocal,
        rationale: "primary-button tool gesture arming is transient host input state",
        lifecycle: "armed on a press outside the canvas, cleared on release",
    },
    AppOwnership {
        field: "tool_outside",
        owner: AppOwner::HostLocal,
        rationale:
            "primary-button tool gesture outside-canvas tracking is transient host input state",
        lifecycle: "tracks the pointer relative to the canvas draw rect while armed",
    },
    AppOwnership {
        field: "last_selection_drag_point",
        owner: AppOwner::HostLocal,
        rationale:
            "the clamped edge point a Select/Lasso drag held while the pointer was outside the canvas is transient host input state",
        lifecycle: "seeded on every selection drag point, cleared on release and on a new press",
    },
    AppOwnership {
        field: "line_mode",
        owner: AppOwner::HostLocal,
        rationale: "LINE-mode latch is transient host input state for the current gesture",        lifecycle: "latched on a Shift+press on a draw tool, cleared on release",
    },
    AppOwnership {
        field: "line_anchor",
        owner: AppOwner::HostLocal,
        rationale: "LINE-mode press anchor is transient host input state",
        lifecycle: "set on the press frame and cleared on release",
    },
    AppOwnership {
        field: "line_end",
        owner: AppOwner::HostLocal,
        rationale: "LINE-mode live end point is transient host input state",
        lifecycle: "refreshed each drag frame and cleared on release",
    },
    AppOwnership {
        field: "line_dynamic",
        owner: AppOwner::HostLocal,
        rationale: "the Alt-driven dynamic line latch is transient host input state",
        lifecycle: "latched with line_mode on an Alt+press and cleared on release",
    },
    AppOwnership {
        field: "lifted_selection_is_masked",
        owner: AppOwner::HostLocal,
        rationale:
            "the mask shape of the selection lifted into the live transform is transient session state",
        lifecycle: "recorded when the selection is lifted and read once at commit",
    },
    AppOwnership {
        field: "curve_hovered",
        owner: AppOwner::HostLocal,
        rationale: "pointer hover over a dynamic-curve gizmo is host-local transient state",
        lifecycle: "recomputed each host frame and cleared when no curve session is active",
    },
    AppOwnership {
        field: "transform_preview",
        owner: AppOwner::HostLocal,
        rationale: "preview texture is an approved host cache",
        lifecycle: "cleared on activation and transform cancellation",
    },
    AppOwnership {
        field: "transform_preview_key",
        owner: AppOwner::HostLocal,
        rationale: "dirty key for the transform preview texture is an approved host cache",
        lifecycle: "updated on each preview re-upload and cleared with the preview",
    },
    AppOwnership {
        field: "os_clipboard",
        owner: AppOwner::HostLocal,
        rationale:
            "the persistent OS clipboard owner keeps the Wayland selection alive across frames",
        lifecycle: "kept for the App lifetime; the handle is created lazily on first use",
    },
    AppOwnership {
        field: "os_paste_rx",
        owner: AppOwner::HostLocal,
        rationale: "in-flight asynchronous OS clipboard paste read",
        lifecycle: "set when a paste read starts, cleared when the result lands",
    },
    AppOwnership {
        field: "os_paste_cursor",
        owner: AppOwner::HostLocal,
        rationale: "the cursor anchor captured at OS paste request time",
        lifecycle: "stored with the in-flight read and consumed when it completes",
    },
    AppOwnership {
        field: "autosave_base",
        owner: AppOwner::ShellGlobal,
        rationale: "autosave configuration is shell-global",
        lifecycle: "resolved by the host and shared by session recovery",
    },
    AppOwnership {
        field: "_project_cache_generation",
        owner: AppOwner::HostLocal,
        rationale: "cache generation is an approved host cache",
        lifecycle: "increments when host project cache invalidates",
    },
];

fn app_field_names(source: &str) -> BTreeSet<String> {
    let marker = "struct App {";
    let start = source.find(marker).expect("App struct exists");
    let body_start = start + marker.len();
    let body_end = source[body_start..].find("\n}").expect("App struct closes") + body_start;
    source[body_start..body_end]
        .lines()
        .filter_map(|line| {
            let field = line.trim().split_once(':')?.0.trim();
            (!field.is_empty() && !field.starts_with("//")).then(|| field.to_string())
        })
        .collect()
}

#[test]
fn app_ownership_inventory_covers_every_source_field() {
    let source = include_str!("../src/ui/mod.rs");
    let source_fields = app_field_names(source);
    let inventory_fields: BTreeSet<String> = APP_OWNERSHIP
        .iter()
        .map(|entry| entry.field.to_string())
        .collect();
    assert_eq!(
        source_fields, inventory_fields,
        "App ownership inventory must match the source field list"
    );
    assert!(APP_OWNERSHIP
        .iter()
        .all(|entry| !entry.rationale.is_empty() && !entry.lifecycle.is_empty()));
    assert!(APP_OWNERSHIP
        .iter()
        .any(|entry| entry.field == "canvas_cache_token" && entry.owner == AppOwner::HostLocal));
    assert!(
        APP_OWNERSHIP
            .iter()
            .filter(|entry| entry.owner == AppOwner::ProjectLocal)
            .count()
            == 0
    );
    assert!(APP_OWNERSHIP
        .iter()
        .any(|entry| entry.field == "projects" && entry.owner == AppOwner::ShellGlobal));
    assert!(APP_OWNERSHIP
        .iter()
        .filter(|entry| entry.owner == AppOwner::ShellGlobal)
        .any(|entry| entry.field == "keymap"));
    assert!(APP_OWNERSHIP
        .iter()
        .filter(|entry| entry.owner == AppOwner::ShellGlobal)
        .any(|entry| entry.field == "theme"));
}

#[test]
fn two_projects_keep_document_dirty_undo_selection_animation_and_canvas_state_isolated() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 8, 8).project();
    let beta = store.new_project("beta", 8, 8).project();

    store.activate(alpha).expect("alpha is open");
    {
        let session = store.current_mut();
        session.mark_dirty();
        session.selection = Selection::capture(
            &session.layers.active_layer().buffer,
            Rect2i::new(1, 2, 3, 4),
        );
        session.canvas_generation = 7;
        session.animation.goto(0);
        session
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(1, 1, Color::rgba(255, 0, 0, 255));
    }

    store.activate(beta).expect("beta is open");
    assert!(!store.current().is_dirty());
    assert!(store.current().selection.is_none());
    assert_eq!(store.current().canvas_generation, 0);
    assert_eq!(store.current().animation.current_index(), 0);
    assert_eq!(store.current().undo.undo_len(), 0);
    assert_eq!(
        store.current().layers.active_layer().buffer.get_pixel(1, 1),
        Some(Color::TRANSPARENT)
    );

    store.activate(alpha).expect("alpha is open");
    assert!(store.current().is_dirty());
    assert_eq!(
        store.current().selection.as_ref().map(Selection::rect),
        Some(Rect2i::new(1, 2, 3, 4))
    );
    assert_eq!(store.current().canvas_generation, 7);
    assert_eq!(
        store.current().layers.active_layer().buffer.get_pixel(1, 1),
        Some(Color::rgba(255, 0, 0, 255))
    );
}

#[test]
fn recovery_journals_are_isolated_by_project_id() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("same-name", 4, 4).project();
    let beta = store.new_project("same-name", 4, 4).project();
    let base = TempDir::new("recovery");
    let alpha_dir = base.path().join(
        store
            .session(alpha)
            .expect("alpha exists")
            .recovery_key
            .clone(),
    );
    let beta_dir = base.path().join(
        store
            .session(beta)
            .expect("beta exists")
            .recovery_key
            .clone(),
    );
    let alpha_journal = RecoveryJournal {
        format_version: FORMAT_VERSION,
        project_dir: Some(PathBuf::from("alpha.pyxross")),
        autosave_dir: PathBuf::from("alpha-autosave"),
        project_name: "same-name".to_string(),
        saved_at: 1,
    };
    let beta_journal = RecoveryJournal {
        saved_at: 2,
        ..alpha_journal.clone()
    };
    write_recovery_journal(&alpha_dir, &alpha_journal).expect("alpha journal writes");
    write_recovery_journal(&beta_dir, &beta_journal).expect("beta journal writes");

    assert_eq!(read_recovery_journal(&alpha_dir), Some(alpha_journal));
    assert_eq!(read_recovery_journal(&beta_dir), Some(beta_journal));
    clear_recovery_journal(&alpha_dir);
    assert!(read_recovery_journal(&alpha_dir).is_none());
    assert!(read_recovery_journal(&beta_dir).is_some());
}

#[test]
fn activation_returns_explicit_cache_token_and_generation() {
    let mut store = ProjectStore::new();
    let alpha_creation = store.new_project("alpha", 2, 2);
    let alpha = alpha_creation.project();
    assert!(alpha_creation.activation().changed());
    assert_eq!(
        alpha_creation
            .activation()
            .token()
            .expect("active token")
            .project(),
        alpha
    );
    let beta_creation = store.new_project("beta", 2, 2);
    let beta = beta_creation.project();
    assert!(beta_creation.activation().changed());
    let first = store.activate(alpha).expect("alpha is open");
    let second = store.activate(beta).expect("beta is open");
    let repeat = store.activate(beta).expect("beta is open");
    assert!(first.changed());
    assert!(second.changed());
    assert!(!repeat.changed());
    assert_eq!(second.token().expect("active token").project(), beta);
    assert!(
        second.token().expect("active token").generation()
            > first.token().expect("active token").generation()
    );
    assert_eq!(repeat.token(), second.token());
}

#[test]
fn lifecycle_state_isolation_covers_recovery_actions_errors_and_live_gestures() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 8, 8).project();
    let beta = store.new_project("beta", 8, 8).project();
    store.activate(alpha).expect("alpha is open");
    {
        let session = store.current_mut();
        session.recovery_pending = true;
        session.last_error = Some("alpha error".to_string());
        session.last_autosave = Some(Instant::now());
        session.pending_action =
            Some(pyxross::ui::project::PendingAction::Close { project: alpha });
        session.gesture = SelectGesture::AreaMove {
            origin: (2, 2),
            dest: Rect2i::new(3, 3, 2, 2),
            preview: None,
        };
        let stroke = Stroke::new(
            BrushSpec::new(1, BrushShape::Square),
            DrawMode::Pen,
            Color::WHITE,
        );
        session.stroke = StrokeSession::begin(
            &session.layers.active_layer().buffer,
            stroke,
            Rect2i::new(0, 0, 4, 4),
            session.layers.active_layer_id(),
        );
    }
    store.activate(beta).expect("beta is open");
    assert!(!store.current().recovery_pending);
    assert!(store.current().last_error.is_none());
    assert!(store.current().last_autosave.is_none());
    assert!(store.current().pending_action.is_none());
    assert!(store.current().gesture.marquee().is_none());
    assert!(store.current().gesture.move_dest().is_none());
    assert!(store.current().stroke.is_none());
    store.activate(alpha).expect("alpha is open");
    assert!(store.current().recovery_pending);
    assert_eq!(store.current().last_error.as_deref(), Some("alpha error"));
    assert!(store.current().last_autosave.is_some());
    assert!(store.current().pending_action.is_some());
    assert_eq!(
        store.current().gesture.marquee().map(|(_, rect)| rect),
        None
    );
    assert_eq!(
        store.current().gesture.move_dest(),
        Some(Rect2i::new(3, 3, 2, 2))
    );
    assert!(store.current().stroke.is_some());
}

#[test]
fn dirty_inactive_close_is_guarded() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 4, 4).project();
    let beta = store.new_project("beta", 4, 4).project();
    store.activate(alpha).expect("alpha is open");
    store.current_mut().mark_dirty();
    store.activate(beta).expect("beta is open");
    assert_eq!(
        store.close(alpha, CloseProject::Discard),
        Err(pyxross::ui::project::CloseError::Dirty { project: alpha })
    );
    assert_eq!(store.active_id(), Some(beta));
    assert!(store.contains(alpha));
}

#[test]
fn dirty_save_and_cancel_are_atomic() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 4, 4).project();
    let _beta = store.new_project("beta", 4, 4).project();
    store.activate(alpha).expect("alpha is open");
    store.current_mut().mark_dirty();
    assert_eq!(
        store.close(alpha, CloseProject::Cancel),
        Err(pyxross::ui::project::CloseError::Cancelled { project: alpha })
    );
    assert_eq!(
        store.close(alpha, CloseProject::Save),
        Err(pyxross::ui::project::CloseError::SaveRequired { project: alpha })
    );
    assert!(store.contains(alpha));
    assert!(store.session(alpha).expect("alpha remains open").is_dirty());
    assert_eq!(store.active_id(), Some(alpha));
}

#[test]
fn active_close_returns_replacement_activation_outcome() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 2, 2).project();
    let beta = store.new_project("beta", 2, 2).project();
    store.activate(alpha).expect("alpha is open");
    let outcome = store
        .close(alpha, CloseProject::Discard)
        .expect("alpha closes");
    let activation = outcome
        .activation()
        .expect("beta becomes active explicitly");
    assert_eq!(
        activation.token().expect("replacement token").project(),
        beta
    );
    assert_eq!(store.active_id(), Some(beta));
}

#[test]
fn ownership_table_is_exhaustive_and_classifies_each_task_two_category() {
    let expected = [
        "document/layers",
        "project metadata/path/dirty",
        "save/recovery/autosave",
        "undo",
        "selection",
        "transform",
        "clipboard",
        "active layer/frame",
        "camera",
        "animation/playback",
        "palette/colors",
        "tool/brush/grid",
        "canvas texture",
        "dialogs",
        "keymap",
        "theme",
        "panel placement/UI memory",
        "host input/render state",
    ];
    let actual: Vec<_> = Ownership::ALL.iter().map(|entry| entry.category).collect();
    assert_eq!(actual, expected);
    assert!(Ownership::ALL
        .iter()
        .all(|entry| entry.owner.is_project_local()
            || entry.owner.is_shell_global()
            || entry.owner.is_host_local()));
}

#[test]
fn ownership_source_inventory_matches_session_and_host_boundaries() {
    let project_source = include_str!("../src/ui/project.rs");
    let session_fields = struct_fields(project_source, "ProjectSession");
    let owned_fields: BTreeSet<String> = Ownership::ALL
        .iter()
        .filter(|entry| entry.owner.is_project_local())
        .flat_map(|entry| entry.fields.split(", ").map(str::to_owned))
        .collect();
    assert_eq!(session_fields, owned_fields);

    let host_fields = ["window", "renderer", "egui_state", "ctx", "keymap", "theme"];
    assert!(host_fields
        .iter()
        .all(|field| !session_fields.contains(*field)));

    let view_sources = [
        ("src/ui/mod.rs", include_str!("../src/ui/mod.rs")),
        ("src/ui/canvas.rs", include_str!("../src/ui/canvas.rs")),
        ("src/ui/layers.rs", include_str!("../src/ui/layers.rs")),
        (
            "src/ui/keybindings.rs",
            include_str!("../src/ui/keybindings.rs"),
        ),
        ("src/ui/settings.rs", include_str!("../src/ui/settings.rs")),
        ("src/ui/theme.rs", include_str!("../src/ui/theme.rs")),
        ("src/ui/timeline.rs", include_str!("../src/ui/timeline.rs")),
        ("src/ui/toolbar.rs", include_str!("../src/ui/toolbar.rs")),
    ];
    for (path, source) in view_sources {
        for struct_body in struct_bodies(source) {
            assert!(
                !struct_body.contains("Document"),
                "{path} stores Document in a view/host struct"
            );
            assert!(
                !struct_body.contains("ProjectSession"),
                "{path} stores ProjectSession in a view/host struct"
            );
        }
    }
}

#[test]
fn project_ids_are_stable_and_not_reused_after_close() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 2, 2).project();
    let closed = store
        .close(alpha, CloseProject::Discard)
        .expect("alpha closes");
    assert_eq!(closed.closed(), alpha);
    assert_eq!(
        closed
            .activation()
            .expect("active clear is explicit")
            .token(),
        None
    );
    let beta = store.new_project("beta", 2, 2).project();
    assert!(beta > alpha);
    assert_ne!(ProjectId::default(), alpha);
}

#[test]
fn every_project_local_category_survives_switch_without_cross_contamination() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 8, 8).project();
    let beta = store.new_project("beta", 8, 8).project();
    store.activate(alpha).expect("alpha is open");
    {
        let session = store.current_mut();
        session.tile_size = 4;
        session.mark_dirty();
        session.save_generation = 3;
        session.recovery_pending = true;
        session.autosave_generation = 5;
        session.selection = Selection::capture(
            &session.layers.active_layer().buffer,
            Rect2i::new(1, 1, 2, 2),
        );
        session.clipboard = Some(ClipboardRegion::new(1, 1, vec![1, 2, 3, 4]));
        let layer = session.layers.add_layer("Alpha layer");
        session.layers.set_active(layer);
        session.animation.goto(0);
        session.camera.pan_by(3, 4);
        session.sequence.set_looping(false);
        session.animation.play();
        session.palettes.push(
            pyxross::core::palette::Palette::new("Alpha", vec![[1, 2, 3, 255]])
                .expect("valid palette"),
        );
        session.active_palette = 1;
        session.color = Color::rgba(9, 8, 7, 255);
        session.pen_draw_settings.size = 9;
        session.pen_draw_settings.shape = BrushShape::Round;
        session.pen_draw_settings.scatter = 5;
        session.grid_visible = false;
        session.canvas_generation = 11;
        session.dialog_open = true;
        let bytes = session
            .layers
            .active_layer()
            .buffer
            .export_region(Rect2i::new(0, 0, 1, 1), None)
            .expect("pixel region");
        session.transform = Some(TransformSession::Selection(SelectionTransform {
            source: pyxross::core::layer_commands::TransformSource {
                layer_id: session.layers.active_layer_id(),
                rect: Rect2i::new(0, 0, 1, 1),
                snapshot: bytes.clone(),
                mask: None,
            },
            object: TransformObject::lift(
                LayerBuffer {
                    layer_id: 0,
                    w: 1,
                    h: 1,
                    buf: bytes,
                },
                (0.0, 0.0),
            ),
            drag: GizmoHit::None,
            last_pt: (0.0, 0.0),
            cut_source: true,
            start_angle: 0.0,
            start_pointer_angle: 0.0,
            start_drag: GizmoHit::None,
            start_pointer: (0.0, 0.0),
            start_pos: (0.0, 0.0),
            start_local_dims: (1, 1),
            start_anchor: (0.0, 0.0),
            start_anchor_end: (AnchorEnd::Center, AnchorEnd::Center),
            start_pivot: (0.0, 0.0),
            start_flip_h: false,
            start_flip_v: false,
            start_alt: false,
            move_axis: None,
            drag_initialized: false,
            preview_dragging: false,
        }));
    }
    store.activate(beta).expect("beta is open");
    assert!(!store.current().is_dirty());
    assert!(store.current().selection.is_none());
    assert!(store.current().transform.is_none());
    assert_eq!(store.current().camera.pan(), (0, 0));
    assert!(store.current().sequence.looping());
    assert!(!store.current().animation.is_playing());
    assert_eq!(store.current().active_palette, 0);
    assert_eq!(store.current().color, Color::BLACK);
    assert_eq!(store.current().pen_draw_settings, DrawSettings::default());
    assert!(store.current().grid_visible);
    store.activate(alpha).expect("alpha is open");
    let preserved = store.current();
    assert_eq!(preserved.tile_size, 4);
    assert!(preserved.is_dirty());
    assert_eq!(preserved.save_generation, 3);
    assert!(preserved.recovery_pending);
    assert!(preserved.selection.is_some());
    assert!(preserved.transform.is_some());
    assert_eq!(preserved.camera.pan(), (3, 4));
    assert!(!preserved.sequence.looping());
    assert!(preserved.animation.is_playing());
    assert_eq!(preserved.active_palette, 1);
    assert_eq!(preserved.color, Color::rgba(9, 8, 7, 255));
    assert_eq!(preserved.pen_draw_settings.size, 9);
    assert_eq!(preserved.pen_draw_settings.shape, BrushShape::Round);
    assert_eq!(preserved.pen_draw_settings.scatter, 5);
    assert!(!preserved.grid_visible);
    assert_eq!(preserved.canvas_generation, 11);
    assert!(preserved.dialog_open);
}

#[test]
fn pen_and_eraser_keep_independent_draw_settings() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 8, 8).project();
    store.activate(alpha).expect("alpha is open");
    let session = store.current_mut();

    session.tool_state.select_tool(Tool::Pencil);
    {
        let pen = session.draw_settings_mut(Tool::Pencil);
        pen.size = 7;
        pen.shape = BrushShape::Round;
        pen.scatter = 4;
        pen.scatter_shape = ScatterShape::Diamond;
        pen.tail = 30;
    }

    session.tool_state.select_tool(Tool::Eraser);
    {
        let eraser = session.draw_settings_mut(Tool::Eraser);
        eraser.size = 2;
        eraser.shape = BrushShape::Square;
        eraser.scatter = 9;
        eraser.scatter_shape = ScatterShape::Circle;
        eraser.tail = -40;
    }

    session.tool_state.select_tool(Tool::Pencil);
    let pen = *session.draw_settings(Tool::Pencil);
    assert_eq!(
        pen,
        DrawSettings {
            size: 7,
            shape: BrushShape::Round,
            scatter: 4,
            scatter_shape: ScatterShape::Diamond,
            tail: 30,
        },
        "the pen's settings must survive an eraser edit"
    );
    let eraser = *session.draw_settings(Tool::Eraser);
    assert_eq!(
        eraser,
        DrawSettings {
            size: 2,
            shape: BrushShape::Square,
            scatter: 9,
            scatter_shape: ScatterShape::Circle,
            tail: -40,
        },
        "the eraser keeps its own settings"
    );
    assert_ne!(pen, eraser, "pen and eraser must not share one bundle");
}

#[test]
fn frame_context_and_session_document_roundtrip_use_active_session() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 2, 2).project();
    let beta = store.new_project("beta", 2, 2).project();
    store.activate(alpha).expect("alpha is open");
    store.current_mut().tile_size = 3;
    store.current_mut().mark_dirty();
    assert_eq!(store.frame().id(), alpha);
    assert_eq!(store.frame().to_document().tile_size, 3);
    store.activate(beta).expect("beta is open");
    assert_eq!(store.frame().id(), beta);
    assert_eq!(store.frame().to_document().tile_size, 16);
    assert!(!store.frame().session().is_dirty());
}

#[test]
fn project_metadata_path_and_dirty_state_are_isolated_by_active_session() {
    let mut store = ProjectStore::new();
    let alpha = store.new_project("alpha", 2, 2).project();
    let beta = store.new_project("beta", 2, 2).project();
    store.activate(alpha).expect("alpha is open");
    store.current_mut().path = Some(PathBuf::from("alpha.pyxross"));
    store.current_mut().mark_dirty();
    store.activate(beta).expect("beta is open");
    assert_eq!(store.current().path, None);
    assert!(!store.current().is_dirty());
    store.activate(alpha).expect("alpha is open");
    assert_eq!(store.current().path, Some(PathBuf::from("alpha.pyxross")));
    assert!(store.current().is_dirty());
}
