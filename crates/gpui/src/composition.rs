//! Typed identities and the compact preorder tree for composed windows.

use anyhow::{Result, bail};
use smallvec::SmallVec;
use std::{
    num::NonZeroU64,
    rc::Rc,
    slice,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    Bounds, CompositionContent, CompositionSurface, DevicePixels, PlatformSurfaceAttachment,
    PlatformSurfaceHandle,
};

static NEXT_SURFACE_ID: AtomicU64 = AtomicU64::new(1);

fn next_surface_id() -> NonZeroU64 {
    NEXT_SURFACE_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .ok()
        .and_then(NonZeroU64::new)
        .expect("GPUI composition surface ID space exhausted")
}

macro_rules! surface_id {
    ($name:ident, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(NonZeroU64);

        impl $name {
            pub(crate) fn fresh() -> Self {
                Self(next_surface_id())
            }
        }
    };
}

surface_id!(
    GpuiSurfaceId,
    "Identity of a GPUI-rendered composition surface."
);
surface_id!(
    NativeSurfaceId,
    "Identity of a platform-native composition surface."
);
surface_id!(
    ExternalGpuSurfaceId,
    "Identity of an externally rendered composition surface."
);

/// Identity of any surface in a window's composition tree.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CompositionSurfaceId {
    /// A GPUI-rendered surface.
    Gpui(GpuiSurfaceId),
    /// A platform-native surface.
    Native(NativeSurfaceId),
    /// A surface rendered by an external GPU producer.
    ExternalGpu(ExternalGpuSurfaceId),
}

impl From<GpuiSurfaceId> for CompositionSurfaceId {
    fn from(value: GpuiSurfaceId) -> Self {
        Self::Gpui(value)
    }
}

impl From<NativeSurfaceId> for CompositionSurfaceId {
    fn from(value: NativeSurfaceId) -> Self {
        Self::Native(value)
    }
}

impl From<ExternalGpuSurfaceId> for CompositionSurfaceId {
    fn from(value: ExternalGpuSurfaceId) -> Self {
        Self::ExternalGpu(value)
    }
}

/// The role of a GPUI-rendered node in the composition tree.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GpuiSurfaceRole {
    /// The main GPUI scene, below native child surfaces.
    Base,
    /// Window overlays, above native child surfaces.
    Overlay,
    /// An application-created GPUI surface.
    Additional,
}

/// A platform surface handle paired with its typed identity.
#[derive(Clone)]
pub struct AttachedSurface<Id> {
    id: Id,
    attachment: Rc<dyn PlatformSurfaceAttachment>,
}

impl<Id> AttachedSurface<Id> {
    pub(crate) fn new(id: Id, attachment: Rc<dyn PlatformSurfaceAttachment>) -> Self {
        Self { id, attachment }
    }

    /// Returns a handle that can host or connect platform-owned content.
    pub fn platform_handle(&self) -> PlatformSurfaceHandle<'_> {
        self.attachment.platform_handle()
    }

    /// Registers for native-handle replacement after compositor recovery.
    pub fn on_handle_changed(
        &self,
        callback: Rc<dyn for<'a> Fn(PlatformSurfaceHandle<'a>)>,
    ) -> crate::Result<()> {
        self.attachment.on_handle_changed(callback)
    }
}

impl<Id: Copy> AttachedSurface<Id> {
    /// Returns the identity used to position or remove this surface.
    pub fn id(&self) -> Id {
        self.id
    }
}

/// A native surface slot created by a platform window.
pub type NativeSurface = AttachedSurface<NativeSurfaceId>;

/// A composition slot backed by an external GPU producer.
pub type ExternalGpuSurface = AttachedSurface<ExternalGpuSurfaceId>;

enum SurfaceKind {
    Gpui {
        id: GpuiSurfaceId,
        role: GpuiSurfaceRole,
    },
    Native {
        id: NativeSurfaceId,
        bounds: Bounds<DevicePixels>,
        attachment: Rc<dyn PlatformSurfaceAttachment>,
    },
    ExternalGpu {
        id: ExternalGpuSurfaceId,
        bounds: Bounds<DevicePixels>,
        attachment: Rc<dyn PlatformSurfaceAttachment>,
    },
}

impl SurfaceKind {
    fn id(&self) -> CompositionSurfaceId {
        self.content().id()
    }

    fn content(&self) -> CompositionContent<'_> {
        match self {
            Self::Gpui { id, role } => CompositionContent::Gpui {
                id: *id,
                role: *role,
            },
            Self::Native {
                id,
                bounds,
                attachment,
            } => CompositionContent::Native {
                id: *id,
                bounds: *bounds,
                attachment,
            },
            Self::ExternalGpu {
                id,
                bounds,
                attachment,
            } => CompositionContent::ExternalGpu {
                id: *id,
                bounds: *bounds,
                attachment,
            },
        }
    }

    fn is_fixed(&self) -> bool {
        matches!(
            self,
            Self::Gpui {
                role: GpuiSurfaceRole::Base | GpuiSurfaceRole::Overlay,
                ..
            }
        )
    }

    fn gpui_id(&self) -> Option<GpuiSurfaceId> {
        match self {
            Self::Gpui { id, .. } => Some(*id),
            _ => None,
        }
    }
}

struct SurfaceEntry {
    depth: usize,
    kind: SurfaceKind,
}

/// A borrowed, bottom-to-top view of a window's composition surfaces.
///
/// Each entry has a unique typed ID that matches its content kind, and each parent appears before
/// its children. Native and external bounds have nonnegative dimensions and representable ends.
/// The fixed GPUI base remains the first root and the fixed overlay remains the last root; child
/// surfaces may be nested under either.
#[derive(Clone, Copy)]
pub struct CompositionSurfaces<'a> {
    entries: &'a [SurfaceEntry],
}

impl<'a> CompositionSurfaces<'a> {
    /// Iterates the ordered surfaces without allocating a per-frame snapshot.
    pub fn iter(self) -> CompositionSurfaceIter<'a> {
        CompositionSurfaceIter {
            entries: self.entries.iter(),
            ancestors: SmallVec::new(),
        }
    }

    /// Returns the number of composition surfaces.
    pub fn len(self) -> usize {
        self.entries.len()
    }

    /// Returns whether the window has no composition surfaces.
    pub fn is_empty(self) -> bool {
        self.entries.is_empty()
    }
}

impl<'a> IntoIterator for CompositionSurfaces<'a> {
    type Item = CompositionSurface<'a>;
    type IntoIter = CompositionSurfaceIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Iterates a [`CompositionSurfaces`] view and derives parentage from preorder depth.
pub struct CompositionSurfaceIter<'a> {
    entries: slice::Iter<'a, SurfaceEntry>,
    ancestors: SmallVec<[&'a SurfaceEntry; 8]>,
}

impl<'a> Iterator for CompositionSurfaceIter<'a> {
    type Item = CompositionSurface<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.entries.next()?;
        self.ancestors.truncate(entry.depth);
        debug_assert_eq!(self.ancestors.len(), entry.depth);
        let content = entry.kind.content();
        let parent_entry = self.ancestors.last().copied();
        let parent = parent_entry.map(|entry| entry.kind.id());
        let parent_bounds = parent_entry.and_then(|entry| entry.kind.content().bounds());
        self.ancestors.push(entry);
        Some(CompositionSurface {
            parent,
            content,
            parent_bounds,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.entries.size_hint()
    }
}

impl ExactSizeIterator for CompositionSurfaceIter<'_> {}

/// A preorder sequence is enough to encode both sibling order and nesting: each subtree occupies
/// one contiguous range, so moving or removing it needs no parent/children index to keep in sync.
pub(crate) struct CompositionTree {
    surfaces: Vec<SurfaceEntry>,
}

impl CompositionTree {
    pub(crate) fn new() -> Self {
        let base = GpuiSurfaceId::fresh();
        let overlay = GpuiSurfaceId::fresh();
        Self {
            surfaces: vec![
                SurfaceEntry {
                    depth: 0,
                    kind: SurfaceKind::Gpui {
                        id: base,
                        role: GpuiSurfaceRole::Base,
                    },
                },
                SurfaceEntry {
                    depth: 0,
                    kind: SurfaceKind::Gpui {
                        id: overlay,
                        role: GpuiSurfaceRole::Overlay,
                    },
                },
            ],
        }
    }

    pub(crate) fn base(&self) -> GpuiSurfaceId {
        match &self
            .surfaces
            .first()
            .expect("base surface is permanent")
            .kind
        {
            SurfaceKind::Gpui {
                id,
                role: GpuiSurfaceRole::Base,
            } => *id,
            _ => unreachable!("base remains the first preorder entry"),
        }
    }

    pub(crate) fn overlay(&self) -> GpuiSurfaceId {
        let entry = self
            .surfaces
            .iter()
            .rfind(|entry| entry.depth == 0)
            .expect("overlay surface is permanent");
        match &entry.kind {
            SurfaceKind::Gpui {
                id,
                role: GpuiSurfaceRole::Overlay,
            } => *id,
            _ => unreachable!("overlay remains the last root surface"),
        }
    }

    pub(crate) fn contains_gpui(&self, surface: GpuiSurfaceId) -> bool {
        self.surfaces
            .iter()
            .any(|entry| entry.kind.gpui_id() == Some(surface))
    }

    fn index(&self, surface: CompositionSurfaceId) -> Result<usize> {
        self.surfaces
            .iter()
            .position(|entry| entry.kind.id() == surface)
            .ok_or_else(|| anyhow::anyhow!("composition surface does not exist in this window"))
    }

    fn parent_index(&self, index: usize) -> Option<usize> {
        let depth = self.surfaces[index].depth;
        if depth == 0 {
            return None;
        }
        (0..index)
            .rev()
            .find(|candidate| self.surfaces[*candidate].depth + 1 == depth)
    }

    fn parent(&self, index: usize) -> Option<CompositionSurfaceId> {
        self.parent_index(index)
            .map(|parent| self.surfaces[parent].kind.id())
    }

    fn subtree_end(&self, index: usize) -> usize {
        let depth = self.surfaces[index].depth;
        (index + 1..self.surfaces.len())
            .find(|candidate| self.surfaces[*candidate].depth <= depth)
            .unwrap_or(self.surfaces.len())
    }

    pub(crate) fn validate_parent(
        &self,
        parent: Option<CompositionSurfaceId>,
    ) -> Result<Option<usize>> {
        parent.map(|parent| self.index(parent)).transpose()
    }

    pub(crate) fn validate_bounds(bounds: Bounds<DevicePixels>) -> Result<()> {
        anyhow::ensure!(
            bounds.size.width.0 >= 0 && bounds.size.height.0 >= 0,
            "composition surface dimensions cannot be negative"
        );
        bounds
            .origin
            .x
            .0
            .checked_add(bounds.size.width.0)
            .ok_or_else(|| anyhow::anyhow!("composition surface horizontal bounds overflow"))?;
        bounds
            .origin
            .y
            .0
            .checked_add(bounds.size.height.0)
            .ok_or_else(|| anyhow::anyhow!("composition surface vertical bounds overflow"))?;
        Ok(())
    }

    pub(crate) fn parent_of(
        &self,
        surface: CompositionSurfaceId,
    ) -> Result<Option<CompositionSurfaceId>> {
        let index = self.index(surface)?;
        Ok(self.parent(index))
    }

    pub(crate) fn children_of(
        &self,
        parent: Option<CompositionSurfaceId>,
    ) -> Result<Vec<CompositionSurfaceId>> {
        let parent_index = self.validate_parent(parent)?;
        let (depth, mut index, end) = match parent_index {
            Some(index) => (
                self.surfaces[index].depth + 1,
                index + 1,
                self.subtree_end(index),
            ),
            None => (0, 0, self.surfaces.len()),
        };
        let mut children = Vec::new();
        while index < end {
            let entry = &self.surfaces[index];
            if entry.depth == depth {
                children.push(entry.kind.id());
                index = self.subtree_end(index);
            } else {
                index += 1;
            }
        }
        Ok(children)
    }

    pub(crate) fn insert_gpui(
        &mut self,
        id: GpuiSurfaceId,
        role: GpuiSurfaceRole,
        parent: Option<CompositionSurfaceId>,
    ) -> Result<()> {
        self.insert(SurfaceKind::Gpui { id, role }, parent)
    }

    pub(crate) fn insert_native(
        &mut self,
        id: NativeSurfaceId,
        bounds: Bounds<DevicePixels>,
        attachment: Rc<dyn PlatformSurfaceAttachment>,
        parent: Option<CompositionSurfaceId>,
    ) -> Result<()> {
        Self::validate_bounds(bounds)?;
        self.insert(
            SurfaceKind::Native {
                id,
                bounds,
                attachment,
            },
            parent,
        )
    }

    pub(crate) fn insert_external_gpu(
        &mut self,
        id: ExternalGpuSurfaceId,
        bounds: Bounds<DevicePixels>,
        attachment: Rc<dyn PlatformSurfaceAttachment>,
        parent: Option<CompositionSurfaceId>,
    ) -> Result<()> {
        Self::validate_bounds(bounds)?;
        self.insert(
            SurfaceKind::ExternalGpu {
                id,
                bounds,
                attachment,
            },
            parent,
        )
    }

    fn insert(&mut self, kind: SurfaceKind, parent: Option<CompositionSurfaceId>) -> Result<()> {
        let parent_index = self.validate_parent(parent)?;
        let (depth, index) = if let Some(parent_index) = parent_index {
            (
                self.surfaces[parent_index].depth + 1,
                self.subtree_end(parent_index),
            )
        } else {
            (0, self.index(self.overlay().into())?)
        };
        self.surfaces.insert(index, SurfaceEntry { depth, kind });
        Ok(())
    }

    pub(crate) fn set_bounds(
        &mut self,
        surface: CompositionSurfaceId,
        bounds: Bounds<DevicePixels>,
    ) -> Result<bool> {
        Self::validate_bounds(bounds)?;
        let index = self.index(surface)?;
        let entry = &mut self.surfaces[index];
        match &mut entry.kind {
            SurfaceKind::Gpui { .. } => bail!("GPUI surfaces use the full window bounds"),
            SurfaceKind::Native {
                bounds: current, ..
            }
            | SurfaceKind::ExternalGpu {
                bounds: current, ..
            } => {
                if *current == bounds {
                    return Ok(false);
                }
                *current = bounds;
            }
        }
        Ok(true)
    }

    pub(crate) fn remove(&mut self, surface: CompositionSurfaceId) -> Result<()> {
        let index = self.index(surface)?;
        if self.surfaces[index].kind.is_fixed() {
            bail!("the default GPUI surfaces cannot be removed");
        }
        let end = self.subtree_end(index);
        self.surfaces.remove(index);
        for child in &mut self.surfaces[index..end - 1] {
            child.depth -= 1;
        }
        Ok(())
    }

    pub(crate) fn reparent(
        &mut self,
        surface: CompositionSurfaceId,
        parent: Option<CompositionSurfaceId>,
    ) -> Result<bool> {
        let start = self.index(surface)?;
        if self.surfaces[start].kind.is_fixed() {
            bail!("the default GPUI surfaces cannot be reparented");
        }
        let end = self.subtree_end(start);
        let parent_index = self.validate_parent(parent)?;
        if parent_index.is_some_and(|index| (start..end).contains(&index)) {
            bail!("composition surfaces cannot contain themselves");
        }
        if self.parent(start) == parent {
            return Ok(false);
        }

        let old_depth = self.surfaces[start].depth;
        let (new_depth, destination) = if let Some(parent_index) = parent_index {
            (
                self.surfaces[parent_index].depth + 1,
                self.subtree_end(parent_index),
            )
        } else {
            (0, self.index(self.overlay().into())?)
        };
        let len = end - start;
        let moved_to = self.rotate_subtree(start, end, destination);
        self.shift_depth(moved_to..moved_to + len, old_depth, new_depth);
        Ok(true)
    }

    pub(crate) fn place_relative(
        &mut self,
        surface: CompositionSurfaceId,
        sibling: CompositionSurfaceId,
        above: bool,
    ) -> Result<bool> {
        if surface == sibling {
            bail!("a composition surface cannot be ordered relative to itself");
        }
        let start = self.index(surface)?;
        let sibling_index = self.index(sibling)?;
        if self.surfaces[start].kind.is_fixed() {
            bail!("the default GPUI surfaces cannot be reordered");
        }
        if self.parent(start) != self.parent(sibling_index) {
            bail!("composition surfaces must share a parent to be reordered");
        }
        if above && sibling == CompositionSurfaceId::Gpui(self.overlay()) {
            bail!("the GPUI overlay must remain the topmost root surface");
        }
        if !above && sibling == CompositionSurfaceId::Gpui(self.base()) {
            bail!("the GPUI base must remain the bottommost root surface");
        }

        let end = self.subtree_end(start);
        if (start..end).contains(&sibling_index) {
            bail!("a composition surface cannot be ordered relative to its child");
        }
        let sibling_end = self.subtree_end(sibling_index);
        if (above && sibling_end == start) || (!above && end == sibling_index) {
            return Ok(false);
        }
        let destination = if above {
            self.subtree_end(sibling_index)
        } else {
            sibling_index
        };
        self.rotate_subtree(start, end, destination);
        Ok(true)
    }

    /// Moves one contiguous preorder subtree to a gap measured before mutation.
    fn rotate_subtree(&mut self, start: usize, end: usize, destination: usize) -> usize {
        debug_assert!(destination <= self.surfaces.len());
        debug_assert!(destination <= start || destination >= end);
        let len = end - start;
        if destination < start {
            self.surfaces[destination..end].rotate_right(len);
            destination
        } else if destination > end {
            self.surfaces[start..destination].rotate_left(len);
            destination - len
        } else {
            start
        }
    }

    fn shift_depth(&mut self, range: std::ops::Range<usize>, old: usize, new: usize) {
        for entry in &mut self.surfaces[range] {
            entry.depth = entry.depth - old + new;
        }
    }

    pub(crate) fn surfaces(&self) -> CompositionSurfaces<'_> {
        CompositionSurfaces {
            entries: &self.surfaces,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompositionSurfaceId as Id, point, size};

    struct NoPlatformHandle;

    unsafe impl PlatformSurfaceAttachment for NoPlatformHandle {
        fn platform_handle(&self) -> PlatformSurfaceHandle<'_> {
            panic!("test attachment has no platform handle")
        }
    }

    fn bounds(x: i32, y: i32) -> Bounds<DevicePixels> {
        Bounds {
            origin: point(DevicePixels(x), DevicePixels(y)),
            size: size(DevicePixels(30), DevicePixels(20)),
        }
    }

    fn insert_gpui(tree: &mut CompositionTree, parent: Option<Id>) -> crate::GpuiSurfaceId {
        let id = crate::GpuiSurfaceId::fresh();
        tree.insert_gpui(id, GpuiSurfaceRole::Additional, parent)
            .unwrap();
        id
    }

    fn order(tree: &CompositionTree) -> Vec<Id> {
        tree.surfaces().iter().map(|surface| surface.id()).collect()
    }

    #[derive(Clone)]
    struct ReferenceNode {
        id: Id,
        children: Vec<ReferenceNode>,
    }

    impl ReferenceNode {
        fn leaf(id: Id) -> Self {
            Self {
                id,
                children: Vec::new(),
            }
        }
    }

    fn reference_parent(nodes: &[ReferenceNode], id: Id) -> Option<Option<Id>> {
        fn search(nodes: &[ReferenceNode], id: Id, parent: Option<Id>) -> Option<Option<Id>> {
            for node in nodes {
                if node.id == id {
                    return Some(parent);
                }
                if let Some(found) = search(&node.children, id, Some(node.id)) {
                    return Some(found);
                }
            }
            None
        }
        search(nodes, id, None)
    }

    fn reference_contains(node: &ReferenceNode, id: Id) -> bool {
        node.id == id
            || node
                .children
                .iter()
                .any(|child| reference_contains(child, id))
    }

    fn reference_node(nodes: &[ReferenceNode], id: Id) -> Option<&ReferenceNode> {
        for node in nodes {
            if node.id == id {
                return Some(node);
            }
            if let Some(found) = reference_node(&node.children, id) {
                return Some(found);
            }
        }
        None
    }

    fn reference_take(nodes: &mut Vec<ReferenceNode>, id: Id) -> Option<ReferenceNode> {
        for index in 0..nodes.len() {
            if nodes[index].id == id {
                return Some(nodes.remove(index));
            }
            if let Some(found) = reference_take(&mut nodes[index].children, id) {
                return Some(found);
            }
        }
        None
    }

    fn reference_node_mut(nodes: &mut [ReferenceNode], id: Id) -> Option<&mut ReferenceNode> {
        for node in nodes {
            if node.id == id {
                return Some(node);
            }
            if let Some(found) = reference_node_mut(&mut node.children, id) {
                return Some(found);
            }
        }
        None
    }

    fn reference_children_mut(
        roots: &mut Vec<ReferenceNode>,
        parent: Option<Id>,
    ) -> Option<&mut Vec<ReferenceNode>> {
        match parent {
            Some(parent) => reference_node_mut(roots, parent).map(|node| &mut node.children),
            None => Some(roots),
        }
    }

    fn reference_reparent(
        roots: &mut Vec<ReferenceNode>,
        surface: Id,
        parent: Option<Id>,
        fixed: [Id; 2],
        overlay: Id,
    ) -> Result<bool, ()> {
        let Some(old_parent) = reference_parent(roots, surface) else {
            return Err(());
        };
        if fixed.contains(&surface) {
            return Err(());
        }
        let destination = if let Some(parent) = parent {
            let Some(source_node) = reference_node(roots, surface) else {
                return Err(());
            };
            if reference_contains(source_node, parent) {
                return Err(());
            }
            let Some(parent_node) = reference_node_mut(roots, parent) else {
                return Err(());
            };
            Some(parent_node.children.len())
        } else {
            Some(roots.iter().position(|node| node.id == overlay).ok_or(())?)
        };
        if old_parent == parent {
            return Ok(false);
        }

        let node = reference_take(roots, surface).ok_or(())?;
        let children = reference_children_mut(roots, parent).ok_or(())?;
        let index = destination.ok_or(())?.min(children.len());
        children.insert(index, node);
        Ok(true)
    }

    fn reference_place_relative(
        roots: &mut Vec<ReferenceNode>,
        surface: Id,
        sibling: Id,
        above: bool,
        fixed: [Id; 2],
        base: Id,
        overlay: Id,
    ) -> Result<bool, ()> {
        if surface == sibling || fixed.contains(&surface) {
            return Err(());
        }
        let Some(parent) = reference_parent(roots, surface) else {
            return Err(());
        };
        if reference_parent(roots, sibling) != Some(parent) {
            return Err(());
        }
        if parent.is_none() && ((above && sibling == overlay) || (!above && sibling == base)) {
            return Err(());
        }
        let source = reference_node_mut(roots, surface).ok_or(())?;
        if reference_contains(source, sibling) {
            return Err(());
        }

        let children = reference_children_mut(roots, parent).ok_or(())?;
        let source_index = children
            .iter()
            .position(|node| node.id == surface)
            .ok_or(())?;
        let sibling_index = children
            .iter()
            .position(|node| node.id == sibling)
            .ok_or(())?;
        if (above && sibling_index + 1 == source_index)
            || (!above && source_index + 1 == sibling_index)
        {
            return Ok(false);
        }

        let node = reference_take(children, surface).ok_or(())?;
        let sibling_index = children
            .iter()
            .position(|node| node.id == sibling)
            .ok_or(())?;
        children.insert(sibling_index + usize::from(above), node);
        Ok(true)
    }

    fn reference_preorder(nodes: &[ReferenceNode]) -> Vec<(Id, Option<Id>)> {
        fn visit(nodes: &[ReferenceNode], parent: Option<Id>, result: &mut Vec<(Id, Option<Id>)>) {
            for node in nodes {
                result.push((node.id, parent));
                visit(&node.children, Some(node.id), result);
            }
        }
        let mut result = Vec::new();
        visit(nodes, None, &mut result);
        result
    }

    fn seeded_word(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[test]
    fn preorder_encodes_parentage_and_default_planes_stay_fixed() {
        let mut tree = CompositionTree::new();
        let base = Id::Gpui(tree.base());
        let overlay = Id::Gpui(tree.overlay());
        let root = insert_gpui(&mut tree, None);
        let child = insert_gpui(&mut tree, Some(root.into()));

        assert_eq!(order(&tree), [base, root.into(), child.into(), overlay]);
        assert_eq!(tree.parent_of(child.into()).unwrap(), Some(root.into()));
        assert!(tree.reparent(base, None).is_err());
        assert!(tree.remove(overlay).is_err());
        assert!(tree.place_relative(root.into(), base, false).is_err());
        assert!(tree.place_relative(root.into(), overlay, true).is_err());
        assert_eq!(order(&tree), [base, root.into(), child.into(), overlay]);
    }

    #[test]
    fn nested_subtrees_move_and_reorder_as_contiguous_units() {
        let mut tree = CompositionTree::new();
        let left = insert_gpui(&mut tree, None);
        let left_child = insert_gpui(&mut tree, Some(left.into()));
        let right = insert_gpui(&mut tree, None);
        let right_child = insert_gpui(&mut tree, Some(right.into()));

        tree.place_relative(left.into(), right.into(), true)
            .unwrap();
        assert_eq!(
            tree.parent_of(left_child.into()).unwrap(),
            Some(left.into())
        );
        assert_eq!(
            order(&tree),
            [
                Id::Gpui(tree.base()),
                right.into(),
                right_child.into(),
                left.into(),
                left_child.into(),
                Id::Gpui(tree.overlay()),
            ]
        );

        tree.reparent(left.into(), Some(right_child.into()))
            .unwrap();
        assert_eq!(
            tree.parent_of(left.into()).unwrap(),
            Some(right_child.into())
        );
        assert_eq!(
            tree.parent_of(left_child.into()).unwrap(),
            Some(left.into())
        );
        tree.reparent(right_child.into(), None).unwrap();
        assert_eq!(tree.parent_of(right_child.into()).unwrap(), None);
        assert_eq!(
            tree.parent_of(left.into()).unwrap(),
            Some(right_child.into())
        );
    }

    #[test]
    fn seeded_edits_match_a_recursive_hierarchy_model() {
        for seed in 1..=32 {
            let mut tree = CompositionTree::new();
            let base = tree.base();
            let overlay = tree.overlay();
            let base_id = Id::Gpui(base);
            let overlay_id = Id::Gpui(overlay);
            let left = insert_gpui(&mut tree, None);
            let left_child = insert_gpui(&mut tree, Some(left.into()));
            let left_grandchild = insert_gpui(&mut tree, Some(left_child.into()));
            let left_sibling = insert_gpui(&mut tree, Some(left.into()));
            let middle = insert_gpui(&mut tree, None);
            let middle_child = insert_gpui(&mut tree, Some(middle.into()));
            let right = insert_gpui(&mut tree, None);
            let right_child = insert_gpui(&mut tree, Some(right.into()));
            let fixed = [base_id, overlay_id];
            let movable = [
                Id::Gpui(left),
                Id::Gpui(left_child),
                Id::Gpui(left_grandchild),
                Id::Gpui(left_sibling),
                Id::Gpui(middle),
                Id::Gpui(middle_child),
                Id::Gpui(right),
                Id::Gpui(right_child),
            ];
            let all = [
                base_id, overlay_id, movable[0], movable[1], movable[2], movable[3], movable[4],
                movable[5], movable[6], movable[7],
            ];
            let mut reference = vec![
                ReferenceNode::leaf(base_id),
                ReferenceNode {
                    id: movable[0],
                    children: vec![
                        ReferenceNode {
                            id: movable[1],
                            children: vec![ReferenceNode::leaf(movable[2])],
                        },
                        ReferenceNode::leaf(movable[3]),
                    ],
                },
                ReferenceNode {
                    id: movable[4],
                    children: vec![ReferenceNode::leaf(movable[5])],
                },
                ReferenceNode {
                    id: movable[6],
                    children: vec![ReferenceNode::leaf(movable[7])],
                },
                ReferenceNode::leaf(overlay_id),
            ];
            let mut state = seed;

            for _ in 0..512 {
                let word = seeded_word(&mut state);
                let surface = movable[(word as usize >> 8) % movable.len()];
                let target = all[(word as usize >> 16) % all.len()];
                match word % 3 {
                    0 => {
                        let parent = match (word >> 32) as usize % (all.len() + 1) {
                            0 => None,
                            index => Some(all[index - 1]),
                        };
                        let expected =
                            reference_reparent(&mut reference, surface, parent, fixed, overlay_id);
                        let actual = tree.reparent(surface, parent).map_err(|_| ());
                        assert_eq!(actual, expected);
                    }
                    1 | 2 => {
                        let above = word & 1 != 0;
                        let expected = reference_place_relative(
                            &mut reference,
                            surface,
                            target,
                            above,
                            fixed,
                            base_id,
                            overlay_id,
                        );
                        let actual = tree.place_relative(surface, target, above).map_err(|_| ());
                        assert_eq!(actual, expected);
                    }
                    _ => unreachable!(),
                }
                assert_eq!(
                    tree.surfaces()
                        .iter()
                        .map(|surface| (surface.id(), surface.parent))
                        .collect::<Vec<_>>(),
                    reference_preorder(&reference),
                    "seed {seed}, operation {word:#x}"
                );
            }
        }
    }

    #[test]
    fn removing_a_node_promotes_descendants_without_changing_window_bounds() {
        let mut tree = CompositionTree::new();
        let parent = insert_gpui(&mut tree, None);
        let child = insert_gpui(&mut tree, Some(parent.into()));
        let grandchild = insert_gpui(&mut tree, Some(child.into()));
        tree.remove(parent.into()).unwrap();

        assert_eq!(tree.parent_of(child.into()).unwrap(), None);
        assert_eq!(
            tree.parent_of(grandchild.into()).unwrap(),
            Some(child.into())
        );
        assert!(tree.index(parent.into()).is_err());
        assert_eq!(tree.surfaces[1].depth, 0);
        assert_eq!(tree.surfaces[2].depth, 1);
    }

}
