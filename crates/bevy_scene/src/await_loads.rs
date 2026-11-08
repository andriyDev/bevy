//! Tools for waiting on scenes to load more easily.
//!
//! This is used in conjunction with [`bevy_asset::await_loads`].

use bevy_asset::await_loads::{AssetLoadMetadata, AwaitLoadsBuilder};
use bevy_ecs::template::TemplateMetadata;
use bevy_utils::AnyMap;

use crate::{Scene, SceneDependencies, SceneList};

/// Trait to enable [`AwaitLoadsBuilder`] to work with scenes.
pub trait AwaitLoadsExt {
    /// Loads all assets that are expected to be loaded by `scene`.
    ///
    /// Important: the [`AwaitLoads`] produced later will be the only value holding the loaded
    /// handles. Make sure to spawn/apply the scene **before** dropping the [`AwaitLoads`]
    /// (otherwise the assets may drop and need to be reloaded when the scene spawns).
    fn load_scene_assets(&mut self, scene: &dyn Scene);

    /// Loads all assets that are expected to be loaded by `scene_list`.
    ///
    /// Important: the [`AwaitLoads`] produced later will be the only value holding the loaded
    /// handles. Make sure to spawn/apply the scene **before** dropping the [`AwaitLoads`]
    /// (otherwise the assets may drop and need to be reloaded when the scene spawns).
    fn load_scene_list_assets(&mut self, scene: &dyn SceneList);
}

impl AwaitLoadsExt for AwaitLoadsBuilder {
    fn load_scene_assets(&mut self, scene: &dyn Scene) {
        let mut metadata = AnyMap::default();
        metadata.insert(AssetLoadMetadata::new());

        scene.collect_template_metadata(&mut TemplateMetadata {
            values: &mut metadata,
        });

        // In practice, no one should have removed this metadata, but just in case, we should make
        // sure it's present (rather than unwrapping).
        if let Some(metadata) = metadata.remove::<AssetLoadMetadata>() {
            for (type_id, path) in metadata.into_iter() {
                // We don't care about the returned handle. The `AwaitLoadsBuilder` will hold it.
                self.load_erased(type_id, path);
            }
        }

        let mut scene_dependencies = SceneDependencies::default();
        scene.register_dependencies(&mut scene_dependencies);

        for dependency in scene_dependencies.iter() {
            // We don't care about the returned handle. The `AwaitLoadsBuilder` will hold it.
            self.load_erased(dependency.type_id, dependency.path.clone());
        }
    }

    fn load_scene_list_assets(&mut self, scene_list: &dyn SceneList) {
        let mut metadata = AnyMap::default();
        metadata.insert(AssetLoadMetadata::new());

        scene_list.collect_template_metadata(&mut TemplateMetadata {
            values: &mut metadata,
        });

        // In practice, no one should have removed this metadata, but just in case, we should make
        // sure it's present (rather than unwrapping).
        if let Some(metadata) = metadata.remove::<AssetLoadMetadata>() {
            for (type_id, path) in metadata.into_iter() {
                // We don't care about the returned handle. The `AwaitLoadsBuilder` will hold it.
                self.load_erased(type_id, path);
            }
        }

        let mut scene_dependencies = SceneDependencies::default();
        scene_list.register_dependencies(&mut scene_dependencies);

        for dependency in scene_dependencies.iter() {
            // We don't care about the returned handle. The `AwaitLoadsBuilder` will hold it.
            self.load_erased(dependency.type_id, dependency.path.clone());
        }
    }
}
