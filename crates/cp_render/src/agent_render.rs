//! Bevy side of vehicle and pedestrian rendering (§13.3): one instanced draw per vehicle
//! kind and detail level and per pedestrian detail level. The instance lists are rebuilt every
//! frame from `SimView::agents` (interpolated by [`TickFraction`], culled by camera height
//! and frustum, see [`crate::agent`]) and uploaded as an instance-stepped vertex buffer, which
//! works on WebGL2 too (no storage buffers, no indirect drawing). Reads only `SimView`.
//!
//! The camera must carry `NoIndirectDrawing` (set where it is spawned).

use std::sync::Arc;

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::primitives::Frustum;
use bevy::camera::visibility::{NoFrustumCulling, VisibilitySystems};
use bevy::core_pipeline::core_3d::{Transparent3d, TransparentSortingInfo3d};
use bevy::ecs::query::QueryItem;
use bevy::ecs::system::{SystemParamItem, lifetimeless::*};
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology, VertexBufferLayout};
use bevy::pbr::{
    self, MeshInputUniform, MeshPipeline, MeshPipelineKey, MeshPipelineSystems, MeshUniform,
    RenderMeshInstances, SetMeshBindGroup, SetMeshViewBindGroup, SetMeshViewBindingArrayBindGroup,
    ViewKeyCache,
};
use bevy::prelude::*;
use bevy::render::batching::gpu_preprocessing::BatchedInstanceBuffers;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::mesh::allocator::MeshAllocator;
use bevy::render::mesh::{RenderMesh, RenderMeshBufferInfo};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::{
    AddRenderCommand, DrawFunctions, PhaseItem, PhaseItemExtraIndex, RenderCommand,
    RenderCommandResult, SetItemPipeline, TrackedRenderPass, ViewSortedRenderPhases,
};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::sync_component::SyncComponent;
use bevy::render::sync_world::MainEntity;
use bevy::render::view::ExtractedView;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use cp_sim::SimView;

use crate::agent::mesh::{pedestrian_mesh, vehicle_mesh};
use crate::agent::{
    AgentInstances, Cull, InstanceData, VEHICLE_KINDS, VehicleLooks, build_pedestrians,
    build_vehicles,
};
use crate::chunk::MeshData;

/// How far real time is into the tick after the latest `SimView` snapshot, `0..1`. The app
/// sets it every frame (it owns the clock); the sim never sees it.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct TickFraction(pub f32);

/// Numbers for the debug window and benchmarks.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct AgentRenderStats {
    pub vehicles_in_sim: usize,
    pub pedestrians_in_sim: usize,
    pub vehicles_drawn: usize,
    pub pedestrians_drawn: usize,
    /// Instanced draws issued (batches with at least one instance).
    pub draws: usize,
    /// CPU time of building the instance lists last frame, milliseconds.
    pub prepare_ms: f32,
    /// CPU time of the last `SimView` snapshot (pose extraction included), set by the app
    /// when it publishes a view.
    pub view_ms: f32,
}

/// The instances of one batch, shared with the render world.
#[derive(Component, Clone, Default)]
pub struct InstanceBatch(pub Arc<Vec<InstanceData>>);

impl SyncComponent<RenderApp> for InstanceBatch {
    type Target = Self;
}

impl ExtractComponent<RenderApp> for InstanceBatch {
    type QueryData = &'static InstanceBatch;
    type QueryFilter = ();
    type Out = Self;

    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self> {
        Some(item.clone())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Vehicle { kind: usize, detail: usize },
    Pedestrian { detail: usize },
}

#[derive(Component)]
struct AgentBatch(Slot);

pub struct AgentRenderPlugin;

impl Plugin for AgentRenderPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "agent_render.wesl");
        app.init_resource::<TickFraction>()
            .init_resource::<AgentRenderStats>()
            .init_resource::<FrameInstances>()
            .add_plugins(ExtractComponentPlugin::<InstanceBatch>::default())
            .add_systems(Startup, spawn_batches)
            .add_systems(
                PostUpdate,
                build_agent_instances
                    .after(VisibilitySystems::UpdateFrusta)
                    .after(TransformSystems::Propagate),
            );
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render
                .add_render_command::<Transparent3d, DrawAgents>()
                .init_resource::<SpecializedMeshPipelines<AgentPipeline>>()
                .init_resource::<InstanceBuffers>()
                .add_systems(
                    RenderStartup,
                    init_agent_pipeline.after(MeshPipelineSystems),
                )
                .add_systems(
                    Render,
                    (
                        queue_agents.in_set(RenderSystems::QueueMeshes),
                        upload_instances.in_set(RenderSystems::PrepareResources),
                    ),
                );
        }
    }
}

fn to_bevy_mesh(m: MeshData) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, m.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, m.colors)
    .with_inserted_indices(Indices::U32(m.indices))
}

fn spawn_batches(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let mut spawn = |slot: Slot, data: MeshData, name: &'static str| {
        commands.spawn((
            Mesh3d(meshes.add(to_bevy_mesh(data))),
            InstanceBatch::default(),
            AgentBatch(slot),
            // Instances carry their own transforms; the entity's AABB means nothing.
            NoFrustumCulling,
            Name::new(name),
        ));
    };
    for kind in 0..VEHICLE_KINDS {
        for detail in 0..2 {
            spawn(
                Slot::Vehicle { kind, detail },
                vehicle_mesh(kind, detail),
                "vehicle batch",
            );
        }
    }
    for detail in 0..2 {
        spawn(
            Slot::Pedestrian { detail },
            pedestrian_mesh(detail),
            "pedestrian batch",
        );
    }
}

/// Scratch lists kept across frames, so the per-frame build does not reallocate.
#[derive(Resource, Default)]
struct FrameInstances(AgentInstances);

#[derive(Default)]
struct Looks {
    db_hash: u64,
    looks: VehicleLooks,
}

#[allow(clippy::too_many_arguments)]
fn build_agent_instances(
    view: Option<Res<SimView>>,
    fraction: Res<TickFraction>,
    time: Res<Time>,
    camera: Query<(&GlobalTransform, &Frustum), With<Camera3d>>,
    mut scratch: ResMut<FrameInstances>,
    mut batches: Query<(&AgentBatch, &mut InstanceBatch)>,
    mut stats: ResMut<AgentRenderStats>,
    mut looks: Local<Looks>,
) {
    let (Some(view), Some((cam, frustum))) = (view, camera.iter().next()) else {
        return;
    };
    let start = bevy::platform::time::Instant::now();
    if let Some(db) = view.content.as_ref()
        && looks.db_hash != db.content_hash()
    {
        looks.db_hash = db.content_hash();
        looks.looks = VehicleLooks::from_content(db);
    }
    let snap = &view.agents;
    let cull = Cull {
        height: cam.translation().y
            - view.terrain.as_ref().map_or(0.0, |t| {
                t.height(Vec2::new(cam.translation().x, cam.translation().z))
            }),
        planes: frustum.half_spaces.map(|h| h.normal_d()),
        alpha: fraction.0,
        time: time.elapsed_secs(),
    };
    let out = &mut scratch.0;
    out.clear();
    build_vehicles(
        &snap.prev.vehicles,
        &snap.cur.vehicles,
        &looks.looks,
        &cull,
        out,
    );
    if let Some(terrain) = view.terrain.as_ref() {
        build_pedestrians(
            &snap.prev.pedestrians,
            &snap.cur.pedestrians,
            |x, z| terrain.height(Vec2::new(x, z)),
            &cull,
            out,
        );
    }
    let mut draws = 0;
    for (batch, mut data) in &mut batches {
        let list = match batch.0 {
            Slot::Vehicle { kind, detail } => &out.vehicles[kind][detail],
            Slot::Pedestrian { detail } => &out.pedestrians[detail],
        };
        draws += usize::from(!list.is_empty());
        data.0 = Arc::new(list.clone());
    }
    *stats = AgentRenderStats {
        vehicles_in_sim: snap.cur.vehicles.len(),
        pedestrians_in_sim: snap.cur.pedestrians.len(),
        vehicles_drawn: out.vehicle_count(),
        pedestrians_drawn: out.pedestrian_count(),
        draws,
        prepare_ms: start.elapsed().as_secs_f32() * 1000.0,
        view_ms: stats.view_ms,
    };
}

// ---- render world ----------------------------------------------------------------------

#[derive(Resource)]
struct AgentPipeline {
    shader: Handle<Shader>,
    mesh_pipeline: MeshPipeline,
}

fn init_agent_pipeline(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mesh_pipeline: Res<MeshPipeline>,
) {
    commands.insert_resource(AgentPipeline {
        shader: asset_server.load("embedded://cp_render/agent_render.wesl"),
        mesh_pipeline: mesh_pipeline.clone(),
    });
}

impl SpecializedMeshPipeline for AgentPipeline {
    type Key = MeshPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mut descriptor = self.mesh_pipeline.specialize(key, layout)?;
        descriptor.vertex.shader = self.shader.clone();
        let f4 = VertexFormat::Float32x4;
        descriptor.vertex.buffers.push(VertexBufferLayout {
            array_stride: size_of::<InstanceData>() as u64,
            step_mode: VertexStepMode::Instance,
            // Locations 0, 1, 5 are position, normal, colour of the mesh.
            attributes: (0..3)
                .map(|i| VertexAttribute {
                    format: f4,
                    offset: f4.size() * i,
                    shader_location: 8 + i as u32,
                })
                .collect(),
        });
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader = self.shader.clone();
        }
        Ok(descriptor)
    }
}

#[allow(clippy::too_many_arguments)]
fn queue_agents(
    draw_functions: Res<DrawFunctions<Transparent3d>>,
    pipeline: Res<AgentPipeline>,
    mut pipelines: ResMut<SpecializedMeshPipelines<AgentPipeline>>,
    pipeline_cache: Res<PipelineCache>,
    meshes: Res<RenderAssets<RenderMesh>>,
    render_mesh_instances: Res<RenderMeshInstances>,
    batched: Option<Res<BatchedInstanceBuffers<MeshUniform, MeshInputUniform>>>,
    batches: Query<(Entity, &MainEntity, &InstanceBatch)>,
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    views: Query<&ExtractedView>,
    view_key_cache: Res<ViewKeyCache>,
) {
    let draw = draw_functions.read().id::<DrawAgents>();
    for view in &views {
        let Some(phase) = phases.get_mut(&view.retained_view_entity) else {
            continue;
        };
        let Some(&view_key) = view_key_cache.get(&view.retained_view_entity) else {
            continue;
        };
        for (entity, main_entity, batch) in &batches {
            if batch.0.is_empty() {
                continue;
            }
            let Some(instance) = render_mesh_instances.render_mesh_queue_data(*main_entity) else {
                continue;
            };
            let Some(mesh) = meshes.get(instance.mesh_asset_id()) else {
                continue;
            };
            let key = view_key
                | MeshPipelineKey::from_primitive_topology_and_strip_index(
                    mesh.primitive_topology(),
                    mesh.index_format(),
                );
            let Ok(pipeline_id) =
                pipelines.specialize(&pipeline_cache, &pipeline, key, &mesh.layout)
            else {
                continue;
            };
            phase.add_retained(Transparent3d {
                sorting_info: TransparentSortingInfo3d::Sorted {
                    mesh_center: pbr::get_mesh_instance_world_from_local(
                        *main_entity,
                        instance.current_uniform_index,
                        &render_mesh_instances,
                        batched.as_deref(),
                    )
                    .transform_point3(mesh.aabb_center),
                    depth_bias: 0.0,
                },
                entity: (entity, *main_entity),
                pipeline: pipeline_id,
                draw_function: draw,
                distance: 0.0,
                batch_range: 0..1,
                extra_index: PhaseItemExtraIndex::None,
                indexed: true,
            });
        }
    }
}

/// A growing GPU buffer per batch entity, rewritten each frame.
struct GpuInstances {
    buffer: Buffer,
    capacity: u64,
    count: u32,
}

#[derive(Resource, Default)]
struct InstanceBuffers(bevy::platform::collections::HashMap<Entity, GpuInstances>);

fn upload_instances(
    mut buffers: ResMut<InstanceBuffers>,
    batches: Query<(Entity, &InstanceBatch)>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    for (entity, batch) in &batches {
        let bytes: &[u8] = bytemuck::cast_slice(batch.0.as_slice());
        let needed = bytes.len() as u64;
        let slot = buffers.0.get_mut(&entity);
        let slot = match slot {
            Some(s) if s.capacity >= needed => s,
            _ => {
                let capacity = needed.next_power_of_two().max(4096);
                let buffer = device.create_buffer(&BufferDescriptor {
                    label: Some("agent instances"),
                    size: capacity,
                    usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                buffers.0.insert(
                    entity,
                    GpuInstances {
                        buffer,
                        capacity,
                        count: 0,
                    },
                );
                buffers.0.get_mut(&entity).expect("just inserted")
            }
        };
        if needed > 0 {
            queue.write_buffer(&slot.buffer, 0, bytes);
        }
        slot.count = batch.0.len() as u32;
    }
}

type DrawAgents = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    SetMeshBindGroup<2>,
    DrawInstanced,
);

struct DrawInstanced;

impl<P: PhaseItem> RenderCommand<P> for DrawInstanced {
    type Param = (
        SRes<RenderAssets<RenderMesh>>,
        SRes<RenderMeshInstances>,
        SRes<MeshAllocator>,
        SRes<InstanceBuffers>,
    );
    type ViewQuery = ();
    type ItemQuery = ();

    #[inline]
    fn render<'w>(
        item: &P,
        _view: (),
        _entity: Option<()>,
        (meshes, render_mesh_instances, mesh_allocator, buffers): SystemParamItem<
            'w,
            '_,
            Self::Param,
        >,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let mesh_allocator = mesh_allocator.into_inner();
        let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(item.main_entity())
        else {
            return RenderCommandResult::Skip;
        };
        let Some(gpu_mesh) = meshes.into_inner().get(mesh_instance.mesh_asset_id()) else {
            return RenderCommandResult::Skip;
        };
        let Some(instances) = buffers.into_inner().0.get(&item.entity()) else {
            return RenderCommandResult::Skip;
        };
        if instances.count == 0 {
            return RenderCommandResult::Skip;
        }
        let Some(vertex_slice) = mesh_allocator.mesh_vertex_slice(&mesh_instance.mesh_asset_id())
        else {
            return RenderCommandResult::Skip;
        };
        pass.set_vertex_buffer(0, vertex_slice.buffer.slice(..));
        pass.set_vertex_buffer(1, instances.buffer.slice(..));
        match &gpu_mesh.buffer_info {
            RenderMeshBufferInfo::Indexed {
                index_format,
                count,
            } => {
                let Some(index_slice) =
                    mesh_allocator.mesh_index_slice(&mesh_instance.mesh_asset_id())
                else {
                    return RenderCommandResult::Skip;
                };
                pass.set_index_buffer(index_slice.buffer.slice(..), *index_format);
                pass.draw_indexed(
                    index_slice.range.start..(index_slice.range.start + count),
                    vertex_slice.range.start as i32,
                    0..instances.count,
                );
            }
            RenderMeshBufferInfo::NonIndexed => {
                pass.draw(vertex_slice.range, 0..instances.count);
            }
        }
        RenderCommandResult::Success
    }
}
