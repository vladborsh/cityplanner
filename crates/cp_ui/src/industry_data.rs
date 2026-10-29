//! The numbers of the Industry window (T-337, `architecture/industry-ui.md` §3.2): pure
//! functions over the [`SimView`], tested without a window.

use cp_building::BuildingId;
use cp_content::{BuildingDefId, CargoKind, ContentDb, IndustryRole, ResourceKind};
use cp_district::{AreaId, AreaKind, area_at};
use cp_industry::IndustryProblem;
use cp_locale::{tr, trf};
use cp_sim::SimView;

use crate::industry::{AreaSummary, problem_text};

/// One chain building of an area, for the "Buildings" sub-tab and the flows.
#[derive(Clone, Debug, PartialEq)]
pub struct ChainBuilding {
    pub id: BuildingId,
    pub def: BuildingDefId,
    pub role: IndustryRole,
    /// 1-based, among the buildings of the same type in the area, in `BuildingId` order.
    pub number: u32,
    pub filled: u16,
    pub slots: u16,
    pub stock: Vec<(CargoKind, f32)>,
    /// What the building can hold in an area of its level.
    pub cap: f32,
    pub problem: IndustryProblem,
}

impl ChainBuilding {
    pub fn stock_total(&self) -> f32 {
        self.stock.iter().map(|&(_, v)| v).sum()
    }

    /// The share of the job slots that is filled (1 for a building without jobs).
    pub fn load(&self) -> f32 {
        if self.slots == 0 {
            1.0
        } else {
            (f32::from(self.filled) / f32::from(self.slots)).min(1.0)
        }
    }
}

/// The chain buildings (and factories) standing in `area`: by role, then `BuildingId`.
pub fn area_buildings(view: &SimView, area: AreaId) -> Vec<ChainBuilding> {
    let Some(db) = view.content.as_deref() else {
        return Vec::new();
    };
    let level = view.areas.get(area).map_or(1, |a| a.level);
    let per_level = db.balance().industry_storage_per_level;
    let mut out: Vec<ChainBuilding> = Vec::new();
    for (id, b) in view.buildings.iter() {
        let Some(spec) = db.building(b.def).industry.as_ref() else {
            continue;
        };
        if area_at(&view.area_map, b.pos) != Some(area) {
            continue;
        }
        let (filled, slots) = view.industry_jobs.get(&id).copied().unwrap_or((0, 0));
        out.push(ChainBuilding {
            id,
            def: b.def,
            role: spec.role,
            number: 0,
            filled,
            slots,
            stock: view.industry.stock.get(&id).cloned().unwrap_or_default(),
            cap: cp_industry::storage_cap(spec.storage, per_level, level),
            problem: view.industry.problem_of(id),
        });
    }
    out.sort_by_key(|c| (c.role as u8, c.id));
    for i in 0..out.len() {
        out[i].number = 1 + out[..i].iter().filter(|c| c.def == out[i].def).count() as u32;
    }
    out
}

/// What an area made, ate and had left over in the last week, and how busy its buildings were.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AreaFlows {
    pub produced: [f32; 13],
    /// Eaten by the processors and factories of the area, from their output and their recipe.
    pub eaten: [f32; 13],
    /// `produced - eaten`, never below zero: what leaves the area (≈).
    pub surplus: [f32; 13],
    /// The average load of each building type of the area, in `DefId` order.
    pub load: Vec<(BuildingDefId, f32)>,
}

pub fn area_flows(view: &SimView, db: &ContentDb, area: AreaId) -> AreaFlows {
    let mut out = AreaFlows::default();
    if let Some(book) = view.area_books.last_of(area.0) {
        for (&c, &v) in &book.produced {
            out.produced[c as usize] = v;
        }
    }
    let buildings = area_buildings(view, area);
    let mut defs: Vec<BuildingDefId> = buildings.iter().map(|b| b.def).collect();
    defs.sort_unstable_by_key(|d| d.0);
    defs.dedup();
    for def in defs {
        let mine: Vec<&ChainBuilding> = buildings.iter().filter(|b| b.def == def).collect();
        out.load.push((
            def,
            mine.iter().map(|b| b.load()).sum::<f32>() / mine.len() as f32,
        ));
        let Some(spec) = db.building(def).industry.as_ref() else {
            continue;
        };
        if !matches!(spec.role, IndustryRole::Processor | IndustryRole::Factory) {
            continue;
        }
        // The runs of the recipe: its first output made, over the output rate.
        let Some(&(c_out, rate)) = spec.outputs.first() else {
            continue;
        };
        if rate <= 0.0 {
            continue;
        }
        let runs = out.produced[c_out as usize] / rate;
        for &(c_in, r) in &spec.inputs {
            out.eaten[c_in as usize] += runs * r;
        }
    }
    for i in 0..13 {
        out.surplus[i] = (out.produced[i] - out.eaten[i]).max(0.0);
    }
    out
}

/// The buildings the area level `level` opens: the chain of `resource` and the factories whose
/// `min_level` is exactly `level`, by role then id.
pub fn area_unlocks(db: &ContentDb, resource: ResourceKind, level: u8) -> Vec<BuildingDefId> {
    let mut out: Vec<(u8, String, BuildingDefId)> = db
        .buildings()
        .filter_map(|(id, d)| {
            let spec = d.industry.as_ref()?;
            let mine = spec.resource == Some(resource) || spec.role == IndustryRole::Factory;
            (mine && spec.min_level == level && spec.role != IndustryRole::Main)
                .then(|| (spec.role as u8, d.id.clone(), id))
        })
        .collect();
    out.sort();
    out.into_iter().map(|(_, _, d)| d).collect()
}

/// Days until the next level at last week's pace (the xp is the units made): `None` at the top
/// or when nothing was made.
pub fn level_eta_days(s: &AreaSummary, last_produced: f32) -> Option<u32> {
    let next = s.next_xp?;
    if last_produced <= 0.0 {
        return None;
    }
    let left = next.saturating_sub(s.xp) as f32;
    Some((left / (last_produced / 7.0)).ceil() as u32)
}

/// What the city did with one cargo last week.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CargoStatus {
    /// Made in the city, `units` of it, and `imported` bought as well.
    Made {
        units: u32,
        imported: u32,
    },
    None,
}

/// The status of the 13 cargo in [`CargoKind::ALL`] order: the production is the sum of the
/// last week's books of all areas, the imports are the ledger's.
pub fn cargo_status(view: &SimView) -> [CargoStatus; 13] {
    let mut made = [0.0f32; 13];
    for book in view.area_books.last.values() {
        for (&c, &v) in &book.produced {
            made[c as usize] += v;
        }
    }
    std::array::from_fn(|i| {
        if made[i] >= 0.5 {
            CargoStatus::Made {
                units: made[i].round() as u32,
                imported: view.industry_ledger.last_by_cargo[i].import,
            }
        } else {
            CargoStatus::None
        }
    })
}

/// Why a factory cannot be placed yet: the best industrial area of any resource with a main
/// building is too low, or there is none. The rules of `validate_industry_site`.
pub fn factory_lock(view: &SimView, db: &ContentDb, def: BuildingDefId) -> Option<String> {
    let spec = db.building(def).industry.as_ref()?;
    if spec.role != IndustryRole::Factory {
        return None;
    }
    match best_industrial_level(view, db) {
        None => Some(tr("needs an industrial area with a main building")),
        Some(level) if level < spec.min_level => {
            Some(trf!("area level {level}", level = spec.min_level))
        }
        Some(_) => None,
    }
}

/// The highest level among the industrial areas that have a main building.
pub fn best_industrial_level(view: &SimView, db: &ContentDb) -> Option<u8> {
    view.areas
        .iter()
        .filter(|(id, a)| matches!(a.kind, AreaKind::Industry(_)) && has_main(view, db, *id))
        .map(|(_, a)| a.level)
        .max()
}

/// Whether a main building stands in `area`.
pub fn has_main(view: &SimView, db: &ContentDb, area: AreaId) -> bool {
    view.buildings.iter().any(|(_, b)| {
        db.building(b.def)
            .industry
            .as_ref()
            .is_some_and(|s| s.role == IndustryRole::Main)
            && area_at(&view.area_map, b.pos) == Some(area)
    })
}

fn cargo_name(c: CargoKind) -> String {
    tr(&format!("{c:?}"))
}

/// The first problem of an area in words, for the "!" of its card: "Field S №2: storage full".
pub fn area_problem(view: &SimView, db: &ContentDb, area: AreaId) -> Option<String> {
    if !has_main(view, db, area) {
        return Some(tr("No main building: the area does not work"));
    }
    area_buildings(view, area)
        .into_iter()
        .find(|b| b.problem != IndustryProblem::None)
        .and_then(|b| {
            let words = problem_text(b.problem)?;
            Some(trf!(
                "{building} #{n}: {problem}",
                building = tr(&db.building(b.def).name),
                n = b.number,
                problem = tr(words)
            ))
        })
}

/// The advice of an area, the first rule that holds (§3.2); `None` when all is well.
pub fn area_tip(view: &SimView, db: &ContentDb, area: AreaId) -> Option<String> {
    let a = view.areas.get(area)?;
    let AreaKind::Industry(resource) = a.kind else {
        return None;
    };
    if !has_main(view, db, area) {
        return Some(tr(
            "Place a main building: the area does nothing without one",
        ));
    }
    let all = area_buildings(view, area);
    let spec_of = |b: &ChainBuilding| db.building(b.def).industry.as_ref();
    let working: Vec<&ChainBuilding> = all
        .iter()
        .filter(|b| b.role != IndustryRole::Main)
        .collect();
    // 2. A processor starved of input.
    for b in working
        .iter()
        .filter(|b| matches!(b.role, IndustryRole::Processor | IndustryRole::Factory))
    {
        let same: Vec<_> = working.iter().filter(|o| o.def == b.def).collect();
        let starved = same
            .iter()
            .filter(|o| o.problem == IndustryProblem::NoInput)
            .count();
        if starved * 2 >= same.len() && b.problem == IndustryProblem::NoInput {
            let lacking = spec_of(b).and_then(|s| {
                s.inputs.iter().map(|&(c, _)| c).min_by(|&x, &y| {
                    let have = |c| {
                        same.iter()
                            .map(|o| view.industry.stock_of(o.id, c))
                            .sum::<f32>()
                    };
                    have(x).total_cmp(&have(y))
                })
            })?;
            let source = db.buildings().map(|(_, d)| d).find(|d| {
                d.industry.as_ref().is_some_and(|s| {
                    s.role == IndustryRole::Extractor
                        && s.resource == Some(resource)
                        && s.outputs.iter().any(|&(c, _)| c == lacking)
                })
            });
            return Some(match source {
                Some(d) => trf!(
                    "Not enough {cargo}: one more {building} helps",
                    cargo = cargo_name(lacking),
                    building = tr(&d.name)
                ),
                None => trf!(
                    "Not enough {cargo}: the processor waits for it",
                    cargo = cargo_name(lacking)
                ),
            });
        }
    }
    // 3. An extractor whose store is full.
    if working
        .iter()
        .any(|b| b.role == IndustryRole::Extractor && b.problem == IndustryProblem::Full)
    {
        return Some(tr("Storage is full: add a warehouse or a processor"));
    }
    // 4. Raw cargo that leaves as it is.
    let flows = area_flows(view, db, area);
    for &c in &CargoKind::ALL {
        let raw: f32 = working
            .iter()
            .filter(|b| b.role == IndustryRole::Extractor)
            .filter_map(|b| spec_of(b))
            .filter(|s| s.outputs.iter().any(|&(o, _)| o == c))
            .count() as f32;
        if raw > 0.0
            && flows.produced[c as usize] > 0.0
            && flows.surplus[c as usize] > 0.25 * flows.produced[c as usize]
            && db.buildings().any(|(_, d)| {
                d.industry.as_ref().is_some_and(|s| {
                    matches!(s.role, IndustryRole::Processor | IndustryRole::Factory)
                        && s.inputs.iter().any(|&(i, _)| i == c)
                        && s.min_level <= a.level
                        && (s.resource.is_none() || s.resource == Some(resource))
                })
            })
        {
            return Some(trf!(
                "{cargo} leaves raw: a processor or a factory makes more of it",
                cargo = cargo_name(c)
            ));
        }
    }
    // 5. Too few workers.
    let (filled, slots) = working.iter().fold((0u32, 0u32), |(f, s), b| {
        (f + u32::from(b.filled), s + u32::from(b.slots))
    });
    if slots > 0 && (filled as f32) < 0.6 * slots as f32 {
        return Some(tr(
            "Few workers: residential districts and a road are needed",
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::industry::tests::{put, world_with_areas};
    use cp_industry::{AreaBook, AreaBooks, IndustryState};
    use cp_sim::SimRunner;
    use std::sync::Arc;

    fn book(sim: &mut SimRunner, area: u8, produced: &[(CargoKind, f32)]) {
        let mut b = AreaBook::default();
        b.produced.extend(produced.iter().copied());
        sim.world_mut()
            .resource_mut::<AreaBooks>()
            .last
            .insert(area, b);
    }

    fn farm(level: u8) -> (SimRunner, Arc<ContentDb>, [bevy::math::Vec2; 2]) {
        let (mut sim, db, at) = world_with_areas(level);
        sim.world_mut()
            .resource_mut::<cp_district::Areas>()
            .get_mut(AreaId(2))
            .unwrap()
            .level = level;
        put(&mut sim, &db, "farm_main", at[1]);
        (sim, db, at)
    }

    #[test]
    fn the_flows_of_an_area_take_what_its_processors_ate_from_the_books() {
        let (mut sim, db, at) = farm(2);
        put(&mut sim, &db, "farm_processor_a", at[1]);
        put(&mut sim, &db, "farm_processor_b", at[1]);
        book(
            &mut sim,
            2,
            &[
                (CargoKind::Crops, 1190.0),
                (CargoKind::AnimalProducts, 280.0),
                (CargoKind::Flour, 280.0),
            ],
        );
        let f = area_flows(&sim.view(), &db, AreaId(2));
        assert_eq!(f.eaten[CargoKind::Crops as usize], 840.0);
        assert_eq!(f.surplus[CargoKind::Crops as usize], 350.0);
        assert_eq!(f.surplus[CargoKind::Flour as usize], 280.0);
        assert_eq!(f.load.len(), 3, "main and two processors");
    }

    #[test]
    fn a_level_opens_its_chain_buildings_and_the_factories() {
        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let ids: Vec<_> = area_unlocks(&db, ResourceKind::Farming, 2)
            .into_iter()
            .map(|d| db.building(d).id.clone())
            .collect();
        assert_eq!(
            ids,
            [
                "farm_extractor_l",
                "farm_processor_b",
                "food_factory",
                "furniture_factory"
            ]
        );
        assert!(area_unlocks(&db, ResourceKind::Farming, 1).len() >= 3);
        assert!(area_unlocks(&db, ResourceKind::Farming, 5).is_empty());
    }

    #[test]
    fn a_factory_wants_an_industrial_area_with_a_main_building_of_its_level() {
        let (mut sim, db, at) = world_with_areas(1);
        let food = db.building_id("food_factory").unwrap();
        let lock = |sim: &SimRunner| factory_lock(&sim.view(), &db, food);
        assert_eq!(
            lock(&sim).as_deref(),
            Some("needs an industrial area with a main building")
        );
        put(&mut sim, &db, "forest_main", at[0]);
        assert_eq!(lock(&sim).as_deref(), Some("area level 2"));
        sim.world_mut()
            .resource_mut::<cp_district::Areas>()
            .get_mut(AreaId(1))
            .unwrap()
            .level = 2;
        assert_eq!(lock(&sim), None);
        // The same rule answers for the palette.
        assert_eq!(crate::industry::industry_lock(&sim.view(), &db, food), None);
    }

    #[test]
    fn the_eta_follows_last_weeks_output() {
        let s = AreaSummary {
            name: String::new(),
            resource: ResourceKind::Farming,
            level: 1,
            xp: 1000,
            next_xp: Some(2000),
            buildings: 0,
            jobs: 0,
            stock: vec![],
            economy: Default::default(),
        };
        assert_eq!(level_eta_days(&s, 700.0), Some(10));
        assert_eq!(level_eta_days(&s, 0.0), None);
        let top = AreaSummary { next_xp: None, ..s };
        assert_eq!(level_eta_days(&top, 700.0), None);
    }

    #[test]
    fn the_cargo_status_tells_made_from_bought_from_nothing() {
        let (mut sim, _db, _) = farm(1);
        book(&mut sim, 2, &[(CargoKind::Crops, 450.0)]);
        sim.world_mut()
            .resource_mut::<cp_industry::IndustryLedger>()
            .last_by_cargo[CargoKind::Crops as usize]
            .import = 80;
        let s = cargo_status(&sim.view());
        assert_eq!(
            s[CargoKind::Crops as usize],
            CargoStatus::Made {
                units: 450,
                imported: 80
            }
        );
        assert_eq!(s[CargoKind::Flour as usize], CargoStatus::None);
    }

    #[test]
    fn the_buildings_of_an_area_come_by_role_with_their_numbers_and_workers() {
        let (mut sim, db, at) = farm(1);
        let b = put(&mut sim, &db, "farm_extractor_s", at[1]);
        let a = put(&mut sim, &db, "farm_extractor_s", at[1]);
        let w = put(&mut sim, &db, "farm_warehouse", at[1]);
        let mut view = sim.view();
        Arc::make_mut(&mut view.industry_jobs).insert(a, (3, 8));
        let list = area_buildings(&view, AreaId(2));
        let roles: Vec<_> = list.iter().map(|c| c.role as u8).collect();
        assert_eq!(roles, [0, 1, 1, 3], "main, two fields, warehouse");
        assert_eq!(
            list.iter().map(|c| c.number).collect::<Vec<_>>(),
            [1, 1, 2, 1]
        );
        assert_eq!(list[1].id, b.min(a));
        assert!(
            list.iter()
                .any(|c| c.id == a && (c.filled, c.slots) == (3, 8))
        );
        assert!(list.iter().any(|c| c.id == w && c.cap >= 1500.0));
    }

    /// One case per rule of the advice, in order of precedence.
    #[test]
    fn the_tip_is_the_first_rule_that_holds() {
        use IndustryProblem as P;
        let (mut sim, db, at) = world_with_areas(2);
        sim.world_mut()
            .resource_mut::<cp_district::Areas>()
            .get_mut(AreaId(2))
            .unwrap()
            .level = 2;
        let tip = |sim: &SimRunner| area_tip(&sim.view(), &db, AreaId(2));
        // 1. No main building.
        assert_eq!(
            tip(&sim).as_deref(),
            Some("Place a main building: the area does nothing without one")
        );
        put(&mut sim, &db, "farm_main", at[1]);
        let field = put(&mut sim, &db, "farm_extractor_s", at[1]);
        let proc_b = put(&mut sim, &db, "farm_processor_b", at[1]);
        let set = |sim: &mut SimRunner, problems: &[(cp_building::BuildingId, P)]| {
            let mut st = IndustryState::default();
            for &(b, p) in problems {
                st.problems.insert(b, p);
            }
            sim.world_mut().insert_resource(st);
        };
        // The jobs the tip reads come from the view; the sim has no workers here, so the last
        // rule would fire. Rules 2 to 4 are checked on a view with everyone at work.
        let staffed = |sim: &SimRunner| {
            let mut v = sim.view();
            let jobs = Arc::make_mut(&mut v.industry_jobs);
            for (_, (f, s)) in jobs.iter_mut() {
                *f = *s;
            }
            v
        };
        // 2. A processor without input.
        set(&mut sim, &[(proc_b, P::NoInput)]);
        let t = area_tip(&staffed(&sim), &db, AreaId(2)).unwrap();
        assert!(t.starts_with("Not enough Crops"), "{t}");
        // 3. A field whose store is full.
        set(&mut sim, &[(field, P::Full)]);
        assert_eq!(
            area_tip(&staffed(&sim), &db, AreaId(2)).as_deref(),
            Some("Storage is full: add a warehouse or a processor")
        );
        // 4. Raw crops leaving as they are.
        set(&mut sim, &[]);
        book(&mut sim, 2, &[(CargoKind::Crops, 1000.0)]);
        let t = area_tip(&staffed(&sim), &db, AreaId(2)).unwrap();
        assert!(t.starts_with("Crops leaves raw"), "{t}");
        // 5. Few workers.
        book(&mut sim, 2, &[]);
        assert_eq!(
            area_tip(&sim.view(), &db, AreaId(2)).as_deref(),
            Some("Few workers: residential districts and a road are needed")
        );
        // Nothing to say when all is well.
        assert_eq!(area_tip(&staffed(&sim), &db, AreaId(2)), None);
    }
}
