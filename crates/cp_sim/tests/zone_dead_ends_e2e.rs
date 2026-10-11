//! BUG-016: zone blocks run to dead ends and plain bends (degree <= 2 nodes); only real
//! junctions (three or more arms) keep their setback.

use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_sim::SimRunner;
use cp_zone::ZoneGrid;

fn sim() -> (SimRunner, Arc<ContentDb>) {
    let content = Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    );
    let mut sim = SimRunner::new(3);
    sim.set_content(Arc::clone(&content));
    (sim, content)
}

fn road(sim: &mut SimRunner, db: &ContentDb, a: (f32, f32), b: (f32, f32)) -> SegmentId {
    let id = db.road_id("basic_2l").unwrap();
    let def = db.road(id).clone();
    let curve = CubicBezier3::line(Vec3::new(a.0, 0.0, a.1), Vec3::new(b.0, 0.0, b.1));
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(&def, id, curve, Elevation::Ground)
        .unwrap()
}

/// Columns of each side's block of `seg`.
fn cols(sim: &SimRunner, seg: SegmentId) -> Vec<u16> {
    let z = sim.world().resource::<ZoneGrid>();
    z.blocks_of(seg)
        .into_iter()
        .flatten()
        .map(|b| z.block(b).unwrap().cols)
        .collect()
}

#[test]
fn a_lone_80_m_road_gets_ten_columns_per_side() {
    let (mut sim, db) = sim();
    let seg = road(&mut sim, &db, (5120.0, 5000.0), (5200.0, 5000.0));
    sim.tick_once();
    assert_eq!(cols(&sim, seg), vec![10, 10]);
}

#[test]
fn a_degree_two_node_loses_no_zoned_length() {
    let (mut sim, db) = sim();
    let a = road(&mut sim, &db, (5120.0, 5000.0), (5160.0, 5000.0));
    let b = road(&mut sim, &db, (5160.0, 5000.0), (5200.0, 5000.0));
    sim.tick_once();
    assert_eq!(cols(&sim, a), vec![5, 5]);
    assert_eq!(cols(&sim, b), vec![5, 5]);
}

#[test]
fn a_real_junction_keeps_its_setback() {
    let (mut sim, db) = sim();
    let a = road(&mut sim, &db, (5120.0, 5000.0), (5160.0, 5000.0));
    road(&mut sim, &db, (5160.0, 5000.0), (5200.0, 5000.0));
    road(&mut sim, &db, (5160.0, 5000.0), (5160.0, 5040.0));
    sim.tick_once();
    // The dead end at the far end is whole, the junction end is cut: fewer than 5 columns.
    let c = cols(&sim, a);
    assert!(c.iter().all(|&n| (1..5).contains(&n)), "{c:?}");
}
