use std::{
    fmt::Debug,
    hash::{Hash, Hasher},
};

use uuid::Uuid;
use wayland_backend::client::ObjectId;

use gpui::{Bounds, DisplayId, Pixels, PlatformDisplay};
use crate::error::WaylandError;

#[derive(Debug, Clone)]
pub(crate) struct WaylandDisplay {
    /// The ID of the wl_output object
    pub id: ObjectId,
    pub name: Option<String>,
    pub bounds: Bounds<Pixels>,
}

impl Hash for WaylandDisplay {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl PlatformDisplay for WaylandDisplay {
    fn id(&self) -> DisplayId {
        DisplayId::new(self.id.protocol_id() as u64)
    }

    fn uuid(&self) -> gpui::platform::Result<Uuid> {
        let name = self
            .name
            .as_ref()
            .ok_or_else(|| WaylandError::Display { id: format!("{:?}", self.id) })?;
        Ok(Uuid::new_v5(&Uuid::NAMESPACE_DNS, name.as_bytes()))
    }

    fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }
}
