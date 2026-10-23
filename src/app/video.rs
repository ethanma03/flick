//! The video surface: mpv renders into an offscreen framebuffer, which is blitted
//! into egui's frame inside a paint callback. Overlays painted after the callback
//! (controls, toasts) land on top, and the framebuffer doubles as a frame cache:
//! UI-only repaints (hover, fades) re-blit without asking mpv to render again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::glow::{self, HasContext};

use crate::mpv::Renderer;

struct Target {
    fbo: glow::Framebuffer,
    tex: glow::Texture,
    size: [i32; 2],
}

pub struct Surface {
    renderer: Option<Renderer>,
    target: Option<Target>,
    /// Set by mpv's update callback (any thread): a new frame is ready.
    dirty: Arc<AtomicBool>,
    /// A frame has been rendered into `target` since the last resize.
    has_frame: bool,
}

pub type SharedSurface = Arc<Mutex<Surface>>;

impl Surface {
    pub fn new(renderer: Renderer, dirty: Arc<AtomicBool>) -> SharedSurface {
        Arc::new(Mutex::new(Self { renderer: Some(renderer), target: None, dirty, has_frame: false }))
    }

    /// Forget the cached frame; the next paint clears and renders afresh.
    pub fn invalidate(&mut self) {
        self.has_frame = false;
    }

    /// Release mpv's renderer and our GL objects. Must run with the GL context current,
    /// before the mpv handle itself is destroyed.
    pub fn destroy(&mut self, gl: &glow::Context) {
        self.renderer = None;
        if let Some(t) = self.target.take() {
            unsafe {
                gl.delete_framebuffer(t.fbo);
                gl.delete_texture(t.tex);
            }
        }
    }

    fn ensure_target(&mut self, gl: &glow::Context, size: [i32; 2]) -> Option<&Target> {
        if self.target.as_ref().is_some_and(|t| t.size != size) {
            let t = self.target.take().unwrap();
            unsafe {
                gl.delete_framebuffer(t.fbo);
                gl.delete_texture(t.tex);
            }
        }
        if self.target.is_none() {
            unsafe {
                let tex = gl.create_texture().ok()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    size[0],
                    size[1],
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                gl.bind_texture(glow::TEXTURE_2D, None);
                let fbo = gl.create_framebuffer().ok()?;
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                self.target = Some(Target { fbo, tex, size });
            }
            self.has_frame = false;
        }
        self.target.as_ref()
    }

    /// Paint-callback body: render a new frame if mpv has one (or the size changed),
    /// then copy the cached frame into the window at `viewport` (GL pixel coords, origin bottom-left).
    fn paint(&mut self, gl: &glow::Context, viewport: [i32; 4], clip: [i32; 4], bg: [f32; 3]) {
        let [x, y, w, h] = viewport;
        if w <= 0 || h <= 0 || self.renderer.is_none() {
            return;
        }
        let fresh = self.dirty.swap(false, Ordering::AcqRel);
        let wants = fresh && self.renderer.as_ref().is_some_and(|r| r.wants_frame());
        let resized = self.target.as_ref().is_none_or(|t| t.size != [w, h]);
        let Some(fbo) = self.ensure_target(gl, [w, h]).map(|t| t.fbo) else { return };
        unsafe {
            if wants || resized || !self.has_frame {
                gl.disable(glow::SCISSOR_TEST);
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.viewport(0, 0, w, h);
                if !self.has_frame {
                    gl.clear_color(bg[0], bg[1], bg[2], 1.0);
                    gl.clear(glow::COLOR_BUFFER_BIT);
                }
                if let Some(r) = &self.renderer {
                    r.render(fbo.0.get(), w, h, true);
                }
                self.has_frame = true;
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            }
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(clip[0], clip[1], clip[2], clip[3]);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(fbo));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
            gl.blit_framebuffer(0, 0, w, h, x, y, x + w, y + h, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        if let Some(r) = &self.renderer {
            r.report_swap();
        }
    }
}

/// Add the video to `painter` at `rect`.
pub fn paint(painter: &egui::Painter, rect: egui::Rect, surface: &SharedSurface, bg: egui::Color32) {
    let surface = surface.clone();
    let bg = egui::Rgba::from(bg);
    let bg = [bg.r(), bg.g(), bg.b()];
    let cb = eframe::egui_glow::CallbackFn::new(move |info, painter| {
        let v = info.viewport_in_pixels();
        let c = info.clip_rect_in_pixels();
        if let Ok(mut s) = surface.lock() {
            s.paint(
                painter.gl(),
                [v.left_px, v.from_bottom_px, v.width_px, v.height_px],
                [c.left_px, c.from_bottom_px, c.width_px, c.height_px],
                bg,
            );
        }
    });
    painter.add(egui::PaintCallback { rect, callback: Arc::new(cb) });
}
