//! Viewport PoC: wgpu renders offscreen, the pixels are read back and painted
//! by GPUI as an image. See `docs/design/viewport-rendering.md` §8.

mod stats;
mod viewport;

use gpui_kit::component::{ActiveTheme, StyledExt, h_flex, v_flex};
use gpui_kit::*;

use viewport::ViewportView;

actions!(viewport_poc, [Quit]);

struct Workbench {
    viewport: Entity<ViewportView>,
}

impl Workbench {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            viewport: cx.new(|cx| ViewportView::new(window, cx)),
        }
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let row = |label: &'static str| div().px_2().py_1().rounded(theme.radius).child(label);
        v_flex()
            .w_56()
            .flex_none()
            .p_2()
            .gap_1()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .text_sm()
            .child(
                div()
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("项目"),
            )
            .child(row("TiO₂ 相研究").font_medium())
            .child(row("合成负载").bg(theme.muted))
    }
}

impl Render for Workbench {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .items_stretch()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_sidebar(cx))
            .child(self.viewport.clone())
    }
}

fn main() {
    env_logger::init();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_window_closed(|cx, _| cx.quit()).detach();

            let options = WindowOptions {
                // The initial window size is a platform boundary, not layout.
                window_bounds: Some(WindowBounds::centered(size(px(1440.), px(900.)), cx)),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| Workbench::new(window, cx))
            })
            .expect("failed to open window");
            cx.activate(true);
        });
}
