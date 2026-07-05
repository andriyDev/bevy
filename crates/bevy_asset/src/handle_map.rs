use core::any::TypeId;

use bevy_ecs::{
    resource::Resource,
    system::{Commands, ResMut, SystemParam},
    world::World,
};
use bevy_platform::collections::{hash_map::Entry, HashMap};
use uuid::Uuid;

use crate::{Asset, AssetData, Handle, UntypedHandle};

#[derive(SystemParam)]
pub struct AssetUuids<'w, 's> {
    uuid_map: ResMut<'w, AssetUuidMap>,
    commands: Commands<'w, 's>,
}

impl AssetUuids<'_, '_> {
    pub fn get_erased_handle(&mut self, uuid: Uuid, type_id: TypeId) -> UntypedHandle {
        match self.uuid_map.0.entry((uuid, type_id)) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                let entity = self.commands.spawn_empty().id();
                let handle = self.commands.entity_allocator().get_handle_with_data(
                    entity,
                    AssetData {
                        uuid: Some(uuid),
                        type_id_hint: Some(type_id),
                        ..Default::default()
                    },
                );

                self.commands.entity(entity).insert(handle.weak());
                let handle = UntypedHandle(handle);
                entry.insert(handle.clone());
                handle
            }
        }
    }

    pub fn get_handle<A: Asset>(&mut self, uuid: Uuid) -> Handle<A> {
        self.get_erased_handle(uuid, TypeId::of::<A>())
            .typed_unchecked()
    }

    pub fn get_erased_handle_immediate(
        world: &mut World,
        uuid: Uuid,
        type_id: TypeId,
    ) -> UntypedHandle {
        if let Some(handle) = world
            .get_resource_or_init::<AssetUuidMap>()
            .get((uuid, type_id))
        {
            return handle;
        }

        let handle = UntypedHandle(world.spawn_empty().handle_with_data(AssetData {
            uuid: Some(uuid),
            type_id_hint: Some(type_id),
            ..Default::default()
        }));
        world
            .resource_mut::<AssetUuidMap>()
            .insert((uuid, type_id), handle.clone());
        handle
    }

    pub fn get_handle_immediate<A: Asset>(world: &mut World, uuid: Uuid) -> Handle<A> {
        Self::get_erased_handle_immediate(world, uuid, TypeId::of::<A>()).typed_unchecked()
    }

    pub fn insert_uuid_asset<A: Asset>(world: &mut World, uuid: Uuid, asset: A) -> Handle<A> {
        let handle = Self::get_handle_immediate(world, uuid);
        world.entity_mut(handle.entity()).insert(asset);
        handle
    }
}

#[derive(Resource, Default)]
pub(crate) struct AssetUuidMap(pub(crate) HashMap<(Uuid, TypeId), UntypedHandle>);

impl AssetUuidMap {
    pub(crate) fn get(&self, key: (Uuid, TypeId)) -> Option<UntypedHandle> {
        self.0.get(&key).cloned()
    }

    pub(crate) fn insert(&mut self, key: (Uuid, TypeId), handle: UntypedHandle) {
        self.0.insert(key, handle);
    }
}
