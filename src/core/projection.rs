//! Core interface for the projection system (detached windows) [D65].
//!
//! Saflık Kuralı: Bu modül egui/wgpu/winit içermez, tamamen saf mantıktan oluşur.

use std::fmt;

/// Projeksiyon pencerelerini benzersiz şekilde tanımlayan opaque kimlikleyici.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ProjectionId(pub u64);

impl ProjectionId {
    /// Yeni bir benzersiz `ProjectionId` oluşturur.
    pub fn next() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for ProjectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "projection-{}", self.0)
    }
}

/// Projeksiyonun yerleştirileceği hedef monitör.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MonitorTarget {
    /// Birincil sistem monitörü.
    #[default]
    Primary,
    /// İsme göre özel monitör (örn. "DP-1").
    Named(&'static str),
    /// En yüksek çözünürlüğe sahip monitör.
    Largest,
    /// Birincil monitörün solu veya sağı.
    LeftOfPrimary,
    RightOfPrimary,
}

/// Projeksiyon modu (arayüz HUD veya temiz çıktı).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ProjectionMode {
    /// Normal düzenleyici görünümü: tuval + kılavuzlar.
    #[default]
    Editor,
    /// Temiz çıktı: kılavuzlar ve HUD kapalı.
    Capture,
    /// Oynatma görünümü.
    Playback,
}

/// Projeksiyonun hangi görünüm durumunu yansıttığı (D65: görünüm durumunun
/// projeksiyonu — asla ikinci bir belge değil).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ViewSource {
    /// Düzenleyici tuvali (varsayılan).
    #[default]
    EditorCanvas,
    /// Oynatma görünümü.
    Playback,
    /// Kare düzenleme görünümü.
    FrameEdit,
}

/// Projeksiyon durum değişikliklerini bildiren olaylar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProjectionEvent {
    Created {
        id: ProjectionId,
        width: u32,
        height: u32,
    },
    Destroyed {
        id: ProjectionId,
    },
    Resized {
        id: ProjectionId,
        width: u32,
        height: u32,
    },
    Died {
        id: ProjectionId,
    },
}

/// Projeksiyon hata türleri.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProjectionError {
    UnsupportedMode,
    MonitorNotFound,
    Backend(&'static str),
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedMode => write!(f, "unsupported projection mode"),
            Self::MonitorNotFound => write!(f, "monitor not found"),
            Self::Backend(msg) => write!(f, "backend error: {}", msg),
        }
    }
}

impl std::error::Error for ProjectionError {}

/// Projeksiyon durumunu tutan veri yapısı.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ProjectionStatus {
    pub id: ProjectionId,
    pub mode: ProjectionMode,
    pub target: MonitorTarget,
    pub width: u32,
    pub height: u32,
    pub capture_mode: bool,
    pub alive: bool,
}

/// Projeksiyon işlemlerini yöneten soyut arayüz.
pub trait Projector {
    /// Yeni bir projeksiyon penceresi oluşturur.
    fn create(
        &mut self,
        target: MonitorTarget,
        mode: ProjectionMode,
    ) -> Result<ProjectionId, ProjectionError>;

    /// Projeksiyon penceresini yok eder.
    fn destroy(&mut self, id: ProjectionId) -> Result<(), ProjectionError>;

    /// Projeksiyon pencerelerini ve durumlarını sorgular.
    fn status(&self, id: ProjectionId) -> Option<ProjectionStatus>;

    /// Event kuyruğundaki olayları boşaltır.
    fn poll_events(&mut self) -> Vec<ProjectionEvent>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MockProjector {
        projections: HashMap<ProjectionId, ProjectionStatus>,
        events: Vec<ProjectionEvent>,
    }

    impl MockProjector {
        fn new() -> Self {
            Self {
                projections: HashMap::new(),
                events: Vec::new(),
            }
        }
    }

    impl Projector for MockProjector {
        fn create(
            &mut self,
            target: MonitorTarget,
            mode: ProjectionMode,
        ) -> Result<ProjectionId, ProjectionError> {
            let id = ProjectionId::next();
            let status = ProjectionStatus {
                id,
                mode,
                target,
                width: 1280,
                height: 720,
                capture_mode: mode == ProjectionMode::Capture,
                alive: true,
            };
            self.projections.insert(id, status);
            self.events.push(ProjectionEvent::Created {
                id,
                width: 1280,
                height: 720,
            });
            Ok(id)
        }

        fn destroy(&mut self, id: ProjectionId) -> Result<(), ProjectionError> {
            if self.projections.remove(&id).is_some() {
                self.events.push(ProjectionEvent::Destroyed { id });
                Ok(())
            } else {
                Err(ProjectionError::Backend("not found"))
            }
        }

        fn status(&self, id: ProjectionId) -> Option<ProjectionStatus> {
            self.projections.get(&id).copied()
        }

        fn poll_events(&mut self) -> Vec<ProjectionEvent> {
            std::mem::take(&mut self.events)
        }
    }

    #[test]
    fn test_projection_id_uniqueness() {
        let id1 = ProjectionId::next();
        let id2 = ProjectionId::next();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_mock_projector_lifecycle() {
        let mut projector = MockProjector::new();
        let id = projector
            .create(MonitorTarget::Primary, ProjectionMode::Capture)
            .unwrap();

        let status = projector.status(id).unwrap();
        assert_eq!(status.mode, ProjectionMode::Capture);
        assert_eq!(status.width, 1280);
        assert_eq!(status.height, 720);
        assert!(status.capture_mode);

        let events = projector.poll_events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            ProjectionEvent::Created {
                id,
                width: 1280,
                height: 720
            }
        );

        projector.destroy(id).unwrap();
        assert!(projector.status(id).is_none());

        let events = projector.poll_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ProjectionEvent::Destroyed { id });
    }

    #[test]
    fn view_source_default_is_editor_canvas() {
        assert_eq!(ViewSource::default(), ViewSource::EditorCanvas);
    }

    #[test]
    fn view_source_clone_eq() {
        let a = ViewSource::Playback;
        let b = a;
        assert_eq!(a, b);
        assert_ne!(ViewSource::Playback, ViewSource::FrameEdit);
    }
}
