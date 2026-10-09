//! Window backgrounds, native materials, and window corners.
//!
//! [`WindowOptions::background`](crate::WindowOptions::background) asks for
//! an opaque color, a transparent surface, or a native material: the
//! translucent, blurred backdrop the desktop compositor draws behind a
//! window (macOS vibrancy, Windows 11 Mica and Acrylic, KWin blur).
//! [`WindowOptions::corners`](crate::WindowOptions::corners) asks for a
//! corner shape. Neither is promised: the runner resolves each request
//! against the platform, the user's accessibility settings, and whether the
//! GPU surface can composite alpha, and reports what the window got through
//! [`EventContext::window_surface`](crate::EventContext::window_surface).
//!
//! A material the window cannot have becomes the request's opaque
//! [`MaterialOptions::fallback`] color, with a [`FallbackReason`]. Reduced
//! transparency and increased contrast always select the fallback. The app
//! should paint its panels the same way in both cases and leave the window
//! root unpainted where the material should show: a transparent pixel over
//! the fallback shows the fallback color.
//!
//! On macOS each material is a native `NSVisualEffectView` behind the GPU
//! view, and [`EventContext::set_material_regions`](crate::EventContext::set_material_regions)
//! adds one per region ([`MaterialScope::Regions`]). Windows and Linux have
//! one backdrop per window, which shows through every transparent pixel
//! ([`MaterialScope::WindowWide`]).

use quark::{Color, Rect};

/// What a native material looks like, by its role in the window. Platforms
/// map each kind to their closest material; kinds they lack share one.
pub use quark::scene::MaterialKind;

/// A material and the opaque color drawn when the window cannot have it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterialOptions {
    pub kind: MaterialKind,
    /// Shown instead of the material, and behind its transparent pixels,
    /// where the material is unavailable or turned off.
    pub fallback: Color,
}

impl MaterialOptions {
    pub fn new(kind: MaterialKind, fallback: Color) -> Self {
        Self { kind, fallback }
    }
}

/// What a window shows behind the app's paint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowBackground {
    /// An opaque color: the default, black as before.
    Opaque(Color),
    /// No background: whatever is behind the window shows through
    /// transparent pixels, unblurred. Needs a compositing desktop.
    Transparent,
    /// A native material, or its fallback color.
    Material(MaterialOptions),
}

/// The default background, and what a transparent window falls back to.
const DEFAULT_COLOR: Color = Color::rgba(0, 0, 0, 255);

impl Default for WindowBackground {
    fn default() -> Self {
        Self::Opaque(DEFAULT_COLOR)
    }
}

/// A window's corner shape.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum WindowCorners {
    /// What the platform gives windows of this kind.
    #[default]
    System,
    Square,
    /// Rounded by this radius, in logical points.
    Rounded(f32),
}

/// What a window's background resolved to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EffectiveBackground {
    Opaque(Color),
    Transparent,
    Material {
        kind: MaterialKind,
        scope: MaterialScope,
    },
    /// The requested material or transparency is unavailable: the window is
    /// opaque `color`.
    Fallback {
        color: Color,
        reason: FallbackReason,
    },
}

/// How far a native material reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialScope {
    /// The window's material fills it, and each region set with
    /// [`EventContext::set_material_regions`](crate::EventContext::set_material_regions)
    /// gets a native material of its own kind (macOS).
    Regions,
    /// One backdrop for the whole window shows through every transparent
    /// pixel; regions cannot choose their own kind (Windows, Linux).
    WindowWide,
}

/// Why a window got its fallback color instead of what it asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// The platform or compositor has no such material.
    Unsupported,
    /// The user turned on reduced transparency.
    ReducedTransparency,
    /// The user turned on increased contrast.
    IncreasedContrast,
    /// No compositing manager is running (X11), so nothing can show
    /// through the window.
    NoCompositor,
    /// The OS predates the material (Windows before build 22621).
    OsTooOld,
    /// The GPU surface cannot composite alpha.
    OpaqueSurface,
    /// The platform refused the request.
    Failed,
}

/// What a window's corners resolved to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CornerResult {
    /// The platform's own corners; also the answer when a requested shape
    /// is unavailable.
    System,
    Square,
    /// The content and its materials are clipped to this radius, inside the
    /// system window shape, which may round more (macOS titled windows).
    Clipped(f32),
    /// The platform rounds by its own fixed radius nearest the request
    /// (Windows 11 corner preferences).
    Approximate {
        requested: f32,
        native: f32,
    },
}

/// What a window's surface got: [`EventContext::window_surface`](crate::EventContext::window_surface).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowSurface {
    pub background: EffectiveBackground,
    pub corners: CornerResult,
}

/// A rectangle of a window that shows a native material of its own kind,
/// for [`EventContext::set_material_regions`](crate::EventContext::set_material_regions).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterialRect {
    /// Stable across frames, so a region's native view is reused.
    pub id: u64,
    /// In the window's logical points, already clipped.
    pub rect: Rect,
    pub corner_radius: f32,
    pub kind: MaterialKind,
}

/// What a desktop lets a window's surface do, as the runner detects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SurfaceEnvironment {
    pub(crate) backend: Backend,
    pub(crate) reduced_transparency: bool,
    pub(crate) increased_contrast: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Each platform builds only its own variants.
#[allow(dead_code)]
pub(crate) enum Backend {
    MacOs,
    /// `build` is the Windows build number, when known.
    Windows {
        build: Option<u32>,
    },
    /// `blur`: the compositor advertises KWin's blur-behind property.
    X11 {
        compositor: bool,
        blur: bool,
    },
    /// `blur`: the compositor has `org_kde_kwin_blur_manager`.
    Wayland {
        blur: bool,
    },
    /// The headless runner: no desktop behind its windows.
    Headless,
}

/// The first Windows build with `DWMWA_SYSTEMBACKDROP_TYPE`.
pub(crate) const WINDOWS_BACKDROP_BUILD: u32 = 22621;
/// The first Windows build with `DWMWA_WINDOW_CORNER_PREFERENCE`.
pub(crate) const WINDOWS_CORNERS_BUILD: u32 = 22000;

impl SurfaceEnvironment {
    /// What `request` resolves to on this desktop, given whether the GPU
    /// surface composites alpha.
    pub(crate) fn background(
        &self,
        request: WindowBackground,
        surface_alpha: bool,
    ) -> EffectiveBackground {
        let fallback = |color, reason| EffectiveBackground::Fallback { color, reason };
        match request {
            WindowBackground::Opaque(color) => EffectiveBackground::Opaque(color),
            WindowBackground::Transparent => match self.see_through() {
                Err(reason) => fallback(DEFAULT_COLOR, reason),
                Ok(()) if !surface_alpha => fallback(DEFAULT_COLOR, FallbackReason::OpaqueSurface),
                Ok(()) => EffectiveBackground::Transparent,
            },
            WindowBackground::Material(options) => match self.material() {
                Err(reason) => fallback(options.fallback, reason),
                Ok(_) if !surface_alpha => {
                    fallback(options.fallback, FallbackReason::OpaqueSurface)
                }
                Ok(scope) => EffectiveBackground::Material {
                    kind: options.kind,
                    scope,
                },
            },
        }
    }

    /// Whether `request` needs a surface that composites alpha: whether
    /// anything could show through it, whatever the user's accessibility
    /// settings say now. They can change while the window is open, and the
    /// surface (and on X11 the visual) is chosen only once.
    pub(crate) fn wants_alpha(&self, request: WindowBackground) -> bool {
        let unrestricted = Self {
            reduced_transparency: false,
            increased_contrast: false,
            ..*self
        };
        !matches!(
            unrestricted.background(request, true),
            EffectiveBackground::Opaque(_) | EffectiveBackground::Fallback { .. }
        )
    }

    /// Whether anything behind the window can show through it.
    fn see_through(&self) -> Result<(), FallbackReason> {
        match self.backend {
            Backend::X11 {
                compositor: false, ..
            } => Err(FallbackReason::NoCompositor),
            Backend::Headless => Err(FallbackReason::Unsupported),
            _ => Ok(()),
        }
    }

    fn material(&self) -> Result<MaterialScope, FallbackReason> {
        if self.reduced_transparency {
            return Err(FallbackReason::ReducedTransparency);
        }
        if self.increased_contrast {
            return Err(FallbackReason::IncreasedContrast);
        }
        self.see_through()?;
        match self.backend {
            Backend::MacOs => Ok(MaterialScope::Regions),
            Backend::Windows { build: Some(build) } if build < WINDOWS_BACKDROP_BUILD => {
                Err(FallbackReason::OsTooOld)
            }
            Backend::Windows { .. } => Ok(MaterialScope::WindowWide),
            Backend::X11 { blur, .. } | Backend::Wayland { blur } => blur
                .then_some(MaterialScope::WindowWide)
                .ok_or(FallbackReason::Unsupported),
            Backend::Headless => Err(FallbackReason::Unsupported),
        }
    }

    /// What `request` resolves to on this desktop.
    pub(crate) fn corners(&self, request: WindowCorners) -> CornerResult {
        let radius = match request {
            WindowCorners::System => return CornerResult::System,
            WindowCorners::Square => 0.0,
            WindowCorners::Rounded(radius) if radius.is_finite() => radius.max(0.0),
            WindowCorners::Rounded(_) => return CornerResult::System,
        };
        match self.backend {
            // Titled windows keep their system shape; the content layer
            // clips inside it.
            Backend::MacOs if radius > 0.0 => CornerResult::Clipped(radius),
            Backend::Windows { build }
                if build.is_none_or(|build| build >= WINDOWS_CORNERS_BUILD) =>
            {
                match windows_corner(radius) {
                    None => CornerResult::Square,
                    Some(native) => CornerResult::Approximate {
                        requested: radius,
                        native,
                    },
                }
            }
            _ => CornerResult::System,
        }
    }
}

/// The DWM corner preference nearest `radius`: square, `ROUNDSMALL` (4
/// points), or `ROUND` (8 points).
pub(crate) fn windows_corner(radius: f32) -> Option<f32> {
    match radius {
        r if r < 2.0 => None,
        r if r < 6.0 => Some(4.0),
        _ => Some(8.0),
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(windows)]
use windows as imp;

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub(crate) use imp::NativeMaterial;

/// No native materials on this platform.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
#[derive(Default)]
pub(crate) struct NativeMaterial;

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
impl NativeMaterial {
    pub(crate) fn apply(
        &mut self,
        _window: &winit::window::Window,
        surface: &WindowSurface,
    ) -> bool {
        !matches!(surface.background, EffectiveBackground::Material { .. })
    }

    pub(crate) fn set_regions(
        &mut self,
        _window: &winit::window::Window,
        _regions: &[MaterialRect],
    ) {
    }
}

/// The desktop `event_loop` talks to, as far as it matters to surfaces.
pub(crate) fn environment(event_loop: &winit::event_loop::ActiveEventLoop) -> SurfaceEnvironment {
    let (reduced_transparency, increased_contrast) = accessibility();
    SurfaceEnvironment {
        backend: backend(event_loop),
        reduced_transparency,
        increased_contrast,
    }
}

impl SurfaceEnvironment {
    /// This environment with the user's accessibility settings read again,
    /// which can change while the app runs.
    pub(crate) fn refreshed(self) -> Self {
        let (reduced_transparency, increased_contrast) = accessibility();
        Self {
            reduced_transparency,
            increased_contrast,
            ..self
        }
    }
}

/// Wake the event loop through `waker` when the user's accessibility
/// display settings change, where the platform says so; see
/// [`take_accessibility_change`]. Once per process.
pub(crate) fn watch_accessibility(waker: &crate::runner::Waker) {
    #[cfg(target_os = "macos")]
    macos::watch_accessibility(waker);
    let _ = waker;
}

/// Whether the accessibility display settings changed since the last call.
pub(crate) fn take_accessibility_change() -> bool {
    #[cfg(target_os = "macos")]
    return macos::take_accessibility_change();
    #[cfg(not(target_os = "macos"))]
    false
}

/// The user's reduced transparency and increased contrast settings.
fn accessibility() -> (bool, bool) {
    #[cfg(any(target_os = "macos", windows))]
    return imp::accessibility();
    // Desktop environments keep these in their own settings stores, with
    // no common key to read.
    #[cfg(not(any(target_os = "macos", windows)))]
    (false, false)
}

#[cfg(target_os = "macos")]
fn backend(_event_loop: &winit::event_loop::ActiveEventLoop) -> Backend {
    Backend::MacOs
}

#[cfg(windows)]
fn backend(_event_loop: &winit::event_loop::ActiveEventLoop) -> Backend {
    Backend::Windows {
        build: imp::build(),
    }
}

#[cfg(target_os = "linux")]
fn backend(event_loop: &winit::event_loop::ActiveEventLoop) -> Backend {
    use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};

    match event_loop.display_handle().map(|handle| handle.as_raw()) {
        Ok(RawDisplayHandle::Wayland(display)) => Backend::Wayland {
            blur: linux::wayland_blur(display.display),
        },
        _ => {
            let compositor = super::x11_root::compositor();
            Backend::X11 {
                compositor,
                blur: compositor && super::x11_root::blur_available(),
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn backend(_event_loop: &winit::event_loop::ActiveEventLoop) -> Backend {
    Backend::Headless
}

/// What a double-click on a title bar does here.
pub(crate) fn title_double_click() -> crate::platform::chrome::TitleAction {
    #[cfg(target_os = "macos")]
    return macos::title_double_click();
    #[cfg(not(target_os = "macos"))]
    crate::platform::chrome::TitleAction::ToggleMaximize
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRAY: Color = Color::rgba(40, 40, 40, 255);

    fn on(backend: Backend) -> SurfaceEnvironment {
        SurfaceEnvironment {
            backend,
            reduced_transparency: false,
            increased_contrast: false,
        }
    }

    fn sidebar() -> WindowBackground {
        WindowBackground::Material(MaterialOptions::new(MaterialKind::Sidebar, GRAY))
    }

    fn fallback(reason: FallbackReason) -> EffectiveBackground {
        EffectiveBackground::Fallback {
            color: GRAY,
            reason,
        }
    }

    fn material(scope: MaterialScope) -> EffectiveBackground {
        EffectiveBackground::Material {
            kind: MaterialKind::Sidebar,
            scope,
        }
    }

    // Catches a material promised where the desktop, the OS version, the
    // GPU surface, or the user's accessibility settings cannot show it, and
    // an alpha surface asked for where nothing could show through (an X11
    // ARGB visual without a compositor shows garbage).
    #[test]
    fn a_background_request_resolves_to_what_the_desktop_can_show() {
        use FallbackReason::*;
        use MaterialScope::*;
        let reduced = SurfaceEnvironment {
            reduced_transparency: true,
            ..on(Backend::MacOs)
        };
        let contrast = SurfaceEnvironment {
            increased_contrast: true,
            ..on(Backend::Windows { build: Some(26100) })
        };
        let x11 = |compositor, blur| on(Backend::X11 { compositor, blur });
        // (case, environment, request, surface composites alpha,
        //  expected, asks for an alpha surface)
        let cases = [
            (
                "macOS",
                on(Backend::MacOs),
                sidebar(),
                true,
                material(Regions),
                true,
            ),
            (
                "reduced transparency",
                reduced,
                sidebar(),
                true,
                fallback(ReducedTransparency),
                true,
            ),
            (
                "increased contrast",
                contrast,
                sidebar(),
                true,
                fallback(IncreasedContrast),
                true,
            ),
            (
                "Windows 11 22H2",
                on(Backend::Windows { build: Some(22621) }),
                sidebar(),
                true,
                material(WindowWide),
                true,
            ),
            (
                "Windows 11 21H2",
                on(Backend::Windows { build: Some(22000) }),
                sidebar(),
                true,
                fallback(OsTooOld),
                false,
            ),
            (
                "KWin on X11",
                x11(true, true),
                sidebar(),
                true,
                material(WindowWide),
                true,
            ),
            (
                "X11 without a compositor",
                x11(false, false),
                sidebar(),
                true,
                fallback(NoCompositor),
                false,
            ),
            (
                "an X11 compositor without blur",
                x11(true, false),
                sidebar(),
                true,
                fallback(Unsupported),
                false,
            ),
            (
                "Wayland without KWin's blur",
                on(Backend::Wayland { blur: false }),
                sidebar(),
                true,
                fallback(Unsupported),
                false,
            ),
            (
                "an opaque GPU surface",
                on(Backend::MacOs),
                sidebar(),
                false,
                fallback(OpaqueSurface),
                true,
            ),
            (
                "transparent on an X11 compositor",
                x11(true, false),
                WindowBackground::Transparent,
                true,
                EffectiveBackground::Transparent,
                true,
            ),
            (
                "transparent without a compositor",
                x11(false, false),
                WindowBackground::Transparent,
                true,
                EffectiveBackground::Fallback {
                    color: DEFAULT_COLOR,
                    reason: NoCompositor,
                },
                false,
            ),
            (
                "opaque stays opaque",
                on(Backend::MacOs),
                WindowBackground::Opaque(GRAY),
                true,
                EffectiveBackground::Opaque(GRAY),
                false,
            ),
        ];
        for (case, environment, request, alpha, expected, asks) in cases {
            assert_eq!(
                (
                    environment.background(request, alpha),
                    environment.wants_alpha(request)
                ),
                (expected, asks),
                "{case}"
            );
        }
    }

    // Catches a corner shape reported as granted where the platform keeps
    // its own, and a DWM preference reported as the exact radius asked for.
    #[test]
    fn a_corner_request_resolves_to_the_platforms_nearest_shape() {
        let windows = on(Backend::Windows { build: Some(22631) });
        let cases = [
            (
                "macOS clips inside the system shape",
                on(Backend::MacOs),
                WindowCorners::Rounded(12.0),
                CornerResult::Clipped(12.0),
            ),
            (
                "macOS keeps titled corners",
                on(Backend::MacOs),
                WindowCorners::Square,
                CornerResult::System,
            ),
            (
                "Windows square",
                windows,
                WindowCorners::Square,
                CornerResult::Square,
            ),
            (
                "Windows small",
                windows,
                WindowCorners::Rounded(3.0),
                CornerResult::Approximate {
                    requested: 3.0,
                    native: 4.0,
                },
            ),
            (
                "Windows large",
                windows,
                WindowCorners::Rounded(12.0),
                CornerResult::Approximate {
                    requested: 12.0,
                    native: 8.0,
                },
            ),
            (
                "Windows 10 has no preference",
                on(Backend::Windows { build: Some(19045) }),
                WindowCorners::Rounded(12.0),
                CornerResult::System,
            ),
            (
                "X11 leaves corners to the window manager",
                on(Backend::X11 {
                    compositor: true,
                    blur: true,
                }),
                WindowCorners::Rounded(12.0),
                CornerResult::System,
            ),
            (
                "a non-finite radius",
                on(Backend::MacOs),
                WindowCorners::Rounded(f32::NAN),
                CornerResult::System,
            ),
        ];
        for (case, environment, request, expected) in cases {
            assert_eq!(environment.corners(request), expected, "{case}");
        }
    }
}
