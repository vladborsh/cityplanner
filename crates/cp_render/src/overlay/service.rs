//! The service overlays (`architecture/system-design.md` §16 M2 D "UI"): per service kind the
//! roads its buildings reach, coloured by the coverage `0..1`, and the buildings of the kind
//! marked. A pure function of the sim snapshot, like the other layers.

use cp_building::{BuildingGrid, BuildingState};
use cp_content::{ContentDb, ServiceKind};
use cp_net::NetGraph;
use cp_service::{MailState, ServiceCoverage};
use cp_terrain::Terrain;
use cp_zone::ZoneGrid;

use super::utility::lot_quads;
use super::{ramp, road_ribbons};
use crate::chunk::MeshData;

/// Colour of a service building of the overlay's kind.
pub const SERVICE_MARK: [f32; 4] = [0.25, 0.60, 1.0, 0.9];

/// Colour of a building with mail piling up (the Mail overlay): red at the MAIL problem, orange
/// from the point a van is sent.
pub const MAIL_PILE: [f32; 4] = [0.95, 0.22, 0.18, 0.9];
pub const MAIL_WAITING: [f32; 4] = [0.95, 0.60, 0.15, 0.85];

/// Poor (orange) → fair (yellow) → full (green) coverage `0..=1`.
pub fn coverage_color(v: f32) -> [f32; 4] {
    ramp(
        v,
        &[
            [0.90, 0.45, 0.10, 0.75],
            [0.90, 0.85, 0.15, 0.8],
            [0.15, 0.75, 0.30, 0.85],
        ],
    )
}

/// The layer of `kind`: a ribbon on every road with coverage above 0 (the others are left
/// bare), and the lot of every Active building of the kind. For the post, `mail` also marks
/// the buildings holding mail: orange from `mail_van_min`, red from `mail_problem` (T-249).
#[allow(clippy::too_many_arguments)]
pub fn service_overlay_mesh(
    kind: ServiceKind,
    mail: &MailState,
    net: &NetGraph,
    coverage: &ServiceCoverage,
    buildings: &BuildingGrid,
    zones: &ZoneGrid,
    db: &ContentDb,
    terrain: &Terrain,
) -> MeshData {
    let mut m = road_ribbons(net, |id| {
        let c = coverage.on_segment(kind, id);
        (c > 0.0).then(|| coverage_color(c))
    });
    for (_, b) in buildings.iter() {
        let is_kind = b.state == BuildingState::Active
            && db
                .building(b.def)
                .service
                .as_deref()
                .and_then(|s| db.service_id(s))
                .is_some_and(|s| db.service(s).kind == kind);
        if is_kind {
            lot_quads(&mut m, b, zones, terrain, SERVICE_MARK);
        }
    }
    if kind == ServiceKind::Post {
        let bal = db.balance();
        for (id, pile) in mail.piles() {
            let pile = f32::from(pile);
            let color = if pile >= bal.mail_problem {
                MAIL_PILE
            } else if pile >= bal.mail_van_min {
                MAIL_WAITING
            } else {
                continue;
            };
            if let Some(b) = buildings.get(id) {
                lot_quads(&mut m, b, zones, terrain, color);
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use bevy::ecs::schedule::Schedule;
    use bevy::ecs::world::World;
    use bevy::math::{Vec2, Vec3};
    use cp_building::{AccessPoint, Building, BuildingFlags, Footprint, Lot};
    use cp_content::SimContent;
    use cp_core::curve::CubicBezier3;
    use cp_core::{Id, Tick};
    use cp_net::Elevation;

    use super::*;

    const ROOT: &str = env!("CARGO_MANIFEST_DIR");

    /// A landfill beside a street at z = 0, and a second street 3 km away that no road joins.
    fn world() -> (World, Arc<ContentDb>) {
        let db =
            Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
        let mut world = World::new();
        world.insert_resource(Tick(0));
        world.insert_resource(SimContent(Arc::clone(&db)));
        world.init_resource::<NetGraph>();
        world.init_resource::<BuildingGrid>();
        cp_service::ServicePlugin::init(&mut world);
        let road = db.road_id("basic_2l").unwrap();
        let def = db.road(road).clone();
        let mut segs = Vec::new();
        for z in [0.0, 3000.0] {
            let curve = CubicBezier3::line(Vec3::new(0.0, 0.0, z), Vec3::new(100.0, 0.0, z));
            segs.push(
                world
                    .resource_mut::<NetGraph>()
                    .add_segment(&def, road, curve, Elevation::Ground)
                    .unwrap(),
            );
        }
        world.resource_mut::<BuildingGrid>().insert(Building {
            def: db.building_id("landfill").unwrap(),
            footprint: Footprint::Lot(Lot {
                block: Id::new(0, 0),
                col: 0,
                w: 4,
                d: 4,
            }),
            level: 1,
            state: cp_building::BuildingState::Active,
            ready_at: Tick(0),
            pos: Vec2::new(50.0, 10.0),
            rot: 0.0,
            access: Some(AccessPoint {
                segment: segs[0],
                t: 0.5,
            }),
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
        let mut schedule = Schedule::default();
        schedule.add_systems(cp_service::update_coverage);
        schedule.run(&mut world);
        (world, db)
    }

    fn mesh(world: &World, db: &ContentDb, kind: ServiceKind) -> MeshData {
        let terrain = Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap();
        service_overlay_mesh(
            kind,
            &MailState::default(),
            world.resource::<NetGraph>(),
            world.resource::<cp_service::ServiceCoverage>(),
            world.resource::<BuildingGrid>(),
            &ZoneGrid::default(),
            db,
            &terrain,
        )
    }

    #[test]
    fn the_garbage_overlay_paints_the_street_by_the_landfill_and_not_the_far_one() {
        let (world, db) = world();
        let m = mesh(&world, &db, ServiceKind::Garbage);
        assert!(!m.indices.is_empty());
        let near = m.positions.iter().filter(|p| p[2].abs() < 10.0).count();
        let far = m.positions.iter().filter(|p| p[2] > 2900.0).count();
        assert!(near > 0, "the street under the landfill is painted");
        assert_eq!(far, 0, "the street beyond its reach is left bare");
        // Full coverage at the access point: green.
        assert!(m.colors.iter().any(|c| c[1] > c[0]));
        // A kind nothing provides paints nothing.
        assert!(mesh(&world, &db, ServiceKind::Fire).indices.is_empty());
    }

    #[test]
    fn the_mail_overlay_marks_buildings_with_mail_piling_up() {
        let (mut world, db) = world();
        let terrain = Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap();
        // A free-standing house (it has a footprint without the zone grid).
        let mut house = world
            .resource::<BuildingGrid>()
            .iter()
            .next()
            .unwrap()
            .1
            .clone();
        house.footprint = Footprint::Free { w: 2, d: 2 };
        let id = world.resource_mut::<BuildingGrid>().insert(house);
        let paint = |pile: u16| {
            let mut mail = MailState::default();
            mail.set_pile(id, pile);
            service_overlay_mesh(
                ServiceKind::Post,
                &mail,
                world.resource::<NetGraph>(),
                world.resource::<cp_service::ServiceCoverage>(),
                world.resource::<BuildingGrid>(),
                &ZoneGrid::default(),
                &db,
                &terrain,
            )
        };
        let has = |m: &MeshData, c: [f32; 4]| m.colors.iter().any(|x| *x == c);
        assert!(!has(&paint(5), MAIL_PILE) && !has(&paint(5), MAIL_WAITING));
        assert!(has(&paint(40), MAIL_WAITING) && !has(&paint(40), MAIL_PILE));
        assert!(has(&paint(200), MAIL_PILE));
    }
}
