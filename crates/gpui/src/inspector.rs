/// A unique identifier for an element that can be inspected.
#[derive(Debug, Eq, PartialEq, Hash, Clone)]
pub struct InspectorElementId {
    /// Stable part of the ID.
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub path: std::rc::Rc<InspectorElementPath>,
    /// Disambiguates elements that have the same path.
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub instance_id: usize,
}

impl Into<InspectorElementId> for &InspectorElementId {
    fn into(self) -> InspectorElementId {
        self.clone()
    }
}

#[cfg(any(feature = "inspector", debug_assertions))]
pub use conditional::*;

#[cfg(any(feature = "inspector", debug_assertions))]
mod capture;
#[cfg(any(feature = "inspector", debug_assertions))]
pub(crate) mod causes;
#[cfg(any(feature = "inspector", debug_assertions))]
pub(crate) mod describe;
#[cfg(any(feature = "inspector", debug_assertions))]
mod entities;
#[cfg(any(feature = "inspector", debug_assertions))]
mod input;
#[cfg(any(feature = "inspector", debug_assertions))]
mod keys;
#[cfg(any(feature = "inspector", debug_assertions))]
mod model;
#[cfg(any(feature = "inspector", debug_assertions))]
pub(crate) mod recorder;
#[cfg(all(test, any(feature = "inspector", debug_assertions)))]
mod tests;
pub(crate) mod user_span;
#[cfg(any(feature = "inspector", debug_assertions))]
pub use capture::*;
#[cfg(any(feature = "inspector", debug_assertions))]
pub use model::*;
pub use user_span::{SpanGuard, span};
#[cfg(any(feature = "inspector", debug_assertions))]
pub(crate) use {
    entities::live_entities,
    input::{InputInFlight, InputScope},
    keys::resolve_keystrokes,
};

#[cfg(any(feature = "inspector", debug_assertions))]
mod conditional {
    use super::*;
    use crate::{AnyElement, App, Context, Empty, IntoElement, Render, Window};
    use collections::{FxHashMap, TypeIdHashMap};
    use std::any::{Any, TypeId};

    /// `GlobalElementId` qualified by source location of element construction.
    #[derive(Debug, Eq, PartialEq, Hash)]
    pub struct InspectorElementPath {
        /// The path to the nearest ancestor element that has an `ElementId`.
        #[cfg(any(feature = "inspector", debug_assertions))]
        pub global_id: crate::GlobalElementId,
        /// Source location where this element was constructed.
        #[cfg(any(feature = "inspector", debug_assertions))]
        pub source_location: &'static std::panic::Location<'static>,
    }

    impl Clone for InspectorElementPath {
        fn clone(&self) -> Self {
            Self {
                global_id: self.global_id.clone(),
                source_location: self.source_location,
            }
        }
    }

    impl Into<InspectorElementPath> for &InspectorElementPath {
        fn into(self) -> InspectorElementPath {
            self.clone()
        }
    }

    /// Function set on `App` to render the inspector UI.
    pub type InspectorRenderer =
        Box<dyn Fn(&mut Inspector, &mut Window, &mut Context<Inspector>) -> AnyElement>;

    /// The inspector of one window: owns the inspector UI's state and emits
    /// [`InspectorEvent`]s while the user picks elements. Selection, picking
    /// and overlays live in the window's [`InspectorCapture`].
    pub struct Inspector {
        active_element: Option<InspectedElement>,
        ui_state: Option<Box<dyn Any>>,
    }

    impl crate::EventEmitter<InspectorEvent> for Inspector {}

    struct InspectedElement {
        id: InspectorElementId,
        states: TypeIdHashMap<Box<dyn Any>>,
    }

    impl InspectedElement {
        fn new(id: InspectorElementId) -> Self {
            InspectedElement {
                id,
                states: Default::default(),
            }
        }
    }

    impl Inspector {
        pub(crate) fn new() -> Self {
            Self {
                active_element: None,
                ui_state: None,
            }
        }

        /// State owned by the inspector UI for this window, created by `init`
        /// on first use. Lives exactly as long as the inspector stays open.
        ///
        /// Panics if a previous call created the state with a different type.
        pub fn ui_state<T: 'static>(&mut self, init: impl FnOnce() -> T) -> &mut T {
            self.ui_state
                .get_or_insert_with(|| Box::new(init()))
                .downcast_mut()
                .expect("inspector UI state has a different type")
        }

        /// Makes `id` the element whose registered inspector states are kept,
        /// dropping the states of the previous one.
        pub(crate) fn set_active_element_id(&mut self, id: &InspectorElementId) {
            if Some(id) != self.active_element_id() {
                self.active_element = Some(InspectedElement::new(id.clone()));
            }
        }

        /// ID of the selected element, once it has been drawn with inspector
        /// states (see [`Window::with_inspector_state`]).
        pub fn active_element_id(&self) -> Option<&InspectorElementId> {
            self.active_element.as_ref().map(|e| &e.id)
        }

        pub(crate) fn with_active_element_state<T: 'static, R>(
            &mut self,
            window: &mut Window,
            f: impl FnOnce(&mut Option<T>, &mut Window) -> R,
        ) -> R {
            let Some(active_element) = &mut self.active_element else {
                return f(&mut None, window);
            };

            let type_id = TypeId::of::<T>();
            let mut inspector_state = active_element
                .states
                .remove(&type_id)
                .map(|state| *state.downcast().unwrap());

            let result = f(&mut inspector_state, window);

            if let Some(inspector_state) = inspector_state {
                active_element
                    .states
                    .insert(type_id, Box::new(inspector_state));
            }

            result
        }

        /// Renders elements for all registered inspector states of the active inspector element.
        pub fn render_inspector_states(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> Vec<AnyElement> {
            let mut elements = Vec::new();
            if let Some(active_element) = self.active_element.take() {
                for (type_id, state) in &active_element.states {
                    if let Some(render_inspector) = cx
                        .inspector_element_registry
                        .renderers_by_type_id
                        .remove(type_id)
                    {
                        let mut element = (render_inspector)(
                            active_element.id.clone(),
                            state.as_ref(),
                            window,
                            cx,
                        );
                        elements.push(element);
                        cx.inspector_element_registry
                            .renderers_by_type_id
                            .insert(*type_id, render_inspector);
                    }
                }

                self.active_element = Some(active_element);
            }

            elements
        }
    }

    impl Render for Inspector {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if let Some(inspector_renderer) = cx.inspector_renderer.take() {
                let result = inspector_renderer(self, window, cx);
                cx.inspector_renderer = Some(inspector_renderer);
                result
            } else {
                Empty.into_any_element()
            }
        }
    }

    #[derive(Default)]
    pub(crate) struct InspectorElementRegistry {
        renderers_by_type_id: FxHashMap<
            TypeId,
            Box<dyn Fn(InspectorElementId, &dyn Any, &mut Window, &mut App) -> AnyElement>,
        >,
    }

    impl InspectorElementRegistry {
        pub fn register<T: 'static, R: IntoElement>(
            &mut self,
            f: impl 'static + Fn(InspectorElementId, &T, &mut Window, &mut App) -> R,
        ) {
            self.renderers_by_type_id.insert(
                TypeId::of::<T>(),
                Box::new(move |id, value, window, cx| {
                    let value = value.downcast_ref().unwrap();
                    f(id, value, window, cx).into_any_element()
                }),
            );
        }
    }
}

/// Provides definitions used by `#[derive_inspector_reflection]`.
#[cfg(any(feature = "inspector", debug_assertions))]
pub mod inspector_reflection {
    use std::any::Any;

    /// Reification of a function that has the signature `fn some_fn(T) -> T`. Provides the name,
    /// documentation, and ability to invoke the function.
    #[derive(Clone, Copy)]
    pub struct FunctionReflection<T> {
        /// The name of the function
        pub name: &'static str,
        /// The method
        pub function: fn(Box<dyn Any>) -> Box<dyn Any>,
        /// Documentation for the function
        pub documentation: Option<&'static str>,
        /// `PhantomData` for the type of the argument and result
        pub _type: std::marker::PhantomData<T>,
    }

    impl<T: 'static> FunctionReflection<T> {
        /// Invoke this method on a value and return the result.
        pub fn invoke(&self, value: T) -> T {
            let boxed = Box::new(value) as Box<dyn Any>;
            let result = (self.function)(boxed);
            *result
                .downcast::<T>()
                .expect("Type mismatch in reflection invoke")
        }
    }
}
