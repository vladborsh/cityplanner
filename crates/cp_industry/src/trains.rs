//! Cargo trains and ships (T-216, T-216b, T-217). At a working cargo station a train rides in
//! from the map edge every `cargo_train_interval_h` when there is cargo or an order, stands for
//! `cargo_train_dwell_h` and rides out; at a cargo port a ship sails in every `ship_interval_h`,
//! stands at the berth for `ship_dwell_h` and sails out (`ship_sail_h` each way). The imports
//! step off when it has come and a truck takes each to its building; the exports (up to
//! `cargo_train_capacity` / `ship_capacity` units) go on when it leaves, and `export_price × load`
//! goes to the [`IndustryLedger`] then. The unique factories' premium (T-215) follows the goods.
//! A station that is gone loses its stock and its orders; a station that is not joined to the
//! railway (any more) sends no train. Trains are timed runs of `cp_traffic` on the track (like the
//! intercity train); ships are timers.

use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingGrid, BuildingId};
use cp_content::{CargoKind, ContentDb, Market, SimContent, VehicleKind};
use cp_core::time::TICKS_PER_HOUR;
use cp_core::{Cost, Tick};
use cp_traffic::{PathNetwork, RunPoint, RunState, ServiceRuns};
use cp_transit::{
    CargoHubs, Endpoint, Goods, Hub, HubTrain, ShipPhase, ShipVisit, TrainPhase, edge_access,
    hub_sites, is_port_def, platform_goal,
};

use crate::freight::IndustryLedger;

fn ticks_of_hours(h: f32) -> u64 {
    (h * TICKS_PER_HOUR).ceil().max(1.0) as u64
}

/// `Agents` phase, after `run_freight`.
#[allow(clippy::too_many_arguments)]
pub fn run_cargo_trains(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    transit: Res<cp_transit::TransitNet>,
    rail: Res<cp_net::RailReach>,
    net: Res<cp_net::NetGraph>,
    pnet: Res<PathNetwork>,
    buildings: Res<BuildingGrid>,
    mut hubs: ResMut<CargoHubs>,
    mut goods: ResMut<Goods>,
    mut runs: ResMut<ServiceRuns>,
    mut agents: ResMut<cp_traffic::Agents>,
    mut ledger: ResMut<IndustryLedger>,
    market: Option<Res<Market>>,
) {
    let Some(content) = content else { return };
    let db = &*content.0;
    let bal = db.balance();
    let now = *tick;
    let fallback = Market::default();
    let prices = Prices {
        db,
        market: market.as_deref().unwrap_or(&fallback),
        now,
    };

    // A station that is gone takes its stock (and its train) with it.
    let gone: Vec<_> = hubs
        .hubs
        .iter()
        .filter(|(id, _)| buildings.get(**id).is_none())
        .map(|(id, h)| (*id, h.train))
        .collect();
    for (id, train) in gone {
        if let Some(t) = train {
            runs.release(t.run, &mut agents);
        }
        hubs.hubs.remove(&id);
    }
    if hubs.hubs.is_empty() {
        return;
    }
    let working: Vec<_> = hub_sites(db, &transit, &rail, &buildings)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let interval = ticks_of_hours(bal.cargo_train_interval_h);
    let ship_interval = ticks_of_hours(bal.ship_interval_h);
    let ids: Vec<_> = hubs.hubs.keys().copied().collect();
    for id in ids {
        let hub = hubs.hubs.get_mut(&id).expect("listed");
        let port = buildings.get(id).is_some_and(|b| is_port_def(db, b.def));
        let ok = working.contains(&id);
        if port {
            ship_cycle(
                prices,
                hub,
                id,
                ok,
                now,
                ship_interval,
                &buildings,
                &mut goods,
                &mut runs,
                &mut ledger,
            );
            continue;
        }
        // 1. The train on the line.
        if let Some(train) = hub.train {
            let state = runs.get(train.run).map(|r| r.state);
            let at_goal = matches!(
                state,
                Some(RunState::Parked { .. } | RunState::Arrived { .. })
            );
            let lost = state.is_none() || state == Some(RunState::Failed) || !ok;
            if lost {
                runs.release(train.run, &mut agents);
                hub.train = None;
                hub.next_at = Tick(now.0 + interval);
            } else if at_goal {
                match train.phase {
                    TrainPhase::Inbound => {
                        unload(&buildings, &mut goods, &mut runs, hub, id);
                        let until = Tick(now.0 + ticks_of_hours(bal.cargo_train_dwell_h));
                        hub.train = Some(HubTrain {
                            phase: TrainPhase::Dwell { until },
                            ..train
                        });
                    }
                    TrainPhase::Dwell { until } if now >= until => {
                        load_and_pay(prices, hub, bal.cargo_train_capacity, &mut ledger);
                        let out = edge_access(&net).map(|e| e.1);
                        let sent = out.is_some_and(|out| {
                            runs.continue_run_reversed(train.run, RunPoint::Access(out))
                                .is_ok()
                        });
                        if sent {
                            hub.train = Some(HubTrain {
                                phase: TrainPhase::Outbound,
                                ..train
                            });
                        } else {
                            runs.release(train.run, &mut agents);
                            hub.train = None;
                            hub.next_at = Tick(now.0 + interval);
                        }
                    }
                    TrainPhase::Dwell { .. } => {}
                    TrainPhase::Outbound => {
                        runs.release(train.run, &mut agents);
                        hub.train = None;
                        hub.trains += 1;
                        hub.next_at = Tick(now.0 + interval);
                    }
                }
            }
            continue;
        }
        // 2. A new train when something waits.
        if now < hub.next_at || !ok {
            continue;
        }
        hub.next_at = Tick(now.0 + interval);
        if hub.out.is_empty() && hub.orders.is_empty() {
            continue;
        }
        let ride = edge_access(&net).and_then(|(inbound, _, _)| {
            platform_goal(&transit, &pnet, id, &inbound).map(|goal| (inbound, goal))
        });
        if let Some((inbound, goal)) = ride {
            let run = runs.start_run(
                VehicleKind::CargoTrain,
                RunPoint::Access(inbound),
                RunPoint::Access(goal),
                true,
            );
            hub.train = Some(HubTrain {
                run,
                phase: TrainPhase::Inbound,
            });
        } else {
            // No ride to draw: the exchange happens at once.
            hub.trains += 1;
            load_and_pay(prices, hub, bal.cargo_train_capacity, &mut ledger);
            unload(&buildings, &mut goods, &mut runs, hub, id);
        }
    }
}

/// A port's ship: sails in when something waits, stands at the berth and sails out.
#[allow(clippy::too_many_arguments)]
fn ship_cycle(
    prices: Prices,
    hub: &mut Hub,
    id: BuildingId,
    ok: bool,
    now: Tick,
    interval: u64,
    buildings: &BuildingGrid,
    goods: &mut Goods,
    runs: &mut ServiceRuns,
    ledger: &mut IndustryLedger,
) {
    let bal = prices.db.balance();
    let sail = ticks_of_hours(bal.ship_sail_h);
    let Some(visit) = hub.ship else {
        if ok && now >= hub.next_at && !(hub.out.is_empty() && hub.orders.is_empty()) {
            hub.ship = Some(ShipVisit {
                phase: ShipPhase::Arriving,
                until: Tick(now.0 + sail),
            });
        }
        return;
    };
    if now < visit.until {
        return;
    }
    match visit.phase {
        ShipPhase::Arriving => {
            unload(buildings, goods, runs, hub, id);
            hub.ship = Some(ShipVisit {
                phase: ShipPhase::Docked,
                until: Tick(now.0 + ticks_of_hours(bal.ship_dwell_h)),
            });
        }
        ShipPhase::Docked => {
            load_and_pay(prices, hub, bal.ship_capacity, ledger);
            hub.ship = Some(ShipVisit {
                phase: ShipPhase::Leaving,
                until: Tick(now.0 + sail),
            });
        }
        ShipPhase::Leaving => {
            hub.ship = None;
            hub.trains += 1;
            hub.next_at = Tick(now.0 + interval);
        }
    }
}

/// The day's export prices (§21): the market at `now`.
#[derive(Clone, Copy)]
struct Prices<'a> {
    db: &'a ContentDb,
    market: &'a Market,
    now: Tick,
}

impl Prices<'_> {
    fn export(&self, cargo: CargoKind) -> f32 {
        self.market.export_price(self.db, cargo, self.now)
    }
}

/// The exports go on board, as many as `capacity` allows, and are paid for at the day's price.
fn load_and_pay(prices: Prices, hub: &mut Hub, capacity: f32, ledger: &mut IndustryLedger) {
    let mut room = capacity;
    let mut left = Vec::new();
    for (cargo, amount) in std::mem::take(&mut hub.out) {
        let take = amount.min(room.max(0.0));
        if take > 0.0 {
            let price = prices.export(cargo);
            let mut pay = Cost::scaled(price, f64::from(take));
            if cargo == CargoKind::Goods && amount > 0.0 {
                // The premium of the unique factories' goods goes with its share.
                let share = hub.premium * (take / amount);
                hub.premium -= share;
                pay += Cost::scaled(price, f64::from(share));
            }
            ledger.exports += pay;
            ledger.export_loads += 1;
            ledger.count(cargo, take, 0.0);
            room -= take;
        }
        if amount - take > 1e-3 {
            left.push((cargo, amount - take));
        }
    }
    hub.out = left;
    if !hub.out.iter().any(|&(c, _)| c == CargoKind::Goods) {
        hub.premium = 0.0;
    }
}

/// The ordered imports step off and a truck takes each to its building.
fn unload(
    buildings: &BuildingGrid,
    goods: &mut Goods,
    runs: &mut ServiceRuns,
    hub: &mut Hub,
    id: BuildingId,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_goods_premium_is_paid_pro_rata_with_the_load_that_goes() {
        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let market = Market::default();
        let prices = Prices {
            db: &db,
            market: &market,
            now: Tick(0),
        };
        let price = prices.export(CargoKind::Goods);
        let mut hub = Hub {
            out: vec![(CargoKind::Goods, 100.0)],
            premium: 300.0,
            ..Hub::default()
        };
        let mut ledger = IndustryLedger::default();
        // A vehicle that takes half: half the load and half the premium (x4 of 50 units).
        load_and_pay(prices, &mut hub, 50.0, &mut ledger);
        assert_eq!(ledger.exports, Cost::scaled(price, 200.0));
        assert_eq!(hub.premium, 150.0);
        load_and_pay(prices, &mut hub, 50.0, &mut ledger);
        assert_eq!(ledger.exports, Cost::scaled(price, 400.0));
        assert_eq!(hub.premium, 0.0);
    }
}
