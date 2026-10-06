use refineable::{Cascade, Refineable};

#[derive(Clone, Default, Refineable)]
pub struct Style {
    pub size: u32,
}

pub fn update_base_style() {
    let mut cascade = Cascade::<Style>::default();
    cascade.base().size = Some(42);
    let _merged = cascade.merged();
}
