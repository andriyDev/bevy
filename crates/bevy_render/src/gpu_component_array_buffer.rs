use crate::{
    render_resource::{GpuArrayBuffer, GpuArrayBufferable},
    renderer::{RenderDevice, RenderQueue},
    Render, RenderApp, RenderStartup, RenderSystems,
};
use bevy_app::{App, Plugin};
use bevy_ecs::{
    prelude::{Component, Entity},
    schedule::IntoScheduleConfigs,
    system::{Commands, Query, Res, ResMut},
};
use core::marker::PhantomData;

/// This plugin prepares the components of the corresponding type for the GPU
/// by storing them in a [`GpuArrayBuffer`].
pub struct GpuComponentArrayBufferPlugin<C: Component + GpuArrayBufferable>(PhantomData<C>);

impl<C: Component + GpuArrayBufferable> Plugin for GpuComponentArrayBufferPlugin<C> {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app.add_systems(RenderStartup, init_gpu_array_buffer::<C>);
        render_app.add_systems(
            Render,
            prepare_gpu_component_array_buffers::<C>.in_set(RenderSystems::PrepareResources),
        );
    }
}

impl<C: Component + GpuArrayBufferable> Default for GpuComponentArrayBufferPlugin<C> {
    fn default() -> Self {
        Self(PhantomData::<C>)
    }
}

fn init_gpu_array_buffer<C: Component + GpuArrayBufferable>(
    render_device: Res<RenderDevice>,
    mut commands: Commands,
) {
    commands.insert_resource(GpuArrayBuffer::<C>::new(&render_device));
}

fn prepare_gpu_component_array_buffers<C: Component + GpuArrayBufferable>(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut gpu_array_buffer: ResMut<GpuArrayBuffer<C>>,
    components: Query<(Entity, &C)>,
) {
    gpu_array_buffer.clear();

    let entities = components
        .iter()
        .map(|(entity, component)| (entity, gpu_array_buffer.push(component.clone())))
        .collect::<Vec<_>>();
    commands.try_insert_batch(entities);

    gpu_array_buffer.write_buffer(&render_device, &render_queue);
}
