use uuid::Uuid;
use x11rb::{connection::Connection as _, xcb_ffi::XCBConnection};

use gpui::{Bounds, DisplayId, Pixels, PlatformDisplay, Size, px};
use crate::error::X11Error;

#[derive(Debug)]
pub(crate) struct X11Display {
    x_screen_index: usize,
    bounds: Bounds<Pixels>,
    uuid: Uuid,
}

impl X11Display {
    pub(crate) fn new(
        xcb: &XCBConnection,
        scale_factor: f32,
        x_screen_index: usize,
    ) -> Result<Self, X11Error> {
        let screen = xcb
            .setup()
            .roots
            .get(x_screen_index)
            .ok_or(X11Error::ScreenNotFound { screen_index: x_screen_index, total_screens: xcb.setup().roots.len() })?;
        Ok(Self {
            x_screen_index,
            bounds: Bounds {
                origin: Default::default(),
                size: Size {
                    width: px(screen.width_in_pixels as f32 / scale_factor),
                    height: px(screen.height_in_pixels as f32 / scale_factor),
                },
            },
            uuid: Uuid::from_bytes([0; 16]),
        })
    }
}

impl PlatformDisplay for X11Display {
    fn id(&self) -> DisplayId {
        DisplayId::new(self.x_screen_index as u64)
    }

    fn uuid(&self) -> gpui::platform::Result<Uuid> {
        Ok(self.uuid)
    }

    fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }
}
