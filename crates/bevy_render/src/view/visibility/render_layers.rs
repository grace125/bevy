use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{component::require, entity::Entity, hierarchy::{ChildOf, Children}, prelude::{Component, ReflectComponent}, query::{Changed, Or, With}, system::Query};
use bevy_reflect::{std_traits::ReflectDefault, Reflect};
use smallvec::SmallVec;

// TODO GRACE: make sure imports in render_layers.rs align with that in mod.rs
// TODO GRACE: read through the original PR
// XXX GRACE: render layers
// TODO GRACE: remove DEFAULT_LAYERS
pub const DEFAULT_LAYERS: &RenderLayers = &RenderLayers::layer(0);

/// An identifier for a rendering layer.
pub type Layer = usize;

/// Describes which rendering layers an entity belongs to.
///
/// Cameras with this component will only render entities with intersecting
/// layers.
///
/// Entities may belong to one or more layers, or no layer at all.
///
/// The [`Default`] instance of `RenderLayers` contains layer `0`, the first layer.
///
/// An entity with this component without any layers is invisible.
///
/// Entities without this component belong to layer `0`.
#[derive(Clone, Reflect, PartialEq, Eq, PartialOrd, Ord)]
#[reflect(Default, PartialEq, Debug)]
pub struct RenderLayers(SmallVec<[u64; INLINE_BLOCKS]>);

/// The number of memory blocks stored inline
const INLINE_BLOCKS: usize = 1;

impl Default for &RenderLayers {
    fn default() -> Self {
        DEFAULT_LAYERS
    }
}

impl core::fmt::Debug for RenderLayers {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("RenderLayers")
            .field(&self.iter().collect::<Vec<_>>())
            .finish()
    }
}

impl FromIterator<Layer> for RenderLayers {
    fn from_iter<T: IntoIterator<Item = Layer>>(i: T) -> Self {
        i.into_iter().fold(Self::none(), RenderLayers::with)
    }
}

impl Default for RenderLayers {
    /// By default, this structure includes layer `0`, which represents the first layer.
    ///
    /// This is distinct from [`RenderLayers::none`], which doesn't belong to any layers.
    fn default() -> Self {
        const { Self::layer(0) }
    }
}

impl RenderLayers {
    /// Create a new `RenderLayers` belonging to the given layer.
    ///
    /// This `const` constructor is limited to `size_of::<usize>()` layers.
    /// If you need to support an arbitrary number of layers, use [`with`](RenderLayers::with)
    /// or [`from_layers`](RenderLayers::from_layers).
    pub const fn layer(n: Layer) -> Self {
        let (buffer_index, bit) = Self::layer_info(n);
        assert!(
            buffer_index < INLINE_BLOCKS,
            "layer is out of bounds for const construction"
        );
        let mut buffer = [0; INLINE_BLOCKS];
        buffer[buffer_index] = bit;
        RenderLayers(SmallVec::from_const(buffer))
    }

    /// Create a new `RenderLayers` that belongs to no layers.
    ///
    /// This is distinct from [`RenderLayers::default`], which belongs to the first layer.
    pub const fn none() -> Self {
        RenderLayers(SmallVec::from_const([0; INLINE_BLOCKS]))
    }

    /// Create a `RenderLayers` from a list of layers.
    pub fn from_layers(layers: &[Layer]) -> Self {
        layers.iter().copied().collect()
    }

    /// Add the given layer.
    ///
    /// This may be called multiple times to allow an entity to belong
    /// to multiple rendering layers.
    #[must_use]
    pub fn with(mut self, layer: Layer) -> Self {
        let (buffer_index, bit) = Self::layer_info(layer);
        self.extend_buffer(buffer_index + 1);
        self.0[buffer_index] |= bit;
        self
    }

    /// Removes the given rendering layer.
    #[must_use]
    pub fn without(mut self, layer: Layer) -> Self {
        let (buffer_index, bit) = Self::layer_info(layer);
        if buffer_index < self.0.len() {
            self.0[buffer_index] &= !bit;
            // Drop trailing zero memory blocks.
            // NOTE: This is not just an optimization, it is necessary for the derived PartialEq impl to be correct.
            if buffer_index == self.0.len() - 1 {
                self = self.shrink();
            }
        }
        self
    }

    /// Get an iterator of the layers.
    pub fn iter(&self) -> impl Iterator<Item = Layer> + '_ {
        self.0.iter().copied().zip(0..).flat_map(Self::iter_layers)
    }

    /// Determine if a `RenderLayers` intersects another.
    ///
    /// `RenderLayers`s intersect if they share any common layers.
    ///
    /// A `RenderLayers` with no layers will not match any other
    /// `RenderLayers`, even another with no layers.
    pub fn intersects(&self, other: &RenderLayers) -> bool {
        // Check for the common case where the view layer and entity layer
        // both point towards our default layer.
        if self.0.as_ptr() == other.0.as_ptr() {
            return true;
        }

        for (self_layer, other_layer) in self.0.iter().zip(other.0.iter()) {
            if (*self_layer & *other_layer) != 0 {
                return true;
            }
        }

        false
    }

    /// Get the bitmask representation of the contained layers.
    pub fn bits(&self) -> &[u64] {
        self.0.as_slice()
    }

    const fn layer_info(layer: usize) -> (usize, u64) {
        let buffer_index = layer / 64;
        let bit_index = layer % 64;
        let bit = 1u64 << bit_index;

        (buffer_index, bit)
    }

    fn extend_buffer(&mut self, other_len: usize) {
        let new_size = core::cmp::max(self.0.len(), other_len);
        self.0.reserve_exact(new_size - self.0.len());
        self.0.resize(new_size, 0u64);
    }

    fn iter_layers(buffer_and_offset: (u64, usize)) -> impl Iterator<Item = Layer> + 'static {
        let (mut buffer, mut layer) = buffer_and_offset;
        layer *= 64;
        core::iter::from_fn(move || {
            if buffer == 0 {
                return None;
            }
            let next = buffer.trailing_zeros() + 1;
            buffer = buffer.checked_shr(next).unwrap_or(0);
            layer += next as usize;
            Some(layer - 1)
        })
    }

    /// Returns the set of [layers](Layer) shared by two instances of [`RenderLayers`].
    ///
    /// This corresponds to the `self & other` operation.
    pub fn intersection(&self, other: &Self) -> Self {
        self.combine_blocks(other, |a, b| a & b).shrink()
    }

    /// Returns all [layers](Layer) included in either instance of [`RenderLayers`].
    ///
    /// This corresponds to the `self | other` operation.
    pub fn union(&self, other: &Self) -> Self {
        self.combine_blocks(other, |a, b| a | b) // doesn't need to be shrunk, if the inputs are nonzero then the result will be too
    }

    /// Returns all [layers](Layer) included in exactly one of the instances of [`RenderLayers`].
    ///
    /// This corresponds to the "exclusive or" (XOR) operation: `self ^ other`.
    pub fn symmetric_difference(&self, other: &Self) -> Self {
        self.combine_blocks(other, |a, b| a ^ b).shrink()
    }

    /// Deallocates any trailing-zero memory blocks from this instance
    fn shrink(mut self) -> Self {
        let mut any_dropped = false;
        while self.0.len() > INLINE_BLOCKS && self.0.last() == Some(&0) {
            self.0.pop();
            any_dropped = true;
        }
        if any_dropped && self.0.len() <= INLINE_BLOCKS {
            self.0.shrink_to_fit();
        }
        self
    }

    /// Creates a new instance of [`RenderLayers`] by applying a function to the memory blocks
    /// of self and another instance.
    ///
    /// If the function `f` might return `0` for non-zero inputs, you should call [`Self::shrink`]
    /// on the output to ensure that there are no trailing zero memory blocks that would break
    /// this type's equality comparison.
    fn combine_blocks(&self, other: &Self, mut f: impl FnMut(u64, u64) -> u64) -> Self {
        let mut a = self.0.iter();
        let mut b = other.0.iter();
        let mask = core::iter::from_fn(|| {
            let a = a.next().copied();
            let b = b.next().copied();
            if a.is_none() && b.is_none() {
                return None;
            }
            Some(f(a.unwrap_or_default(), b.unwrap_or_default()))
        });
        Self(mask.collect())
    }
}

impl core::ops::BitAnd for RenderLayers {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self::Output {
        self.intersection(&rhs)
    }
}

impl core::ops::BitOr for RenderLayers {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(&rhs)
    }
}

impl core::ops::BitXor for RenderLayers {
    type Output = Self;
    fn bitxor(self, rhs: Self) -> Self::Output {
        self.symmetric_difference(&rhs)
    }
}

// TODO GRACE: prelude?
// TODO GRACE: register component
// TODO GRACE: document VisibilityLayers
// TODO GRACE: impl Debug
// TODO GRACE: implement helper methods on VisibilityLayers (see Visibility)
#[derive(Component, Clone, Reflect, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[reflect(Component, Default, PartialEq, Debug)]
#[require(ComputedVisibleLayers)]
pub enum VisibleLayers {
    #[default]
    Inherited,
    Layers(RenderLayers),
}


impl Default for &VisibleLayers {
    fn default() -> Self {
        &VisibleLayers::Inherited
    }
}

// TODO GRACE: register component
// TODO GRACE: document ComputedVisibilityLayers
// TODO GRACE: impl Debug
// TODO GRACE: make ReadOnly
#[derive(Component, Clone, Reflect, PartialEq, Eq, PartialOrd, Ord, Debug, Default, Deref, DerefMut)]
#[reflect(Component, Default, PartialEq, Debug)]
pub struct ComputedVisibleLayers(pub RenderLayers);

impl Default for &ComputedVisibleLayers {
    fn default() -> Self {
        const { &ComputedVisibleLayers(RenderLayers::layer(0)) }
    }
}

// TODO GRACE: re-write this system
// TODO GRACE: add this system to the schedule
pub(super) fn visible_layers_propagate_system(
    changed: Query<
        (Entity, &VisibleLayers, Option<&ChildOf>, Option<&Children>),
        (
            With<ComputedVisibleLayers>,
            Or<(Changed<VisibleLayers>, Changed<ChildOf>)>,
        ),
    >,
    mut visible_layer_query: Query<(&VisibleLayers, &mut ComputedVisibleLayers)>,
    children_query: Query<&Children, (With<VisibleLayers>, With<ComputedVisibleLayers>)>,
) {
    for (entity, visible_layers, parent, children) in &changed {
        
        let render_layers = match visible_layers {
            VisibleLayers::Layers(layers) => layers.clone(),
            VisibleLayers::Inherited => parent
                .and_then(|p| visible_layer_query.get(p.get()).ok())
                .map(|(_, x)| x.0.clone())
                .unwrap_or_default()
        };

        let (_, mut computed_visible_layers) = visible_layer_query
            .get_mut(entity)
            .expect("With<ComputedVisibleLayers> ensures this query will return a value");

        // Only updates visible layers if they have changed.
        // This will also prevent visible layers from propagating multiple times in the same frame
        // if this entity's visible layers has been updated recursively by its parent.
        if computed_visible_layers.0 != render_layers {
            computed_visible_layers.0 = render_layers.clone();

            for &child in children.into_iter().flatten() {
                let _ = propagate_recursive(&render_layers, child, &mut visible_layer_query, &children_query);
            }
        }
    }
}

fn propagate_recursive(
    parent_render_layers: &RenderLayers,
    entity: Entity,
    mut visible_layer_query: &mut Query<(&VisibleLayers, &mut ComputedVisibleLayers)>,
    children_query: &Query<&Children, (With<VisibleLayers>, With<ComputedVisibleLayers>)>,
    // BLOCKED: https://github.com/rust-lang/rust/issues/31436
    // We use a result here to use the `?` operator. Ideally we'd use a try block instead
) -> Result<(), ()> {
    // Get the visible_layer components for the current entity.
    // If the entity does not have the required components, just return early.
    let (visibility, mut inherited_visibility) = visible_layer_query.get_mut(entity).map_err(drop)?;

    let render_layers = match visibility {
        VisibleLayers::Layers(layers) => layers.clone(),
        VisibleLayers::Inherited => parent_render_layers.clone()
    };

    if inherited_visibility.0 != render_layers {
        inherited_visibility.0 = render_layers;
        let new_render_layers = inherited_visibility.0.clone();

        for &child in children_query.into_iter().flatten() {
            let _ = propagate_recursive(&new_render_layers, child, &mut visible_layer_query, &children_query);
        }
    }

    Ok(())
}

// TODO GRACE: hierarchy tests(?)
#[cfg(test)]
mod rendering_mask_tests {
    use super::{Layer, RenderLayers};
    use smallvec::SmallVec;

    #[test]
    fn rendering_mask_sanity() {
        let layer_0 = RenderLayers::layer(0);
        assert_eq!(layer_0.0.len(), 1, "layer 0 is one buffer");
        assert_eq!(layer_0.0[0], 1, "layer 0 is mask 1");
        let layer_1 = RenderLayers::layer(1);
        assert_eq!(layer_1.0.len(), 1, "layer 1 is one buffer");
        assert_eq!(layer_1.0[0], 2, "layer 1 is mask 2");
        let layer_0_1 = RenderLayers::layer(0).with(1);
        assert_eq!(layer_0_1.0.len(), 1, "layer 0 + 1 is one buffer");
        assert_eq!(layer_0_1.0[0], 3, "layer 0 + 1 is mask 3");
        let layer_0_1_without_0 = layer_0_1.without(0);
        assert_eq!(
            layer_0_1_without_0.0.len(),
            1,
            "layer 0 + 1 - 0 is one buffer"
        );
        assert_eq!(layer_0_1_without_0.0[0], 2, "layer 0 + 1 - 0 is mask 2");
        let layer_0_2345 = RenderLayers::layer(0).with(2345);
        assert_eq!(layer_0_2345.0.len(), 37, "layer 0 + 2345 is 37 buffers");
        assert_eq!(layer_0_2345.0[0], 1, "layer 0 + 2345 is mask 1");
        assert_eq!(
            layer_0_2345.0[36], 2199023255552,
            "layer 0 + 2345 is mask 2199023255552"
        );
        assert!(
            layer_0_2345.intersects(&layer_0),
            "layer 0 + 2345 intersects 0"
        );
        assert!(
            RenderLayers::layer(1).intersects(&RenderLayers::layer(1)),
            "layers match like layers"
        );
        assert!(
            RenderLayers::layer(0).intersects(&RenderLayers(SmallVec::from_const([1]))),
            "a layer of 0 means the mask is just 1 bit"
        );

        assert!(
            RenderLayers::layer(0)
                .with(3)
                .intersects(&RenderLayers::layer(3)),
            "a mask will match another mask containing any similar layers"
        );

        assert!(
            RenderLayers::default().intersects(&RenderLayers::default()),
            "default masks match each other"
        );

        assert!(
            !RenderLayers::layer(0).intersects(&RenderLayers::layer(1)),
            "masks with differing layers do not match"
        );
        assert!(
            !RenderLayers::none().intersects(&RenderLayers::none()),
            "empty masks don't match"
        );
        assert_eq!(
            RenderLayers::from_layers(&[0, 2, 16, 30])
                .iter()
                .collect::<Vec<_>>(),
            vec![0, 2, 16, 30],
            "from_layers and get_layers should roundtrip"
        );
        assert_eq!(
            format!("{:?}", RenderLayers::from_layers(&[0, 1, 2, 3])).as_str(),
            "RenderLayers([0, 1, 2, 3])",
            "Debug instance shows layers"
        );
        assert_eq!(
            RenderLayers::from_layers(&[0, 1, 2]),
            <RenderLayers as FromIterator<Layer>>::from_iter(vec![0, 1, 2]),
            "from_layers and from_iter are equivalent"
        );

        let tricky_layers = vec![0, 5, 17, 55, 999, 1025, 1026];
        let layers = RenderLayers::from_layers(&tricky_layers);
        let out = layers.iter().collect::<Vec<_>>();
        assert_eq!(tricky_layers, out, "tricky layers roundtrip");
    }

    const MANY: RenderLayers = RenderLayers(SmallVec::from_const([u64::MAX]));

    #[test]
    fn render_layer_ops() {
        let a = RenderLayers::from_layers(&[2, 4, 6]);
        let b = RenderLayers::from_layers(&[1, 2, 3, 4, 5]);

        assert_eq!(
            a.clone() | b.clone(),
            RenderLayers::from_layers(&[1, 2, 3, 4, 5, 6])
        );
        assert_eq!(a.clone() & b.clone(), RenderLayers::from_layers(&[2, 4]));
        assert_eq!(a ^ b, RenderLayers::from_layers(&[1, 3, 5, 6]));

        assert_eq!(RenderLayers::none() & MANY, RenderLayers::none());
        assert_eq!(RenderLayers::none() | MANY, MANY);
        assert_eq!(RenderLayers::none() ^ MANY, MANY);
    }

    #[test]
    fn render_layer_shrink() {
        // Since it has layers greater than 64, the instance should take up two memory blocks
        let layers = RenderLayers::from_layers(&[1, 77]);
        assert!(layers.0.len() == 2);
        // When excluding that layer, it should drop the extra memory block
        let layers = layers.without(77);
        assert!(layers.0.len() == 1);
    }

    #[test]
    fn render_layer_iter_no_overflow() {
        let layers = RenderLayers::from_layers(&[63]);
        layers.iter().count();
    }
}
