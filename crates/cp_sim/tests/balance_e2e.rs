//! T-244: the numbers of the balance pass hold. Taxes never pay less for a higher level, a new
//! park area starts with the default fee (and keeps the one the player sets), and the chains of
//! the industrial areas pay: a processor or factory beats importing what it makes, a unique
//! factory is clearly profitable. The long-run table is `balance_probe`.

use std::path::Path;
use std::sync::Arc;

use cp_content::{CargoKind, ContentDb, IndustryRole, ParkKind};
use cp_core::time::TICKS_PER_DAY;
use cp_district::{AreaId, AreaKind, DistrictCommand};
use cp_sim::{SimCommand, SimRunner};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn content() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap())
}

#[test]
fn a_higher_level_never_pays_less_tax() {
    let db = content();
    let b = db.balance();
    assert!(
        b.tax_household.windows(2).all(|w| w[0] <= w[1]),
        "{:?}",
        b.tax_household
    );
    assert!(
        b.level_mul.windows(2).all(|w| w[0] <= w[1]),
        "{:?}",
        b.level_mul
    );
    assert!(b.park_fee_default >= 0.0 && b.park_fee_default <= b.park_fee_max);
}

#[test]
fn a_new_park_area_starts_with_the_default_fee_and_keeps_the_one_set() {
    let db = content();
    let default = db.balance().park_fee_default;
    assert!(default > 0.0, "parks start paying");
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(&db));
    let tick = sim.tick();
    sim.push_command(
        SimCommand::District(DistrictCommand::CreateArea {
            kind: AreaKind::Park(ParkKind::CityPark),
            name: "Park".into(),
        }),
        tick,
    );
    for _ in 0..TICKS_PER_DAY / 12 {
        sim.tick_once();
    }
    let fee = |sim: &SimRunner| sim.view().parks.get(AreaId(1)).map(|a| a.fee);
    assert_eq!(fee(&sim), Some(default));
    let tick = sim.tick();
    sim.push_command(
        SimCommand::Park(cp_park::ParkCommand::SetFee {
            area: AreaId(1),
            fee: 0.0,
        }),
        tick,
    );
    for _ in 0..TICKS_PER_DAY / 12 {
        sim.tick_once();
    }
    assert_eq!(fee(&sim), Some(0.0), "a fee the player set stays");
}

/// Weekly money of a building at full output, outputs sold at the export price (a unique
/// factory's goods at `factory_export_mul` times it), inputs counted at the export price they
/// could have fetched, against the upkeep; and what it saves against importing its output.
#[test]
fn the_chains_pay_against_imports_and_the_unique_factories_pay_well() {
    let db = content();
    let b = db.balance();
    let price = |k: CargoKind| {
        db.cargo(k)
            .map(|c| (c.export_price, c.import_price))
            .unwrap()
    };
    let mut checked = 0;
    for (_, def) in db.buildings() {
        let Some(spec) = &def.industry else { continue };
        let (outs, ins): (f32, f32) = (
            spec.outputs
                .iter()
                .map(|&(k, q)| q * price(k).0)
                .sum::<f32>(),
            spec.inputs
                .iter()
                .map(|&(k, q)| q * price(k).0)
                .sum::<f32>(),
        );
        let saved: f32 = spec.outputs.iter().map(|&(k, q)| q * price(k).1).sum();
        match spec.role {
            IndustryRole::Processor => {
                // Making it beats buying it.
                assert!(
                    (saved - ins) * 7.0 > def.upkeep_week * 1.5,
                    "{}: saves {} a week against {} upkeep",
                    def.id,
                    (saved - ins) * 7.0,
                    def.upkeep_week
                );
                checked += 1;
            }
            IndustryRole::Factory => {
                let sold = outs * b.factory_export_mul;
                assert!(
                    (sold - ins) * 7.0 > def.upkeep_week * 3.0,
                    "{}: {} a week against {} upkeep",
                    def.id,
                    (sold - ins) * 7.0,
                    def.upkeep_week
                );
                checked += 1;
            }
            IndustryRole::Extractor => {
                assert!(outs * 7.0 > def.upkeep_week, "{}", def.id);
                checked += 1;
            }
            _ => {}
        }
    }
    assert!(checked >= 20, "{checked}");
}
