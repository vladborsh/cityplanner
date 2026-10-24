//! Bevy side of vehicle and pedestrian rendering (§13.3): one instanced draw per vehicle
//! kind and detail level and per pedestrian detail level. The instance lists are rebuilt every
//! frame from `SimView::agents` (interpolated by [`TickFraction`], culled by camera height
//! and frustum, see [`crate::agent`]) and uploaded as an instance-stepped vertex buffer, which
//! works on WebGL2 too (no storage buffers, no indirect drawing). Reads only `SimView`.
//!
//! The camera must carry `NoIndirectDrawing` (set where it is spawned).
//!
//! Shadows (BUG-094): every batch is also queued into each sun cascade's shadow phase, a
//! depth-only draw of the same instances, so cars and pedestrians cast into the sun's shadow
//! map like buildings and lamp posts and get the same filtered, soft shadow on the ground
//! (BUG-063's flattened copy of the model drawn onto the ground was hard-edged and blotchy).

use std::sync::Arc;

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::primitives::Frustum;
use bevy::camera::visibility::{NoFrustumCulling, VisibilitySystems};
use bevy::core_pipeline::core_3d::{Transparent3d, TransparentSortingInfo3d};
use bevy::ecs::query::QueryItem;
use bevy::ecs::system::{SystemParamItem, lifetimeless::*};
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology, VertexBufferLayout};
use bevy::pbr::{
    self, LightEntity, MeshInputUniform, MeshPipeline, MeshPipelineKey, MeshPipelineSystems,
    MeshUniform, PrepassPipeline, RenderMeshInstances, SetMeshBindGroup, SetMeshViewBindGroup,
    SetMeshViewBindingArrayBindGroup, SetPrepassViewBindGroup, Shadow, ShadowBatchSetKey,
    ShadowBinKey, ViewKeyCache,
};
use bevy::prelude::*;
use bevy::render::batching::gpu_preprocessing::BatchedInstanceBuffers;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::mesh::allocator::MeshAllocator;
use bevy::render::mesh::{RenderMesh, RenderMeshBufferInfo};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::{
    AddRenderCommand, BinnedRenderPhaseType, DrawFunctions, InputUniformIndex, PhaseItem,
    PhaseItemExtraIndex, RenderCommand, RenderCommandResult, SetItemPipeline, TrackedRenderPass,
    ViewBinnedRenderPhases, ViewSortedRenderPhases,
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
    AgentInstances, Cull, Headings, InstanceData, VEHICLE_KINDS, VehicleLooks, build_pedestrians,
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
                .add_render_command::<Shadow, DrawAgentDepth>()
                .init_resource::<SpecializedMeshPipelines<AgentPipeline>>()
                .init_resource::<SpecializedMeshPipelines<AgentDepthPipeline>>()
                .init_resource::<InstanceBuffers>()
                .add_systems(
                    RenderStartup,
                    init_agent_pipeline
                        .after(MeshPipelineSystems)
                        .after(pbr::init_prepass_pipeline),
                )
                .add_systems(
                    Render,
                    (
                        (queue_agents, queue_agent_shadows).in_set(RenderSystems::QueueMeshes),
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
        let mesh = meshes.add(to_bevy_mesh(data));
        commands.spawn((
            Mesh3d(mesh),
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

/// How often (real seconds) the parked cars are laid again while the city changes: they follow
/// the buildings slowly, never per tick.
const PARKED_REFRESH_S: f32 = 2.0;

/// The parked cars and the bays of the net they were laid on.
#[derive(Default)]
struct ParkedCache {
    key: Option<(usize, usize)>,
    built_at: f32,
    cars: Arc<Vec<crate::parked::ParkedBucket>>,
    index: Arc<crate::parked::BayIndex>,
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
    mut headings: Local<Headings>,
    settings: Option<Res<crate::LightingSettings>>,
    mut parked_cache: Local<ParkedCache>,
    drawn_buildings: Option<Res<crate::building_render::BuildingState>>,
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
        alpha: snap.blend(fraction.0),
        time: time.elapsed_secs(),
    };
    // Parked cars in the bays (T-276): the settings switch, on by default.
    let parked_on = settings.is_none_or(|s| s.parked_cars);
    if parked_on && let Some(db) = view.content.as_deref() {
        let key = (
            Arc::as_ptr(&view.net) as usize,
            Arc::as_ptr(&view.buildings) as usize,
        );
        let now = time.elapsed_secs();
        if parked_cache.key != Some(key)
            && (parked_cache.key.is_none() || now - parked_cache.built_at >= PARKED_REFRESH_S)
        {
            let (cars, index) =
                crate::parked::parked_cars(&view.net, &view.junctions, db, &view.buildings);
            *parked_cache = ParkedCache {
                key: Some(key),
                built_at: now,
                cars: Arc::new(crate::parked::bucketed(&cars)),
                index: Arc::new(index),
            };
        }
    }
    let out = &mut scratch.0;
    out.clear();
    build_vehicles(
        &snap.prev.vehicles,
        &snap.cur.vehicles,
        &looks.looks,
        |id| view.buildings.get(id)?.portals(&view.net),
        &cull,
        out,
    );
    if parked_on {
        crate::agent::build_parked(&parked_cache.cars, &looks.looks, &cull, out);
    }
    if let Some(terrain) = view.terrain.as_ref() {
        let index = &parked_cache.index;
        build_pedestrians(
            &snap.prev.pedestrians,
            &snap.cur.pedestrians,
            |x, z| terrain.height(Vec2::new(x, z)),
            |p, side, lo, hi| {
                if parked_on {
                    index.clip(p, side, lo, hi)
                } else {
                    (lo, hi)
                }
            },
            // The door the building chunk drew in the facade, else the one on the lot front.
            |id| {
                drawn_buildings
                    .as_ref()
                    .and_then(|d| d.set.piece(id)?.door)
                    .or_else(|| Some(view.buildings.get(id)?.portals(&view.net)?.door))
            },
            &cull,
            &mut headings,
            out,
        );
    }
    let mut draws = 0;
    for (batch, mut data) in &mut batches {
        let list = Arc::new(match batch.0 {
            Slot::Vehicle { kind, detail } => out.vehicles[kind][detail].clone(),
            Slot::Pedestrian { detail } => out.pedestrians[detail].clone(),
        });
        draws += usize::from(!list.is_empty());
        data.0 = list;
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

/// The depth-only draw of the instances into a sun cascade's shadow map (BUG-094).
#[derive(Resource)]
struct AgentDepthPipeline {
    shader: Handle<Shader>,
    /// The view bind group the shadow pass sets (`SetPrepassViewBindGroup`).
    view_layout: BindGroupLayoutDescriptor,
    /// Depth clamping instead of clipping, as Bevy's shadow casters: a caster between the sun
    /// and a cascade's near plane still lands in its map. Where the device lacks it the vertex
    /// stage clamps (see `vertex_depth`).
    unclipped_depth: bool,
}

fn init_agent_pipeline(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mesh_pipeline: Res<MeshPipeline>,
    prepass_pipeline: Res<PrepassPipeline>,
    device: Res<RenderDevice>,
) {
    let shader = asset_server.load("embedded://cp_render/agent_render.wesl");
    commands.insert_resource(AgentPipeline {
        shader: shader.clone(),
        mesh_pipeline: mesh_pipeline.clone(),
    });
    commands.insert_resource(AgentDepthPipeline {
        shader,
        view_layout: prepass_pipeline.view_layout_no_motion_vectors.clone(),
        unclipped_depth: device.features().contains(WgpuFeatures::DEPTH_CLIP_CONTROL),
    });
}

/// The per-instance vertex buffer: three `vec4`s at locations 8, 9, 10 (locations 0, 1, 5 are
/// position, normal and colour of the mesh).
fn instance_layout() -> VertexBufferLayout {
    let f4 = VertexFormat::Float32x4;
    VertexBufferLayout {
        array_stride: size_of::<InstanceData>() as u64,
        step_mode: VertexStepMode::Instance,
        attributes: (0..3)
            .map(|i| VertexAttribute {
                format: f4,
                offset: f4.size() * i,
                shader_location: 8 + i as u32,
            })
            .collect(),
    }
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
        descriptor.vertex.entry_point = Some("vertex".into());
        descriptor.vertex.buffers.push(instance_layout());
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader = self.shader.clone();
            fragment.entry_point = Some("fragment".into());
        }
        Ok(descriptor)
    }
}

impl SpecializedMeshPipeline for AgentDepthPipeline {
    type Key = ();

    fn specialize(
        &self,
        _key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mesh = layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            Mesh::ATTRIBUTE_NORMAL.at_shader_location(1),
            Mesh::ATTRIBUTE_COLOR.at_shader_location(5),
        ])?;
        Ok(RenderPipelineDescriptor {
            label: Some("agent_shadow_depth_pipeline".into()),
            layout: vec![self.view_layout.clone()],
            vertex: VertexState {
                shader: self.shader.clone(),
                entry_point: Some("vertex_depth".into()),
                buffers: vec![mesh, instance_layout()],
                ..default()
            },
            fragment: None,
            primitive: PrimitiveState {
                cull_mode: Some(Face::Back),
                unclipped_depth: self.unclipped_depth,
                ..default()
            },
            // As Bevy's own shadow casters: reversed depth, no bias here (the receivers bias).
            depth_stencil: Some(DepthStencilState {
                format: bevy::core_pipeline::core_3d::CORE_3D_DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: StencilState::default(),
                bias: DepthBiasState::default(),
            }),
            ..default()
        })
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
            let mesh_key = view_key
                | MeshPipelineKey::from_primitive_topology_and_strip_index(
                    mesh.primitive_topology(),
                    mesh.index_format(),
                );
            let mesh_center = pbr::get_mesh_instance_world_from_local(
                *main_entity,
                instance.current_uniform_index,
                &render_mesh_instances,
                batched.as_deref(),
            )
            .transform_point3(mesh.aabb_center);
            let Ok(pipeline_id) =
                pipelines.specialize(&pipeline_cache, &pipeline, mesh_key, &mesh.layout)
            else {
                continue;
            };
            phase.add_retained(Transparent3d {
                sorting_info: TransparentSortingInfo3d::Sorted {
                    mesh_center,
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

/// Queues every batch into the shadow phase of each sun cascade (BUG-094). The phase is
/// retained and adding an entity again only refreshes its bin, so this runs every frame for
/// cascades that come and go. An empty batch draws nothing (`DrawInstanced` skips it).
#[allow(clippy::too_many_arguments)]
fn queue_agent_shadows(
    draw_functions: Res<DrawFunctions<Shadow>>,
    pipeline: Res<AgentDepthPipeline>,
    mut pipelines: ResMut<SpecializedMeshPipelines<AgentDepthPipeline>>,
    pipeline_cache: Res<PipelineCache>,
    meshes: Res<RenderAssets<RenderMesh>>,
    render_mesh_instances: Res<RenderMeshInstances>,
    mesh_allocator: Res<MeshAllocator>,
    batches: Query<(Entity, &MainEntity), With<InstanceBatch>>,
    mut phases: ResMut<ViewBinnedRenderPhases<Shadow>>,
    lights: Query<(&ExtractedView, &LightEntity)>,
) {
    let draw = draw_functions.read().id::<DrawAgentDepth>();
    for (view, light) in &lights {
        // The sun only: lamps cast no shadows.
        if !matches!(light, LightEntity::Directional { .. }) {
            continue;
        }
        let Some(phase) = phases.get_mut(&view.retained_view_entity) else {
            continue;
        };
        for (entity, main_entity) in &batches {
            let Some(instance) = render_mesh_instances.render_mesh_queue_data(*main_entity) else {
                continue;
            };
            let Some(mesh) = meshes.get(instance.mesh_asset_id()) else {
                continue;
            };
            let Some(slabs) = mesh_allocator.mesh_slabs(&instance.mesh_asset_id()) else {
                continue;
            };
            let Ok(pipeline_id) =
                pipelines.specialize(&pipeline_cache, &pipeline, (), &mesh.layout)
            else {
                continue;
            };
            phase.add(
                ShadowBatchSetKey {
                    pipeline: pipeline_id,
                    draw_function: draw,
                    material_bind_group_index: None,
                    slabs,
                },
                ShadowBinKey {
                    asset_id: instance.mesh_asset_id().into(),
                },
                (entity, *main_entity),
                InputUniformIndex::default(),
                BinnedRenderPhaseType::NonMesh,
            );
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
struct InstanceBuffers(bevy::platform::collections::HashMap<MainEntity, GpuInstances>);

fn upload_instances(
    mut buffers: ResMut<InstanceBuffers>,
    batches: Query<(&MainEntity, &InstanceBatch)>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    for (&entity, batch) in &batches {
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

/// The instances into a shadow map: the shadow view's uniforms, then the instanced draw.
type DrawAgentDepth = (SetItemPipeline, SetPrepassViewBindGroup<0>, DrawInstanced);

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
        let Some(instances) = buffers.into_inner().0.get(&item.main_entity()) else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::MeshVertexBufferLayouts;

    /// BUG-094: the agents' shadow draw is a depth-only pass like Bevy's own casters (reversed
    /// depth, written, no fragment stage) with the shadow view as its only bind group and the
    /// instances as a second vertex buffer.
    #[test]
    fn the_shadow_draw_writes_depth_only_with_the_instances() {
        let pipeline = AgentDepthPipeline {
            shader: Handle::default(),
            view_layout: BindGroupLayoutDescriptor::new("view", &[]),
            unclipped_depth: true,
        };
        let mesh = to_bevy_mesh(vehicle_mesh(0, 0));
        let layout = mesh.get_mesh_vertex_buffer_layout(&mut MeshVertexBufferLayouts::default());
        let d = pipeline.specialize((), &layout).expect("specialized");
        assert_eq!(d.vertex.entry_point.as_deref(), Some("vertex_depth"));
        assert!(d.fragment.is_none());
        assert_eq!(d.layout.len(), 1);
        assert_eq!(d.vertex.buffers.len(), 2);
        assert_eq!(d.vertex.buffers[1].step_mode, VertexStepMode::Instance);
        assert!(d.primitive.unclipped_depth);
        let depth = d.depth_stencil.expect("depth");
        assert_eq!(depth.depth_write_enabled, Some(true));
        assert_eq!(depth.depth_compare, Some(CompareFunction::GreaterEqual));
    }
}
