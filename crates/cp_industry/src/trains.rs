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
use cp_transit::{CargoHubs, Endpoint, Goods, hub_sites, is_port_def};

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
        let port = buildings.get(id).is_some_and(|b| is_port_def(db, b.def));
        if !working.contains(&id) {
            continue;
        }
        // A port: the ship stands at the berth for `ship_dwell_h`, then leaves with the exports.
        let capacity = if port {
            bal.ship_capacity
        } else {
            bal.cargo_train_capacity
        };
        if port {
            if let Some(until) = hub.ship {
                if now < until {
                    continue;
                }
                hub.ship = None;
                hub.next_at = Tick(now.0 + interval);
            } else if now < hub.next_at || (hub.out.is_empty() && hub.orders.is_empty()) {
                continue;
            } else {
                hub.trains += 1;
                let dwell = (bal.ship_dwell_h * TICKS_PER_HOUR).ceil().max(1.0) as u64;
                hub.ship = Some(Tick(now.0 + dwell));
                unload(&buildings, &mut goods, &mut runs, hub, id);
                continue;
            }
        } else {
            if now < hub.next_at {
                continue;
            }
            hub.next_at = Tick(now.0 + interval);
            if hub.out.is_empty() && hub.orders.is_empty() {
                continue;
            }
            hub.trains += 1;
        }
        // Out: the exports, as many as the train or ship takes.
        let mut room = capacity;
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
        if !port {
            unload(&buildings, &mut goods, &mut runs, hub, id);
        }
    }
}

/// The ordered imports step off and a truck takes each to its building.
fn unload(
    buildings: &BuildingGrid,
    goods: &mut Goods,
    runs: &mut ServiceRuns,
    hub: &mut cp_transit::Hub,
    id: cp_building::BuildingId,
) {
    for (cargo, to, load) in std::mem::take(&mut hub.orders) {
        if buildings.get(to).is_some() {
            goods.start_cargo_haul(
                runs,
                cargo,
                Endpoint::Building(id),
                Endpoint::Building(to),
                load,
            );
        }
    }
}
