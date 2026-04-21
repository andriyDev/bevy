use alloc::{vec, vec::Vec};
use core::cmp::Ordering;

use bevy_ecs::{
    component::Component,
    entity::Entity,
    lifecycle::Add,
    observer::On,
    query::Without,
    system::{Commands, Query, Res},
};
use tracing::warn;

use crate::{AssetServer, UntypedAssetId, VisitAssetDependencies};

/// A basic "loading screen" that waits until all pending tasks have been completed.
///
/// Importantly, this does not have any *visual* component. It is up to users to display the
/// progress of loading. This just provides a convenient framework for tracking tasks. Once
/// [`Self::ready`] equals [`Self::pending`], the [`LoadingScreenLoaded`] component is added to this
/// entity (which can be reacted to using observers).
///
/// Once a loading screen loads, it is no longer tracked - adding new tasks to it will do nothing.
/// In general, callers should set up all their loading tasks, then just wait for the "loaded"
/// signal. Attempting to add more tasks to this type after the initial setup can have confusing
/// results (like the percentage of ready-to-pending going down, which looks like the game is
/// "unloading"). Users should prefer to despawn an existing [`LoadingScreen`] and create an
/// entirely new one.
#[derive(Component, Default)]
pub struct LoadingScreen {
    /// The number of pending tasks that have been registered.
    pending: usize,
    /// The number of tasks that have been reported as ready.
    ready: usize,
}

/// A component to indicate that a [`LoadingScreen`] has completed.
///
/// Once the [`LoadingScreen`] has finished, this component is added. Listening to the
/// [`On<Add, LoadingScreenLoaded>`] event in an observer allows users to react to loading
/// completing.
///
/// This component also prevents the [`LoadingScreen`] from making any more progress. Users should
/// prefer to despawn the entity, and spawn brand new [`LoadingScreen`] instead.
#[derive(Component)]
pub struct LoadingScreenLoaded;

impl LoadingScreen {
    /// Adds a number of pending tasks that the loading screen should wait for.
    ///
    /// This method is public to allow users to implement their own tasks that a loading screen can
    /// wait on. Adding tasks here without marking them ready later will result in the loading
    /// screen **never being marked loaded**.
    pub fn add_pending(&mut self, tasks: usize) {
        self.pending = self.pending.saturating_add(tasks);
    }

    /// Notifies the loading screen that some number of tasks have completed.
    ///
    /// This method is public to allow users to implement their own tasks that a loading screen can
    /// wait on. Make sure to mark as many tasks ready as you add. Marking too many tasks ready will
    /// result in the loading screen being loaded before all tasks are ready. Marking too few tasks
    /// ready will make the loading screen never being loaded.
    pub fn mark_ready(&mut self, tasks: usize) {
        self.ready = self.ready.saturating_add(tasks);
    }

    /// Returns the number of tasks that have been registered.
    ///
    /// This is useful to track the progress of loading.
    pub fn pending(&self) -> usize {
        self.pending
    }

    /// Returns the number of tasks that have been reported as ready.
    ///
    /// In general, this value should be less than or equal to [`Self::pending`].
    ///
    /// This is useful to track the progress of loading.
    pub fn ready(&self) -> usize {
        self.ready
    }
}

/// System that polls all currently in-progress loading screens and tries to mark them as completed.
pub fn poll_loading_screens(
    loading_screens: Query<(Entity, &LoadingScreen), Without<LoadingScreenLoaded>>,
    mut commands: Commands,
) {
    for (entity, loading_screen) in loading_screens.iter() {
        match loading_screen.ready.cmp(&loading_screen.pending) {
            Ordering::Equal => {
                commands.entity(entity).insert(LoadingScreenLoaded);
            }
            // Still not ready.
            Ordering::Less => {}
            Ordering::Greater => {
                // There are more ready tasks than pending tasks! This is definitely a user doing
                // something cursed. Tell them, and then just call it loaded.
                warn!("Loading screen for entity {entity:?} has more ready tasks than pending tasks! Some tasks may not have registered themselves. Marking as loaded anyway...");
                commands.entity(entity).insert(LoadingScreenLoaded);
            }
        }
    }
}

/// Relationship indicating that an entity intends to be a "blocker" for the loading screen.
///
/// This also performs automatically cleanup of this entity if the loading screen is despawned.
#[derive(Component)]
#[relationship(relationship_target = BlockedOn)]
pub struct BlockLoadingScreen(pub Entity);

/// Relationship target tracking all the entities blocking this loading screen.
///
/// Despawning the loading screen will despawn all these related entities.
#[derive(Component)]
#[relationship_target(relationship = BlockLoadingScreen, linked_spawn)]
pub struct BlockedOn(Vec<Entity>);

/// A component that blocks a loading screen on a list of asset IDs.
#[derive(Component)]
pub struct PendingAssetDependencies(Vec<UntypedAssetId>);

impl PendingAssetDependencies {
    /// Creates a new instance from the dependencies of `value`.
    pub fn from_value(value: &impl VisitAssetDependencies) -> Self {
        let mut ids = vec![];
        value.visit_dependencies(&mut |asset_ids| {
            ids.push(asset_ids);
        });
        Self(ids)
    }
}

/// Observer that adds the pending asset IDs as tasks to the associated [`LoadingScreen`].
pub(crate) fn on_add_pending_asset_dependencies(
    event: On<Add, PendingAssetDependencies>,
    dependencies: Query<(&PendingAssetDependencies, &BlockLoadingScreen)>,
    mut loading_screen: Query<&mut LoadingScreen>,
) {
    let Ok((dependencies, loading_screen_entity)) = dependencies.get(event.entity) else {
        warn!("Added PendingAssetDependencies to entity {} without BlockLoadingScreen component. This configuration is not supported", event.entity);
        return;
    };

    let Ok(mut loading_screen) = loading_screen.get_mut(loading_screen_entity.0) else {
        warn!("BlockLoadingScreen component on entity {} references an entity {} without a LoadingScreen component.", event.entity, loading_screen_entity.0);
        return;
    };
    loading_screen.add_pending(dependencies.0.len());
}

/// A system that polls [`PendingAssetDependencies`] and updates the corresponding [`LoadingScreen`]
/// as those dependencies are loaded.
pub fn poll_pending_asset_dependencies(
    mut pending_assets: Query<(&mut PendingAssetDependencies, &BlockLoadingScreen)>,
    mut loading_screens: Query<&mut LoadingScreen>,
    asset_server: Res<AssetServer>,
) {
    for (mut pending_assets, loading_screen_entity) in pending_assets.iter_mut() {
        let Ok(mut loading_screen) = loading_screens.get_mut(loading_screen_entity.0) else {
            continue;
        };
        let before = pending_assets.0.len();
        pending_assets.0.retain(|id| {
            if !asset_server.is_loaded_with_dependencies(*id) {
                return true;
            }

            false
        });
        let after = pending_assets.0.len();
        if before != after {
            loading_screen.mark_ready(before - after);
        }
    }
}
