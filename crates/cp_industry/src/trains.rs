//! Cargo trains (T-216): at a working cargo station a train comes every
//! `cargo_train_interval_h`. It takes the exports that trucks brought (up to
//! `cargo_train_capacity` units; `export_price × load` goes to the [`IndustryLedger`] when it
//! leaves) and brings the imports that were ordered at the station, which a truck then carries
//! on to the building that wanted them. A station that is gone loses its stock and its orders.
//! A station that is not joined to the railway (any more) sends no train.

use bevy_ecs::system::{Res, ResMut};
use cp_building::BuildingGrid;
use cp_content::SimContent;
use cp_core::time::TICKS_PER_HOUR;
use cp_core::{Cost, Tick};
use cp_traffic::ServiceRuns;
use cp_transit::{CargoHubs, Endpoint, Goods, hub_sites};

use crate::freight::IndustryLedger;

/// `Agents` phase, after `run_freight`.
#[allow(clippy::too_many_arguments)]
pub fn run_cargo_trains(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    transit: Res<cp_transit::TransitNet>,
    rail: Res<cp_net::RailReach>,
    buildings: Res<BuildingGrid>,
    mut hubs: ResMut<CargoHubs>,
    mut goods: ResMut<Goods>,
    mut runs: ResMut<ServiceRuns>,
    mut ledger: ResMut<IndustryLedger>,
) {
    let Some(content) = content else { return };
    let db = &*content.0;
    let bal = db.balance();
    let now = *tick;

    // A station that is gone takes its stock with it.
    hubs.hubs.retain(|id, _| buildings.get(*id).is_some());
    if hubs.hubs.is_empty() {
        return;
    }
    let working: Vec<_> = hub_sites(db, &transit, &rail, &buildings)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let interval = (bal.cargo_train_interval_h * TICKS_PER_HOUR)
        .ceil()
        .max(1.0) as u64;
    let ids: Vec<_> = hubs.hubs.keys().copied().collect();
    for id in ids {
        let hub = hubs.hubs.get_mut(&id).expect("listed");
        if now < hub.next_at || !working.contains(&id) {
            continue;
        }
        hub.next_at = Tick(now.0 + interval);
        if hub.out.is_empty() && hub.orders.is_empty() {
            continue;
        }
        hub.trains += 1;
        // Out: the exports, as many as the train takes.
        let mut room = bal.cargo_train_capacity;
        let mut left = Vec::new();
        for (cargo, amount) in std::mem::take(&mut hub.out) {
            let take = amount.min(room.max(0.0));
            if take > 0.0 {
                let price = db.cargo(cargo).map_or(bal.export_price, |c| c.export_price);
                ledger.exports += Cost::scaled(price, f64::from(take));
                ledger.export_loads += 1;
                room -= take;
            }
            if amount - take > 1e-3 {
                left.push((cargo, amount - take));
            }
        }
        hub.out = left;
        // In: the ordered imports step off and a truck takes each to its building.
        for (cargo, to, load) in std::mem::take(&mut hub.orders) {
            if buildings.get(to).is_some() {
                goods.start_cargo_haul(
                    &mut runs,
                    cargo,
                    Endpoint::Building(id),
                    Endpoint::Building(to),
                    load,
                );
            }
        }
    }
}
