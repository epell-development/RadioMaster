//! Embedded web pages (Microsoft Edge WebView2 on Windows). Each panel that shows a web tab gets
//! its own native child web view, created on first use and positioned over the panel. Works in the
//! main window and in Additional windows. A native view draws above egui, so the app hides the views
//! of a window while one of its menus or dialogs overlaps them.
use eframe::egui;

#[cfg(target_os = "windows")]
mod native {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use wry::{
        dpi::{PhysicalPosition, PhysicalSize},
        raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle},
        Rect, WebView, WebViewBuilder,
    };

    #[link(name = "user32")]
    extern "system" {
        fn FindWindowW(class_name: *const u16, window_name: *const u16) -> isize;
    }

    /// A parent window given by its raw handle.
    struct Parent(RawWindowHandle);
    impl HasWindowHandle for Parent {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            // SAFETY: the handle belongs to a window of this process that is alive while we build the view.
            Ok(unsafe { WindowHandle::borrow_raw(self.0) })
        }
    }

    /// Native handle of a top-level window by its (unique) title.
    fn find_window(title: &str) -> Option<RawWindowHandle> {
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: plain Win32 call with a NUL-terminated UTF-16 string.
        let h = unsafe { FindWindowW(std::ptr::null(), wide.as_ptr()) };
        std::num::NonZeroIsize::new(h).map(|nz| RawWindowHandle::Win32(Win32WindowHandle::new(nz)))
    }

    struct Pane {
        window: String,
        view: Option<WebView>,
        error: Option<String>,
        visible: bool,
        loaded: String,
        /// this frame: (rect in points, rect in physical pixels, url)
        want: Option<(egui::Rect, egui::Rect, String)>,
        last_px: Option<egui::Rect>,
    }

    pub struct Browsers {
        panes: HashMap<String, Pane>,
        main: Option<RawWindowHandle>,
        blocked: HashSet<String>,
    }

    impl Browsers {
        pub fn new() -> Self {
            Browsers { panes: HashMap::new(), main: None, blocked: HashSet::new() }
        }

        pub fn set_main(&mut self, frame: &eframe::Frame) {
            if self.main.is_none() {
                if let Ok(h) = frame.window_handle() {
                    self.main = Some(h.as_raw());
                }
            }
        }

        pub fn start_frame(&mut self) {
            for p in self.panes.values_mut() {
                p.want = None;
            }
            self.blocked.clear();
        }

        pub fn mark(&mut self, key: &str, window: &str, rect: egui::Rect, ppp: f32, url: &str) {
            let px = egui::Rect::from_min_max(rect.min * ppp, rect.max * ppp);
            let p = self.panes.entry(key.to_string()).or_insert_with(|| Pane {
                window: window.to_string(),
                view: None,
                error: None,
                visible: false,
                loaded: String::new(),
                want: None,
                last_px: None,
            });
            p.window = window.to_string();
            p.want = Some((rect, px, url.to_string()));
        }

        pub fn rects(&self, window: &str) -> Vec<egui::Rect> {
            self.panes.values().filter(|p| p.window == window).filter_map(|p| p.want.as_ref().map(|w| w.0)).collect()
        }

        pub fn block(&mut self, window: &str) {
            self.blocked.insert(window.to_string());
        }

        pub fn finish(&mut self) {
            let main = self.main;
            for p in self.panes.values_mut() {
                let blocked = self.blocked.contains(&p.window);
                let want = p.want.clone().filter(|(_, px, _)| px.width() >= 80.0 && px.height() >= 60.0);
                // create the view the first time it is needed
                if p.view.is_none() && p.error.is_none() && want.is_some() && !blocked {
                    let parent = if p.window.is_empty() { main } else { find_window(&p.window) };
                    if let (Some(parent), Some((_, px, url))) = (parent, want.as_ref()) {
                        let built = WebViewBuilder::new()
                            .with_url(url)
                            .with_visible(false)
                            .with_focused(false) // never steal keyboard focus from egui
                            .with_background_color((241, 243, 247, 255)) // no black flash while a page loads
                            .with_bounds(px_rect(*px))
                            .build_as_child(&Parent(parent));
                        match built {
                            Ok(v) => {
                                p.view = Some(v);
                                p.loaded = url.clone();
                                p.last_px = Some(*px);
                            }
                            Err(e) => p.error = Some(e.to_string()),
                        }
                    }
                }
                let Some(v) = &p.view else { continue };
                let show = !blocked && want.is_some();
                if let Some((_, px, url)) = &want {
                    if *url != p.loaded && v.load_url(url).is_ok() {
                        p.loaded = url.clone();
                    }
                    if show && p.last_px != Some(*px) {
                        let _ = v.set_bounds(px_rect(*px));
                        p.last_px = Some(*px);
                    }
                }
                if p.visible != show {
                    let _ = v.set_visible(show);
                    p.visible = show;
                }
            }
        }

        pub fn error(&self, key: &str) -> Option<String> {
            self.panes.get(key).and_then(|p| p.error.clone())
        }

        /// Hand keyboard focus back to `window` (WebView2 keeps it after being clicked).
        pub fn focus_parent(&self, window: &str) {
            if let Some(v) = self.panes.values().filter(|p| p.window == window).find_map(|p| p.view.as_ref()) {
                let _ = v.focus_parent();
            }
        }

        pub fn back(&self, key: &str) {
            if let Some(v) = self.panes.get(key).and_then(|p| p.view.as_ref()) {
                let _ = v.evaluate_script("history.back()");
            }
        }
        pub fn forward(&self, key: &str) {
            if let Some(v) = self.panes.get(key).and_then(|p| p.view.as_ref()) {
                let _ = v.evaluate_script("history.forward()");
            }
        }
        pub fn reload(&self, key: &str) {
            if let Some(v) = self.panes.get(key).and_then(|p| p.view.as_ref()) {
                let _ = v.reload();
            }
        }

        /// An Additional window closed: destroy its web views.
        pub fn close_window(&mut self, window: &str) {
            self.panes.retain(|_, p| p.window != window);
        }
    }

    fn px_rect(r: egui::Rect) -> Rect {
        Rect {
            position: PhysicalPosition::new(r.min.x.round() as i32, r.min.y.round() as i32).into(),
            size: PhysicalSize::new(r.width().round().max(1.0) as u32, r.height().round().max(1.0) as u32).into(),
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod native {
    use super::*;
    /// Embedded pages need WebView2 (Windows). Elsewhere the panel offers "open in browser"; the
    /// rectangles are still tracked so overlap handling behaves the same.
    pub struct Browsers {
        rects: Vec<(String, egui::Rect)>,
    }
    impl Browsers {
        pub fn new() -> Self {
            Self { rects: vec![] }
        }
        pub fn set_main(&mut self, _: &eframe::Frame) {}
        pub fn start_frame(&mut self) {
            self.rects.clear();
        }
        pub fn mark(&mut self, _: &str, window: &str, rect: egui::Rect, _: f32, _: &str) {
            self.rects.push((window.to_string(), rect));
        }
        pub fn rects(&self, window: &str) -> Vec<egui::Rect> {
            self.rects.iter().filter(|(w, _)| w == window).map(|(_, r)| *r).collect()
        }
        pub fn block(&mut self, _: &str) {}
        pub fn finish(&mut self) {}
        pub fn error(&self, _: &str) -> Option<String> {
            Some("Embedded web pages need Windows with the Microsoft Edge WebView2 runtime.".into())
        }
        pub fn focus_parent(&self, _: &str) {}
        pub fn back(&self, _: &str) {}
        pub fn forward(&self, _: &str) {}
        pub fn reload(&self, _: &str) {}
        pub fn close_window(&mut self, _: &str) {}
    }
}
pub use native::Browsers;
