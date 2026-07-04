use crate::Asset;
use bevy_ecs::entity::Entity;
use bevy_reflect::Reflect;
use uuid::Uuid;

use core::{
    any::TypeId,
    fmt::{Debug, Display},
    hash::Hash,
    marker::PhantomData,
};
use derive_more::derive::From;
use thiserror::Error;

/// A unique runtime-only identifier for an [`Asset`]. This is cheap to [`Copy`]/[`Clone`] and is not directly tied to the
/// lifetime of the Asset. This means it _can_ point to an [`Asset`] that no longer exists.
///
/// For an identifier tied to the lifetime of an asset, see [`Handle`](`crate::Handle`).
///
/// For an "untyped" / "generic-less" id, see [`UntypedAssetId`].
#[derive(Reflect, From)]
#[reflect(Clone, Debug, PartialEq, Hash)]
pub struct AssetId<A: Asset> {
    /// The entity
    pub entity: Entity,
    /// A marker to store the type information of the asset.
    #[reflect(ignore, clone)]
    pub(crate) marker: PhantomData<fn() -> A>,
}

impl<A: Asset> AssetId<A> {
    /// The UUID for the default [`AssetId`]. It is valid to assign a value to this in [`Assets`](crate::Assets)
    /// and by convention (where appropriate) assets should support this pattern.
    #[deprecated(since = "0.20.0", note = "Use AssetReference::Default")]
    pub const DEFAULT_UUID: Uuid = Uuid::from_u128(200809721996911295814598172825939264631);

    /// This asset id _should_ never be valid. Assigning a value to this in [`Assets`](crate::Assets) will
    /// produce undefined behavior, so don't do it!
    #[deprecated(
        since = "0.20.0",
        note = "Use `Option<AssetId>` if possible. `AssetId::default` may also work, but note that the default can map to a valid asset."
    )]
    pub const INVALID_UUID: Uuid = Uuid::from_u128(108428345662029828789348721013522787528);

    #[inline]
    pub fn entity(&self) -> Entity {
        self.entity
    }

    /// Converts this to an "untyped" / "generic-less" [`Asset`] identifier that stores the type information
    /// _inside_ the [`UntypedAssetId`].
    #[inline]
    pub fn untyped(self) -> UntypedAssetId {
        self.into()
    }
}

impl<A: Asset> Clone for AssetId<A> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A: Asset> Copy for AssetId<A> {}

impl<A: Asset> Display for AssetId<A> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        Debug::fmt(self, f)
    }
}

impl<A: Asset> Debug for AssetId<A> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "AssetId<{}>{{ entity: {} }}",
            core::any::type_name::<A>(),
            self.entity
        )
    }
}

impl<A: Asset> Hash for AssetId<A> {
    #[inline]
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.entity().hash(state);
    }
}

impl<A: Asset> PartialEq for AssetId<A> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.entity.eq(&other.entity)
    }
}

impl<A: Asset> Eq for AssetId<A> {}

impl<A: Asset> PartialOrd for AssetId<A> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<A: Asset> Ord for AssetId<A> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.entity.cmp(&other.entity)
    }
}

impl<A: Asset> Into<Entity> for &AssetId<A> {
    fn into(self) -> Entity {
        self.entity()
    }
}

impl<A: Asset> From<Entity> for AssetId<A> {
    #[inline]
    fn from(entity: Entity) -> Self {
        Self {
            entity,
            marker: PhantomData,
        }
    }
}

impl<A: Asset> Into<Entity> for AssetId<A> {
    #[inline]
    fn into(self) -> Entity {
        self.entity()
    }
}

/// An "untyped" / "generic-less" [`Asset`] identifier that behaves much like [`AssetId`], but stores the [`Asset`] type
/// information at runtime instead of compile-time. This increases the size of the type, but it enables storing asset ids
/// across asset types together and enables comparisons between them.
#[derive(Debug, Copy, Clone, Reflect, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UntypedAssetId {
    /// The entity that holds/will hold the asset data.
    pub entity: Entity,
    /// The type of asset that this ID represents.
    pub type_id: TypeId,
}

impl UntypedAssetId {
    /// Converts this to a "typed" [`AssetId`] without checking the stored type to see if it matches the target `A` [`Asset`] type.
    /// This should only be called if you are _absolutely certain_ the asset type matches the stored type. And even then, you should
    /// consider using [`UntypedAssetId::typed_debug_checked`] instead.
    #[inline]
    pub fn typed_unchecked<A: Asset>(self) -> AssetId<A> {
        self.entity.into()
    }

    /// Converts this to a "typed" [`AssetId`]. When compiled in debug-mode it will check to see if the stored type
    /// matches the target `A` [`Asset`] type. When compiled in release-mode, this check will be skipped.
    ///
    /// # Panics
    ///
    /// Panics if compiled in debug mode and the [`TypeId`] of `A` does not match the stored [`TypeId`].
    #[inline]
    pub fn typed_debug_checked<A: Asset>(self) -> AssetId<A> {
        debug_assert_eq!(
            self.type_id,
            TypeId::of::<A>(),
            "The target AssetId<{}>'s TypeId does not match the TypeId of this UntypedAssetId",
            core::any::type_name::<A>()
        );
        self.typed_unchecked()
    }

    /// Converts this to a "typed" [`AssetId`].
    ///
    /// # Panics
    ///
    /// Panics if the [`TypeId`] of `A` does not match the stored type id.
    #[inline]
    pub fn typed<A: Asset>(self) -> AssetId<A> {
        let Ok(id) = self.try_typed() else {
            panic!(
                "The target AssetId<{}>'s TypeId does not match the TypeId of this UntypedAssetId",
                core::any::type_name::<A>()
            )
        };

        id
    }

    /// Try to convert this to a "typed" [`AssetId`].
    #[inline]
    pub fn try_typed<A: Asset>(self) -> Result<AssetId<A>, UntypedAssetIdConversionError> {
        AssetId::try_from(self)
    }
}

impl Display for UntypedAssetId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut writer = f.debug_struct("UntypedAssetId");
        writer.field("entity", &self.entity);
        writer.field("type_id", &self.type_id);
        writer.finish()
    }
}

// Cross Operations

impl<A: Asset> PartialEq<UntypedAssetId> for AssetId<A> {
    #[inline]
    fn eq(&self, other: &UntypedAssetId) -> bool {
        TypeId::of::<A>() == other.type_id && self.entity.eq(&other.entity)
    }
}

impl<A: Asset> PartialEq<AssetId<A>> for UntypedAssetId {
    #[inline]
    fn eq(&self, other: &AssetId<A>) -> bool {
        other.eq(self)
    }
}

impl<A: Asset> PartialOrd<UntypedAssetId> for AssetId<A> {
    #[inline]
    fn partial_cmp(&self, other: &UntypedAssetId) -> Option<core::cmp::Ordering> {
        if TypeId::of::<A>() != other.type_id {
            None
        } else {
            Some(self.entity.cmp(&other.entity))
        }
    }
}

impl<A: Asset> PartialOrd<AssetId<A>> for UntypedAssetId {
    #[inline]
    fn partial_cmp(&self, other: &AssetId<A>) -> Option<core::cmp::Ordering> {
        Some(other.partial_cmp(self)?.reverse())
    }
}

impl<A: Asset> From<AssetId<A>> for UntypedAssetId {
    #[inline]
    fn from(value: AssetId<A>) -> Self {
        Self {
            entity: value.entity,
            type_id: TypeId::of::<A>(),
        }
    }
}

impl<A: Asset> TryFrom<UntypedAssetId> for AssetId<A> {
    type Error = UntypedAssetIdConversionError;

    #[inline]
    fn try_from(value: UntypedAssetId) -> Result<Self, Self::Error> {
        let expected = TypeId::of::<A>();

        if value.type_id != expected {
            return Err(UntypedAssetIdConversionError::TypeIdMismatch {
                expected,
                found: value.type_id,
            });
        }
        Ok(AssetId {
            entity: value.entity,
            marker: PhantomData,
        })
    }
}

/// Errors preventing the conversion of to/from an [`UntypedAssetId`] and an [`AssetId`].
#[derive(Error, Debug, PartialEq, Clone)]
#[non_exhaustive]
pub enum UntypedAssetIdConversionError {
    /// Caused when trying to convert an [`UntypedAssetId`] into an [`AssetId`] of the wrong type.
    #[error("This UntypedAssetId is for {found:?} and cannot be converted into an AssetId<{expected:?}>")]
    TypeIdMismatch {
        /// The [`TypeId`] of the asset that we are trying to convert to.
        expected: TypeId,
        /// The [`TypeId`] of the asset that we are trying to convert from.
        found: TypeId,
    },
}
