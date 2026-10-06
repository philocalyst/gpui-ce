use refineable::{Cascade, Refineable};

#[derive(Clone, Default, Refineable)]
pub struct Style {
    pub size: u32,
}

pub fn reserve_style_slot() {
    let mut cascade = Cascade::<Style>::default();
    let _slot = cascade.reserve();
}
