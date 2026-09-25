use gpui::{
    AppContext as _, Context, HeadlessAppContext, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, px, rgb, size,
};
use gpui_wgpu::{CosmicTextSystem, WgpuHeadlessRenderer};
use std::sync::Arc;

struct App;

impl Render for App {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0xffffff))
            .child(div().m_4().p_2().bg(rgb(0x3366ff)).child("Hello"))
    }
}

#[test]
fn loupe_docks_and_renders() {
    let text = Arc::new(CosmicTextSystem::new_without_system_fonts("IBM Plex Sans"));
    let mut cx = HeadlessAppContext::with_platform(text, Arc::new(()), || {
        WgpuHeadlessRenderer::new()
            .ok()
            .map(|renderer| Box::new(renderer) as Box<dyn gpui::PlatformHeadlessRenderer>)
    });
    cx.update(gpui_inspector::init);
    let window = cx
        .open_window(size(px(1000.), px(600.)), |_, cx| cx.new(|_| App))
        .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.toggle_inspector(cx);
        window.draw(cx);
    })
    .unwrap();
    cx.run_until_parked();
    let image = cx.capture_screenshot(window.into()).unwrap();
    let out = std::env::var("LOUPE_SHOTS").unwrap_or_else(|_| "target/loupe-shots".into());
    std::fs::create_dir_all(&out).unwrap();
    image.save(format!("{out}/smoke.png")).unwrap();
    // The dock is on the right: its background differs from the app's white.
    let scale = image.width() / 1000;
    let app_pixel = image.get_pixel(100 * scale, 300 * scale);
    let dock_pixel = image.get_pixel(900 * scale, 300 * scale);
    assert_eq!(app_pixel.0, [255, 255, 255, 255]);
    assert_eq!(dock_pixel.0, [0x16, 0x17, 0x1a, 255]);
    cx.update_window(window.into(), |_, window, _| {
        assert_eq!(window.app_bounds().size.width, px(440.));
        assert!(window.inspector_capture().is_some());
    })
    .unwrap();
}
