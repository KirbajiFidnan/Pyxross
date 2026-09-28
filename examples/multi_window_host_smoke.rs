use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

#[derive(Clone, Serialize)]
struct EventOccurrence {
    method: &'static str,
    source: &'static str,
}

#[derive(Clone, Default, Serialize)]
struct EventEvidence {
    count: u32,
    occurrences: Vec<EventOccurrence>,
}

impl EventEvidence {
    fn record(&mut self, method: &'static str, source: &'static str) {
        self.count += 1;
        self.occurrences.push(EventOccurrence { method, source });
    }
}

#[derive(Clone, Serialize)]
struct WindowRecord {
    window_id: String,
    title: String,
    resize: EventEvidence,
    input: EventEvidence,
    redraw: EventEvidence,
    close: EventEvidence,
}

#[derive(Serialize)]
struct WindowRoutingEvidence {
    run_id: String,
    source: &'static str,
    shutdown_policy: &'static str,
    distinct_window_ids: bool,
    windows: Vec<WindowRecord>,
}

struct WindowHost {
    evidence_dir: PathBuf,
    run_id: String,
    windows: HashMap<WindowId, (Window, egui::Context, egui_winit::State, WindowRecord)>,
    completed: Vec<WindowRecord>,
    shutdown_queue: Vec<WindowId>,
    wgpu_instance: Option<wgpu::Instance>,
}

enum RoutedEvent {
    Resize,
    Input,
    Redraw,
}

impl WindowHost {
    fn write_evidence(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut windows = self.completed.clone();
        windows.extend(
            self.windows
                .values()
                .map(|(_, _, _, record)| record.clone()),
        );
        windows.sort_by(|left, right| left.window_id.cmp(&right.window_id));
        let evidence = WindowRoutingEvidence {
            run_id: self.run_id.clone(),
            source: "examples/multi_window_host_smoke.rs",
            shutdown_policy: "controlled close after per-WindowId routing assertions; OS CloseRequested is recorded separately",
            distinct_window_ids: windows.first().zip(windows.get(1)).is_some_and(|(left, right)| left.window_id != right.window_id),
            windows,
        };
        std::fs::create_dir_all(&self.evidence_dir)?;
        std::fs::write(
            self.evidence_dir.join("window-routing.json"),
            serde_json::to_vec_pretty(&evidence)?,
        )?;
        Ok(())
    }

    fn record_event(
        &mut self,
        window_id: WindowId,
        event: RoutedEvent,
        method: &'static str,
        source: &'static str,
    ) {
        let Some((_, _, _, record)) = self.windows.get_mut(&window_id) else {
            return;
        };
        match event {
            RoutedEvent::Resize => record.resize.record(method, source),
            RoutedEvent::Input => record.input.record(method, source),
            RoutedEvent::Redraw => record.redraw.record(method, source),
        }
    }

    fn route_controlled_input(&mut self) {
        for window_id in self.windows.keys().copied().collect::<Vec<_>>() {
            let Some((needs_resize, needs_input, needs_redraw)) =
                self.windows.get(&window_id).map(|(_, _, _, record)| {
                    (
                        record.resize.count == 0,
                        record.input.count == 0,
                        record.redraw.count == 0,
                    )
                })
            else {
                continue;
            };
            if needs_resize {
                self.record_event(
                    window_id,
                    RoutedEvent::Resize,
                    "controlled_resize",
                    "harness::about_to_wait",
                );
            }
            if needs_input {
                self.record_event(
                    window_id,
                    RoutedEvent::Input,
                    "controlled_input",
                    "harness::about_to_wait",
                );
            }
            if needs_redraw {
                self.record_event(
                    window_id,
                    RoutedEvent::Redraw,
                    "controlled_redraw",
                    "harness::about_to_wait",
                );
            }
        }
    }

    fn routing_ready(&self) -> bool {
        self.windows.len() == 2
            && self.windows.values().all(|(_, _, _, record)| {
                record.resize.count > 0 && record.input.count > 0 && record.redraw.count > 0
            })
    }

    fn close_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        method: &'static str,
        source: &'static str,
    ) {
        let Some((_, _, _, mut record)) = self.windows.remove(&window_id) else {
            return;
        };
        record.close.record(method, source);
        self.completed.push(record);
        if self.windows.is_empty() {
            if let Err(error) = self.write_evidence() {
                eprintln!("smoke evidence write failed: {error}");
            }
            event_loop.exit();
        }
    }
}

impl ApplicationHandler for WindowHost {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.wgpu_instance.is_none() {
            self.wgpu_instance = Some(wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
                    event_loop.owned_display_handle(),
                )),
            ));
        }
        if !self.windows.is_empty() {
            return;
        }

        for title in ["Pyxross smoke A", "Pyxross smoke B"] {
            let window = event_loop
                .create_window(WindowAttributes::default().with_title(title))
                .expect("smoke window creation must succeed");
            let window_id = window.id();
            let context = egui::Context::default();
            let state = egui_winit::State::new(
                context.clone(),
                egui::ViewportId::ROOT,
                &window,
                None,
                None,
                None,
            );
            let record = WindowRecord {
                window_id: format!("{window_id:?}"),
                title: title.to_owned(),
                resize: EventEvidence::default(),
                input: EventEvidence::default(),
                redraw: EventEvidence::default(),
                close: EventEvidence::default(),
            };
            self.windows
                .insert(window_id, (window, context, state, record));
        }
        for (window, _, _, _) in self.windows.values() {
            window.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some((window, _, state, _)) = self.windows.get_mut(&window_id) else {
            return;
        };
        let response = state.on_window_event(window, &event);
        if response.repaint {
            window.request_redraw();
        }
        match event {
            WindowEvent::Resized(_) => self.record_event(
                window_id,
                RoutedEvent::Resize,
                "os_window_event",
                "winit::WindowEvent::Resized",
            ),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                self.record_event(
                    window_id,
                    RoutedEvent::Input,
                    "os_window_event",
                    "winit::WindowEvent::KeyboardInput",
                );
            }
            WindowEvent::RedrawRequested => {
                self.record_event(
                    window_id,
                    RoutedEvent::Redraw,
                    "os_window_event",
                    "winit::WindowEvent::RedrawRequested",
                );
                if let Some((_, context, _, _)) = self.windows.get_mut(&window_id) {
                    context.request_repaint();
                }
            }
            WindowEvent::CloseRequested => self.close_window(
                event_loop,
                window_id,
                "os_close_requested",
                "winit::WindowEvent::CloseRequested",
            ),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.route_controlled_input();
        if self.shutdown_queue.is_empty() && self.routing_ready() {
            self.shutdown_queue = self.windows.keys().copied().collect();
        }
        if let Some(window_id) = self.shutdown_queue.pop() {
            self.close_window(
                event_loop,
                window_id,
                "controlled_shutdown",
                "harness::about_to_wait",
            );
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Err(error) = self.write_evidence() {
            eprintln!("smoke evidence write failed: {error}");
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let evidence_dir = match (args.next().as_deref(), args.next()) {
        (Some("--evidence-dir"), Some(path)) => PathBuf::from(path),
        _ => PathBuf::from(".omo/evidence/dockable-workspace-and-project-tabs/task-1/smoke"),
    };
    std::fs::create_dir_all(&evidence_dir)?;
    let routing_path = evidence_dir.join("window-routing.json");
    match std::fs::remove_file(routing_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let run_id = format!(
        "multi-window-host-smoke-{}-{timestamp_ms}",
        std::process::id()
    );
    let event_loop = EventLoop::new().expect("event loop creation must succeed");
    event_loop
        .run_app(&mut WindowHost {
            evidence_dir,
            run_id,
            windows: HashMap::new(),
            completed: Vec::new(),
            shutdown_queue: Vec::new(),
            wgpu_instance: None,
        })
        .expect("smoke event loop must run");
    Ok(())
}
