use std::sync::Arc;
use bevy_math::Vec3;
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, ZoneType::*};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_terrain::TileOwnership;
use cp_zone::{ZoneCellRef, ZoneGrid};

#[test]
#[ignore]
fn diag() {
    let content = Arc::new(ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap());
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(7);
    sim.set_content(Arc::clone(&content));
    sim.world_mut().insert_resource(TileOwnership::only((2, 2)));
    *sim.world_mut().resource_mut::<cp_economy::Decay>() = cp_economy::Decay::disabled();
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        let lines: usize = std::env::var("LINES").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
        let p = |x: f32, z: f32| Vec3::new(2000.0 + x, 0.0, 2000.0 + z);
        for line in 0..lines { let o = line as f32 * 90.0; for piece in 0..lines - 1 { let a = piece as f32 * 90.0;
            for (from, to) in [(p(a, o), p(a + 90.0, o)), (p(o, a), p(o, a + 90.0))] {
                g.add_segment(&def, road, CubicBezier3::line(from, to), Elevation::Ground).unwrap(); } } }
    }
    sim.tick_once();
    let kinds: Vec<_> = if std::env::var("MIX").is_ok() { vec![ResHigh, ResLow, ComHigh, Industrial, ResHigh, ResHigh, Office, ComLow] } else { vec![ResLow, ResLow, ResHigh, ComLow, Industrial, ResLow] };
    {
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = grid.blocks().map(|(id, b)| (id, b.cell_count())).collect();
        for (n, (id, cells)) in ids.into_iter().enumerate() { for cell in 0..cells {
            grid.set_zone(ZoneCellRef { block: id, cell: cell as u16 }, kinds[n % kinds.len()]); } }
    }
    for day in 1..=30u64 {
        for _ in 0..TICKS_PER_DAY { sim.tick_once(); }
        if day % 6 != 0 { continue; }
        let v = sim.view();
        let st = &*v.city;
        let c = sim.world().resource::<CitizenGrid>();
        let mut by_edu = [0u32; 4];
        let mut by_edu_all = [0u32; 4];
        for (_, ct) in c.citizens_bucket(1, 0) {
            if ct.is_adult() { by_edu_all[ct.education as usize] += 1; if ct.work.is_none() { by_edu[ct.education as usize] += 1; } }
        }
        {
            let zg = sim.world().resource::<ZoneGrid>();
            for z in [Industrial, ComLow, ResLow] {
                let mut free = 0; let mut maxrun = 0u16;
                for (_, b) in zg.blocks() { let mut run = 0u16; for c in 0..b.cols { let full = (0..cp_zone::BLOCK_DEPTH).all(|r| { let i = usize::from(c*cp_zone::BLOCK_DEPTH+r); b.valid[i] && b.zones[i]==z }); free += (0..cp_zone::BLOCK_DEPTH).filter(|r| { let i = usize::from(c*cp_zone::BLOCK_DEPTH+r); b.valid[i] && b.zones[i]==z }).count(); if full { run+=1; maxrun=maxrun.max(run);} else {run=0;} } }
                println!("  {z:?} free valid cells {free}, longest full-depth free run {maxrun}");
            }
            for (id, d) in content.buildings() { if d.level==1 && d.service.is_none() && matches!(d.zone, Industrial|ComLow) && d.spec.is_none() { println!("  def {} {:?} lot {:?} jobs {:?}", content.building(id).name, d.zone, d.lot, d.jobs); } }
        }
        println!("day {day} pop {} adults {} emp {} jobless by edu {:?} of {:?} demand {:?}", c.citizen_count(), c.adults(), c.employed(), by_edu, by_edu_all, sim.world().resource::<cp_building::Demand>());
        for (z, zs) in cp_sim::stats::ZONES.iter().zip(st.zones.iter()) {
            println!("   {z:?}: painted {} occ {} homes {} free {} jobs {} jfree {} lv {:?}", zs.painted, zs.occupied, zs.homes, zs.homes_free, zs.jobs, zs.jobs_free, zs.by_level);
        }
    }
}
